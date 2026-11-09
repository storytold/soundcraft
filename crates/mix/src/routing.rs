use super::{BusId, Route, Session, Track, TrackKind};
use std::collections::HashMap;
use std::sync::Arc;

/// Hash of everything `sync`/`processing_order` depend on (tracks, kinds, routing, plugins, sends).
pub(crate) fn structure_fingerprint(s: &Session) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for t in &s.tracks {
        t.id.hash(&mut h);
        t.kind.hash(&mut h);
        t.format.hash(&mut h);
        t.inactive.hash(&mut h);
        t.mixer.inserts.len().hash(&mut h);
        t.mixer.input.hash(&mut h);
        t.mixer.output.hash(&mut h);
        for i in &t.mixer.inserts {
            i.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        }
        for snd in t.mixer.sends.iter().flatten() {
            snd.target.hash(&mut h);
        }
        t.instrument.as_ref().map(|x| x.plugin.as_str()).hash(&mut h);
        for id in &t.midi_out {
            id.hash(&mut h);
        }
        let sample = t.instrument.as_ref().and_then(|x| x.sample);
        sample.hash(&mut h);
        // Which decoded audio it is, so new audio under the same id (another session) re-syncs.
        sample.and_then(|id| s.pool.get(id)).map(|a| Arc::as_ptr(a) as usize).hash(&mut h);
    }
    for b in &s.busses {
        b.id.hash(&mut h);
        b.format.hash(&mut h);
    }
    s.main_format().hash(&mut h);
    h.finish() as usize
}

/// How many tracks one track may feed, and how many sources one track may wait for.
const MAX_MIDI_DESTS: usize = 64;

/// Strips grouped into the order the real-time MIDI routing demands: a track that is fed MIDI
/// runs in a later group than its sources, so it hears their events in the same block. With no
/// routes there is one group, which is the plain parallel render. A cycle (only reachable from a
/// session whose routes predate the check) is broken by running what is left in one last group.
pub(crate) fn midi_levels(s: &Session, order: &[usize]) -> Vec<Vec<usize>> {
    if s.tracks.iter().all(|t| t.midi_out.is_empty()) {
        return vec![order.to_vec()];
    }
    // Track i waits for every track that routes MIDI to it (self-routes are ignored).
    let mut deps = vec![Vec::new(); s.tracks.len()];
    for (i, t) in s.tracks.iter().enumerate() {
        if t.inactive || !t.kind.is_midi() || t.instrument.is_none() {
            continue;
        }
        for &to in t.midi_out.iter().take(MAX_MIDI_DESTS) {
            let Some(j) = s.tracks.iter().position(|x| x.id == to) else { continue };
            if let Some(d) = deps.get_mut(j).filter(|d| i != j && d.len() < MAX_MIDI_DESTS) {
                d.push(i);
            }
        }
    }
    let mut done = vec![false; s.tracks.len()];
    let mut left: Vec<usize> = order.to_vec();
    let mut levels: Vec<Vec<usize>> = Vec::new();
    while !left.is_empty() {
        let ready: Vec<usize> =
            left.iter().copied().filter(|&i| deps.get(i).is_none_or(|d| d.iter().all(|&j| done.get(j).copied().unwrap_or(true)))).collect();
        if ready.is_empty() {
            // A cycle: run the rest together (their events then arrive one block later).
            levels.push(std::mem::take(&mut left));
            break;
        }
        for &i in &ready {
            if let Some(x) = done.get_mut(i) {
                *x = true;
            }
        }
        left.retain(|&i| !done.get(i).copied().unwrap_or(true));
        levels.push(ready);
    }
    levels
}

/// Audio/instrument tracks first, then auxes ordered so bus producers come before consumers.
pub(crate) fn processing_order(s: &Session) -> Vec<usize> {
    let mut order: Vec<usize> =
        (0..s.tracks.len()).filter(|&i| s.tracks.get(i).is_some_and(|t| !matches!(t.kind, TrackKind::Aux | TrackKind::Folder))).collect();
    let mut auxes: Vec<usize> =
        (0..s.tracks.len()).filter(|&i| s.tracks.get(i).is_some_and(|t| matches!(t.kind, TrackKind::Aux | TrackKind::Folder))).collect();
    if let Some(depths) = acyclic_aux_depths(s, &auxes) {
        // Stable sorting preserves session order among auxes at the same longest-path depth.
        auxes.sort_by_key(|&i| depths.get(i).copied().unwrap_or(0));
    } else {
        // A cycle keeps the existing bounded ordering for the entire graph, including any
        // independent acyclic branches. Feedback does not have a finite longest-path depth.
        let depths = bounded_aux_depths(s);
        auxes.sort_by_key(|&i| depths.get(i).copied().unwrap_or(0));
    }
    order.extend(auxes);
    order
}

/// Exact aux-hop depths when the whole Aux/Folder graph is acyclic. Bus joins retain only
/// producer counts and consumers, avoiding a producer-by-consumer adjacency matrix.
fn acyclic_aux_depths(s: &Session, auxes: &[usize]) -> Option<Vec<usize>> {
    #[derive(Default)]
    struct BusDependencies {
        remaining: usize,
        depth: usize,
        last_producer: Option<usize>,
        last_processed: Option<usize>,
        consumers: Vec<usize>,
    }
    let mut busses = HashMap::<BusId, BusDependencies>::new();
    for &i in auxes {
        for bus in aux_output_busses(s.tracks.get(i)?) {
            let dependency = busses.entry(bus).or_default();
            // Output and multiple sends from this producer may name the same bus.
            if dependency.last_producer != Some(i) {
                dependency.last_producer = Some(i);
                dependency.remaining = dependency.remaining.checked_add(1)?;
            }
        }
    }
    let mut depths = vec![0usize; s.tracks.len()];
    let mut ready = Vec::with_capacity(auxes.len());
    for &i in auxes {
        if let Route::Bus(bus) = &s.tracks.get(i)?.mixer.input {
            let dependency = busses.entry(*bus).or_default();
            dependency.consumers.push(i);
            if dependency.remaining > 0 {
                continue;
            }
        }
        ready.push(i);
    }
    let mut processed = 0usize;
    while let Some(&i) = ready.get(processed) {
        processed = processed.checked_add(1)?;
        let next_depth = depths.get(i)?.checked_add(1)?;
        for bus in aux_output_busses(s.tracks.get(i)?) {
            let dependency = busses.get_mut(&bus)?;
            if dependency.last_processed == Some(i) {
                continue;
            }
            dependency.last_processed = Some(i);
            dependency.depth = dependency.depth.max(next_depth);
            dependency.remaining = dependency.remaining.checked_sub(1)?;
            if dependency.remaining == 0 {
                for &consumer in &dependency.consumers {
                    *depths.get_mut(consumer)? = dependency.depth;
                    ready.push(consumer);
                }
            }
        }
    }
    (processed == auxes.len()).then_some(depths)
}

fn aux_output_busses(t: &Track) -> impl Iterator<Item = BusId> + '_ {
    std::iter::once(&t.mixer.output)
        .chain(t.mixer.sends.iter().flatten().map(|send| &send.target))
        .filter_map(|route| if let Route::Bus(bus) = route { Some(*bus) } else { None })
}

/// Hop cap for the bounded depth used when the aux graph has a cycle.
const MAX_BOUNDED_DEPTH: usize = 8;

/// For each track, the bounded depth used for a cyclic graph: the length of the longest chain of
/// Aux/Folder feeders into it (through a bus its input reads and the feeder's output or a send
/// writes), capped at [`MAX_BOUNDED_DEPTH`] hops. One pass per hop over the tracks and their bus
/// writes, so the cost is linear in the routing, not a fresh feeder search per track per
/// sort comparison.
fn bounded_aux_depths(s: &Session) -> Vec<usize> {
    let is_aux = |t: &Track| matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
    // Tracks with a feeder chain of the current length ending at them (length 0: every aux).
    let mut reached: Vec<bool> = s.tracks.iter().map(is_aux).collect();
    let mut depths = vec![0usize; s.tracks.len()];
    let mut fed = std::collections::HashSet::<BusId>::new();
    for hop in 1..=MAX_BOUNDED_DEPTH {
        fed.clear();
        for (t, _) in s.tracks.iter().zip(&reached).filter(|(t, r)| **r && is_aux(t)) {
            fed.extend(aux_output_busses(t));
        }
        let mut any = false;
        for ((t, r), d) in s.tracks.iter().zip(reached.iter_mut()).zip(depths.iter_mut()) {
            *r = matches!(&t.mixer.input, Route::Bus(b) if fed.contains(b));
            if *r {
                *d = hop;
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    depths
}

/// Implicit solo: an aux is not muted when a soloed track feeds its input bus.
pub(crate) fn receives_solo(s: &Session, t: &Track) -> bool {
    let Route::Bus(b) = &t.mixer.input else { return false };
    s.tracks
        .iter()
        .any(|o| o.mixer.solo && (o.mixer.output == Route::Bus(*b) || o.mixer.sends.iter().flatten().any(|sn| sn.target == Route::Bus(*b))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod processing_order_tests {
    use super::*;
    use soundcraft_model::ChannelFormat;
    use soundcraft_model::SendSlot;

    #[test]
    fn acyclic_bus_joins_use_longest_depth_and_keep_session_order_for_ties() {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let consumer = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let first_root = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let second_root = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let middle = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, None);
        let first_bus = s.add_bus("First", ChannelFormat::Stereo);
        let join = s.add_bus("Join", ChannelFormat::Stereo);
        s.track_mut(consumer).unwrap().mixer.input = Route::Bus(join);
        let root = s.track_mut(first_root).unwrap();
        root.mixer.output = Route::Main;
        root.mixer.sends[0] = Some(SendSlot::new(Route::Bus(first_bus)));
        let root = s.track_mut(second_root).unwrap();
        root.mixer.output = Route::Bus(join);
        root.mixer.sends[0] = Some(SendSlot::new(Route::Bus(join)));
        root.mixer.sends[1] = Some(SendSlot::new(Route::Bus(join)));
        let folder = s.track_mut(middle).unwrap();
        folder.mixer.input = Route::Bus(first_bus);
        folder.mixer.output = Route::Bus(join);
        folder.mixer.sends[0] = Some(SendSlot::new(Route::Bus(join)));

        assert_eq!(processing_order(&s), vec![0, 2, 3, 4, 1]);
    }

    #[test]
    fn a_cycle_preserves_bounded_order_for_the_whole_graph() {
        let mut s = Session::default();
        let busses: Vec<_> = (0..10).map(|i| s.add_bus(&format!("Chain {i}"), ChannelFormat::Stereo)).collect();
        let tracks: Vec<_> = (0..10).map(|_| s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None)).collect();
        for (i, id) in tracks.iter().enumerate() {
            let track = s.track_mut(*id).unwrap();
            track.mixer.input = Route::Bus(busses[i]);
            track.mixer.output = if i == 0 { Route::Main } else { Route::Bus(busses[i - 1]) };
        }
        assert_eq!(processing_order(&s), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        let cycle_bus = s.add_bus("Feedback", ChannelFormat::Stereo);
        let cycle = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
        let track = s.track_mut(cycle).unwrap();
        track.mixer.input = Route::Bus(cycle_bus);
        track.mixer.output = Route::Bus(cycle_bus);

        // Both deepest independent returns tie with the cycle at the legacy eight-hop cap.
        assert_eq!(processing_order(&s), vec![9, 8, 7, 6, 5, 4, 3, 2, 0, 1, 10]);
    }

    /// The per-track feeder search `bounded_aux_depths` replaced, kept as its reference.
    fn reference_depth(s: &Session, i: usize) -> usize {
        let mut d = 0;
        let mut cur = vec![i];
        for _ in 0..MAX_BOUNDED_DEPTH {
            let inputs: Vec<&Route> = cur.iter().filter_map(|&k| s.tracks.get(k)).map(|t| &t.mixer.input).collect();
            let feeders: Vec<usize> = s
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| matches!(t.kind, TrackKind::Aux | TrackKind::Folder))
                .filter(|(_, t)| {
                    inputs
                        .iter()
                        .any(|r| matches!(r, Route::Bus(_)) && (&&t.mixer.output == r || t.mixer.sends.iter().flatten().any(|sn| &&sn.target == r)))
                })
                .map(|(i, _)| i)
                .collect();
            if feeders.is_empty() {
                break;
            }
            d += 1;
            cur = feeders;
        }
        d
    }

    #[test]
    fn bounded_depths_match_the_per_track_feeder_search() {
        // A small pseudo-random graph of auxes, folders and audio tracks with cycles, joins,
        // sends and unrouted inputs.
        let mut s = Session::default();
        let busses: Vec<_> = (0..6).map(|i| s.add_bus(&format!("B{i}"), ChannelFormat::Stereo)).collect();
        let mut x = 12345u32;
        let mut next = |m: usize| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (x >> 16) as usize % m
        };
        for i in 0..30 {
            let kind = [TrackKind::Aux, TrackKind::Folder, TrackKind::Audio][next(3)];
            let id = s.add_track(kind, ChannelFormat::Stereo, None);
            let pick = |n: usize| if n < busses.len() { Route::Bus(busses[n]) } else { Route::Main };
            let (input, output, send) = (pick(next(8)), pick(next(8)), pick(next(8)));
            let t = s.track_mut(id).unwrap();
            t.mixer.input = input;
            t.mixer.output = output;
            if i % 2 == 0 {
                t.mixer.sends[0] = Some(SendSlot::new(send));
            }
        }
        let depths = bounded_aux_depths(&s);
        for (i, (t, depth)) in s.tracks.iter().zip(&depths).enumerate() {
            if matches!(t.kind, TrackKind::Aux | TrackKind::Folder) {
                assert_eq!(*depth, reference_depth(&s, i), "track {i}");
            }
        }
        assert!(depths.contains(&MAX_BOUNDED_DEPTH));
    }

    #[test]
    fn a_large_cyclic_graph_orders_quickly() {
        // Thousands of auxes all reading and writing one bus: every aux feeds every other.
        let mut s = Session::default();
        let bus = s.add_bus("Loop", ChannelFormat::Stereo);
        for _ in 0..3000 {
            let id = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
            let t = s.track_mut(id).unwrap();
            t.mixer.input = Route::Bus(bus);
            t.mixer.output = Route::Bus(bus);
            t.mixer.sends[0] = Some(SendSlot::new(Route::Bus(bus)));
        }
        let start = std::time::Instant::now();
        let order = processing_order(&s);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        assert_eq!(order, (0..3000).collect::<Vec<_>>());
    }
}
