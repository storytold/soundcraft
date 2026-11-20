//! Forward MIDI Time Code chase. The selected input's callback assembles pieces; the UI
//! consumes the newest complete position, so a busy frame cannot replay a backlog of locates.
use soundcraft_time::{FrameRate, SampleRate, Samples, Timecode};

/// One bounded acquisition step per forward group. Shared with the silent UI transport.
pub fn chase_rate(error: i64, tolerance: i64, sr: SampleRate, speed: f64) -> f64 {
    let trim = if error.unsigned_abs() <= (tolerance / 48) as u64 { 0. } else { (error as f64 / sr.as_f64()).clamp(-0.02, 0.04) };
    (1. + trim).min(speed.max(1.) + 0.002)
}

const TIMEOUT_US: i64 = 300_000;

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

    pub fn drift_tolerance(self, sr: SampleRate) -> Samples {
        self.rate.samples_at_frame(3, sr)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChaseEvent {
    Locate(Position),
    Roll(Position, i64),
    Stop,
}

#[derive(Default)]
pub(crate) struct Reader {
    pub(crate) source: Option<u8>,
    parts: [u8; 8],
    next: u8,
    // Candidate for the first playback block only. Stopped locates can precede play by minutes.
    located_start: Option<Position>,
    previous: Option<Position>,
    locked: bool,
    rolling: Option<(Position, i64)>,
    last_at: Option<i64>,
    pending: Option<ChaseEvent>,
}

impl Reader {
    pub(crate) fn reset(&mut self, source: Option<u8>) {
        *self = Self { source, ..Self::default() };
    }

    pub(crate) fn loss(&mut self) {
        let stop = self.locked || self.pending == Some(ChaseEvent::Stop);
        self.reset(self.source);
        if stop {
            self.pending = Some(ChaseEvent::Stop);
        }
    }

    fn expire(&mut self, now: i64) {
        if self.last_at.is_some_and(|at| now.saturating_sub(at) >= TIMEOUT_US) {
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
                        if last.advance(8) == position {
                            if self.locked {
                                self.located_start = None;
                            }
                            self.locked = true;
                            self.rolling = Some((position.advance(7), now));
                            self.pending = Some(ChaseEvent::Roll(position.advance(7), now));
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

    pub(crate) fn tracking(&mut self, now: i64) -> Option<(Position, i64)> {
        self.expire(now);
        self.rolling
    }

    fn discontinuity(&mut self) {
        self.rolling = None;
        self.located_start = None;
        self.next = 0;
        self.previous = None;
        if self.locked {
            self.locked = false;
            self.pending = Some(ChaseEvent::Stop);
        }
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
        assert_eq!(reader.take(160_001), Some(ChaseEvent::Stop));
        cycle(20, |b| reader.read(b, 240_000));
        assert!(matches!(reader.take(240_000), Some(ChaseEvent::Roll(_, 0))));
        assert_eq!(reader.take(540_000), Some(ChaseEvent::Stop));
        assert_eq!(reader.take(541_000), None);
        // A gap in an incomplete set cannot be hidden by an immediately following cycle.
        reader.read(&[0xf1, 0], 550_000);
        reader.read(&[0xf1, 0x20], 551_000);
        cycle(0, |b| reader.read(b, 560_000));
        assert_eq!(reader.take(560_001), None);
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
            assert_eq!(reader.take(0), Some(ChaseEvent::Stop));
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
