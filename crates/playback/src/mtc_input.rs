//! Forward MIDI Time Code chase. The selected input assembles pieces. The audio callback
//! reads the latest position directly, and the UI mirrors transitions without replaying a backlog.
use soundcraft_time::{FrameRate, SampleRate, Samples, Timecode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    quarter: i64,
    rate: FrameRate,
}

impl Position {
    fn decode(hours: u8, minutes: u8, seconds: u8, frames: u8) -> Option<Self> {
        let rate = match hours >> 5 {
            0 => FrameRate::Fps24,
            1 => FrameRate::Fps25,
            2 => FrameRate::Fps29_97Drop,
            3 => FrameRate::Fps30,
            _ => return None,
        };
        let label = Timecode {
            negative: false,
            hours: i64::from(hours & 31),
            minutes: i64::from(minutes),
            seconds: i64::from(seconds),
            frames: i64::from(frames),
        };
        if label.hours > 23
            || minutes > 59
            || seconds > 59
            || label.frames >= rate.nominal()
            || (rate.is_drop() && !minutes.is_multiple_of(10) && seconds == 0 && frames < 2)
        {
            return None;
        }
        Some(Self { quarter: label.to_frames(rate) * 4, rate })
    }

    fn advance(self, quarters: i64) -> Self {
        Self { quarter: self.quarter.saturating_add(quarters), ..self }
    }

    fn quarters_per_day(self) -> i64 {
        Timecode { negative: false, hours: 24, minutes: 0, seconds: 0, frames: 0 }.to_frames(self.rate) * 4
    }

    // Wire hours wrap at midnight. A neighbouring group or locate (up to two frames)
    // carries the day forward or back. A non-contiguous explicit locate starts on day zero.
    fn on_reader_day(self, previous: Option<Self>, explicit_locate: bool) -> Self {
        let Some(last) = previous.filter(|p| p.rate == self.rate) else { return self };
        let day = self.quarters_per_day();
        let delta = self.quarter - last.quarter.rem_euclid(day);
        let nearest = (delta + day / 2).rem_euclid(day) - day / 2;
        if nearest.abs() <= 8 {
            last.advance(nearest)
        } else if explicit_locate {
            self
        } else {
            last.advance(delta)
        }
    }

    pub fn in_session_rate(self, session: FrameRate) -> Self {
        match (self.rate, session) {
            (FrameRate::Fps30, FrameRate::Fps29_97) | (FrameRate::Fps24, FrameRate::Fps23_976) => Self { rate: session, ..self },
            _ => self,
        }
    }

    pub fn samples(self, sr: SampleRate) -> Samples {
        if self.quarter % 4 == 0 {
            return self.rate.samples_at_frame(self.quarter / 4, sr);
        }
        (self.quarter as f64 * (self.rate.samples_per_frame(sr) / 4.)).round() as Samples
    }

    pub fn label(self) -> String {
        Timecode::from_frames(self.quarter.rem_euclid(self.quarters_per_day()) / 4, self.rate).format(self.rate)
    }

    fn frame_us(self) -> f64 {
        let (n, d) = self.rate.rational();
        1e6 * d as f64 / n as f64
    }
    pub fn frame_samples(self, sr: SampleRate) -> Samples {
        self.rate.samples_at_frame(1, sr)
    }
    /// A positive external offset advances the local transport against incoming code.
    pub fn session_sample(self, session_rate: FrameRate, sr: SampleRate, start: Samples, external_offset_samples: i64) -> Samples {
        self.in_session_rate(session_rate).samples(sr).saturating_sub(start).saturating_add(external_offset_samples)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChaseEvent {
    Locate(Position),
    Roll(Position, i64),
    Stop,
}

#[derive(Clone, Copy)]
pub(crate) struct Options {
    pub(crate) rate: FrameRate,
    pub(crate) freewheel: u8,
    pub(crate) jam: bool,
    pub(crate) offset: i64,
}
impl Default for Options {
    fn default() -> Self {
        Self { rate: FrameRate::Fps30, freewheel: 8, jam: false, offset: 0 }
    }
}
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct ChaseStats {
    #[serde(serialize_with = "serialize_incoming_timecode")]
    pub incoming_timecode: Option<Position>,
    pub incoming_rate: Option<&'static str>,
    pub rate_mismatch: Option<RateMismatch>,
    pub freewheel_state: &'static str,
    pub elapsed_frames: u64,
    pub limit_frames: u8,
    pub locks: u64,
    pub freewheel_entries: u64,
    pub losses: u64,
    pub hard_relocates: u64,
    pub input_jitter_p95_us: u32,
    pub arrival_delay_p95_us: u32,
}
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RateMismatch {
    pub incoming: &'static str,
    pub session: &'static str,
    pub speed_deviation_ppm: Option<i64>,
}
fn serialize_incoming_timecode<S: serde::Serializer>(p: &Option<Position>, serializer: S) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    p.map(Position::label).serialize(serializer)
}

#[derive(Clone, Copy)]
pub(crate) enum Tracking {
    Idle,
    Located(Position),
    Rolling { position: Position, stamp: i64, now: i64, anchor: Option<Position>, seek_generation: u64 },
}

/// A quarter-frame pair or a universal full-frame SysEx can span input callbacks.
/// System Real-Time bytes have no effect on the message being assembled.
#[derive(Default)]
struct WireMessage {
    bytes: [u8; 10],
    len: usize,
}

impl WireMessage {
    fn push(&mut self, byte: u8) -> Option<&[u8]> {
        match byte {
            0xfc => return Some(&[0xfc]),
            0xf8..=0xff => {}
            0xf0 | 0xf1 => {
                self.bytes[0] = byte;
                self.len = 1;
            }
            0xf7 if self.len == 9 && self.bytes[0] == 0xf0 => {
                self.bytes[9] = byte;
                self.len = 0;
                return Some(&self.bytes);
            }
            0x80..=0xf7 => self.len = 0,
            _ if self.len == 1 && self.bytes[0] == 0xf1 => {
                self.bytes[1] = byte;
                self.len = 0;
                return Some(&self.bytes[..2]);
            }
            _ if (1..9).contains(&self.len) => {
                self.bytes[self.len] = byte;
                self.len += 1;
            }
            _ => self.len = 0,
        }
        None
    }
}

#[derive(Default)]
pub(crate) struct Reader {
    wire: WireMessage,
    pub(crate) source: Option<u8>,
    pub(crate) options: Options,
    pub(crate) stats: ChaseStats,
    incoming: Option<Position>,
    // Absolute group position, mapped arrival, and the session interpretation for this window.
    speed_anchor: Option<(Position, i64, FrameRate)>,
    speed_mismatch: Option<RateMismatch>,
    parts: [u8; 8],
    next: u8,
    located_start: Option<Position>,
    first_anchor: Option<Position>,
    previous: Option<Position>,
    // While coasting, a timeline jump needs two consecutive groups at the new position.
    seek_candidate: Option<Position>,
    seek_generation: u64,
    locked: bool,
    coasting: bool,
    lost_code: bool,
    rolling: Option<(Position, i64)>,
    last_at: Option<i64>,
    last_group: Option<i64>,
    pub(crate) pending: Option<ChaseEvent>,
    fit: crate::sync_input::PulseFit<16>,
    arrival_delays: [u32; 32],
    arrival_count: usize,
    arrival_cursor: usize,
    jitter: [u32; 32],
    jitter_count: usize,
    jitter_index: usize,
}
// With at most 32 observations, the nearest-rank 95th percentile is either
// the largest value or the second largest (ties count as separate samples).
fn percentile95(window: &[u32; 32], count: usize) -> u32 {
    let samples = &window[..count];
    if samples.len() < 20 {
        return samples.iter().copied().max().unwrap_or(0);
    }
    let mut largest = 0;
    let mut second = 0;
    for &value in samples {
        second = second.max(largest.min(value));
        largest = largest.max(value);
    }
    second
}

impl Reader {
    pub(crate) fn reset(&mut self, source: Option<u8>) {
        self.speed_anchor = None;
        self.speed_mismatch = None;
        if self.source != source {
            self.incoming = None;
            self.arrival_count = 0;
            self.arrival_cursor = 0;
        }
        self.source = source;
        self.wire.len = 0;
        self.next = 0;
        self.located_start = None;
        self.first_anchor = None;
        self.previous = None;
        self.seek_candidate = None;
        self.locked = false;
        self.coasting = false;
        self.lost_code = false;
        self.rolling = None;
        self.last_at = None;
        self.last_group = None;
        self.pending = None;
        self.fit.clear();
        self.jitter_count = 0;
        self.jitter_index = 0;
    }
    pub(crate) fn set_options(&mut self, options: Options) {
        if (options.offset != self.options.offset || options.rate != self.options.rate)
            && !self.locked
            && let Some(p) = self.located_start
        {
            self.pending = Some(ChaseEvent::Locate(p));
        }
        self.options = options;
    }
    pub(crate) fn incoming_status(&self) -> (Option<Position>, Option<RateMismatch>) {
        let mismatch = self.incoming.and_then(|p| {
            self.source?;
            if p.in_session_rate(self.options.rate).rate != self.options.rate {
                Some(RateMismatch { incoming: p.rate.label(), session: self.options.rate.label(), speed_deviation_ppm: None })
            } else {
                self.speed_mismatch.filter(|m| m.session == self.options.rate.label())
            }
        });
        (self.incoming, mismatch)
    }
    #[cfg(test)]
    pub(crate) fn diagnostics(&self) -> ChaseStats {
        self.diagnostics_at(self.last_at.unwrap_or(0))
    }
    pub(crate) fn diagnostics_at(&self, now: i64) -> ChaseStats {
        let mut stats = self.stats;
        stats.freewheel_state = match (self.locked, self.coasting, self.options.jam, self.lost_code) {
            (true, true, true, _) => "jam sync running on internal clock",
            (true, true, false, _) => "freewheeling",
            (true, _, _, _) => "locked",
            (_, _, _, true) => "stopped: timecode lost",
            _ => "waiting",
        };
        stats.limit_frames = self.options.freewheel;
        if self.coasting
            && let (Some(last), Some((p, _))) = (self.last_at, self.rolling)
        {
            stats.elapsed_frames = (now.saturating_sub(last).max(0) as f64 / p.in_session_rate(self.options.rate).frame_us()) as u64;
        }
        stats.incoming_timecode = self.incoming;
        stats.incoming_rate = self.incoming.map(|p| if p.rate.is_drop() { "30df" } else { p.rate.label() });
        stats.rate_mismatch = self.incoming_status().1;
        stats.input_jitter_p95_us = percentile95(&self.jitter, self.jitter_count);
        stats.arrival_delay_p95_us = percentile95(&self.arrival_delays, self.arrival_count);
        stats
    }
    /// The host only needs notification when a transport transition is accepted.
    /// Steady quarter-frame updates stay available to the audio callback.
    pub(crate) fn receive(&mut self, bytes: &[u8], stamp: i64, arrival: i64) -> bool {
        let had_transport = self.locked || self.located_start.is_some();
        let located = self.located_start;
        let (locks, losses) = (self.stats.locks, self.stats.losses);
        let mut received = false;
        let mut stopped = false;
        // A transport Stop resets the chase state but must preserve the surrounding
        // wire message while this callback (or a later one) finishes it.
        let mut wire = std::mem::take(&mut self.wire);
        for &byte in bytes {
            if let Some(message) = wire.push(byte) {
                received = true;
                stopped |= message == [0xfc];
                self.read_message(message, stamp);
            }
        }
        self.wire = wire;
        if !received {
            return false;
        }
        let delay = arrival.saturating_sub(stamp).clamp(0, i64::from(u32::MAX)) as u32;
        self.arrival_delays[self.arrival_cursor] = delay;
        self.arrival_cursor = (self.arrival_cursor + 1) % self.arrival_delays.len();
        self.arrival_count = (self.arrival_count + 1).min(self.arrival_delays.len());
        self.stats.locks != locks
            || self.stats.losses != losses
            || (stopped && had_transport)
            || (self.located_start.is_some() && located != self.located_start)
    }
    pub(crate) fn loss(&mut self) {
        let stop = self.locked || self.pending == Some(ChaseEvent::Stop);
        if self.locked {
            self.stats.losses += 1;
        }
        self.reset(self.source);
        if stop {
            self.lost_code = true;
            self.pending = Some(ChaseEvent::Stop);
        }
    }
    fn freewheel(&mut self) {
        if self.locked && !self.coasting {
            self.coasting = true;
            // A plausible recovery can resume a few quarter frames off the old phase. Measure
            // the wire rate again from the first group accepted after the dropout.
            self.speed_anchor = None;
            self.stats.freewheel_entries += 1;
        }
    }
    fn expire(&mut self, now: i64) {
        let Some((p, _)) = self.rolling else { return };
        let frame_us = p.in_session_rate(self.options.rate).frame_us();
        // Accept a replacement group completing exactly four frames after the previous one.
        // Limit extending the deadline with partial or invalid groups to one group interval.
        let last = self.last_at.unwrap_or(now).min(self.last_group.unwrap_or(now).saturating_add((2. * frame_us).ceil() as i64));
        let elapsed = now.saturating_sub(last);
        if self.options.freewheel == 0 && !self.options.jam {
            // MTC verifies the full time every eight quarter frames (two frames). Allow a 10%
            // delivery margin, as MIDI Active Sensing does, so whole groups delivered in one
            // burst every two frames stay locked.
            if elapsed as f64 > frame_us * 2.2 {
                self.loss();
            }
            return;
        }
        if self.last_at.is_some_and(|last| now.saturating_sub(last) as f64 > frame_us / 2.) {
            self.freewheel();
        }
        if elapsed as f64 >= frame_us * f64::from(self.options.freewheel) && !self.options.jam {
            self.loss();
        }
    }
    #[cfg(test)]
    pub(crate) fn read(&mut self, bytes: &[u8], now: i64) {
        self.receive(bytes, now, now);
    }

    fn read_message(&mut self, bytes: &[u8], now: i64) {
        self.expire(now);
        match *bytes {
            [0xf0, 0x7f, device, 1, 1, h, m, s, f, 0xf7] if device < 128 => {
                if let Some(position) = Position::decode(h, m, s, f) {
                    let position = position.on_reader_day(self.incoming, true);
                    self.reset(self.source);
                    self.incoming = Some(position);
                    self.located_start = Some(position);
                    self.pending = Some(ChaseEvent::Locate(position));
                }
            }
            [0xfc] => {
                self.reset(self.source);
                self.pending = Some(ChaseEvent::Stop);
            }
            [0xf1, data] if data < 128 => {
                let piece = data >> 4;
                if piece != self.next {
                    self.discontinuity();
                    if piece != 0 {
                        return;
                    }
                }
                self.parts[usize::from(piece)] = data & 15;
                self.next = (piece + 1) % 8;
                if let (Some(last), Some((position, _))) = (self.last_at, self.rolling) {
                    let quarter_us = position.in_session_rate(self.options.rate).frame_us() / 4.;
                    self.jitter[self.jitter_index] = ((now.saturating_sub(last) as f64 - quarter_us).abs()).round() as u32;
                    self.jitter_index = (self.jitter_index + 1) % self.jitter.len();
                    self.jitter_count = (self.jitter_count + 1).min(self.jitter.len());
                }
                self.last_at = Some(now);
                if piece == 7 {
                    let p = self.parts;
                    if p[1] > 1 || p[3] > 3 || p[5] > 3 || p[7] > 7 {
                        self.discontinuity();
                        return;
                    }
                    let Some(position) = Position::decode(p[6] | p[7] << 4, p[4] | p[5] << 4, p[2] | p[3] << 4, p[0] | p[1] << 4) else {
                        self.discontinuity();
                        return;
                    };
                    // A still-locked reader can miss a whole group at midnight. Use elapsed
                    // quarter frames to recognize that forward wrap while the reader coasts.
                    let expected = self.rolling.filter(|(live, _)| self.coasting && live.rate == position.rate).map(|(live, stamp)| {
                        let elapsed = now.saturating_sub(stamp) as f64;
                        let quarters = (elapsed * 4. / live.in_session_rate(self.options.rate).frame_us()).round() as i64;
                        live.advance(quarters.saturating_sub(7))
                    });
                    let position = expected
                        .and_then(|anchor| {
                            let candidate = position.on_reader_day(Some(anchor), false);
                            (candidate.quarter.abs_diff(anchor.quarter) <= 4).then_some(candidate)
                        })
                        .unwrap_or_else(|| position.on_reader_day(self.previous.or(self.located_start), false));
                    self.incoming = Some(position);
                    if let Some(last) = self.previous {
                        // A missing group during lock is a dropout, not a seek.
                        // A valid pair at a new position can also re-establish the timeline.
                        let contiguous = last.advance(8) == position;
                        let plausible = self.rolling.is_some_and(|(live, stamp)| {
                            if live.rate != position.rate {
                                return false;
                            }
                            let elapsed = now.saturating_sub(stamp) as f64;
                            let expected = (elapsed * 4. / live.in_session_rate(self.options.rate).frame_us()).round() as i64;
                            live.advance(expected).quarter.abs_diff(position.advance(7).quarter) <= 4
                        });
                        let far_recovery = self.locked && self.coasting && !plausible;
                        let accepted = if far_recovery {
                            self.seek_candidate.is_some_and(|candidate| candidate.advance(8) == position)
                        } else {
                            contiguous || (self.locked && self.coasting && plausible)
                        };
                        if accepted {
                            if far_recovery {
                                self.seek_generation = self.seek_generation.wrapping_add(1);
                                self.fit.clear();
                            }
                            self.seek_candidate = None;
                            if !contiguous {
                                self.fit.clear();
                            }
                            if self.locked {
                                self.located_start = None;
                            }
                            self.first_anchor = if self.locked { None } else { self.located_start };
                            if !self.locked {
                                self.stats.locks += 1;
                            }
                            self.measure_wire_speed(position, now);
                            self.lost_code = false;
                            self.locked = true;
                            self.coasting = false;
                            self.last_group = Some(now);
                            let live = position.advance(7);
                            self.fit.push(live.quarter, now);
                            let stamp = self.fit.predict_us(live.quarter).unwrap_or(now).min(now);
                            self.rolling = Some((live, stamp));
                            self.pending = Some(ChaseEvent::Roll(live, stamp));
                        } else {
                            self.discontinuity();
                            if self.locked {
                                self.seek_candidate = Some(position);
                            }
                        }
                    } else if let Some(start) = self.located_start {
                        // A sender can label the first group at the locate or up to two frames ahead.
                        if start.rate != position.rate || !(0..=8).contains(&(position.quarter - start.quarter)) {
                            self.located_start = None;
                        }
                    }
                    self.previous = Some(position);
                }
            }
            _ => {}
        }
    }

    fn measure_wire_speed(&mut self, position: Position, now: i64) {
        let rate = self.options.rate;
        let anchor = (position, now, rate);
        let Some((start, time, old_rate)) = self.speed_anchor else {
            self.speed_anchor = Some(anchor);
            return;
        };
        let elapsed = now.saturating_sub(time);
        if !matches!(position.rate, FrameRate::Fps24 | FrameRate::Fps30)
            || rate != old_rate
            || position.rate != start.rate
            || position.quarter < start.quarter
            || elapsed < 0
        {
            self.speed_anchor = Some(anchor);
            self.speed_mismatch = None;
            return;
        }
        // Ten seconds bounds the error from 2.4 ms peak-to-peak arrival jitter to 240 ppm.
        if elapsed < 10_000_000 {
            return;
        }
        self.speed_anchor = Some(anchor);
        let position_us = (position.quarter - start.quarter) as f64 * position.in_session_rate(rate).frame_us() / 4.;
        let ppm = ((position_us / elapsed as f64 - 1.) * 1_000_000.).round() as i64;
        let candidate = match (rate, ppm) {
            (FrameRate::Fps23_976, 501..) => Some("24"),
            (FrameRate::Fps29_97, 501..) => Some("30"),
            (FrameRate::Fps24, ..=-501) => Some("23.976"),
            (FrameRate::Fps30, ..=-501) => Some("29.97"),
            _ => None,
        };
        self.speed_mismatch = candidate.map(|incoming| RateMismatch { incoming, session: rate.label(), speed_deviation_ppm: Some(ppm) });
    }

    /// A full-frame locate is stationary. Recent quarter frames can start rolling the
    /// callback before the host sees a complete group, or an established transport.
    pub(crate) fn transport_in_motion(&self, now: i64) -> bool {
        // Before the first complete group, even its rate bits have not arrived yet.
        // Eight quarter frames at 24000/1001 fps cover at most 83,417 microseconds.
        self.rolling.is_some() || self.last_at.is_some_and(|at| now.saturating_sub(at) <= 83_417)
    }

    pub(crate) fn tracking(&mut self, now: i64) -> Tracking {
        self.expire(now);
        if let Some((position, stamp)) = self.rolling {
            let anchor = self.first_anchor.filter(|_| now.saturating_sub(stamp) < 2_000_000 / position.rate.nominal());
            Tracking::Rolling { position, stamp, now, anchor, seek_generation: self.seek_generation }
        } else if let Some(p) = self.located_start {
            Tracking::Located(p)
        } else {
            Tracking::Idle
        }
    }

    fn discontinuity(&mut self) {
        self.seek_candidate = None;
        self.speed_anchor = None;
        self.speed_mismatch = None;
        if self.locked && self.options.freewheel == 0 && !self.options.jam {
            self.loss();
            return;
        }
        self.freewheel();
        self.located_start = None;
        self.first_anchor = None;
        self.next = 0;
        if !self.locked {
            self.previous = None;
        }
        self.fit.clear();
    }

    pub(crate) fn take(&mut self, now: i64) -> Option<ChaseEvent> {
        self.expire(now);
        self.pending.take().map(|e| match e {
            ChaseEvent::Roll(p, at) => {
                // Two complete groups confirm forward motion. Start at the corroborated locate
                // exactly once so its note-on enters the first audio block. If the consumer missed
                // the initial lock window, follow the live position instead of replaying old audio.
                let age = now.saturating_sub(at);
                if let Some(start) = self.located_start.take()
                    && age < 2_000_000 / p.rate.nominal()
                    && p.quarter >= start.quarter
                {
                    ChaseEvent::Roll(start, 0)
                } else {
                    ChaseEvent::Roll(p, age)
                }
            }
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiFeed;
    use std::sync::atomic::AtomicBool;

    pub(super) fn cycle(frame: u8, mut receive: impl FnMut(&[u8])) {
        for (i, nibble) in [frame & 15, frame >> 4, 0, 0, 0, 0, 0, 2].into_iter().enumerate() {
            receive(&[0xf1, (i as u8) << 4 | nibble]);
        }
    }

    #[test]
    fn full_frame_locate_matches_fractional_rate_session_start() {
        let rate = FrameRate::Fps29_97;
        let sr = SampleRate::default();
        let p = Position::decode(0x60, 0, 0, 2).unwrap();
        assert_eq!(p.session_sample(rate, sr, rate.samples_at_frame(2, sr), 0), 0);
    }

    #[test]
    fn arrival_delay_uses_the_selected_input_and_remains_readable_after_stop() {
        let mut reader = Reader::default();
        reader.reset(Some(1));
        for n in 0..32_i64 {
            reader.receive(&[0xf1, 0], n * 10_000, n * 10_000 + n * 100);
        }
        assert_eq!(reader.diagnostics().arrival_delay_p95_us, 3000);
        reader.receive(&[0xfc], 400_000, 400_010);
        assert_eq!(reader.diagnostics().arrival_delay_p95_us, 3000);
        reader.reset(Some(2));
        assert_eq!(reader.diagnostics().arrival_delay_p95_us, 0);
    }

    #[test]
    fn rolling_requires_two_sequential_forward_cycles_and_expires() {
        let mut reader = Reader::default();
        cycle(0, |b| reader.read(b, 0));
        assert_eq!(reader.take(1), None);
        cycle(2, |b| reader.read(b, 80_000));
        assert_eq!(reader.take(81_000), Some(ChaseEvent::Roll(Position { quarter: 15, rate: FrameRate::Fps25 }, 1000)));
        assert_eq!(reader.take(82_000), None);
        cycle(18, |b| reader.read(b, 160_000));
        assert_eq!(reader.take(160_001), None, "coast until the new location is corroborated");
        cycle(20, |b| reader.read(b, 240_000));
        assert!(matches!(reader.take(240_000), Some(ChaseEvent::Roll(_, 0))));
        assert_eq!(reader.take(560_000), Some(ChaseEvent::Stop));
        assert_eq!(reader.take(561_000), None);
        // A gap in an incomplete set cannot be hidden by an immediately following cycle.
        reader.read(&[0xf1, 0], 570_000);
        reader.read(&[0xf1, 0x20], 571_000);
        cycle(0, |b| reader.read(b, 580_000));
        assert_eq!(reader.take(580_001), None);
    }

    #[test]
    fn locate_only_anchors_the_matching_first_roll() {
        let start = Position { quarter: 0, rate: FrameRate::Fps25 };
        for (first, consume_late, expected) in [(0, false, 0), (2, false, 0), (12, false, 16), (2, true, 6)] {
            let mut reader = Reader::default();
            reader.read(&[0xf0, 0x7f, 127, 1, 1, 0x20, 0, 0, 0, 0xf7], 0);
            assert_eq!(reader.take(0), Some(ChaseEvent::Locate(start)));
            cycle(first, |b| reader.read(b, 1_000_000));
            assert_eq!(reader.take(1_000_000), None);
            cycle(first + 2, |b| reader.read(b, 1_080_000));
            let age = if consume_late { 100_000 } else { 0 };
            assert_eq!(
                reader.take(1_080_000 + age),
                Some(ChaseEvent::Roll(Position { quarter: if expected == 0 { 0 } else { expected * 4 - 1 }, ..start }, age))
            );
            cycle(first + 4, |b| reader.read(b, 1_200_000));
            assert_eq!(reader.take(1_200_000), Some(ChaseEvent::Roll(Position { quarter: i64::from(first + 6) * 4 - 1, ..start }, 0)));
        }
    }

    #[test]
    fn full_frame_validates_drop_labels_and_does_not_lock() {
        let mut reader = Reader::default();
        let msg = |frame| [0xf0, 0x7f, 0x7f, 1, 1, 0x40, 1, 0, frame, 0xf7];
        reader.read(&msg(1), 0);
        assert_eq!(reader.take(0), None);
        reader.read(&msg(2), 0);
        assert_eq!(reader.take(0), Some(ChaseEvent::Locate(Position { quarter: 7200, rate: FrameRate::Fps29_97Drop })));
        assert!(!reader.locked);
        for invalid in
            [[0xf0, 0x7f, 127, 1, 1, 24, 0, 0, 0, 0xf7], [0xf0, 0x7f, 127, 1, 1, 0, 60, 0, 0, 0xf7], [0xf0, 0x7f, 127, 1, 1, 0, 0, 0, 24, 0xf7]]
        {
            reader.read(&invalid, 0);
            assert_eq!(reader.take(0), None);
        }
    }

    #[test]
    fn all_wire_rates_roll_across_midnight_and_reverse_never_starts_playback() {
        for (bits, rate) in [(0, FrameRate::Fps24), (1, FrameRate::Fps25), (2, FrameRate::Fps29_97Drop), (3, FrameRate::Fps30)] {
            let mut reader = Reader::default();
            let send = |reader: &mut Reader, h: u8, m: u8, s: u8, f: u8, reverse: bool| {
                let pieces = [f & 15, f >> 4, s & 15, s >> 4, m & 15, m >> 4, h & 15, h >> 4 | bits << 1];
                for n in 0..8 {
                    let i = if reverse { 7 - n } else { n };
                    reader.read(&[0xf1, (i as u8) << 4 | pieces[i]], 0);
                }
            };
            send(&mut reader, 23, 59, 59, rate.nominal() as u8 - 2, false);
            send(&mut reader, 0, 0, 0, 0, false);
            let midnight = Timecode { negative: false, hours: 24, minutes: 0, seconds: 0, frames: 0 }.to_frames(rate);
            assert_eq!(reader.take(0), Some(ChaseEvent::Roll(Position { quarter: midnight * 4 + 7, rate }, 0)));
            send(&mut reader, 0, 0, 0, 0, true);
            assert_eq!(reader.take(500_000), Some(ChaseEvent::Stop));
            send(&mut reader, 23, 59, 59, rate.nominal() as u8 - 2, true);
            assert_eq!(reader.take(0), None);
        }
    }

    #[test]
    fn framing_selected_port_accepts_interleaved_realtime_and_split_quarter_frames() {
        let feed = MidiFeed::default();
        feed.set_mtc_source(Some(1));
        feed.set_clock(0);
        for status in [0xf8, 0xfa, 0xfc] {
            let locate = [0xf0, 0x7f, 0x7f, 1, 1, status, 0x61, 0, 0, 0, 0xf7];
            feed.receive(1, &locate);
            assert!(matches!(feed.take_mtc(), Some(ChaseEvent::Locate(p)) if p.label() == "01:00:00:00"), "real-time status {status:02x}");
        }
        // Each real-time event is independent of the adjacent quarter-frame data.
        cycle(0, |b| feed.receive(1, &[b[0], 0xf8, b[1]]));
        cycle(2, |b| {
            feed.receive(1, &[b[0]]);
            feed.receive(1, &[0xf8]);
            feed.receive(2, &[b[1] ^ 1]);
            feed.receive(1, &[b[1]]);
        });
        assert!(matches!(feed.take_mtc(), Some(ChaseEvent::Roll(p, _)) if p.rate == FrameRate::Fps25));
        assert_eq!(feed.activity().0, 0);
    }

    #[test]
    fn framing_ignores_incomplete_and_oversized_timecode_then_accepts_fresh_locate() {
        let mut reader = Reader::default();
        for status in 0xf8..=0xff {
            reader.read(&[0xf0, 0x7f, 0x7f, 1, 1, status], 0);
            for _ in 0..32 {
                reader.read(&[0, status, 0], 0);
            }
            reader.read(&[0xf7], 0);
            assert!(reader.located_start.is_none());
        }
        reader.read(&[0xf0, 0x7f, 0x7f, 1, 1, 0xf8], 0);
        reader.read(&[0x60, 0, 1, 0, 0xf7], 0);
        assert!(matches!(reader.take(0), Some(ChaseEvent::Locate(p)) if p.label() == "00:00:01:00"));
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn framing_generator_and_chase_keep_midnight_session_samples_continuous() {
        let sr = SampleRate::new(48_000).unwrap();
        for (rate, day) in [(FrameRate::Fps30, 2_592_000), (FrameRate::Fps29_97Drop, 2_589_408)] {
            let mut reader = Reader::default();
            let session_start = rate.samples_at_frame(day / 24, sr);
            let mut last = None;
            for (n, frame) in [day - 4, day - 2, day, day + 2].into_iter().enumerate() {
                for (i, bytes) in crate::mtc::quarters(frame, rate).into_iter().enumerate() {
                    reader.read(&bytes, ((n * 8 + i) as f64 * Position { quarter: 0, rate }.frame_us() / 4.).round() as i64);
                }
                if n == 0 {
                    continue;
                }
                let (position, _) = reader.rolling.expect("two contiguous forward groups lock");
                let sample = position.session_sample(rate, sr, session_start, 0);
                if let Some(previous) = last {
                    assert!(
                        sample > previous && (sample - previous).abs_diff(rate.samples_at_frame(2, sr)) <= 1,
                        "{rate:?} midnight: {previous} → {sample}"
                    );
                }
                last = Some(sample);
                if frame == day {
                    assert!(reader.incoming.unwrap().label().starts_with("00:00:00"), "generator and incoming display must wrap");
                }
            }
            assert_eq!((reader.stats.locks, reader.stats.losses, reader.stats.hard_relocates, reader.seek_generation), (1, 0, 0, 0));
            reader.read(&[0xf0, 0x7f, 127, 1, 1, if rate.is_drop() { 0x40 } else { 0x60 }, 10, 0, 0, 0xf7], 300_000);
            let Some(ChaseEvent::Locate(p)) = reader.take(300_000) else { panic!("explicit locate") };
            assert!(p.samples(sr) < rate.samples_at_frame(day, sr), "unrelated full-frame locate starts a new day");
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn short_dropout_across_midnight_keeps_the_locked_reader_day() {
        let rate = FrameRate::Fps25;
        let day = 24 * 3600 * 25;
        let mut reader = Reader::default();
        reader.options.rate = rate;
        for group in [0, 1, 3, 4] {
            let frame = day - 6 + group * 2;
            for (piece, bytes) in crate::mtc::quarters(frame, rate).into_iter().enumerate() {
                reader.read(&bytes, (group * 8 + piece as i64) * 10_000);
            }
            if group > 0 {
                let (pos, _) = reader.rolling.unwrap();
                assert_eq!(pos.quarter, frame * 4 + 7, "group {group} after the missing 23:59:59:23 block");
            }
        }
        assert_eq!((reader.stats.locks, reader.stats.losses, reader.seek_generation), (1, 0, 0));
    }

    #[test]
    fn framing_adjacent_full_frame_rewind_crosses_to_the_previous_day() {
        let mut reader = Reader::default();
        let locate = |reader: &mut Reader, h: u8, m: u8, s: u8, f: u8| {
            reader.read(&[0xf0, 0x7f, 127, 1, 1, 0x60 | h, m, s, f, 0xf7], 0);
            let Some(ChaseEvent::Locate(p)) = reader.take(0) else { panic!("full-frame locate") };
            p
        };
        let midnight = locate(&mut reader, 0, 0, 0, 0);
        let prev = locate(&mut reader, 23, 59, 59, 29);
        assert_eq!(prev.quarter, midnight.quarter - 4);
        assert_eq!(prev.label(), "23:59:59:29");
    }

    #[test]
    fn selected_feed_separates_chase_from_channel_recording_and_closed_callbacks() {
        let feed = MidiFeed::default();
        feed.set_mtc_source(Some(1));
        feed.set_clock(0);
        let gate = AtomicBool::new(true);
        cycle(0, |b| feed.receive(2, b));
        cycle(2, |b| feed.receive(1, b));
        assert_eq!(feed.take_mtc(), None);
        cycle(4, |b| feed.receive(1, b));
        assert!(matches!(feed.take_mtc(), Some(ChaseEvent::Roll(_, 0))));
        assert_eq!(feed.activity().0, 0);
        feed.receive(1, &[0x90, 60, 90]);
        assert_eq!(feed.activity().0, 1);
        feed.close_port(&gate, 1);
        cycle(6, |b| feed.receive_while_open(&gate, 1, b));
        assert_eq!(feed.take_mtc(), Some(ChaseEvent::Stop));
        assert_eq!(feed.take_mtc(), None);
    }
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod dropout_tests {
    use super::*;
    fn pulse(r: &mut Reader, i: i64) {
        let group = crate::mtc::quarters(2 * (i / 8), FrameRate::Fps30);
        r.read(&group[(i % 8) as usize], i * 1_000_000 / 120);
    }
    #[test]
    fn reader_reports_the_latest_absolute_incoming_timecode_and_rate() {
        let mut r = Reader::default();
        r.reset(Some(3));
        r.options.offset = 123_456;
        let first = 30 * (3600 + 120 + 3) + 4;
        for b in crate::mtc::quarters(first, FrameRate::Fps30) {
            r.read(&b, 0);
        }
        let d = r.diagnostics();
        assert_eq!(d.incoming_timecode.map(Position::label).as_deref(), Some("01:02:03:04"));
        assert_eq!(d.incoming_rate, Some("30"));
        r.read(&[0xf0, 0x7f, 127, 1, 1, 0x40, 1, 0, 2, 0xf7], 16);
        r.read(&[0xfc], 18);
        assert_eq!(r.diagnostics().incoming_timecode.map(Position::label).as_deref(), Some("00:01:00;02"));
        assert_eq!(r.diagnostics().incoming_rate, Some("30df"));
        r.reset(None);
        assert_eq!(r.diagnostics().incoming_timecode, None);
    }

    #[test]
    fn incompatible_rate_code_reports_a_mismatch() {
        let mut reader = Reader::default();
        reader.reset(Some(1));
        for message in crate::mtc::quarters(0, FrameRate::Fps25) {
            reader.read(&message, 0);
        }
        assert_eq!(reader.diagnostics().rate_mismatch.map(|m| (m.incoming, m.speed_deviation_ppm)), Some(("25", None)));
    }

    #[test]
    fn diagnostic_percentiles_match_sorted_prefixes_with_ties_and_outliers() {
        for seed in 0..64u32 {
            let values = std::array::from_fn(|i| match (i as u32 + seed) % 5 {
                0 => u32::MAX,
                1 | 2 => 0,
                _ => (i as u32 * 7919 + seed * 613) % 1009,
            });
            for count in 0..=values.len() {
                let mut expected = values[..count].to_vec();
                expected.sort_unstable();
                assert_eq!(percentile95(&values, count), expected.get((count * 95).div_ceil(100).wrapping_sub(1)).copied().unwrap_or(0));
            }
        }
    }

    fn read_with_jitter(reader: &mut Reader, source: FrameRate, group: i64) {
        let (num, den) = source.rational();
        for (piece, bytes) in crate::mtc::quarters(group * 2, source).into_iter().enumerate() {
            let time = ((group * 8 + piece as i64) * 1_000_000 * den) / (4 * num);
            reader.read(&bytes, time + if group % 2 == 0 { 1200 } else { -1200 });
        }
    }

    #[test]
    fn ntsc_speed_mismatch_is_reported_within_30_seconds() {
        for (session, source, sign) in [
            (FrameRate::Fps29_97, FrameRate::Fps30, 1),
            (FrameRate::Fps30, FrameRate::Fps29_97, -1),
            (FrameRate::Fps23_976, FrameRate::Fps24, 1),
            (FrameRate::Fps24, FrameRate::Fps23_976, -1),
        ] {
            let mut reader = Reader::default();
            reader.reset(Some(1));
            reader.options.rate = session;
            for group in 0..source.nominal() * 14 {
                read_with_jitter(&mut reader, source, group);
            }
            assert!(reader.locked);
            let warning = reader.diagnostics().rate_mismatch.expect("missed rate warning");
            assert_eq!((warning.incoming, warning.session), (source.label(), session.label()));
            let ppm = warning.speed_deviation_ppm.expect("compatible code has measured speed");
            assert!((750..=1250).contains(&(ppm * sign)), "{source:?} vs {session:?}: {ppm} ppm");
        }
    }

    #[test]
    fn a_late_recovery_after_coasting_does_not_report_a_rate_mismatch() {
        let source = FrameRate::Fps30;
        let mut reader = Reader::default();
        reader.reset(Some(1));
        reader.options.rate = source;
        reader.options.freewheel = 30;
        let quarter_us = 1_000_000 / 120;
        let feed = |reader: &mut Reader, group: i64, late: i64| {
            for (piece, bytes) in crate::mtc::quarters(group * 2, source).into_iter().enumerate() {
                reader.read(&bytes, (group * 8 + piece as i64) * quarter_us + late);
            }
        };
        for group in 0..75 {
            feed(&mut reader, group, 0);
        }
        // A dropout of ten groups; the source then resumes two quarter frames late.
        for group in 75..85 {
            reader.tracking(group * 8 * quarter_us);
        }
        assert_eq!(reader.stats.freewheel_entries, 1);
        for group in 85..270 {
            feed(&mut reader, group, 2 * quarter_us);
        }
        assert!(reader.locked);
        assert!(reader.diagnostics().rate_mismatch.is_none(), "{:?}", reader.diagnostics().rate_mismatch);
    }

    #[test]
    #[ignore = "ten simulated minutes for each of four rate settings"]
    fn matching_rate_with_jitter_never_reports_a_mismatch() {
        for source in [FrameRate::Fps24, FrameRate::Fps30, FrameRate::Fps23_976, FrameRate::Fps29_97] {
            let mut reader = Reader::default();
            reader.reset(Some(1));
            reader.options.rate = source;
            for group in 0..source.nominal() * 300 {
                read_with_jitter(&mut reader, source, group);
                assert!(reader.diagnostics().rate_mismatch.is_none());
            }
            assert!(reader.locked);
        }
    }

    #[test]
    fn none_stops_on_missing_or_discontinuous_code_without_coasting() {
        for discontinuity in [false, true] {
            let mut r = Reader::default();
            r.options.freewheel = 0;
            for i in 0..16 {
                pulse(&mut r, i);
            }
            assert!(matches!(r.tracking(150_000), Tracking::Rolling { .. }));
            assert_eq!(r.stats.freewheel_entries, 0);
            if discontinuity {
                r.read(&[0xf1, 0x30], 151_000);
            }
            if !discontinuity {
                assert!(matches!(r.tracking(198_333), Tracking::Rolling { .. }));
            }
            assert!(matches!(r.tracking(if discontinuity { 151_001 } else { 198_334 }), Tracking::Idle));
            assert_eq!(r.stats.freewheel_entries, 0);
            assert_eq!(r.take(198_335), Some(ChaseEvent::Stop));
        }
    }

    #[test]
    fn none_stays_locked_when_whole_groups_arrive_every_two_frames() {
        let mut r = Reader::default();
        r.options.freewheel = 0;
        for group in 0..12_u8 {
            let at = i64::from(group) * 80_000;
            // The audio callback may check tracking just before the MIDI callback delivers the group.
            if group > 1 {
                assert!(matches!(r.tracking(at + 5_000), Tracking::Rolling { .. }), "group {group}");
            }
            super::tests::cycle(2 * group, |b| r.read(b, at + 5_000));
            assert_ne!(r.take(at + 5_001), Some(ChaseEvent::Stop), "group {group}");
        }
        assert_eq!((r.stats.locks, r.stats.losses), (1, 0));
        assert!(matches!(r.tracking(11 * 80_000 + 5_000 + 88_000), Tracking::Rolling { .. }));
        assert!(matches!(r.tracking(11 * 80_000 + 5_000 + 88_001), Tracking::Idle));
    }

    #[test]
    fn jam_sync_runs_on_internal_clock_and_only_accepts_a_corroborated_seek() {
        let mut r = Reader::default();
        r.reset(Some(1));
        r.options.jam = true;
        assert!(matches!(r.tracking(10_000_000), Tracking::Idle));
        for i in 0..16 {
            pulse(&mut r, i);
        }
        r.take(125_000);
        let Tracking::Rolling { position, stamp, now, .. } = r.tracking(10_125_000) else { panic!("lost jam") };
        assert_eq!(now - stamp, 10_000_000);
        assert_eq!(position.quarter, 15);
        assert_eq!(r.diagnostics_at(now).freewheel_state, "jam sync running on internal clock");
        for i in 1216..1224 {
            pulse(&mut r, i);
        }
        let restored = 1223 * 1_000_000 / 120;
        assert_eq!(r.diagnostics_at(restored).freewheel_state, "locked");
        assert!(matches!(r.take(restored), Some(ChaseEvent::Roll(_, _))));
        // The next group claims it is a second ahead. Stay on the old timeline until the next group agrees.
        for piece in 0..8 {
            let b = crate::mtc::quarters(336, FrameRate::Fps30)[piece];
            r.read(&b, (1224 + piece as i64) * 1_000_000 / 120);
        }
        assert!(r.take(1231 * 1_000_000 / 120).is_none());
        for piece in 0..8 {
            let b = crate::mtc::quarters(338, FrameRate::Fps30)[piece];
            r.read(&b, (1232 + piece as i64) * 1_000_000 / 120);
        }
        assert!(matches!(r.take(1239*1_000_000/120),Some(ChaseEvent::Roll(p, _)) if p.quarter==338*4+7));
        assert_eq!(r.stats.locks, 1);
    }

    #[test]
    fn one_lost_quarter_does_not_stop_or_change_phase() {
        for missing in 16..24 {
            let mut r = Reader::default();
            r.options.freewheel = 4;
            for i in 0..48 {
                if i != missing {
                    pulse(&mut r, i);
                }
                assert_ne!(r.take(i * 1_000_000 / 120), Some(ChaseEvent::Stop), "missing {missing} at {i}");
            }
            assert_eq!(r.stats.locks, 1);
            assert_eq!(r.stats.freewheel_entries, 1);
            match r.tracking(47 * 1_000_000 / 120) {
                Tracking::Rolling { position, .. } => assert_eq!(position.quarter, 47),
                _ => panic!("lost lock"),
            }
        }
    }
    #[test]
    fn freewheel_at_configured_rate_and_jam_still_obey_explicit_stop() {
        for frames in [4, 8, 40] {
            let mut r = Reader::default();
            r.options.freewheel = frames;
            for i in 0..16 {
                pulse(&mut r, i);
            }
            let last = 125_000;
            let expiry = last + (i64::from(frames) * 1_000_000 + 29) / 30;
            assert!(matches!(r.tracking(expiry - 1), Tracking::Rolling { .. }));
            let state = r.diagnostics_at(expiry - 1);
            assert_eq!((state.freewheel_state, state.elapsed_frames, state.limit_frames), ("freewheeling", u64::from(frames - 1), frames));
            assert!(matches!(r.tracking(expiry), Tracking::Idle));
            assert_eq!(r.take(expiry), Some(ChaseEvent::Stop));
            assert_eq!(r.diagnostics_at(expiry).freewheel_state, "stopped: timecode lost");
            r.options.jam = true;
            for i in 0..16 {
                pulse(&mut r, i);
            }
            assert!(matches!(r.tracking(expiry + 10_000_000), Tracking::Rolling { .. }));
            r.read(&[0xfc], expiry + 10_000_001);
            assert_eq!(r.take(expiry + 10_000_002), Some(ChaseEvent::Stop));
        }
    }
}
