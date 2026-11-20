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
        let at_midnight = Timecode { negative: false, hours: 24, minutes: 0, seconds: 0, frames: 0 }.to_frames(self.rate);
        Self { quarter: (self.quarter + quarters) % (at_midnight * 4), ..self }
    }

    pub fn in_session_rate(self, session: FrameRate) -> Self {
        match (self.rate, session) {
            (FrameRate::Fps30, FrameRate::Fps29_97) | (FrameRate::Fps24, FrameRate::Fps23_976) => Self { rate: session, ..self },
            _ => self,
        }
    }

    pub fn samples(self, sr: SampleRate) -> Samples {
        (self.quarter as f64 * (self.rate.samples_per_frame(sr) / 4.)).round() as Samples
    }

    fn frame_us(self) -> f64 {
        let (n, d) = self.rate.rational();
        1e6 * d as f64 / n as f64
    }
    pub fn frame_samples(self, sr: SampleRate) -> Samples {
        self.rate.samples_at_frame(1, sr)
    }
    /// A positive external offset advances the local transport against incoming code.
    pub fn session_sample(self, session_rate: FrameRate, sr: SampleRate, start: Samples, external_offset_frames: i64) -> Samples {
        self.in_session_rate(session_rate).samples(sr).saturating_sub(start).saturating_add(session_rate.samples_at_frame(external_offset_frames, sr))
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
    pub locks: u64,
    pub freewheel_entries: u64,
    pub losses: u64,
    pub hard_relocates: u64,
    pub input_jitter_p95_us: u32,
}
#[derive(Clone, Copy)]
pub(crate) enum Tracking {
    Idle,
    Located(Position),
    Rolling { position: Position, stamp: i64, now: i64, anchor: Option<Position> },
}

#[derive(Default)]
pub(crate) struct Reader {
    pub(crate) source: Option<u8>,
    pub(crate) options: Options,
    pub(crate) stats: ChaseStats,
    parts: [u8; 8],
    next: u8,
    located_start: Option<Position>,
    first_anchor: Option<Position>,
    previous: Option<Position>,
    locked: bool,
    coasting: bool,
    rolling: Option<(Position, i64)>,
    last_at: Option<i64>,
    last_group: Option<i64>,
    pending: Option<ChaseEvent>,
    fit: crate::sync_input::PulseFit<16>,
    jitter: [u32; 32],
    jitter_count: usize,
    jitter_index: usize,
}
impl Reader {
    pub(crate) fn reset(&mut self, source: Option<u8>) {
        self.source = source;
        self.next = 0;
        self.located_start = None;
        self.first_anchor = None;
        self.previous = None;
        self.locked = false;
        self.coasting = false;
        self.rolling = None;
        self.last_at = None;
        self.last_group = None;
        self.pending = None;
        self.fit.clear();
        self.jitter_count = 0;
        self.jitter_index = 0;
    }
    pub(crate) fn diagnostics(&self) -> ChaseStats {
        let mut stats = self.stats;
        if self.jitter_count > 0 {
            let mut samples = self.jitter;
            samples[..self.jitter_count].sort_unstable();
            stats.input_jitter_p95_us = samples[(self.jitter_count * 95).div_ceil(100) - 1];
        }
        stats
    }
    pub(crate) fn loss(&mut self) {
        let stop = self.locked || self.pending == Some(ChaseEvent::Stop);
        if self.locked {
            self.stats.losses += 1;
        }
        self.reset(self.source);
        if stop {
            self.pending = Some(ChaseEvent::Stop);
        }
    }
    fn freewheel(&mut self) {
        if self.locked && !self.coasting {
            self.coasting = true;
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
        if self.last_at.is_some_and(|last| now.saturating_sub(last) as f64 > frame_us / 2.) {
            self.freewheel();
        }
        if elapsed as f64 >= frame_us * f64::from(self.options.freewheel) && !self.options.jam {
            self.loss();
        }
    }
    pub(crate) fn read(&mut self, bytes: &[u8], now: i64) {
        self.expire(now);
        match *bytes {
            [0xf0, 0x7f, device, 1, 1, h, m, s, f, 0xf7] if device < 128 => {
                if let Some(position) = Position::decode(h, m, s, f) {
                    self.reset(self.source);
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
                        if contiguous || (self.locked && self.coasting && plausible) {
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

    pub(crate) fn tracking(&mut self, now: i64) -> Tracking {
        self.expire(now);
        if let Some((position, stamp)) = self.rolling {
            let anchor = self.first_anchor.filter(|_| now.saturating_sub(stamp) < 2_000_000 / position.rate.nominal());
            Tracking::Rolling { position, stamp, now, anchor }
        } else if let Some(p) = self.located_start {
            Tracking::Located(p)
        } else {
            Tracking::Idle
        }
    }

    fn discontinuity(&mut self) {
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

    fn cycle(frame: u8, mut receive: impl FnMut(&[u8])) {
        for (i, nibble) in [frame & 15, frame >> 4, 0, 0, 0, 0, 0, 2].into_iter().enumerate() {
            receive(&[0xf1, (i as u8) << 4 | nibble]);
        }
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
            assert_eq!(reader.take(0), Some(ChaseEvent::Roll(Position { quarter: 7, rate }, 0)));
            send(&mut reader, 0, 0, 0, 0, true);
            assert_eq!(reader.take(500_000), Some(ChaseEvent::Stop));
            send(&mut reader, 23, 59, 59, rate.nominal() as u8 - 2, true);
            assert_eq!(reader.take(0), None);
        }
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
            assert!(matches!(r.tracking(expiry), Tracking::Idle));
            assert_eq!(r.take(expiry), Some(ChaseEvent::Stop));
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
