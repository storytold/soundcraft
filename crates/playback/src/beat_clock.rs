//! External beat clock follows the audio transport. MIDI I/O never runs on its callback.
use super::{Cmd, Session};
use craft_midi_sync::sync::{CLOCK, CLOCKS_PER_QUARTER, CONTINUE, MAX_POSITION, START, STOP, song_position};
use midir::{MidiOutput, MidiOutputConnection};
use soundcraft_model::MidiBeatClock;
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICKS_PER_CLOCK: i64 = TICKS_PER_QUARTER / CLOCKS_PER_QUARTER as i64;

#[derive(Default)]
pub(super) struct Output {
    trouble: Arc<Mutex<Option<String>>>,
    worker: Option<(String, Sender<Update>, std::thread::JoinHandle<()>)>,
}

enum Update {
    Session(Arc<Session>),
    Wake,
}

impl Output {
    pub(super) fn configure(&mut self, session: &Arc<Session>, audio: &Sender<Cmd>) {
        let config = MidiBeatClock::from_session(session);
        let destination = config.destination.filter(|_| config.enabled);
        if let Some((name, tx, _)) = &self.worker
            && destination == Some(name.as_str())
            && tx.send(Update::Session(Arc::clone(session))).is_ok()
        {
            return;
        }
        if self.worker.is_some() {
            let _ = audio.send(Cmd::Clock(None));
        }
        self.close();
        self.set_trouble(None);
        if let Some(destination) = destination {
            let port = destination.to_owned();
            let view = Arc::new(Mutex::new(None));
            let _ = audio.send(Cmd::Clock(Some(Publisher::new(Arc::clone(&view)))));
            let trouble = Arc::clone(&self.trouble);
            let session = Arc::clone(session);
            let (tx, rx) = channel();
            match std::thread::Builder::new().name("MIDI Beat Clock".into()).spawn(move || run(&port, view, session, rx, trouble)) {
                Ok(handle) => self.worker = Some((destination.to_owned(), tx, handle)),
                Err(e) => {
                    let _ = audio.send(Cmd::Clock(None));
                    self.set_trouble(Some(e.to_string()));
                }
            }
        }
    }

    pub(super) fn wake(&self) {
        if let Some((_, tx, _)) = &self.worker {
            let _ = tx.send(Update::Wake);
        }
    }

    pub(super) fn trouble(&self) -> Option<String> {
        self.trouble.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
    fn set_trouble(&self, value: Option<String>) {
        report(&self.trouble, value);
    }
    fn close(&mut self) {
        if let Some((_, tx, worker)) = self.worker.take() {
            drop(tx);
            let _ = worker.join();
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn output_names() -> Result<Vec<String>, String> {
    std::panic::catch_unwind(|| {
        let out = MidiOutput::new("SoundCraft Beat Clock").map_err(|e| e.to_string())?;
        Ok(out.ports().iter().filter_map(|p| out.port_name(p).ok()).collect())
    })
    .map_err(|_| "MIDI output enumeration failed".to_string())?
}

fn open(name: &str) -> Result<MidiOutputConnection, String> {
    let out = MidiOutput::new("SoundCraft Beat Clock").map_err(|e| e.to_string())?;
    let mut matches = out.ports().into_iter().filter(|p| out.port_name(p).is_ok_and(|n| n == name));
    let port = matches.next().ok_or_else(|| format!("MIDI output {name} unavailable"))?;
    if matches.next().is_some() {
        return Err(format!("Multiple MIDI outputs named {name}"));
    }
    out.connect(&port, "beat clock").map_err(|e| e.to_string())
}

#[derive(Default)]
struct Cursor {
    pass: Option<u64>,
    next_pulse: i64,
    last_position: Option<i64>,
    delayed_start: bool,
    out_of_range: bool,
}

impl Cursor {
    fn follow(&mut self, block: Block, at: i64, offset: i64, remap: bool, session: &Session, mut send: impl FnMut(&[u8])) {
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
        // Once an F8 deadline has been missed by a complete pulse, re-position rather
        // than sending a burst of stale clocks that would speed up external instruments.
        if self.last_position.is_none() || locate || self.last_position.is_some_and(|old| at < old) || pulse.saturating_sub(self.next_pulse) >= 1 {
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
        while self.next_pulse <= pulse {
            send(&[CLOCK]);
            self.next_pulse += 1;
        }
        self.last_position = Some(at);
    }
}

// Only the audio thread publishes a block, after applying transport commands.
// It never waits for the clock worker: a contended publication is retried on the
// next callback, carrying the same pass and exact origin.
type View = Arc<Mutex<Option<Block>>>;

#[derive(Clone, Copy)]
struct Block {
    pass: u64,
    origin: i64,
    playing: bool,
    position: i64,
    limit: i64,
    samples_per_second: f64,
    looped: Option<super::Range>,
    end: Option<i64>,
    when: Instant,
}

impl Block {
    // A loop or a bounded pass can end inside a device buffer. Project only
    // the samples in that buffer, preserving the callback's generation at the next block.
    fn current(mut self, now: Instant) -> Self {
        self.position = self.at(now);
        if self.playing {
            if let Some(range) = self.looped {
                if self.position >= range.end {
                    let past = self.position.saturating_sub(range.end);
                    let wraps = 1 + past / range.len();
                    self.pass = self.pass.wrapping_add(wraps as u64);
                    self.origin = range.start;
                    self.position = range.start + past % range.len();
                    self.limit = self.limit.saturating_sub(wraps.saturating_mul(range.len()));
                }
            } else if self.end.is_some_and(|end| self.position >= end) {
                self.playing = false;
            }
        }
        self
    }
    fn at(self, now: Instant) -> i64 {
        let elapsed = now.saturating_duration_since(self.when).as_secs_f64();
        self.position.saturating_add((elapsed * self.samples_per_second) as i64).min(self.limit)
    }
}

pub(super) struct Publisher {
    view: View,
    pass: u64,
    origin: Option<i64>,
}
impl Publisher {
    fn new(view: View) -> Self {
        Self { view, pass: 0, origin: None }
    }
    pub(super) fn locate(&mut self, at: i64) {
        self.pass = self.pass.wrapping_add(1);
        self.origin = Some(at);
    }
    pub(super) fn publish(
        &mut self,
        playing: bool,
        position: i64,
        limit: i64,
        samples_per_second: f64,
        looped: Option<super::Range>,
        end: Option<i64>,
    ) {
        let origin = *self.origin.get_or_insert(position);
        if let Ok(mut view) = self.view.try_lock() {
            *view = Some(Block { pass: self.pass, origin, playing, position, limit, samples_per_second, looped, end, when: Instant::now() });
        }
    }
}

fn report(status: &Mutex<Option<String>>, value: Option<String>) {
    let mut old = status.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *old != value {
        if let Some(message) = &value {
            log::warn!("MIDI Beat Clock: {message}");
        }
        *old = value;
    }
}

fn remapped(old: &Session, new: &Session, at: i64) -> bool {
    old.tempo.samples_to_ticks_f(at, old.sample_rate) != new.tempo.samples_to_ticks_f(at, new.sample_rate)
}

fn run(port: &str, view: View, session: Arc<Session>, rx: Receiver<Update>, trouble: Arc<Mutex<Option<String>>>) {
    drive(view, session, rx, trouble, || open(port), |out, bytes| out.send(bytes).map_err(|e| format!("{port}: {e}")));
}

fn drive<C>(
    view: View,
    mut session: Arc<Session>,
    rx: Receiver<Update>,
    trouble: Arc<Mutex<Option<String>>>,
    mut connect: impl FnMut() -> Result<C, String>,
    mut send: impl FnMut(&mut C, &[u8]) -> Result<(), String>,
) {
    let mut cursor = Cursor::default();
    let mut offset = MidiBeatClock::from_session(&session).offset;
    let mut link = None;
    let mut retry = Instant::now();
    let mut awake_until = retry;
    let mut remap = false;
    let mut latest = None;
    let mut update = None;
    loop {
        let now = Instant::now();
        if let Ok(guard) = view.try_lock() {
            latest = *guard;
        }
        let block = latest.map(|b: Block| b.current(now));
        if let Some(message) = update.take() {
            match message {
                Update::Session(s) => {
                    let new_offset = MidiBeatClock::from_session(&s).offset;
                    if let Some(b) = block {
                        let at = b.position.saturating_sub(offset);
                        remap |= offset != new_offset || remapped(&session, &s, at);
                    }
                    offset = new_offset;
                    session = s;
                }
                Update::Wake => awake_until = now + Duration::from_millis(150),
            }
        }
        let mut opened = false;
        if link.is_none() && now >= retry {
            match connect() {
                Ok(out) => {
                    link = Some(out);
                    cursor = Cursor::default();
                    remap = true;
                    opened = true;
                }
                Err(e) => report(&trouble, Some(e)),
            }
            retry = now + Duration::from_secs(1);
        }
        let mut delay = Duration::from_millis(100);
        if let (Some(block), Some(out)) = (block, &mut link) {
            let at = block.position.saturating_sub(offset);
            let mut error = None;
            let was_out = cursor.out_of_range;
            cursor.follow(block, at, offset, remap, &session, |bytes| {
                if error.is_none() {
                    error = send(out, bytes).err();
                }
            });
            remap = false;
            if let Some(error) = error {
                report(&trouble, Some(error));
                link = None;
                retry = now + Duration::from_secs(1);
            } else if opened || was_out != cursor.out_of_range {
                report(&trouble, cursor.out_of_range.then(|| "Position exceeds the MIDI Song Position Pointer range.".into()));
            }
            if block.playing {
                let due =
                    if at < 0 { 0 } else { session.tempo.tick_to_samples(cursor.next_pulse.saturating_mul(TICKS_PER_CLOCK), session.sample_rate) };
                let seconds = due.saturating_sub(at) as f64 / block.samples_per_second;
                // Rust's native sleep uses a high-resolution waitable timer on Windows.
                // Bound transport reaction time and avoid spinning on a stalled device.
                delay = Duration::from_secs_f64(seconds.clamp(0.0005, 0.010));
                if at >= block.limit.saturating_sub(offset) || cursor.out_of_range {
                    delay = Duration::from_millis(3);
                }
            }
        }
        if now < awake_until {
            delay = delay.min(Duration::from_millis(2));
        }
        if delay < Duration::from_millis(100) {
            std::thread::sleep(delay.saturating_sub(now.elapsed()));
            match rx.try_recv() {
                Ok(message) => update = Some(message),
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match rx.recv_timeout(delay) {
                Ok(message) => update = Some(message),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    if cursor.last_position.is_some()
        && let Some(mut out) = link
    {
        let _ = send(&mut out, &[STOP]);
    }
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
            let wrap = messages(&mut cursor, 24_500, false);
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
        let view = Arc::new(Mutex::new(None));
        tx.send(Cmd::Clock(Some(Publisher::new(view.clone())))).unwrap();
        let mut out = [0.; 256];
        audio.fill(&mut out, 2);
        let mut cursor = Cursor::default();
        let mut poll = || {
            let b = view.lock().unwrap().unwrap();
            let mut bytes = Vec::new();
            cursor.follow(b, b.position, 0, false, &session, |m| bytes.push(m.to_vec()));
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
        let block = Block {
            pass: 3,
            origin: 0,
            playing: true,
            position: 29_900,
            limit: 30_380,
            samples_per_second: 48_000.,
            looped: Some(super::super::Range::new(24_500, 30_000)),
            end: None,
            when: start,
        };
        let wrapped = block.current(start + Duration::from_millis(5));
        assert_eq!((wrapped.pass, wrapped.origin, wrapped.position), (4, 24_500, 24_640));
        let session = Session::default();
        let mut cursor = Cursor::default();
        let mut bytes = Vec::new();
        cursor.follow(wrapped, wrapped.position, 0, false, &session, |m| bytes.push(m.to_vec()));
        assert_eq!(bytes, [song_position(5).to_vec(), vec![CONTINUE]]);
        cursor.follow(Block { position: 24_880, ..wrapped }, 24_880, 0, false, &session, |_| panic!("repeated locate"));
        let ended = Block { looped: None, end: Some(30_000), ..block }.current(start + Duration::from_millis(5));
        assert!(!ended.playing);
    }

    #[test]
    fn failed_send_is_reported_and_reopen_clears_the_error() {
        let view = View::default();
        let mut publisher = Publisher::new(view.clone());
        publisher.publish(true, 0, 0, 48_000., None, None);
        let trouble = Arc::new(Mutex::new(None));
        let (tx, rx) = channel();
        let (observed, events) = channel();
        let status = trouble.clone();
        let handle = std::thread::spawn(move || {
            let mut attempt = 0;
            drive(
                view,
                Arc::new(Session::default()),
                rx,
                status,
                || {
                    attempt += 1;
                    Ok(attempt)
                },
                |attempt, bytes| {
                    if *attempt == 1 {
                        return Err("MIDI destination disappeared".into());
                    }
                    if bytes == [START] {
                        observed.send(()).unwrap();
                    }
                    Ok(())
                },
            );
        });
        let deadline = Instant::now() + Duration::from_millis(800);
        while trouble.lock().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(trouble.lock().unwrap().as_deref(), Some("MIDI destination disappeared"));
        events.recv_timeout(Duration::from_secs(4)).unwrap();
        assert!(trouble.lock().unwrap().is_none());
        drop(tx);
        handle.join().unwrap();
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
        let block =
            Block { pass: 0, origin: 0, playing: true, position: 480, limit: 960, samples_per_second: 48_000., looped: None, end: None, when: start };
        assert_eq!(block.at(start), 480);
        assert_eq!(block.at(start + Duration::from_millis(5)), 720);
        assert_eq!(block.at(start + Duration::from_secs(1)), 960);
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
