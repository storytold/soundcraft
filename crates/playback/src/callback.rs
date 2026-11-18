use super::{
    Arc, AtomicBool, BLOCK, BTreeMap, Cmd, HashMap, HashSet, MAX_CHANNELS, MeterSnapshot, MixEngine, OLD_SESSIONS, Ordering, OutputTrouble, Plugin,
    PluginEditor, PluginStateReader, PluginStates, PreparedInstance, Range, Receiver, Samples, Sender, Session, Shared, SyncSender, TrackId,
    itu_stereo_fold, main_channels, midi, record, sync_channel,
};
use soundcraft_mix::{LiveMidi, midi_port_bit, midi_thru};
use soundcraft_model::TrackKind;

/// How the audio output is doing, for the UI.
#[derive(Default)]
pub(crate) struct Fault {
    /// A block panicked: output stays silent from then on.
    failed: AtomicBool,
    /// The device went away or the stream was invalidated: only a reconnect can restore audio.
    pub(crate) gone: AtomicBool,
    /// Blocks so far, to tell a device that stopped asking for audio.
    pub(crate) beats: std::sync::atomic::AtomicU64,
    /// The UI has been told.
    reported: AtomicBool,
}

impl Fault {
    /// What stopped the output, once.
    pub(crate) fn trouble(&self, stalled: bool) -> Option<OutputTrouble> {
        let trouble = if self.failed.load(Ordering::Relaxed) {
            OutputTrouble::Bug
        } else if self.gone.load(Ordering::Relaxed) {
            OutputTrouble::Gone
        } else if stalled {
            OutputTrouble::Stalled
        } else {
            return None;
        };
        (!self.reported.swap(true, Ordering::Relaxed)).then_some(trouble)
    }
}

/// How long a playing output may go without a block before it counts as stalled.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STALL: std::time::Duration = std::time::Duration::from_secs(2);

/// True once the block count `beats` has stood still for `STALL` while playing; `seen` is the
/// count last seen and since when.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn stalled(seen: &mut Option<(u64, std::time::Instant)>, beats: u64, playing: bool) -> bool {
    match *seen {
        Some((b, since)) if playing && b == beats => since.elapsed() >= STALL,
        _ => {
            *seen = Some((beats, std::time::Instant::now()));
            false
        }
    }
}

/// Fill one block with `fill`. If it panics, the block and every later one are silent and the
/// transport stops, rather than the panic taking the app down.
pub(crate) fn guarded_block(fault: &Fault, shared: &Shared, out: &mut [f32], fill: impl FnOnce(&mut [f32])) {
    fault.beats.fetch_add(1, Ordering::Relaxed);
    if fault.failed.load(Ordering::Relaxed) || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill(out))).is_err() {
        fault.failed.store(true, Ordering::Relaxed);
        shared.playing.store(false, Ordering::Relaxed);
        out.fill(0.0);
    }
}

/// State owned by the audio callback.
pub(crate) struct AudioState {
    rx: Receiver<Cmd>,
    pub(crate) shared: Arc<Shared>,
    pub(crate) session: Arc<Session>,
    pub(crate) mix: MixEngine,
    pos: Samples,
    playing: bool,
    end: Option<Samples>,
    looped: Option<Range>,
    /// Rendered frames waiting to be consumed (planar, main channels) and the fractional read index.
    pending: Vec<Vec<f32>>,
    /// Stereo fold-down gains per main channel (used when the device has fewer channels).
    fold: [(f32, f32); MAX_CHANNELS],
    pub(crate) pending_len: usize,
    read: f64,
    pub(crate) device_rate: f64,
    input: Option<Arc<record::InputRing>>,
    recording: bool,
    midi: Option<Arc<midi::MidiFeed>>,
    /// Live MIDI events of this callback not played yet: frames (session rate) until each is due.
    midi_pending: Vec<(i64, LiveMidi)>,
    /// The delay every live event gets so that all keep their spacing: one callback (frames).
    midi_delay: i64,
    /// Ports ([`midi_port_bit`]) that lost events and still have to let go of their notes in
    /// the engine: it is told with the next block it renders.
    midi_lost: u64,
    /// For each port (by the place of its bit in `midi_lost`), the time up to which its losses
    /// were dealt with at their mark in the queue: the port's flag for the same loss, seen a
    /// callback later, is not a second loss.
    midi_loss_seen: [i64; 64],
    /// The transport speed of the last callback: the playhead moved at it while the events the
    /// next callback takes were arriving.
    midi_speed: f64,
    /// Times loop recording went back to the start of the loop since the take was last written
    /// to (2 stands for any more: a note held that long covers the whole loop either way).
    midi_wraps: u8,
    /// Retired plugin instances go back to the UI thread to be dropped there.
    garbage: Sender<Vec<Box<dyn Plugin>>>,
    old_sessions: Option<SyncSender<Arc<Session>>>,
    /// Meters of blocks rendered while the UI held the shared snapshot (`try_lock` failed), so
    /// their peaks reach it with the next block instead of being lost.
    pub(crate) unsent: MeterSnapshot,
    /// Deleted tracks still to drop from the shared snapshot (it was locked when they went).
    forget_shared: Vec<TrackId>,
}

/// Live MIDI events one device callback can take in. More than that in one callback counts as
/// lost input for the ports concerned (see `take_midi`).
pub(crate) const MIDI_EVENTS: usize = 256;

fn new_engine(sr: f32) -> MixEngine {
    let mut m = MixEngine::new(sr, BLOCK);
    m.midi = Vec::with_capacity(MIDI_EVENTS);
    m.set_external_instances(true);
    m.metronome = true;
    m
}

impl AudioState {
    pub(crate) fn new(
        rx: Receiver<Cmd>,
        shared: Arc<Shared>,
        session: Arc<Session>,
        device_rate: f64,
        garbage: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Self {
        let sr = session.sample_rate.as_f64() as f32;
        let nch = main_channels(&session);
        AudioState {
            garbage,
            old_sessions: None,
            fold: fold_gains(&session),
            rx,
            shared,
            mix: new_engine(sr),
            session,
            pos: 0,
            playing: false,
            end: None,
            looped: None,
            pending: vec![vec![0.0; BLOCK]; nch],
            pending_len: 0,
            read: 0.0,
            device_rate,
            input: None,
            recording: false,
            midi: None,
            midi_pending: Vec::with_capacity(MIDI_EVENTS),
            midi_delay: 0,
            midi_lost: 0,
            midi_loss_seen: [i64::MIN; 64],
            midi_speed: 1.0,
            midi_wraps: 0,
            unsent: MeterSnapshot::default(),
            forget_shared: Vec::new(),
        }
    }

    fn drain(&mut self) {
        let mut changed = false;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Input(r) => self.input = Some(r),
                Cmd::Midi(f) => {
                    self.mix.midi_ports = Some(Arc::clone(f.ports()));
                    self.midi = Some(f);
                    // Another feed: releases of what is held now would never come.
                    self.midi_pending.clear();
                    self.midi_lost = u64::MAX;
                    // Its times are on its own clock.
                    self.midi_loss_seen = [i64::MIN; 64];
                }
                Cmd::Forget(gone) => {
                    self.unsent.forget(&gone);
                    if self.forget_shared.is_empty() {
                        self.forget_shared = gone;
                    } else {
                        self.forget_shared.extend_from_slice(&gone);
                    }
                    self.forget_meters();
                }
                Cmd::Recording(r, pass) => {
                    self.recording = r;
                    self.midi_wraps = 0;
                    // Everything played up to here is in the take (`capture_midi` ran first):
                    // tell the UI thread, which waits for this pass before it builds the clips.
                    if !r
                        && pass != 0
                        && let Some(feed) = &self.midi
                    {
                        feed.capture_done(pass);
                    }
                }
                Cmd::Session(s, instances) => {
                    changed = true;
                    if (s.sample_rate.as_f64() - self.session.sample_rate.as_f64()).abs() > 0.5 {
                        let mut old = std::mem::replace(&mut self.mix, new_engine(s.sample_rate.as_f64() as f32));
                        let _ = self.garbage.send(old.drain_plugins());
                        self.mix.midi_ports = self.midi.as_ref().map(|f| Arc::clone(f.ports()));
                    }
                    self.mix.adopt(instances);
                    // The main format changed (a document edit, not steady state): resize.
                    let nch = main_channels(&s);
                    if self.pending.len() != nch {
                        self.pending = vec![vec![0.0; BLOCK]; nch];
                        self.pending_len = 0;
                        self.read = 0.0;
                    }
                    self.fold = fold_gains(&s);
                    let old = std::mem::replace(&mut self.session, s);
                    if let Some(tx) = &self.old_sessions {
                        let _ = tx.try_send(old);
                    }
                }
                Cmd::OldSessions(tx) => self.old_sessions = Some(tx),
                Cmd::Play { from, end, looped } => {
                    self.pos = from;
                    self.end = end;
                    self.looped = looped.filter(|r| r.len() > 64);
                    self.midi_wraps = 0;
                    self.playing = true;
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.mix.reset();
                }
                Cmd::Stop => {
                    self.playing = false;
                    self.end = None;
                    self.looped = None;
                    self.mix.reset();
                }
                Cmd::Locate(at) => {
                    self.pos = at;
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.mix.reset();
                }
            }
        }
        if changed {
            // Place adopted instances now (not only once playback starts), so editors see them;
            // `sync` creates no third-party plugins here.
            self.mix.ensure_synced(&self.session);
        }
        self.send_garbage();
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    fn send_garbage(&mut self) {
        let g = self.mix.take_retired();
        if !g.is_empty() {
            let _ = self.garbage.send(g);
        }
    }

    /// Events of the ports in `lost` ([`midi_port_bit`]) will not be played, and any of them may
    /// have been a note-off or a pedal release. What is still queued from those ports goes too,
    /// so that no note-on from before the loss is played after the release, and the engine lets
    /// go of what the ports hold at its next block.
    fn midi_lost(&mut self, lost: u64) {
        if lost == 0 {
            return;
        }
        self.midi_pending.retain(|(_, e)| lost & midi_port_bit(e.port) == 0);
        self.midi_lost |= lost;
    }

    /// The transport speed now, as a factor.
    fn speed(&self) -> f64 {
        (f64::from(self.shared.speed.load(Ordering::Relaxed)) / 1000.0).clamp(0.1, 4.0)
    }

    /// The length of a device callback of `device_frames` in session frames.
    fn callback_frames(&self, device_frames: usize) -> i64 {
        let step = self.session.sample_rate.as_f64() / self.device_rate.max(1.0) * self.speed();
        (device_frames as f64 * step).ceil() as i64
    }

    /// Take the MIDI events that arrived since the last callback, before this callback's commands
    /// are read, so that what was played up to a Stop still belongs to the pass.
    ///
    /// Recording and playing are separate from here on. While a pass runs, each event is written
    /// to the take now, at the position it arrived at (the position of this callback less the
    /// event's age). To be heard, each is due `callback` session frames after it arrived, so that
    /// events keep their spacing instead of bunching up at the start of a block; that countdown
    /// is only for playing and never moves the recorded position.
    ///
    /// An age is real time; the playhead covered it at the transport speed of the callback
    /// before this one (half speed: half as many session frames). That speed is kept from then:
    /// by now the UI may have set another (it puts the speed back before a Stop).
    ///
    /// Lost input has its place in that order. The events of a lost port that arrived before
    /// the loss are written to the take but not played (their releases may be missing); the mark
    /// follows them in the take; what arrived after the loss comes after the mark and is played,
    /// after the engine let go of the port's notes at the start of its next block. Every port
    /// has its own time of loss, so one port's loss does not take along what another sent.
    ///
    /// A loss from a full queue also has a mark in the queue itself, ahead of the first event
    /// that fitted again. That is its exact place even when the port's flag was raised after
    /// this callback read the flags (the queue was drained meanwhile and a new note got in): the
    /// mark is dealt with where it stands, and the flag, seen by the next callback, is known to
    /// be the same loss (`midi_loss_seen`) instead of ending the new note.
    fn capture_midi(&mut self, device_frames: usize) {
        let callback = self.callback_frames(device_frames).max(0);
        let now_speed = self.speed();
        let speed = std::mem::replace(&mut self.midi_speed, now_speed);
        let Some(feed) = &self.midi else { return };
        // The loss flags first, then the time they stand for: see `MidiFeed::take_lost`.
        let mut input_lost = feed.take_lost();
        let now = feed.clock();
        // A flag for a loss that was already met at its mark in the queue is not a new loss.
        let flagged = input_lost;
        for bit in (0..64u8).filter(|b| flagged & (1u64 << b) != 0) {
            let seen = self.midi_loss_seen.get(usize::from(bit)).copied().unwrap_or(i64::MIN);
            if feed.lost_at(bit_port(bit)) <= seen {
                input_lost &= !(1u64 << bit);
            }
        }
        let per_us = self.session.sample_rate.as_f64() / 1e6 * speed;
        let frames = |since: i64| (now.saturating_sub(since).max(0) as f64 * per_us) as i64;
        let record = self.playing && self.recording;
        self.midi_delay = callback;
        if input_lost != 0 {
            // What is already waiting from those ports came before the loss.
            self.midi_pending.retain(|(_, e)| input_lost & midi_port_bit(e.port) == 0);
            self.midi_lost |= input_lost;
        }
        let mut take = TakeWriter { feed, pos: self.pos, looped: self.looped, wraps: std::mem::take(&mut self.midi_wraps) };
        // The lost ports whose mark is not in the take yet. Each goes in where its loss was.
        let mut marks = if record { input_lost } else { 0 };
        let mut unplayed = 0u64;
        while let Some((stamp, e)) = feed.next() {
            let age = frames(stamp);
            if e.status == midi::RecordedMidi::LOST {
                // The queue dropped events of this port here. What it sent before is above and
                // may lack its releases: it is not played, and the engine lets go of the port's
                // notes at its next block. What follows was sent after the loss and plays.
                let bit = midi_port_bit(e.port);
                marks &= !bit;
                if record {
                    while let Some((other, at)) = next_mark(feed, marks, stamp) {
                        feed.record_lost(take.at(frames(at).min(callback).max(age)), other);
                        marks &= !other;
                    }
                    feed.record_lost(take.at(age), bit);
                }
                self.midi_pending.retain(|(_, p)| bit & midi_port_bit(p.port) == 0);
                self.midi_lost |= bit;
                if let Some(seen) = self.midi_loss_seen.get_mut(usize::from(e.port).min(63)) {
                    *seen = (*seen).max(stamp);
                }
                continue;
            }
            if record {
                while let Some((bit, at)) = next_mark(feed, marks, stamp) {
                    // Not older than this callback's own events, nor newer than this one.
                    feed.record_lost(take.at(frames(at).min(callback).max(age)), bit);
                    marks &= !bit;
                }
                feed.record(take.at(age), &e);
            }
            // From before its port's loss: its release may be among what was lost.
            if input_lost & midi_port_bit(e.port) != 0 && stamp <= feed.lost_at(e.port) {
                continue;
            }
            if self.midi_pending.len() < self.midi_pending.capacity() {
                self.midi_pending.push((callback.saturating_sub(age).max(0), e));
            } else {
                // More than one callback holds: this event is not played (it is recorded), and
                // its port lets go of its notes.
                unplayed |= midi_port_bit(e.port);
            }
        }
        if record {
            // Nothing arrived after these losses: their marks close the callback's events.
            while let Some((bit, at)) = next_mark(feed, marks, i64::MAX) {
                feed.record_lost(take.at(frames(at).min(callback)), bit);
                marks &= !bit;
            }
            take.flush();
        }
        self.midi_lost(unplayed);
    }

    /// Nothing plays live MIDI: let the queued events go, so that they do not sound late. Their
    /// releases go with them, so every port lets go of its notes when the engine runs again.
    /// (A take already has them: `capture_midi` wrote them down.)
    fn drop_midi(&mut self) {
        self.midi_pending.clear();
        self.midi_lost = u64::MAX;
    }

    /// Hand the engine the live MIDI events due in the next `n` frames.
    fn due_midi(&mut self, n: usize) {
        self.mix.midi.clear();
        // Ports that lost events let go of their notes at the start of this block.
        self.mix.midi_lost = std::mem::take(&mut self.midi_lost);
        let frames = i64::try_from(n).unwrap_or(i64::MAX);
        let out = &mut self.mix.midi;
        let mut lost = 0u64;
        self.midi_pending.retain_mut(|(due, e)| {
            if *due >= frames {
                *due -= frames;
                return true;
            }
            if out.len() >= out.capacity() {
                // The engine's list is full (it holds as much as `midi_pending`, so this does
                // not happen): treated like any other event that is not played.
                lost |= midi_port_bit(e.port);
                return false;
            }
            out.push(LiveMidi { offset: u32::try_from(*due).unwrap_or(0), ..*e });
            false
        });
        self.midi_lost(lost);
    }

    /// Render the next block at the session rate into `pending`.
    fn render_next(&mut self, quantum: usize) {
        let mut n = quantum.clamp(1, BLOCK);
        if let Some(l) = self.looped
            && self.pos >= l.end
        {
            self.pos = l.start;
            if self.playing && self.recording {
                self.midi_wraps = self.midi_wraps.saturating_add(1).min(2);
            }
        }
        if let Some(l) = self.looped {
            n = n.min(usize::try_from((l.end - self.pos).max(1)).unwrap_or(BLOCK));
        } else if let Some(e) = self.end {
            if self.pos >= e {
                self.playing = false;
                self.end = None;
                self.shared.playing.store(false, Ordering::Relaxed);
                n = 0;
            } else {
                n = n.min(usize::try_from(e - self.pos).unwrap_or(BLOCK));
            }
        }
        if n == 0 {
            for c in &mut self.pending {
                c.iter_mut().for_each(|x| *x = 0.0);
            }
            self.pending_len = BLOCK;
            return;
        }
        if let Some(ring) = &self.input {
            let ch = ring.channels.load(Ordering::Relaxed).max(1);
            if self.mix.input.len() != ch {
                self.mix.input = vec![vec![0.0; BLOCK]; ch];
            }
            ring.pop_into(&mut self.mix.input, n, self.session.sample_rate.hz());
        }
        self.due_midi(n);
        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.render(&self.session, self.pos, n, &mut self.pending);
        self.send_garbage();
        self.pending_len = n;
        self.publish_meters();
        if self.playing {
            self.pos += n as i64;
            self.shared.position.store(self.pos, Ordering::Relaxed);
        }
    }

    /// Drop deleted tracks' meters from the shared snapshot, unless the UI holds it right now.
    fn forget_meters(&mut self) {
        if !self.forget_shared.is_empty()
            && let Ok(mut m) = self.shared.meters.try_lock()
        {
            m.forget(&self.forget_shared);
            self.forget_shared.clear();
        }
    }

    /// Hand this block's meters to the UI without blocking: they are folded (max) into what the
    /// UI has not taken yet. If the UI holds the snapshot right now, they wait in `unsent`.
    fn publish_meters(&mut self) {
        self.unsent.accumulate(&self.mix.main_meter, &self.mix.meters);
        self.forget_meters();
        if let Ok(mut m) = self.shared.meters.try_lock() {
            m.accumulate(&self.unsent.main, &self.unsent.tracks);
            self.unsent.reset_levels();
        }
    }

    /// Fill an interleaved device buffer.
    pub(crate) fn fill(&mut self, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        self.capture_midi(out.len() / channels);
        self.drain();
        // A keyboard plays through record-enabled and input-monitored MIDI and instrument tracks.
        let midi_live = self.midi.as_ref().is_some_and(|f| f.connected() > 0)
            && self.session.tracks.iter().any(|t| matches!(t.kind, TrackKind::Instrument | TrackKind::Midi) && midi_thru(t));
        let monitoring = midi_live || (self.input.is_some() && self.session.tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm));
        if !self.playing && !monitoring {
            self.drop_midi();
            out.iter_mut().for_each(|x| *x = 0.0);
            self.unsent.silence();
            self.forget_meters();
            if let Ok(mut m) = self.shared.meters.try_lock() {
                m.silence();
            }
            return;
        }
        let speed = f64::from(self.shared.speed.load(Ordering::Relaxed)) / 1000.0;
        let step = self.session.sample_rate.as_f64() / self.device_rate.max(1.0) * speed.clamp(0.1, 4.0);
        let callback_frames = out.len() / channels;
        let quantum = ((callback_frames as f64 * step).ceil() as usize).clamp(1, BLOCK);
        if !midi_live {
            self.drop_midi();
        }
        for frame in out.chunks_mut(channels) {
            while self.read >= self.pending_len as f64 {
                self.read -= self.pending_len as f64;
                if !self.playing && !monitoring {
                    break;
                }
                self.render_next(quantum);
                if self.pending_len == 0 {
                    break;
                }
            }
            let i0 = self.read.floor() as usize;
            let fr = (self.read - i0 as f64) as f32;
            let sounding = self.playing || monitoring;
            let nch = self.pending.len();
            let sample = |c: usize| {
                let ch = self.pending.get(c);
                let a = ch.and_then(|v| v.get(i0)).copied().unwrap_or(0.0);
                // `pending` is BLOCK long; only `pending_len` frames are valid. Hold the last one at the edge.
                let b = if i0 + 1 < self.pending_len { ch.and_then(|v| v.get(i0 + 1)).copied().unwrap_or(a) } else { a };
                a * (1.0 - fr) + b * fr
            };
            write_frame(frame, channels, nch, sounding, &self.fold, sample);
            self.read += step;
        }
    }
}

/// A port that the bit at place `bit` of a lost-ports mask ([`midi_port_bit`]) stands for. The
/// last bit stands for the ports without an index, which share one time of loss.
fn bit_port(bit: u8) -> u8 {
    if bit >= 63 { u8::MAX } else { bit }
}

/// Of the lost ports in `marks` ([`midi_port_bit`]), the one that lost its input first, if that
/// was before `before` (or at any time, for `i64::MAX`): its bit and the time.
fn next_mark(feed: &midi::MidiFeed, marks: u64, before: i64) -> Option<(u64, i64)> {
    (0..64u8)
        .filter(|b| marks & (1u64 << b) != 0)
        .map(|b| (1u64 << b, feed.lost_at(bit_port(b))))
        .filter(|(_, at)| *at < before || before == i64::MAX)
        .min_by_key(|(_, at)| *at)
}

/// Writes one callback's MIDI into the take: gives each entry its timeline position and says
/// where loop recording went round.
struct TakeWriter<'a> {
    feed: &'a midi::MidiFeed,
    /// The playhead at the start of this callback.
    pos: Samples,
    looped: Option<Range>,
    /// Times the loop went round since the last callback, not yet written to the take.
    wraps: u8,
}

impl TakeWriter<'_> {
    /// The timeline position of what happened `age` session frames ago. Ages only get smaller
    /// from one call to the next.
    fn at(&mut self, age: i64) -> Samples {
        let at = self.pos.saturating_sub(age.max(0));
        if self.wraps > 0 {
            // Earlier than the start of the loop: that was before it went round, near its end.
            if let Some(l) = self.looped
                && at < l.start
            {
                return l.end.saturating_sub(l.start.saturating_sub(at)).max(l.start);
            }
            self.flush();
        }
        // The playhead stands at the end of the loop until the next block takes it round (and
        // stays there if the pass is stopped first): what is played now is the last thing of
        // this round, not the first of a round that may never start.
        match self.looped {
            Some(l) if at >= l.end => l.end.saturating_sub(1).max(l.start),
            _ => at,
        }
    }

    /// Say that the loop went round, before the first entry of the new round.
    fn flush(&mut self) {
        for _ in 0..std::mem::take(&mut self.wraps) {
            self.feed.record_wrap(self.looped.map_or(self.pos, |l| l.end));
        }
    }
}

/// Write one device frame from `nch` rendered channels: fold surround to stereo (a mono device
/// averages the two sides), or copy the channels straight through. `sample(c)` is channel `c`.
fn write_frame(frame: &mut [f32], channels: usize, nch: usize, sounding: bool, fold: &[(f32, f32)], sample: impl Fn(usize) -> f32) {
    if channels < nch {
        // Fold surround to stereo first; a mono device averages the two sides.
        let (mut l, mut r) = (0.0f32, 0.0f32);
        if sounding {
            if nch == 2 {
                (l, r) = (sample(0), sample(1));
            } else {
                for (c, (gl, gr)) in fold.iter().enumerate().take(nch) {
                    let v = sample(c);
                    l += v * gl;
                    r += v * gr;
                }
            }
        }
        for (c, o) in frame.iter_mut().enumerate() {
            *o = match c {
                0 if channels == 1 => out_sample(0.5 * l + 0.5 * r),
                0 => out_sample(l),
                1 => out_sample(r),
                _ => 0.0,
            };
        }
    } else {
        for (c, o) in frame.iter_mut().enumerate() {
            *o = if c < nch.max(1) && sounding { out_sample(sample(c)) } else { 0.0 };
        }
    }
}

/// Clamp to the device range, mapping NaN and infinities to silence.
fn out_sample(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

/// UI-thread bookkeeping of the third-party instances the audio thread holds.
pub(crate) struct Side {
    /// What was last created for each slot: (plugin id, channels, created ok).
    pub(crate) shadow: HashMap<(TrackId, usize), (String, usize, bool)>,
    /// Session sample rate the instances were prepared for.
    rate: f32,
    /// Editor handles of the live instances.
    pub(crate) editors: HashMap<(TrackId, usize), Box<dyn PluginEditor>>,
    /// State readers of the live instances.
    pub(crate) states: BTreeMap<(TrackId, usize), Box<dyn PluginStateReader>>,
    /// Instances that refused their stored state: saves keep that state until the plugin's editor
    /// is opened.
    pub(crate) refused: HashSet<(TrackId, usize)>,
    garbage: Receiver<Vec<Box<dyn Plugin>>>,
    old_sessions: Option<Receiver<Arc<Session>>>,
    /// The tracks of the session last sent, to tell the audio thread which ones went.
    tracks: HashSet<TrackId>,
}

impl Side {
    pub(crate) fn new(rate: f32, garbage: Receiver<Vec<Box<dyn Plugin>>>) -> Side {
        Side {
            shadow: HashMap::new(),
            rate,
            editors: HashMap::new(),
            states: BTreeMap::new(),
            refused: HashSet::new(),
            garbage,
            old_sessions: None,
            tracks: HashSet::new(),
        }
    }

    /// Note `s`'s tracks; the ones the previous session had and `s` doesn't.
    pub(crate) fn gone_tracks(&mut self, s: &Session) -> Vec<TrackId> {
        let now: HashSet<TrackId> = s.tracks.iter().map(|t| t.id).collect();
        let gone = self.tracks.difference(&now).copied().collect();
        self.tracks = now;
        gone
    }

    /// Drops retired instances (closing their editors first, inside the plugin's drop) and
    /// replaced session snapshots.
    pub(crate) fn collect_garbage(&mut self) {
        while let Ok(g) = self.garbage.try_recv() {
            for p in g {
                log::debug!("plugin instance released: {}", p.info().name);
                drop(p);
            }
        }
        if let Some(rx) = &self.old_sessions {
            rx.try_iter().for_each(drop);
        }
    }

    pub(crate) fn return_old_sessions(&mut self, tx: &Sender<Cmd>) {
        let (otx, orx) = sync_channel(OLD_SESSIONS);
        self.old_sessions = Some(orx);
        let _ = tx.send(Cmd::OldSessions(otx));
    }

    pub(crate) fn forget(&mut self, key: (TrackId, usize)) {
        self.shadow.remove(&key);
        self.states.remove(&key);
        self.refused.remove(&key);
        if let Some(mut e) = self.editors.remove(&key) {
            e.close();
        }
    }
}

pub(crate) fn take_meter_snapshot(source: &mut MeterSnapshot, out: &mut MeterSnapshot) {
    if !source.fresh {
        // No block since the last take: keep the readings, but not those of deleted tracks.
        out.tracks.retain(|id, _| source.tracks.contains_key(id));
        return;
    }
    copy_meter_snapshot(source, out);
    source.reset_levels();
}

pub(crate) fn copy_meter_snapshot(source: &MeterSnapshot, out: &mut MeterSnapshot) {
    out.tracks.retain(|id, _| source.tracks.contains_key(id));
    for (id, meter) in &source.tracks {
        out.tracks.entry(*id).or_default().copy_from(meter);
    }
    out.main.copy_from(&source.main);
}

impl Side {
    pub(crate) fn capture_states(&mut self) -> PluginStates {
        self.states.iter_mut().filter(|(k, _)| !self.refused.contains(*k)).filter_map(|(&(t, slot), r)| Some((t, slot, r.save_state()?))).collect()
    }

    pub(crate) fn changed_states(&mut self) -> Vec<(TrackId, usize)> {
        self.editors.iter_mut().filter_map(|(k, e)| e.state_changed().then_some(*k)).collect()
    }

    pub(crate) fn capture_states_of(&mut self, slots: &[(TrackId, usize)]) -> PluginStates {
        self.states
            .iter_mut()
            .filter(|(k, _)| !self.refused.contains(*k) && slots.contains(k))
            .filter_map(|(&(t, slot), r)| Some((t, slot, r.save_state()?)))
            .collect()
    }

    /// A plugin that marks its state changed while it loads or restores it hasn't changed.
    pub(crate) fn add_editor(&mut self, key: (TrackId, usize), mut e: Box<dyn PluginEditor>) {
        let _ = e.state_changed();
        self.editors.insert(key, e);
    }

    pub(crate) fn open_editor(&mut self, key: (TrackId, usize)) -> Result<(), String> {
        let e = self.editors.get_mut(&key).ok_or_else(|| "this plugin has no editor (or it is not loaded)".to_string())?;
        e.open()?;
        self.refused.remove(&key);
        Ok(())
    }

    /// Creates the third-party instances `s` needs that the audio thread does not have yet.
    pub(crate) fn prepare(&mut self, s: &Session) -> Vec<PreparedInstance> {
        self.collect_garbage();
        let sr = s.sample_rate.as_f64() as f32;
        if (sr - self.rate).abs() > 0.5 {
            // The audio thread rebuilds its engine: everything is created anew.
            let keys: Vec<(TrackId, usize)> = self.shadow.keys().copied().collect();
            for k in keys {
                self.forget(k);
            }
            self.rate = sr;
        }
        let specs = soundcraft_mix::instance_specs(s);
        let stale: Vec<(TrackId, usize)> = self.shadow.keys().filter(|k| !specs.iter().any(|x| (x.track, x.slot) == **k)).copied().collect();
        for k in stale {
            self.forget(k);
        }
        let mut out = Vec::new();
        for spec in specs {
            let key = (spec.track, spec.slot);
            if self.shadow.get(&key).is_some_and(|(id, ch, _)| *id == spec.id && *ch == spec.ch) {
                continue;
            }
            self.forget(key);
            let made = soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), sr, BLOCK, spec.ch);
            log::debug!("plugin instance for {:?} slot {}: {} ({} ch, created: {})", spec.track, spec.slot, spec.id, spec.ch, made.is_some());
            self.shadow.insert(key, (spec.id.clone(), spec.ch, made.is_some()));
            if let Some(soundcraft_mix::NewInstance { mut plugin, refused_state }) = made {
                if let Some(e) = plugin.editor() {
                    self.add_editor(key, e);
                }
                if let Some(r) = plugin.state_reader() {
                    self.states.insert(key, r);
                }
                if refused_state {
                    self.refused.insert(key);
                }
                out.push(PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id, ch: spec.ch, plugin });
            }
        }
        out
    }
}

/// ITU stereo fold-down gains for each main channel of a session.
fn fold_gains(s: &Session) -> [(f32, f32); MAX_CHANNELS] {
    let mut g = [(0.0, 0.0); MAX_CHANNELS];
    let f = s.main_format();
    let sp = f.speakers();
    if sp.len() == f.channels() {
        for (o, x) in g.iter_mut().zip(sp.iter()) {
            *o = itu_stereo_fold(*x);
        }
    } else {
        // Ambisonics: W only, at -3 dB per side.
        g[0] = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    }
    g
}
