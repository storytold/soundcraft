//! Bounded output-edge compensation for graphs admitted by the existing render order.
use super::{DelayLine, MAX_CHANNELS, MAX_LATENCY};
use soundcraft_model::{BusId, Route, Session, TrackId, TrackKind};

pub(super) const MAX_NODES: usize = 1024;
const MAX_RING_SAMPLES: usize = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Main,
    Bus(BusId),
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edge {
    track: TrackId,
    input: Option<BusId>,
    destination: Destination,
    arrival: usize,
    channels: usize,
    delay: usize,
}
#[derive(Default, Debug, PartialEq, Eq)]
struct Projection {
    edges: Vec<Edge>,
    buses: Vec<(BusId, usize)>,
    main: usize,
}

pub(super) struct OutputPdc {
    projection: Projection,
    lines: Vec<DelayLine>,
}
#[derive(Default)]
struct Reservations {
    #[cfg(test)]
    allocations: usize,
    failed: bool,
    #[cfg(test)]
    fail_after: Option<usize>,
}
fn reserve<T>(out: &mut Vec<T>, count: usize, reservations: &mut Reservations) -> Option<()> {
    if out.capacity() < count {
        #[cfg(test)]
        {
            reservations.allocations += 1;
        }
        #[cfg(test)]
        if let Some(remaining) = reservations.fail_after.as_mut() {
            if *remaining == 0 {
                reservations.fail_after = None;
                reservations.failed = true;
                return None;
            }
            *remaining -= 1;
        }
        if out.try_reserve_exact(count.checked_sub(out.len())?).is_err() {
            reservations.failed = true;
            return None;
        }
    }
    let _ = reservations;
    Some(())
}
fn reserved<T>(count: usize, reservations: &mut Reservations) -> Option<Vec<T>> {
    let mut out = Vec::new();
    reserve(&mut out, count, reservations)?;
    Some(out)
}

/// Reusable metadata storage, separate from the active projection and its live histories.
#[derive(Default)]
pub(super) struct Scratch {
    pub(super) raw: Vec<usize>,
    ranks: Vec<usize>,
    projection: Projection,
    rejected: Projection,
    rejected_valid: bool,
    metadata_refused: Option<(usize, usize)>,
    reservations: Reservations,
}
impl Scratch {
    pub(super) fn prepare_raw(&mut self, count: usize, buses: usize) -> Option<()> {
        self.raw.clear();
        if count > MAX_NODES {
            return None;
        }
        if self.metadata_refused.is_some_and(|layout| layout != (count, buses)) {
            self.metadata_refused = None;
        }
        if self.metadata_refused.is_some() {
            return None;
        }
        if reserve(&mut self.raw, count, &mut self.reservations).is_none() {
            self.metadata_refused = Some((count, buses));
            return None;
        }
        Some(())
    }
    pub(super) fn retry(&mut self) {
        self.metadata_refused = None;
        self.rejected_valid = false;
        self.reservations.failed = false;
    }
}
fn aux(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Aux | TrackKind::Folder)
}

impl OutputPdc {
    /// `latencies` follows session track order, with raw supported insert sums (not clipped).
    /// `order` is the mix engine's existing processing order, never another topological sorter.
    fn projection(session: &Session, order: &[usize], scratch: &mut Scratch, master: usize) -> Option<()> {
        let Scratch { raw: latencies, ranks, projection, reservations, .. } = scratch;
        let count = session.tracks.len().checked_add(session.busses.len())?;
        if count > MAX_NODES
            || latencies.len() != session.tracks.len()
            || master > MAX_LATENCY
            || session.outputs.first().is_some_and(|output| output.format.channels() > MAX_CHANNELS)
        {
            return None;
        }
        if session.tracks.iter().any(|track| track.mixer.sends.iter().any(Option::is_some) || track.instrument.is_some())
            || session.edit.values.keys().any(|key| key.starts_with("clip_fx."))
        {
            return None;
        }
        for (i, track) in session.tracks.iter().enumerate() {
            if session.tracks.iter().take(i).any(|prior| prior.id == track.id) {
                return None;
            }
        }
        for (i, bus) in session.busses.iter().enumerate() {
            if session.busses.iter().take(i).any(|prior| prior.id == bus.id) || bus.format.channels() > MAX_CHANNELS {
                return None;
            }
        }
        // Rank the existing order once; do not sort or derive another processing order.
        if order.len() != session.tracks.len() {
            return None;
        }
        ranks.clear();
        reserve(ranks, session.tracks.len(), reservations)?;
        ranks.resize(session.tracks.len(), usize::MAX);
        for (rank, &index) in order.iter().enumerate() {
            let slot = ranks.get_mut(index)?;
            if *slot != usize::MAX {
                return None;
            }
            *slot = rank;
        }
        // Check every bus reader against every possible output producer, including inactive
        // nodes, so cycles/order violations always select the whole legacy branch.
        for (consumer_index, consumer) in session.tracks.iter().enumerate().filter(|(_, track)| aux(track.kind)) {
            let Route::Bus(input) = consumer.mixer.input else { return None };
            if !session.busses.iter().any(|bus| bus.id == input) {
                return None;
            }
            let consumer_rank = *ranks.get(consumer_index)?;
            for (producer_index, producer) in session.tracks.iter().enumerate() {
                if !matches!(producer.kind, TrackKind::Master | TrackKind::Vca)
                    && producer.mixer.output == Route::Bus(input)
                    && *ranks.get(producer_index)? >= consumer_rank
                {
                    return None;
                }
            }
        }
        let Projection { edges, buses, .. } = projection;
        buses.clear();
        reserve(buses, session.busses.len(), reservations)?;
        buses.extend(session.busses.iter().map(|bus| (bus.id, 0usize)));
        edges.clear();
        reserve(edges, session.tracks.len(), reservations)?;
        let mut main = 0;
        for &index in order {
            let track = session.tracks.get(index)?;
            if track.inactive || matches!(track.kind, TrackKind::Master | TrackKind::Vca) {
                continue;
            }
            let channels = super::strip_channels(track, super::main_channels(session));
            if track.channels() > MAX_CHANNELS || channels == 0 || channels > MAX_CHANNELS {
                return None;
            }
            let input = if aux(track.kind) {
                match track.mixer.input {
                    Route::Bus(id) => Some(id),
                    _ => return None,
                }
            } else {
                None
            };
            let upstream = match input {
                Some(id) => buses.iter().find(|(bus, _)| *bus == id)?.1,
                None => 0,
            };
            let arrival = upstream.checked_add(*latencies.get(index)?)?;
            if arrival > MAX_LATENCY {
                return None;
            }
            let destination = match track.mixer.output {
                Route::Main => {
                    main = main.max(arrival);
                    Destination::Main
                }
                Route::Bus(id) => {
                    let (_, join) = buses.iter_mut().find(|(bus, _)| *bus == id)?;
                    *join = (*join).max(arrival);
                    Destination::Bus(id)
                }
                Route::None | Route::Hardware(_) => Destination::None,
            };
            edges.push(Edge { track: track.id, input, destination, arrival, channels, delay: 0 });
        }
        let mut samples = 0usize;
        for edge in edges.iter_mut() {
            let join = match edge.destination {
                Destination::Main => main,
                Destination::Bus(id) => buses.iter().find(|(bus, _)| *bus == id)?.1,
                Destination::None => edge.arrival,
            };
            edge.delay = join.checked_sub(edge.arrival)?;
            samples = samples.checked_add(edge.delay.checked_mul(edge.channels)?)?;
            if samples > MAX_RING_SAMPLES {
                return None;
            }
        }
        let main = main.checked_add(master)?;
        if main > MAX_LATENCY {
            return None;
        }
        projection.main = main;
        Some(())
    }
    fn storage(projection: &Projection, reservations: &mut Reservations) -> Option<Vec<DelayLine>> {
        let mut lines = reserved(projection.edges.len(), reservations)?;
        for edge in &projection.edges {
            let mut bufs = reserved(edge.channels, reservations)?;
            for _ in 0..edge.channels {
                let mut channel = reserved(edge.delay, reservations)?;
                channel.resize(edge.delay, 0.0);
                bufs.push(channel);
            }
            lines.push(DelayLine { bufs, idx: 0, len: edge.delay });
        }
        Some(lines)
    }
    fn replacement(scratch: &mut Scratch) -> Option<Self> {
        // Keep both active and scratch metadata buffers warm. Reconfiguration allocates a
        // replacement only when the projection really changes, never on an equal block.
        let mut projection = Projection::default();
        reserve(&mut projection.edges, scratch.projection.edges.capacity().min(MAX_NODES), &mut scratch.reservations)?;
        reserve(&mut projection.buses, scratch.projection.buses.capacity().min(MAX_NODES), &mut scratch.reservations)?;
        let lines = Self::storage(&scratch.projection, &mut scratch.reservations)?;
        std::mem::swap(&mut projection, &mut scratch.projection);
        Some(Self { projection, lines })
    }
    /// Identical projections retain all histories and metadata capacities without allocation.
    pub(super) fn update(current: &mut Option<Self>, session: &Session, order: &[usize], scratch: &mut Scratch, master: Option<usize>) -> bool {
        if scratch.metadata_refused.is_some() {
            return current.take().is_some();
        }
        scratch.reservations.failed = false;
        if master.and_then(|master| Self::projection(session, order, scratch, master)).is_some() {
            if current.as_ref().is_some_and(|old| old.projection == scratch.projection) {
                return false;
            }
            if scratch.rejected_valid && scratch.rejected == scratch.projection {
                return current.take().is_some();
            }
            // Reserve and record the exact rejection identity BEFORE attempting any ring storage.
            // A failed ring/spare allocation can therefore remain on legacy without retries on
            // identical blocks, while a changed topology/delay is eligible for another attempt.
            scratch.rejected.edges.clear();
            scratch.rejected.buses.clear();
            if reserve(&mut scratch.rejected.edges, scratch.projection.edges.len(), &mut scratch.reservations).is_none()
                || reserve(&mut scratch.rejected.buses, scratch.projection.buses.len(), &mut scratch.reservations).is_none()
            {
                scratch.metadata_refused = Some((session.tracks.len(), session.busses.len()));
                return current.take().is_some();
            }
            scratch.rejected.edges.extend_from_slice(&scratch.projection.edges);
            scratch.rejected.buses.extend_from_slice(&scratch.projection.buses);
            scratch.rejected.main = scratch.projection.main;
            scratch.rejected_valid = true;
            let replacement = Self::replacement(scratch);
            if replacement.is_some() {
                scratch.rejected_valid = false;
            }
            let changed = current.is_some() || replacement.is_some();
            *current = replacement;
            changed
        } else {
            // If even metadata storage is refused, retry only at a node-layout change or explicit reset boundary.
            // No rejection-key allocation is possible in that condition.
            if scratch.reservations.failed {
                scratch.metadata_refused = Some((session.tracks.len(), session.busses.len()));
            }
            current.take().is_some()
        }
    }
    pub(super) fn latency(&self) -> usize {
        self.projection.main
    }
    pub(super) fn bus_latency(&self, bus: BusId) -> usize {
        self.projection.buses.iter().find(|(id, _)| *id == bus).map_or(0, |(_, latency)| *latency)
    }
    pub(super) fn process(&mut self, track: TrackId, audio: &mut [Vec<f32>], frames: usize) {
        if let Some(index) = self.projection.edges.iter().position(|edge| edge.track == track)
            && let Some(line) = self.lines.get_mut(index)
        {
            line.process(audio, frames);
        }
    }
    pub(super) fn clear(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use soundcraft_model::ChannelFormat;
    fn projection(s: &Session, order: &[usize], latencies: &[usize], master: usize) -> Option<()> {
        let mut scratch = Scratch::default();
        scratch.prepare_raw(latencies.len(), s.busses.len())?;
        scratch.raw.extend_from_slice(latencies);
        OutputPdc::projection(s, order, &mut scratch, master)
    }
    #[test]
    fn ring_budget_and_path_overflow_are_checked_without_allocating_rings() {
        let mut s = Session::default();
        s.outputs[0].format = ChannelFormat::Atmos916;
        s.add_track(TrackKind::Audio, ChannelFormat::Atmos916, None);
        s.add_track(TrackKind::Audio, ChannelFormat::Atmos916, None);
        assert!(projection(&s, &[0, 1], &[0, MAX_LATENCY], 0).is_some());
        s.add_track(TrackKind::Audio, ChannelFormat::Atmos916, None);
        assert!(projection(&s, &[0, 1, 2], &[0, MAX_LATENCY, 0], 0).is_none());
        assert!(projection(&s, &[0, 1, 2], &[0, usize::MAX, 0], 0).is_none());
        assert!(projection(&s, &[0, 1, 2], &[0, 0, 0], MAX_LATENCY + 1).is_none());
    }
    #[test]
    fn ambiguous_track_and_bus_ids_refuse_projection() {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.tracks.push(s.tracks[0].clone());
        assert!(projection(&s, &[0, 1], &[0, 0], 0).is_none());
        s.tracks.pop();
        let bus = s.add_bus("Bus", ChannelFormat::Stereo);
        s.busses.push(s.busses.iter().find(|candidate| candidate.id == bus).unwrap().clone());
        assert!(projection(&s, &[0], &[0], 0).is_none());
    }
    #[test]
    fn warmed_projection_path_reuses_all_metadata_and_ring_allocations() {
        let mut s = Session::default();
        s.edit.delay_compensation = true;
        let bus = s.add_bus("Input", ChannelFormat::Stereo);
        let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Direct"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let mut insert = soundcraft_model::Insert::new("compressor");
        insert.params.insert("lookahead".into(), 0.0625);
        s.track_mut(aux).unwrap().mixer.inserts[0] = Some(insert);
        let mut mix = super::super::MixEngine::new(48_000.0, 16);
        let mut audio = vec![vec![0.0; 16]; 2];
        mix.render(&s, 0, 16, &mut audio);
        assert!(mix.output_pdc.is_some());
        let allocations = mix.output_pdc_scratch.reservations.allocations;
        assert!(allocations > 0, "the counter observes real capacity-growing reservation sites");
        let pointers = {
            let scratch = &mix.output_pdc_scratch;
            (scratch.raw.as_ptr(), scratch.ranks.as_ptr(), scratch.projection.edges.as_ptr(), scratch.projection.buses.as_ptr())
        };
        let lines = mix.output_pdc.as_ref().unwrap().lines.as_ptr();
        for block in 1..65 {
            mix.render(&s, block * 16, 16, &mut audio);
        }
        let scratch = &mix.output_pdc_scratch;
        assert_eq!(scratch.reservations.allocations, allocations, "no new PDC allocation site runs after warmup");
        assert_eq!((scratch.raw.as_ptr(), scratch.ranks.as_ptr(), scratch.projection.edges.as_ptr(), scratch.projection.buses.as_ptr()), pointers);
        assert_eq!(mix.output_pdc.as_ref().unwrap().lines.as_ptr(), lines);
        // The whole-legacy fallback also retains the warmed scratch rather than rebuilding it.
        let mut send = soundcraft_model::SendSlot::new(Route::Bus(bus));
        send.mute = true;
        s.track_mut(source).unwrap().mixer.sends[0] = Some(send);
        mix.render(&s, 65 * 16, 16, &mut audio);
        assert!(mix.output_pdc.is_none());
        let allocations = mix.output_pdc_scratch.reservations.allocations;
        for block in 66..100 {
            mix.render(&s, block * 16, 16, &mut audio);
        }
        assert_eq!(mix.output_pdc_scratch.reservations.allocations, allocations);
    }

    #[test]
    fn refused_ring_projection_stays_on_legacy_until_the_exact_projection_changes() {
        let mut s = Session::default();
        s.edit.delay_compensation = true;
        let bus = s.add_bus("Input", ChannelFormat::Stereo);
        let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Direct"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let mut insert = soundcraft_model::Insert::new("compressor");
        insert.params.insert("lookahead".into(), 0.125);
        s.track_mut(aux).unwrap().mixer.inserts[0] = Some(insert);
        let mut mix = super::super::MixEngine::new(48_000.0, 16);
        let mut audio = vec![vec![0.0; 16]; 2];
        mix.render(&s, 0, 16, &mut audio);
        assert!(mix.output_pdc.is_some());
        // Rejection metadata is already reserved. Allow spare metadata, line/buffer arrays and
        // the first delayed channel to allocate, then refuse the second channel's ring.
        let before = mix.output_pdc_scratch.reservations.allocations;
        mix.output_pdc_scratch.reservations.fail_after = Some(6);
        s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 0.1875);
        mix.render(&s, 16, 16, &mut audio);
        assert!(mix.output_pdc.is_none());
        assert!(mix.output_pdc_scratch.rejected_valid);
        assert!(mix.output_pdc_scratch.metadata_refused.is_none());
        assert_eq!(mix.output_pdc_scratch.reservations.allocations, before + 7);
        let requests = mix.output_pdc_scratch.reservations.allocations;
        for block in 2..66 {
            mix.render(&s, block * 16, 16, &mut audio);
        }
        assert_eq!(
            mix.output_pdc_scratch.reservations.allocations, requests,
            "a cached ring refusal cannot retry allocation or drop partial rings each callback"
        );
        // A smaller eligible delay is a real reconfiguration and may try storage again.
        s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 0.0625);
        mix.render(&s, 66 * 16, 16, &mut audio);
        assert!(mix.output_pdc.is_some());
        assert!(mix.output_pdc_scratch.reservations.allocations > requests);
    }

    #[test]
    fn metadata_refusal_retains_partial_capacity_and_retries_a_smaller_layout() {
        let mut s = Session::default();
        s.edit.delay_compensation = true;
        for _ in 0..4 {
            s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        }
        let mut mix = super::super::MixEngine::new(48_000.0, 16);
        mix.output_pdc_scratch.reservations.fail_after = Some(1);
        let mut audio = vec![vec![0.0; 16]; 2];
        mix.render(&s, 0, 16, &mut audio);
        assert!(mix.output_pdc.is_none());
        assert!(mix.output_pdc_scratch.metadata_refused.is_some());
        let pointer = mix.output_pdc_scratch.raw.as_ptr();
        let requests = mix.output_pdc_scratch.reservations.allocations;
        for block in 1..17 {
            mix.render(&s, block * 16, 16, &mut audio);
        }
        assert_eq!(mix.output_pdc_scratch.reservations.allocations, requests);
        assert_eq!(mix.output_pdc_scratch.raw.as_ptr(), pointer);
        s.tracks.pop();
        mix.render(&s, 17 * 16, 16, &mut audio);
        assert!(mix.output_pdc.is_some());
        assert_eq!(mix.output_pdc_scratch.raw.as_ptr(), pointer);
    }
}
