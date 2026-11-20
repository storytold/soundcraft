//! External beat clock follows the audio transport. MIDI I/O never runs on its callback.
use super::{Session, Shared};
use craft_midi_sync::sync::{CLOCK, CLOCKS_PER_QUARTER, CONTINUE, MAX_POSITION, START, STOP, song_position};
use midir::{MidiOutput, MidiOutputConnection};
use soundcraft_model::MidiBeatClock;
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER, TempoMap};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

const TICKS_PER_CLOCK: i64 = TICKS_PER_QUARTER / CLOCKS_PER_QUARTER as i64;

#[derive(Default)]
pub(super) struct Output {
    worker: Option<(String, Sender<Update>, std::thread::JoinHandle<()>)>,
}

enum Update {
    Session(Arc<Session>),
    Transport(i64, bool),
}

impl Output {
    pub(super) fn configure(&mut self, session: &Arc<Session>, transport: &Arc<Shared>) {
        let config = MidiBeatClock::from_session(session);
        let destination = config.destination.filter(|_| config.enabled);
        if let Some((name, tx, _)) = &self.worker
            && destination == Some(name.as_str())
            && tx.send(Update::Session(Arc::clone(session))).is_ok()
        {
            return;
        }
        self.close();
        if let Some(destination) = destination {
            let port = destination.to_owned();
            let transport = Arc::clone(transport);
            let session = Arc::clone(session);
            let (tx, rx) = channel();
            match std::thread::Builder::new().name("MIDI Beat Clock".into()).spawn(move || run(&port, transport, session, rx)) {
                Ok(handle) => self.worker = Some((destination.to_owned(), tx, handle)),
                Err(e) => log::warn!("MIDI Beat Clock: {e}"),
            }
        }
    }

    pub(super) fn transport(&self, at: i64, playing: bool) {
        if let Some((_, tx, _)) = &self.worker {
            let _ = tx.send(Update::Transport(at, playing));
        }
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
    let port =
        out.ports().into_iter().find(|p| out.port_name(p).is_ok_and(|n| n == name)).ok_or_else(|| format!("MIDI output {name} unavailable"))?;
    out.connect(&port, "beat clock").map_err(|e| e.to_string())
}

#[derive(Default)]
struct Cursor {
    next_pulse: i64,
    last_position: Option<i64>,
    delayed_start: bool,
}

impl Cursor {
    fn advance(&mut self, playing: bool, at: i64, locate: bool, tempo: &TempoMap, rate: SampleRate, mut send: impl FnMut(&[u8])) {
        if !playing {
            self.delayed_start = false;
            if self.last_position.take().is_some() {
                send(&[STOP]);
            }
            return;
        }
        if at < 0 {
            if self.last_position.take().is_some() {
                send(&[STOP]);
            }
            self.delayed_start = true;
            return;
        }
        if std::mem::take(&mut self.delayed_start) {
            send(&[START]);
            self.next_pulse = 0;
            self.last_position = Some(0);
        }
        let tick = tempo.samples_to_ticks(at, rate).max(0);
        let pulse = tick / TICKS_PER_CLOCK;
        if self.last_position.is_none() || locate || self.last_position.is_some_and(|old| at < old) || pulse.saturating_sub(self.next_pulse) > 24 {
            if self.last_position.is_some() {
                send(&[STOP]);
            }
            if tick == 0 {
                send(&[START]);
            } else {
                let sixteenths = (tick / (TICKS_PER_QUARTER / 4)).min(i64::from(MAX_POSITION));
                send(&song_position(sixteenths as u16));
                send(&[CONTINUE]);
            }
            self.next_pulse = pulse + i64::from(tick % TICKS_PER_CLOCK != 0);
        }
        while self.next_pulse <= pulse {
            send(&[CLOCK]);
            self.next_pulse += 1;
        }
        self.last_position = Some(at);
    }
}

// Audio devices advance the published playhead a block at a time. Interpolate only up to
// one observed block, so a large buffer does not send several clocks in the same burst
// and an audio-device stall cannot leave the external sequencer running on its own.
struct BlockClock {
    position: i64,
    block: i64,
    observed: std::time::Instant,
    emitted: i64,
}

impl BlockClock {
    fn new(position: i64, now: std::time::Instant) -> Self {
        Self { position, block: 0, observed: now, emitted: position }
    }

    fn sample(&mut self, position: i64, hz: u32, speed: u32, now: std::time::Instant) -> i64 {
        if position < self.position {
            *self = Self::new(position, now);
        } else if position > self.position {
            self.block = position.saturating_sub(self.position).min(i64::from(hz) / 10);
            self.observed = now;
            self.position = position;
        }
        let extrapolated = (now.saturating_duration_since(self.observed).as_secs_f64() * f64::from(hz) * f64::from(speed) / 1000.0) as i64;
        self.emitted = self.emitted.max(position.saturating_add(extrapolated.min(self.block)));
        self.emitted
    }
}

fn run(port: &str, transport: Arc<Shared>, mut session: Arc<Session>, rx: Receiver<Update>) {
    let mut cursor = Cursor::default();
    let mut offset = MidiBeatClock::from_session(&session).offset;
    let mut pending = None;
    let mut sampler = BlockClock::new(transport.position.load(Ordering::Relaxed), std::time::Instant::now());
    let mut rolling = transport.playing.load(Ordering::Acquire);
    let mut link = None;
    let mut retry = std::time::Instant::now();
    loop {
        let now = std::time::Instant::now();
        if link.is_none() && now >= retry {
            match open(port) {
                Ok(out) => {
                    link = Some(out);
                    cursor = Cursor::default();
                }
                Err(e) => log::debug!("MIDI Beat Clock: {e}"),
            }
            retry = now + Duration::from_secs(1);
        }
        let (position, playing, locate) = if let Some((at, play)) = pending.take() {
            rolling = play;
            sampler = BlockClock::new(at, now);
            (at, play, true)
        } else {
            rolling &= transport.playing.load(Ordering::Acquire);
            (
                sampler.sample(transport.position.load(Ordering::Relaxed), session.sample_rate.hz(), transport.speed.load(Ordering::Relaxed), now),
                rolling,
                false,
            )
        };
        if let Some(out) = &mut link {
            let at = position.saturating_sub(offset);
            let mut failed = false;
            cursor.advance(playing, at, locate, &session.tempo, session.sample_rate, |bytes| failed |= out.send(bytes).is_err());
            if failed {
                link = None;
                retry = now + Duration::from_secs(1);
            }
        }
        match rx.recv_timeout(Duration::from_millis(if playing { 1 } else { 100 })) {
            Ok(Update::Session(s)) => {
                offset = MidiBeatClock::from_session(&s).offset;
                session = s;
            }
            Ok(Update::Transport(at, play)) => pending = Some((at, play)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if cursor.last_position.is_some()
                    && let Some(mut out) = link
                {
                    let _ = out.send(&[STOP]);
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut clock = BlockClock::new(0, start);
        assert_eq!(clock.sample(480, 48_000, 1000, start), 480);
        assert_eq!(clock.sample(480, 48_000, 1000, start + Duration::from_millis(5)), 720);
        assert_eq!(clock.sample(480, 48_000, 1000, start + Duration::from_secs(1)), 960);
        assert_eq!(clock.sample(0, 48_000, 1000, start + Duration::from_secs(2)), 0);
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
