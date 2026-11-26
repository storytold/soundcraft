//! Track note output: MIDI and instrument tracks play their clips to an external MIDI port.
//!
//! One [`TrackMidi`] encoder per output port covers every track routed to it
//! (`soundcraft_model::MidiOutput`). It reads clips through the same walk as the instrument
//! plug-in path (`soundcraft_mix::clip_events`), so both play the same notes, and sends each event
//! when its presentation deadline is due. Nothing waits in the worker's deadline queue, so a pass
//! change cannot drop a queued Note-Off: the encoder keeps its own table of the notes the device
//! holds, each with the end its starting snapshot gave it.
//!
//! Stop, locate and loop wrap follow the MIDI 1.0 specification (p24, p A-5): a Note-Off for each
//! held note, then sustain pedal up where the clips left it down; on Stop, an All Notes Off
//! (CC 123) on each channel used, as the system is then at rest. A start or locate first sends
//! the last value of each controller, pitch bend, channel pressure and program change before the
//! play position (Pro Tools Reference Guide p1059). No Reset All Controllers and no default
//! values: only real events are chased. Routing changes while playing are followed the same way:
//! a track that leaves a channel ends its notes there (and the pedal, if it was the last track
//! there), and a track that arrives is chased.
use crate::Session;
use crate::sync_clock::{Presentation, SyncEncoder};
use soundcraft_mix::{ClipEvent, chase_events, clip_events, midi_offset, midi_output_muted};
use soundcraft_model::{MidiOutput, Track, TrackId};
use std::time::{Duration, Instant};

/// How far behind the play position a pass may have started for its first events to still be
/// sent (late) rather than skipped. A worker that attaches later starts at the play position.
const LATE_START: f64 = 0.1;
/// How far ahead the encoder looks for its next event before waking anyway.
const LOOKAHEAD: f64 = 0.05;
/// How often a due event past the rendered audio checks for the next block.
const STALL_POLL: Duration = Duration::from_micros(250);

/// A note the device holds: the track that started it, where it ends (presentation samples),
/// and its key.
#[derive(Clone, Copy, Debug)]
struct Held {
    track: TrackId,
    channel: u8,
    pitch: u8,
    off: i64,
}

/// One event of an update, in presentation samples. At equal positions they go out in rank
/// order: a retriggered key is let go before it is struck again, bank select (a control change)
/// comes before its program change (Pro Tools Reference Guide p1052), and controllers and
/// program reach the device before the notes they shape.
const RELEASE: u8 = 0;
const CONTROL: u8 = 1;
const PROGRAM: u8 = 2;
const BEND_OR_PRESSURE: u8 = 3;
const STRIKE: u8 = 4;
const KEY_PRESSURE: u8 = 5;
/// The end of a note too short to outlast its own start: right after it.
const LATE_RELEASE: u8 = 6;

/// The rank of a controller-type message at a position, see [`Pending`].
fn rank(status: u8) -> u8 {
    match status & 0xf0 {
        0xb0 => CONTROL,
        0xc0 => PROGRAM,
        0xa0 => KEY_PRESSURE,
        _ => BEND_OR_PRESSURE,
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    at: i64,
    rank: u8,
    track: TrackId,
    message: [u8; 3],
    off: i64,
}

pub(crate) struct TrackMidi {
    port: String,
    pass: Option<u64>,
    playing: bool,
    /// Presentation sample from which the clips are still to be walked.
    cursor: Option<i64>,
    /// Restart at the play position after a timing gap (as after a locate).
    relocate: bool,
    held: Vec<Held>,
    /// Holders per channel and key: the device gets one Note-Off, when the last one ends. Wide
    /// enough for every note a clip can overlap on one key.
    count: Vec<[u32; 128]>,
    /// Channels whose sustain pedal the clips left down.
    pedal: u16,
    /// Channels sent to since the last Stop.
    used: u16,
    /// Channels sent to since the port was last opened, whatever the clips released since: a
    /// release whose send failed (the port broke) never reached the device.
    since_open: u16,
    /// Channels to quiet after the port reconnected: the device may still hold their notes.
    reconnected: u16,
    /// Channels whose quieting went to the port and is not yet known to have gone out
    /// ([`SyncEncoder::delivered`]): the next reopen quiets them again.
    quieting: u16,
    /// The routed, unmuted tracks and their channels as of the last update while playing: one
    /// that leaves lets go of its channel's pedal if it was the last there, one that arrives is
    /// chased.
    present: Vec<(TrackId, u8)>,
    /// Reused for the next update's [`TrackMidi::present`].
    arriving: Vec<(TrackId, u8)>,
    events: Vec<Pending>,
}

fn message_len(status: u8) -> usize {
    if matches!(status & 0xf0, 0xc0 | 0xd0) { 2 } else { 3 }
}

/// The tracks routed to `port`, with the channel each plays on.
fn routed<'a>(s: &'a Session, port: &'a str) -> impl Iterator<Item = (&'a Track, u8)> + 'a {
    MidiOutput::all(&s.edit)
        .filter(move |(_, out)| out.device == port)
        .filter_map(|(id, out)| s.track(id).filter(|t| t.kind.is_midi()).map(|t| (t, out.channel)))
}

/// The routed tracks of `port` that are not muted at `at`, with their channels, into `into`.
fn unmuted(s: &Session, port: &str, at: i64, into: &mut Vec<(TrackId, u8)>) {
    into.clear();
    into.extend(routed(s, port).filter(|(t, _)| !midi_output_muted(s, t, at)).map(|(t, channel)| (t.id, channel)));
}

/// What All MIDI Notes Off sends on every channel of a port after the held notes' Note-Offs:
/// sustain pedal up, All Notes Off and All Sound Off.
pub(crate) fn panic_messages(send: &mut dyn FnMut(&[u8])) {
    for channel in 0..16u8 {
        for controller in [64, 123, 120] {
            send(&[0xb0 | channel, controller, 0]);
        }
    }
}

impl TrackMidi {
    pub(crate) fn new(port: &str) -> Self {
        Self {
            port: port.to_owned(),
            pass: None,
            playing: false,
            cursor: None,
            relocate: false,
            held: Vec::new(),
            count: vec![[0; 128]; 16],
            pedal: 0,
            used: 0,
            since_open: 0,
            reconnected: 0,
            quieting: 0,
            present: Vec::new(),
            arriving: Vec::new(),
            events: Vec::new(),
        }
    }

    pub(crate) fn boxed(port: &str) -> Box<dyn SyncEncoder> {
        Box::new(Self::new(port))
    }

    fn send_on(&mut self, channel: u8, message: [u8; 3], send: &mut dyn FnMut(&[u8])) {
        let status = (message[0] & 0xf0) | (channel & 0x0f);
        if status & 0xf0 == 0xb0 && message[1] == 64 {
            if message[2] >= 64 {
                self.pedal |= 1 << channel;
            } else {
                self.pedal &= !(1 << channel);
            }
        }
        self.used |= 1 << channel;
        self.since_open |= 1 << channel;
        let bytes = [status, message[1], message[2]];
        send(&bytes[..message_len(status)]);
    }

    /// Release one holder of a key. The device hears the Note-Off when the last holder ends.
    fn release(&mut self, channel: u8, pitch: u8, send: &mut dyn FnMut(&[u8])) {
        let Some(n) = self.count.get_mut(usize::from(channel)).and_then(|keys| keys.get_mut(usize::from(pitch))) else { return };
        *n = n.saturating_sub(1);
        if *n == 0 {
            send(&[0x80 | channel, pitch, 0]);
        }
    }

    /// Every held note off, then the sustain pedal up where it is down (it would hold the notes).
    fn release_all(&mut self, send: &mut dyn FnMut(&[u8])) {
        // Ending in order of their ends keeps the device's voices in a natural order.
        self.held.sort_by_key(|h| h.off);
        for h in std::mem::take(&mut self.held) {
            self.release(h.channel, h.pitch, send);
        }
        for channel in 0..16u8 {
            if self.pedal & (1 << channel) != 0 {
                send(&[0xb0 | channel, 64, 0]);
            }
        }
        self.pedal = 0;
        self.count.fill([0; 128]);
    }

    /// The system is at rest: All Notes Off on each channel used (MIDI 1.0 p24).
    fn at_rest(&mut self, send: &mut dyn FnMut(&[u8])) {
        for channel in 0..16u8 {
            if self.used & (1 << channel) != 0 {
                send(&[0xb0 | channel, 123, 0]);
            }
        }
        self.used = 0;
    }

    /// Begin a pass at `start`: the chased controllers of every routed, unmuted track.
    fn begin(&mut self, s: &Session, start: i64, send: &mut dyn FnMut(&[u8])) {
        let port = std::mem::take(&mut self.port);
        for (t, channel) in routed(s, &port) {
            if midi_output_muted(s, t, start) {
                continue;
            }
            chase_events(s, t, start.saturating_sub(midi_offset(s, t)), |message| self.send_on(channel, message, send));
        }
        unmuted(s, &port, start, &mut self.present);
        self.port = port;
        self.cursor = Some(start);
    }

    /// Follow routing changes made while playing. A destination (this port and a channel) that
    /// the last routed, unmuted track left gets its sustain pedal up, since the pedal holds notes
    /// over their Note-Offs (MIDI 1.0 p A-5) and no track there will lift it. A track that newly
    /// plays here (routed, unmuted or made active) gets its controllers and program at the play
    /// position, as a locate does (Pro Tools Reference Guide p1059).
    fn follow_routing(&mut self, s: &Session, at: i64, send: &mut dyn FnMut(&[u8])) {
        let mut here = std::mem::take(&mut self.arriving);
        unmuted(s, &self.port, at, &mut here);
        for channel in 0..16u8 {
            if self.pedal & (1 << channel) != 0 && !here.iter().any(|&(_, c)| c == channel) {
                self.send_on(channel, [0xb0, 64, 0], send);
            }
        }
        for &(id, channel) in &here {
            if self.present.contains(&(id, channel)) {
                continue;
            }
            if let Some(t) = s.track(id) {
                chase_events(s, t, at.saturating_sub(midi_offset(s, t)), |message| self.send_on(channel, message, send));
            }
        }
        self.arriving = std::mem::replace(&mut self.present, here);
    }

    /// Collect the events of `[from, to)` (presentation samples) of every routed, unmuted track.
    fn collect(&mut self, s: &Session, from: i64, to: i64) {
        self.events.clear();
        for (t, channel) in routed(s, &self.port) {
            let offset = midi_offset(s, t);
            let events = &mut self.events;
            clip_events(s, t, from.saturating_sub(offset), to.saturating_sub(offset), |event| match event {
                ClipEvent::Note { on, off, pitch, velocity } => {
                    let at = on.saturating_add(offset);
                    if (from..to).contains(&at) && !midi_output_muted(s, t, at) {
                        let message = [0x90 | channel, pitch, velocity];
                        events.push(Pending { at, rank: STRIKE, track: t.id, message, off: off.saturating_add(offset) });
                    }
                }
                ClipEvent::Ctrl { at, message } => {
                    let at = at.saturating_add(offset);
                    if !midi_output_muted(s, t, at) {
                        events.push(Pending {
                            at,
                            rank: rank(message[0]),
                            track: t.id,
                            message: [(message[0] & 0xf0) | channel, message[1], message[2]],
                            off: at,
                        });
                    }
                }
            });
        }
    }

    /// The earliest event after `after` within the lookahead, if any.
    fn next_event(&mut self, s: &Session, after: i64, horizon: i64) -> Option<i64> {
        let ends = self.held.iter().map(|h| h.off).filter(|&off| off > after).min();
        self.collect(s, after.saturating_add(1), horizon);
        let starts = self.events.iter().map(|e| e.at).min();
        self.events.clear();
        ends.into_iter().chain(starts).min()
    }

    /// Send what is due up to and including `at`, each with its deadline.
    fn play(&mut self, b: Presentation, now: Instant, s: &Session, at: i64, send: &mut dyn FnMut(Instant, &[u8])) {
        let from = self.cursor.unwrap_or(at);
        let to = at.saturating_add(1);
        // Tracks no longer routed here on the same channel, or now muted, let go of their notes.
        let mut gone = std::mem::take(&mut self.held);
        let mut stay = Vec::with_capacity(gone.len());
        gone.retain(|h| {
            let keep = s
                .track(h.track)
                .filter(|t| t.kind.is_midi())
                .and_then(|t| MidiOutput::of(&s.edit, t.id).map(|out| (t, out)))
                .is_some_and(|(t, out)| out.device == self.port && out.channel == h.channel && !midi_output_muted(s, t, at));
            if keep {
                stay.push(*h);
            }
            !keep
        });
        self.held = stay;
        let deadline = |sample: i64| b.instant_for(sample as f64).min(now);
        {
            let mut out = |m: &[u8]| send(now, m);
            for h in gone {
                self.release(h.channel, h.pitch, &mut out);
            }
            self.follow_routing(s, at, &mut out);
        }
        if from < to {
            self.collect(s, from, to);
        }
        // The ends that fall in this window: held notes, then notes that start and end in it.
        for h in self.held.iter().filter(|h| h.off < to) {
            self.events.push(Pending { at: h.off, rank: RELEASE, track: h.track, message: [0x80 | h.channel, h.pitch, 0], off: h.off });
        }
        self.held.retain(|h| h.off >= to);
        let starts = self.events.len();
        for i in 0..starts {
            let e = self.events[i];
            if e.rank == STRIKE && e.off < to {
                let rank = if e.off > e.at { RELEASE } else { LATE_RELEASE };
                self.events.push(Pending {
                    at: e.off.max(e.at),
                    rank,
                    track: e.track,
                    message: [0x80 | (e.message[0] & 0x0f), e.message[1], 0],
                    off: e.off,
                });
            }
        }
        // A stable sort keeps clip order among equal positions and ranks.
        self.events.sort_by_key(|e| (e.at, e.rank));
        let events = std::mem::take(&mut self.events);
        for e in &events {
            let when = deadline(e.at);
            let channel = e.message[0] & 0x0f;
            match e.rank {
                RELEASE | LATE_RELEASE => self.release(channel, e.message[1], &mut |m| send(when, m)),
                STRIKE => {
                    if let Some(n) = self.count.get_mut(usize::from(channel)).and_then(|keys| keys.get_mut(usize::from(e.message[1]))) {
                        *n = n.saturating_add(1);
                    }
                    self.send_on(channel, e.message, &mut |m| send(when, m));
                    if e.off >= to {
                        self.held.push(Held { track: e.track, channel, pitch: e.message[1], off: e.off });
                    }
                }
                _ => self.send_on(channel, e.message, &mut |m| send(when, m)),
            }
        }
        self.events = events;
        self.events.clear();
        self.cursor = Some(to);
    }
}

impl SyncEncoder for TrackMidi {
    fn update(&mut self, b: Presentation, now: Instant, s: &Session, send: &mut dyn FnMut(Instant, &[u8])) -> Option<Instant> {
        if self.reconnected != 0 {
            for channel in 0..16u8 {
                if self.reconnected & (1 << channel) != 0 {
                    // Pedal up first: All Notes Off leaves notes the pedal holds sounding
                    // (MIDI 1.0 p.A-5).
                    send(now, &[0xb0 | channel, 64, 0]);
                    send(now, &[0xb0 | channel, 123, 0]);
                }
            }
            self.quieting |= std::mem::take(&mut self.reconnected);
        }
        // Taken on the first update after the loss, whatever it is: a stop or a start in between
        // already restarts the pass, and a later update must not restart it again.
        let relocate = std::mem::take(&mut self.relocate);
        let at = b.sample_index_at(now);
        if !b.playing {
            if self.playing {
                let mut out = |m: &[u8]| send(now, m);
                self.release_all(&mut out);
                self.at_rest(&mut out);
            }
            self.playing = false;
            self.pass = Some(b.pass);
            self.cursor = None;
            return None;
        }
        if !self.playing || self.pass != Some(b.pass) || self.cursor.is_none() || relocate {
            // A start, locate or loop wrap. Notes of the old pass end now; no All Notes Off
            // while playing on (MIDI 1.0 p24).
            let mut out = |m: &[u8]| send(now, m);
            self.release_all(&mut out);
            let late = (at - b.origin) as f64 / b.sample_rate.as_f64();
            let start = if self.pass != Some(b.pass) && (0.0..LATE_START).contains(&late) { b.origin } else { at };
            self.begin(s, start, &mut out);
            self.pass = Some(b.pass);
            self.playing = true;
        }
        self.play(b, now, s, at, send);
        let rate = b.sample_rate.as_f64();
        let horizon = at.saturating_add((rate * LOOKAHEAD) as i64);
        let next = self.next_event(s, at, horizon).unwrap_or(horizon);
        let due = b.instant_for(next as f64);
        if next as f64 > b.limit && due <= now {
            // The next event lies past the rendered audio and its time has come: poll for the
            // next callback's block, which follows the audio clock.
            return Some(now + STALL_POLL);
        }
        // A later block normally arrives before an event past this one's end is due.
        Some(due.max(now))
    }

    /// The port reopened: the device may hold notes whose Note-Offs were lost. Quiet the
    /// channels played on since it was last opened (pedal up, All Notes Off), and those whose
    /// quieting after an earlier reopen did not go out, and start again at the play position.
    fn reset(&mut self) {
        self.reconnected |= self.since_open | self.quieting | self.used | self.held.iter().fold(0, |m, h| m | (1 << h.channel));
        self.since_open = 0;
        self.quieting = 0;
        self.held.clear();
        self.count.fill([0; 128]);
        self.pedal = 0;
        self.used = 0;
        self.present.clear();
        self.cursor = None;
        self.pass = None;
        self.playing = false;
    }

    /// Queued deadlines were dropped with the timing: carry on from the play position.
    fn timing_lost(&mut self) {
        self.relocate = true;
    }

    /// The quieting after a reopen went out.
    fn delivered(&mut self) {
        self.quieting = 0;
    }

    fn stop(&mut self, send: &mut dyn FnMut(&[u8])) {
        self.release_all(send);
        self.at_rest(send);
        self.cursor = None;
        self.playing = false;
    }

    fn panic(&mut self, send: &mut dyn FnMut(&[u8])) {
        self.release_all(send);
        self.used = 0;
    }
}

#[cfg(test)]
#[path = "track_midi_tests.rs"]
mod tests;
