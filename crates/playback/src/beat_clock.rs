//! MIDI Beat Clock encoding. The shared worker owns output ports and presentation deadlines.
use crate::Session;
use crate::sync_clock::{BACKWARD_TOLERANCE_PULSES, CATCH_UP_SPACING, Presentation, RELAUNCH_PULSES, SyncEncoder};
use craft_midi_sync::sync::{CLOCK, CLOCKS_PER_QUARTER, CONTINUE, MAX_POSITION, START, STOP, song_position};
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};
use std::time::{Duration, Instant};
#[cfg(test)]
use {
    crate::{
        Cmd,
        sync_clock::{Publisher, View},
    },
    std::sync::{Arc, Mutex, mpsc::channel},
};

const TICKS_PER_CLOCK: i64 = TICKS_PER_QUARTER / CLOCKS_PER_QUARTER as i64;
#[derive(Default)]
pub(crate) struct Cursor {
    remap: bool,
    catchups: u64,
    last_sent: Option<Instant>,
    pass: Option<u64>,
    next_pulse: i64,
    last_position: Option<i64>,
    delayed_start: bool,
    out_of_range: bool,
}

impl Cursor {
    fn follow(&mut self, block: Presentation, at: i64, offset: i64, remap: bool, session: &Session, mut send: impl FnMut(&[u8])) {
        let changed = self.pass.replace(block.pass) != Some(block.pass);
        let (tempo, rate) = (&session.tempo, session.sample_rate);
        let origin = block.origin.saturating_sub(offset);
        // A worker attached or reopened well into a pass must start at the
        // current musical position. Replaying the old origin would send two locates.
        let timely_origin =
            changed && block.playing && tempo.samples_to_ticks_f(at, rate) - tempo.samples_to_ticks_f(origin, rate) < TICKS_PER_CLOCK as f64;
        if timely_origin {
            self.advance(true, origin, true, tempo, rate, &mut send);
        }
        self.advance(block.playing, at, (remap || changed) && !timely_origin, tempo, rate, send);
    }

    fn advance(&mut self, playing: bool, at: i64, locate: bool, tempo: &TempoMap, rate: SampleRate, mut send: impl FnMut(&[u8])) {
        if !playing || at < 0 {
            self.delayed_start = playing;
            self.out_of_range = false;
            if self.last_position.take().is_some() {
                send(&[STOP]);
            }
            return;
        }
        let estimated = (tempo.samples_to_ticks_f(at, rate) / TICKS_PER_CLOCK as f64).floor() as i64;
        let deadline = |pulse: i64| tempo.tick_to_samples(pulse.saturating_mul(TICKS_PER_CLOCK), rate);
        // tick_to_samples rounds to the nearest representable sample. Use the same
        // boundary both to schedule and to decide which side of a pulse we are on.
        let pulse = if deadline(estimated) > at {
            estimated.saturating_sub(1)
        } else if deadline(estimated.saturating_add(1)) <= at {
            estimated.saturating_add(1)
        } else {
            estimated
        };
        let from_zero = std::mem::take(&mut self.delayed_start) && !locate && pulse == 0;
        // Recover short worker stalls one pulse per wake; a full SPP unit of lag requires a new position.
        if self.last_position.is_none()
            || locate
            || self.last_position.is_some_and(|old| {
                old > at
                    && tempo.samples_to_ticks_f(old, rate) - tempo.samples_to_ticks_f(at, rate) > (BACKWARD_TOLERANCE_PULSES * TICKS_PER_CLOCK) as f64
            })
            || pulse.saturating_sub(self.next_pulse) >= RELAUNCH_PULSES
        {
            if self.last_position.take().is_some() {
                send(&[STOP]);
            }
            if at == 0 || from_zero {
                send(&[START]);
                self.next_pulse = 0;
            } else {
                let base = pulse / 6;
                let sixteenth = base + i64::from(at > deadline(base * 6));
                if sixteenth > i64::from(MAX_POSITION) {
                    self.out_of_range = true;
                    return;
                }
                send(&song_position(sixteenth as u16));
                send(&[CONTINUE]);
                self.next_pulse = sixteenth * 6;
            }
        }
        self.out_of_range = false;
        if self.next_pulse <= pulse {
            self.catchups += u64::from(self.next_pulse < pulse);
            send(&[CLOCK]);
            self.next_pulse += 1;
        }
        self.last_position = Some(at);
    }
}

impl SyncEncoder for Cursor {
    fn reset(&mut self) {
        *self = Self::default();
    }
    fn stop(&mut self, send: &mut dyn FnMut(&[u8])) {
        if self.last_position.is_some() {
            send(&[STOP]);
        }
        self.reset();
    }
    fn error(&self) -> Option<&'static str> {
        self.out_of_range.then_some("Position exceeds the MIDI Song Position Pointer range.")
    }
    fn take_catchups(&mut self) -> u64 {
        std::mem::take(&mut self.catchups)
    }
    fn remap(&mut self, old: &Session, new: &Session) {
        if let Some(at) = self.last_position {
            self.remap |= remapped(old, new, at);
        }
    }
    fn update(&mut self, b: Presentation, now: Instant, s: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
        let at = b.sample_index_at(now);
        let remap = std::mem::take(&mut self.remap);
        if self.pass == Some(b.pass)
            && b.playing
            && !remap
            && let Some(last) = self.last_sent
            && now < last + CATCH_UP_SPACING
        {
            return Some(last + CATCH_UP_SPACING);
        }
        let mut clock = false;
        let mut pulse = self.next_pulse;
        let mut transport = false;
        self.follow(b, at, 0, remap, s, |m| {
            // A transport change changes the grid origin. Preserve wire order for
            // Start / SPP / Continue and its first F8 when all are due together.
            if m == [START] {
                pulse = 0;
            }
            if let [0xf2, lo, hi] = m {
                pulse = (i64::from(*lo) + (i64::from(*hi) << 7)) * 6;
            }
            if m != [CLOCK] {
                transport = true;
            }
            let stamp = if m == [CLOCK] && !transport {
                clock = true;
                b.instant_for(s.tempo.tick_to_samples(pulse.saturating_mul(TICKS_PER_CLOCK), s.sample_rate) as f64)
            } else {
                now
            };
            clock |= m == [CLOCK];
            send(stamp.min(now), m);
        });
        if clock {
            self.last_sent = Some(now);
        }
        if !b.playing {
            return None;
        }
        if self.out_of_range {
            return Some(now + Duration::from_millis(10));
        }
        let due = s.tempo.tick_to_samples(self.next_pulse.saturating_mul(TICKS_PER_CLOCK), s.sample_rate);
        if due as f64 > b.limit {
            return Some(now + Duration::from_millis(1));
        }
        Some(b.instant_for(due as f64).max(now + CATCH_UP_SPACING))
    }
}
fn remapped(old: &Session, new: &Session, at: i64) -> bool {
    old.tempo.samples_to_ticks_f(at, old.sample_rate) != new.tempo.samples_to_ticks_f(at, new.sample_rate)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn messages(cursor: &mut Cursor, at: i64, locate: bool) -> Vec<Vec<u8>> {
        let mut result = Vec::new();
        cursor.advance(true, at, locate, &TempoMap::default(), SampleRate::new(48_000).unwrap(), |m| result.push(m.to_vec()));
        result
    }

    #[test]
    fn one_missed_pulse_advances_without_restarting_external_transport() {
        let mut cursor = Cursor::default();
        assert_eq!(messages(&mut cursor, 0, true), [vec![START], vec![CLOCK]]);
        assert_eq!(messages(&mut cursor, 2_100, false), [vec![CLOCK]]);
        assert_eq!(messages(&mut cursor, 2_150, false), [vec![CLOCK]]);
        assert_eq!(messages(&mut cursor, 32_000, false), [vec![STOP], song_position(6).to_vec(), vec![CONTINUE]]);
    }

    #[test]
    fn receiver_plays_the_pulse_identified_by_spp_at_the_correct_sample() {
        for (start, grid) in [(23_999, 24_000), (24_000, 24_000), (24_001, 30_000), (24_500, 30_000), (27_000, 30_000)] {
            let mut cursor = Cursor::default();
            let bytes = messages(&mut cursor, start, true);
            assert_eq!(bytes[0], song_position((grid / 6000) as u16));
            assert_eq!(bytes[1], [CONTINUE]);
            assert_eq!(bytes.len(), if start == grid { 3 } else { 2 });
            for at in start + 1..=grid + 1_000 {
                let bytes = messages(&mut cursor, at, false);
                if at == grid || at == grid + 1000 {
                    assert_eq!(bytes, [vec![CLOCK]]);
                } else {
                    assert!(bytes.is_empty());
                }
            }
            let wrap = messages(&mut cursor, 24_500, true);
            assert_eq!(wrap, [vec![STOP], song_position(5).to_vec(), vec![CONTINUE]]);
        }
    }

    #[test]
    fn out_of_range_relocation_stops_once_and_recovers_back_in_range() {
        let last = i64::from(MAX_POSITION) * 6000;
        let mut cursor = Cursor::default();
        assert_eq!(messages(&mut cursor, last, true)[0], song_position(MAX_POSITION));
        assert_eq!(messages(&mut cursor, last + 1000, false), [vec![CLOCK]]);
        assert_eq!(messages(&mut cursor, last + 1, true), [vec![STOP]]);
        assert!(cursor.out_of_range);
        assert!(messages(&mut cursor, last + 9000, false).is_empty());
        assert_eq!(messages(&mut cursor, 0, true), [vec![START], vec![CLOCK]]);
        assert!(!cursor.out_of_range);
    }

    #[test]
    fn editing_elapsed_beats_resyncs_once_and_a_future_tempo_change_does_not() {
        let fast = Session::default();
        let mut slow = fast.clone();
        slow.tempo.set_tempo(0, 60.).unwrap();
        let rate = fast.sample_rate;
        let at = i64::from(rate.hz()) * 2;
        let mut cursor = Cursor::default();
        cursor.advance(true, at, true, &fast.tempo, rate, |_| {});
        assert!(remapped(&fast, &slow, at));
        let mut bytes = Vec::new();
        cursor.advance(true, at, remapped(&fast, &slow, at), &slow.tempo, rate, |m| bytes.push(m.to_vec()));
        assert_eq!(bytes, [vec![STOP], song_position(8).to_vec(), vec![CONTINUE], vec![CLOCK]]);
        let mut future = slow.clone();
        future.tempo.set_tempo(10 * TICKS_PER_QUARTER, 90.).unwrap();
        assert!(!remapped(&slow, &future, at));
        cursor.advance(true, at, false, &future.tempo, rate, |_| panic!("duplicate message"));
    }

    #[test]
    fn in_flight_audio_and_contended_publication_do_not_undo_play_or_locate() {
        let shared = Arc::new(super::super::Shared { position: 0.into(), playing: false.into(), speed: 1000.into(), meters: Mutex::default() });
        let session = Arc::new(Session::default());
        let (tx, rx) = channel();
        let mut audio = super::super::callback::AudioState::new(rx, Arc::clone(&shared), session.clone(), 48_000., channel().0);
        let view = View::default();
        tx.send(Cmd::Clock(Some(Publisher::new(view.clone(), Arc::default())))).unwrap();
        let mut out = [0.; 256];
        audio.fill(&mut out, 2);
        let mut cursor = Cursor::default();
        let mut poll = || {
            let now = Instant::now();
            let b = view.lock().unwrap().read(now, true, 0.).0.unwrap();
            let mut bytes = Vec::new();
            cursor.follow(b, b.sample.floor() as i64, 0, false, &session, |m| bytes.push(m.to_vec()));
            bytes
        };
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        assert!(poll().is_empty());
        audio.fill(&mut out, 2);
        assert_eq!(poll(), [vec![START], vec![CLOCK]]);
        shared.position.store(27_000, std::sync::atomic::Ordering::Relaxed);
        shared.playing.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(poll().is_empty());
        tx.send(Cmd::Locate(27_000)).unwrap();
        let guard = view.lock().unwrap();
        audio.fill(&mut out, 2);
        drop(guard);
        assert!(poll().is_empty());
        audio.fill(&mut out, 2);
        assert_eq!(poll(), [vec![STOP], song_position(5).to_vec(), vec![CONTINUE]]);
        assert!(poll().is_empty());
    }

    #[test]
    fn wrap_inside_block_matches_generation_of_next_callback() {
        let start = Instant::now();
        let block = Presentation {
            pass: 3,
            origin: 0,
            playing: true,
            lead: Duration::ZERO,
            sample: 29_900.,
            limit: 30_380.,
            samples_per_second: 48_000.,
            looped: Some(super::super::Range::new(24_500, 30_000)),
            end: None,
            heard_at: start,
        };
        let wrapped = block.current(start + Duration::from_millis(5));
        assert_eq!((wrapped.pass, wrapped.origin, wrapped.sample_heard_at(start + Duration::from_millis(5)) as i64), (4, 24_500, 24_640));
        let session = Session::default();
        let mut cursor = Cursor::default();
        let mut bytes = Vec::new();
        cursor.follow(wrapped, wrapped.sample_heard_at(start + Duration::from_millis(5)) as i64, 0, false, &session, |m| bytes.push(m.to_vec()));
        assert_eq!(bytes, [song_position(5).to_vec(), vec![CONTINUE]]);
        cursor.follow(Presentation { sample: 24_880., ..wrapped }, 24_880, 0, false, &session, |_| panic!("repeated locate"));
        let ended = Presentation { looped: None, end: Some(30_000), ..block }.current(start + Duration::from_millis(5));
        assert!(!ended.playing);
    }

    #[test]
    fn delayed_start_stays_silent_until_zero_and_stop_cancels_it() {
        let rate = SampleRate::new(48_000).unwrap();
        let tempo = TempoMap::default();
        let mut cursor = Cursor::default();
        let mut events = Vec::new();
        cursor.advance(true, -1_000, true, &tempo, rate, |m| events.push(m.to_vec()));
        assert!(events.is_empty());
        cursor.advance(true, 50, false, &tempo, rate, |m| events.push(m.to_vec()));
        assert_eq!(events, [vec![START], vec![CLOCK]]);
        events.clear();
        cursor.advance(true, -500, true, &tempo, rate, |m| events.push(m.to_vec()));
        cursor.advance(false, -500, false, &tempo, rate, |m| events.push(m.to_vec()));
        cursor.advance(true, 24_000, true, &tempo, rate, |m| events.push(m.to_vec()));
        assert_eq!(events, [vec![STOP], vec![0xf2, 4, 0], vec![CONTINUE], vec![CLOCK]]);
    }

    #[test]
    fn between_blocks_clock_advances_once_then_waits_for_the_audio_device() {
        let start = std::time::Instant::now();
        let block = Presentation {
            lead: Duration::ZERO,
            pass: 0,
            origin: 0,
            playing: true,
            sample: 480.,
            limit: 960.,
            samples_per_second: 48_000.,
            looped: None,
            end: None,
            heard_at: start,
        };
        assert_eq!(block.sample_heard_at(start), 480.);
        assert_eq!(block.sample_heard_at(start + Duration::from_millis(5)), 720.);
        assert_eq!(block.sample_heard_at(start + Duration::from_secs(1)), 960.);
    }

    #[test]
    fn a_tempo_change_halves_the_rate_without_restarting() {
        let rate = SampleRate::new(48_000).unwrap();
        let mut tempo = TempoMap::default();
        tempo.set_tempo(TICKS_PER_QUARTER * 2, 60.0).unwrap();
        let mut cursor = Cursor::default();
        let mut clocks = [0, 0];
        let mut starts = 0;
        for at in (0..96_000).step_by(128) {
            cursor.advance(true, at, false, &tempo, rate, |msg| {
                if msg == [CLOCK] {
                    clocks[(at / 48_000) as usize] += 1;
                }
                if msg == [START] {
                    starts += 1;
                }
            });
        }
        assert_eq!(clocks, [48, 24]);
        assert_eq!(starts, 1);
    }

    #[test]
    fn musical_clock_counts_pulses_and_relocates_before_continuing() {
        let rate = SampleRate::new(48_000).unwrap();
        let tempo = TempoMap::default();
        let mut cursor = Cursor::default();
        let mut messages = Vec::new();
        for pos in (0..48_000).step_by(128) {
            cursor.advance(true, pos, false, &tempo, rate, |m| messages.push(m.to_vec()));
        }
        assert_eq!(messages.first(), Some(&vec![START]));
        assert_eq!(messages.iter().filter(|m| m.as_slice() == [CLOCK]).count(), 48);
        messages.clear();
        cursor.advance(true, 24_000, true, &tempo, rate, |m| messages.push(m.to_vec()));
        assert_eq!(messages, [vec![STOP], vec![0xf2, 4, 0], vec![CONTINUE], vec![CLOCK]]);
        messages.clear();
        for _ in 0..2 {
            cursor.advance(false, 24_000, false, &tempo, rate, |m| messages.push(m.to_vec()));
        }
        assert_eq!(messages, [vec![STOP]]);
    }
}
