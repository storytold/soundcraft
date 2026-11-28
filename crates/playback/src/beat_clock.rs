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

mod recovery;
use recovery::Repayment;

const TICKS_PER_CLOCK: i64 = TICKS_PER_QUARTER / CLOCKS_PER_QUARTER as i64;
#[derive(Default)]
pub(crate) struct Cursor {
    remap: bool,
    timing_interrupted: bool,
    repayment: Option<Repayment>,
    catchups: u64,
    last_sent: Option<Instant>,
    pass: Option<u64>,
    pass_origin: Option<i64>,
    next_pulse: i64,
    last_position: Option<i64>,
    stopped_spp: Option<u16>,
    delayed_start: bool,
    observed_stop: bool,
    out_of_range: bool,
}

impl Cursor {
    fn follow(&mut self, block: Presentation, at: i64, offset: i64, remap: bool, session: &Session, mut send: impl FnMut(&[u8])) {
        let previous_pass = self.pass.replace(block.pass);
        let changed = previous_pass != Some(block.pass);
        let (tempo, rate) = (&session.tempo, session.sample_rate);
        let origin = block.origin.saturating_sub(offset);
        let origin_changed = self.pass_origin.replace(origin) != Some(origin);
        // A worker attached or reopened well into a pass must start at the
        // current musical position. Replaying the old origin would send two locates.
        let was_stopped = std::mem::replace(&mut self.observed_stop, !block.playing);
        // A continuously observed start at zero is unambiguous even if the first
        // audio buffer arrived a pulse late. Freshly attached workers still locate
        // to live time, as do workers more than a sixteenth into the new pass.
        let prearmed_zero = was_stopped && origin == 0 && !origin_changed;
        let window = if prearmed_zero { RELAUNCH_PULSES } else { 1 };
        let timely_origin = changed
            && block.playing
            && tempo.samples_to_ticks_f(at, rate) - tempo.samples_to_ticks_f(origin, rate) < (window * TICKS_PER_CLOCK) as f64;
        if timely_origin {
            let mut sent_clock = false;
            self.advance(true, origin, true, tempo, rate, |m| {
                sent_clock |= m == [CLOCK];
                send(m);
            });
            if sent_clock {
                // The first F8 was delivered now. Spread a late start over the
                // first beat so the next F8 does not immediately follow it.
                self.repayment = Repayment::after_start(self.next_pulse.saturating_sub(1), at, session, prearmed_zero);
                // Deliver any other pulse that rounds to this sample on a separate wake.
                return;
            }
        }
        // A stopped seek publishes its own origin, including a locate to the origin of
        // the pass that was rolling. A new pass at an unchanged stopped origin repeats
        // the position the receiver already holds; a moved origin is a new locate even
        // when it encodes the same Song Position Pointer.
        let stopped_seek = at == origin && (previous_pass.is_some() || at != 0);
        if !block.playing && origin_changed {
            self.stopped_spp = None;
        }
        let located = changed && (block.playing || stopped_seek);
        self.advance(block.playing, at, (remap || located) && !timely_origin, tempo, rate, send);
    }

    fn advance(&mut self, playing: bool, at: i64, locate: bool, tempo: &TempoMap, rate: SampleRate, mut send: impl FnMut(&[u8])) {
        if !playing || at < 0 {
            self.delayed_start = playing;
            if playing || at < 0 {
                self.out_of_range = false;
            }
            if self.last_position.take().is_some() {
                send(&[STOP]);
            }
            if playing || at < 0 || !locate {
                return;
            }
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
        let position = || {
            let base = pulse / 6;
            let sixteenth = base + i64::from(at > deadline(base * 6));
            u16::try_from(sixteenth).ok().filter(|&n| n <= MAX_POSITION)
        };
        if !playing {
            let spp = position();
            self.out_of_range = spp.is_none();
            if spp != self.stopped_spp {
                self.stopped_spp = spp;
                if let Some(n) = spp {
                    send(&song_position(n));
                }
            }
            return;
        }
        self.stopped_spp = None;
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
                let Some(sixteenth) = position() else {
                    self.out_of_range = true;
                    return;
                };
                send(&song_position(sixteenth));
                send(&[CONTINUE]);
                self.next_pulse = i64::from(sixteenth) * 6;
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
    fn timing_lost(&mut self) {
        self.timing_interrupted = self.last_position.is_some();
    }
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
            // A repayment window paces owed pulses on the map it was planned with. An edit
            // ahead of the current position that moves its end replans it on the new map, so
            // the window rejoins the live grid instead of relaunching the receiver.
            if !self.remap
                && let Some(window) = self.repayment
                && window.moved(old, new)
            {
                self.repayment = Repayment::new(self.next_pulse, at, new);
            }
        }
    }
    fn update(&mut self, b: Presentation, now: Instant, s: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
        let at = b.sample_index_at(now);
        let remap = std::mem::take(&mut self.remap);
        let interrupted = std::mem::take(&mut self.timing_interrupted);
        if self.pass == Some(b.pass) && b.playing && !remap {
            if interrupted {
                self.repayment = Repayment::new(self.next_pulse, at, s);
            }
            if let Some(window) = self.repayment {
                if !window.expired(at) {
                    return Some(self.repay(window, b, now, s, send));
                }
                self.repayment = None;
            }
            if let Some(last) = self.last_sent
                && now < last + CATCH_UP_SPACING
            {
                return Some(last + CATCH_UP_SPACING);
            }
        } else {
            self.repayment = None;
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
    #[test]
    fn a_prearmed_zero_start_one_pulse_late_does_not_jump_to_the_next_sixteenth() {
        let mut song = Session { sample_rate: SampleRate::HZ_48000, ..Session::default() };
        song.tempo.set_tempo(0, 120.).unwrap();
        let first = Instant::now();
        let idle = Presentation {
            generation: 0,
            sample_rate: song.sample_rate,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: false,
            sample: 0.,
            heard_at: first,
            samples_per_second: 48_000.,
            limit: 256.,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        for (observed_stop, start_sample, from_zero) in [(true, 1500, true), (false, 1500, false), (true, 6500, false)] {
            let mut cursor = Cursor::default();
            if observed_stop {
                cursor.update(idle, first, &song, &mut |_, _| {});
            }
            let roll = Presentation { pass: 2, playing: true, sample: start_sample as f64, limit: 48000., ..idle };
            let mut controls = Vec::new();
            let mut clocks = Vec::new();
            for us in (0..=(24_000 - start_sample) * 1_000_000 / 48_000 + 100).step_by(100) {
                cursor.update(roll, first + Duration::from_micros(us), &song, &mut |_, msg| {
                    if msg == [CLOCK] { clocks.push(us) } else { controls.push(msg.to_vec()) }
                });
            }
            if from_zero {
                assert_eq!(controls, [vec![START]]);
                assert_eq!(clocks.len(), 25);
                assert!(clocks.windows(2).all(|p| p[1] - p[0] >= 19_000), "{clocks:?}");
            } else {
                let spp = if start_sample == 1500 { 1 } else { 2 };
                assert_eq!(controls, [song_position(spp).to_vec(), vec![CONTINUE]]);
            }
        }
    }
    #[test]
    fn late_first_clock_keeps_all_pulses_without_squeezing_the_next_interval() {
        let mut song = Session { sample_rate: SampleRate::HZ_48000, ..Session::default() };
        song.tempo.set_tempo(0, 120.).unwrap();
        let first = Instant::now();
        let b = Presentation {
            generation: 0,
            sample_rate: song.sample_rate,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample: 728.,
            heard_at: first,
            samples_per_second: 48_000.,
            limit: 48_728.,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        let mut beat = Cursor::default();
        let mut clocks = Vec::new();
        let mut control = Vec::new();
        for us in (0..=485_000).step_by(100) {
            beat.update(b, first + Duration::from_micros(us), &song, &mut |_, msg| {
                if msg == [CLOCK] { clocks.push(us) } else { control.push(msg.to_vec()) }
            });
        }
        assert_eq!(control, [vec![START]]);
        assert_eq!(clocks.len(), 25);
        assert!(clocks.windows(2).all(|p| p[1] - p[0] >= 19_000), "{clocks:?}");
    }

    #[test]
    fn stopped_locates_announce_the_same_position_as_play_without_starting() {
        let mut cursor = Cursor::default();
        let song = Session::default();
        let now = Instant::now();
        let r = song.sample_rate;
        let mut block = Presentation {
            generation: 0,
            sample_rate: r,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample: 0.,
            heard_at: now,
            samples_per_second: r.as_f64(),
            limit: 0.,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        cursor.update(block, now, &song, &mut |_, _| {});
        block.playing = false;
        for (sixteenth, adjustment) in [(7, 0), (12, -1), (0, 0)] {
            block.pass += 1;
            block.origin = song.tempo.tick_to_samples(sixteenth * TICKS_PER_QUARTER / 4, r) + adjustment;
            block.sample = block.origin as f64;
            block.limit = block.sample;
            let mut batch = Vec::new();
            cursor.update(block, now, &song, &mut |_, m| batch.push(m.to_vec()));
            let mut expected = vec![song_position(sixteenth as u16).to_vec()];
            if sixteenth == 7 {
                expected.insert(0, vec![STOP]);
            }
            assert_eq!(batch, expected);
            block.pass += 1;
            cursor.update(block, now, &song, &mut |_, m| panic!("duplicate stopped locate {m:?}"));
        }
    }

    #[test]
    fn every_stopped_locate_announces_its_position() {
        let song = Session::default();
        let now = Instant::now();
        let r = song.sample_rate;
        let sixteenth = |n: i64| song.tempo.tick_to_samples(n * TICKS_PER_QUARTER / 4, r);
        let rolling = Presentation {
            generation: 0,
            sample_rate: r,
            system_delay_samples: 0,
            pass: 1,
            origin: sixteenth(8),
            playing: true,
            sample: sixteenth(8) as f64,
            heard_at: now,
            samples_per_second: r.as_f64(),
            limit: sixteenth(8) as f64,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        let mut cursor = Cursor::default();
        cursor.update(rolling, now, &song, &mut |_, _| {});
        let rolled = Presentation { sample: sixteenth(20) as f64, limit: sixteenth(20) as f64, ..rolling };
        cursor.update(rolled, now, &song, &mut |_, _| {});
        // Stop and a locate back to the rolling pass origin, read in one callback.
        let returned = Presentation { pass: 2, playing: false, ..rolling };
        let mut batch = Vec::new();
        cursor.update(returned, now, &song, &mut |_, m| batch.push(m.to_vec()));
        assert_eq!(batch, [vec![STOP], song_position(8).to_vec()]);
        // A second locate inside the same sixteenth is still a locate.
        let nearby = Presentation { pass: 3, origin: sixteenth(8) + 7, sample: (sixteenth(8) + 7) as f64, ..returned };
        batch.clear();
        cursor.update(nearby, now, &song, &mut |_, m| batch.push(m.to_vec()));
        assert_eq!(batch, [song_position(9).to_vec()]);
        let again = Presentation { pass: 4, origin: sixteenth(8) + 9, sample: (sixteenth(8) + 9) as f64, ..returned };
        batch.clear();
        cursor.update(again, now, &song, &mut |_, m| batch.push(m.to_vec()));
        assert_eq!(batch, [song_position(9).to_vec()]);
    }

    #[test]
    fn a_tempo_edit_ahead_during_repayment_rejoins_the_live_grid_without_a_relaunch() {
        let mut song = Session { sample_rate: SampleRate::HZ_48000, ..Session::default() };
        song.tempo.set_tempo(0, 120.).unwrap();
        let base = Instant::now();
        let roll = Presentation {
            generation: 0,
            sample_rate: song.sample_rate,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample: 0.,
            heard_at: base,
            samples_per_second: 48_000.,
            limit: 1e9,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        let mut cursor = Cursor::default();
        let mut transport = Vec::new();
        let mut run = |cursor: &mut Cursor, range: std::ops::Range<u64>, session: &Session| {
            for us in range.step_by(250) {
                cursor.update(roll, base + Duration::from_micros(us), session, &mut |_, m| {
                    if m != [CLOCK] {
                        transport.push((us, m.to_vec()));
                    }
                });
            }
        };
        run(&mut cursor, 0..200_000, &song);
        // Timing is lost for 0.425 s, then a repayment window spreads the owed pulses.
        cursor.timing_lost();
        run(&mut cursor, 625_000..626_000, &song);
        assert!(cursor.repayment.is_some());
        // Doubling the tempo from 0.75 s on leaves the current position where it was.
        let mut edited = song.clone();
        edited.tempo.set_tempo(TICKS_PER_QUARTER * 3 / 2, 240.).unwrap();
        cursor.remap(&song, &edited);
        run(&mut cursor, 626_000..1_500_000, &edited);
        assert_eq!(transport, [(0, vec![START])]);
        let live = (edited.tempo.samples_to_ticks_f(72_000, edited.sample_rate) / TICKS_PER_CLOCK as f64) as i64;
        assert!(cursor.next_pulse.abs_diff(live) <= 1, "{} pulses sent, {live} live", cursor.next_pulse);
    }

    fn messages(cursor: &mut Cursor, at: i64, locate: bool) -> Vec<Vec<u8>> {
        let mut result = Vec::new();
        cursor.advance(true, at, locate, &TempoMap::default(), SampleRate::new(48_000).unwrap(), |m| result.push(m.to_vec()));
        result
    }

    #[test]
    fn review_launch_at_a_rounded_pulse_boundary_does_not_send_two_clocks() {
        let mut song = Session::new("rounded pulse", SampleRate::HZ_48000);
        song.tempo.set_tempo(0, 126.).unwrap();
        let now = Instant::now();
        let block = Presentation {
            generation: 0,
            sample_rate: song.sample_rate,
            system_delay_samples: 0,
            pass: 1,
            origin: 0,
            playing: true,
            sample: 952.,
            limit: 1464.,
            samples_per_second: 48_000.,
            heard_at: now,
            looped: None,
            end: None,
            lead: Some(Duration::ZERO),
        };
        let mut messages = Vec::new();
        let mut cursor = Cursor::default();
        let next = cursor.update(block, now, &song, &mut |_, m| messages.push(m.to_vec())).unwrap();
        assert_eq!(messages, [vec![START], vec![CLOCK]]);
        assert!(next >= now + CATCH_UP_SPACING);
        messages.clear();
        cursor.update(block, next, &song, &mut |_, m| messages.push(m.to_vec()));
        assert_eq!(messages, [vec![CLOCK]]);
    }

    #[test]
    fn review_stopped_range_warning_persists_until_the_next_locate() {
        let mut cursor = Cursor::default();
        let tempo = TempoMap::default();
        let sr = SampleRate::HZ_48000;
        let far = tempo.tick_to_samples((i64::from(MAX_POSITION) + 1) * 6 * TICKS_PER_CLOCK, sr);
        for locate in [true, false, false] {
            cursor.advance(false, far, locate, &tempo, sr, |_| panic!("invalid SPP"));
            assert!(cursor.error().is_some());
        }
        cursor.advance(false, 0, true, &tempo, sr, |_| {});
        assert!(cursor.error().is_none());
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
            let b = view.lock().unwrap().read(now, false, 0).0.unwrap();
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
            generation: 0,
            sample_rate: soundcraft_time::SampleRate::default(),
            system_delay_samples: 0,
            pass: 3,
            origin: 0,
            playing: true,
            lead: Some(Duration::ZERO),
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
            generation: 0,
            sample_rate: soundcraft_time::SampleRate::default(),
            system_delay_samples: 0,
            lead: Some(Duration::ZERO),
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
