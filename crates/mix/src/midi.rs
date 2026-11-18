use super::clips::Key;
use super::strip::{PluginSlot, Strip};
use super::{ClipContent, CtrlKind, Range, SampleSource, Samples, Session, SourceAudio, Track, TrackId};
use std::sync::Arc;

pub(crate) type HeldNotes = [[bool; 128]; 16];

/// Note state for one source, so disabling it releases its notes on the shared instrument.
pub(crate) struct RoutedMidi {
    pub(crate) track: TrackId,
    pub(crate) channel: u8,
    enabled: bool,
    held: HeldNotes,
}

impl RoutedMidi {
    pub(crate) fn new(track: TrackId, channel: u8) -> Self {
        Self { track, channel, enabled: false, held: [[false; 128]; 16] }
    }

    pub(crate) fn clear(&mut self) {
        self.enabled = false;
        self.held.fill([false; 128]);
    }

    pub(crate) fn release(&mut self, inst: &mut PluginSlot) {
        if self.enabled {
            inst.plugin.midi(0, [0xB0 | self.channel, 64, 0]);
            inst.plugin.midi(0, [0xB0 | self.channel, 66, 0]);
            release_notes(inst, &mut self.held);
        }
        self.enabled = false;
    }

    pub(crate) fn render(&mut self, s: &Session, source: &Track, dest: &Track, inst: &mut PluginSlot, blk: super::strip::Block, chase: bool) {
        if source.inactive || source.mixer.mute || (blk.any_solo && !source.mixer.solo && !source.mixer.solo_safe && !dest.mixer.solo) {
            self.release(inst);
            return;
        }
        if chase || !self.enabled {
            chase_controllers(s, source, blk.pos, inst, Some(self.channel));
        }
        self.enabled = true;
        schedule_notes(s, source, blk.pos, blk.frames, inst, &mut self.held, Some(self.channel));
    }
}

pub(crate) fn release_notes(inst: &mut PluginSlot, held: &mut HeldNotes) {
    for (channel, notes) in held.iter_mut().enumerate() {
        for (key, sounding) in notes.iter_mut().enumerate() {
            if std::mem::take(sounding) {
                inst.plugin.note_off_channel(0, channel as u8, key as u8);
            }
        }
    }
}

/// The track's MIDI Track Offset (`midi.offset.<id>` in the edit values), in samples. Looked up
/// without formatting the key, since this runs on the audio thread every block.
fn midi_offset(s: &Session, t: &Track) -> i64 {
    const PREFIX: &str = "midi.offset.";
    s.edit
        .values
        .range::<str, _>((std::ops::Bound::Included(PREFIX), std::ops::Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(PREFIX))
        .find(|(k, _)| k.get(PREFIX.len()..).and_then(|id| id.parse::<u64>().ok()) == Some(t.id.0))
        .map(|(_, v)| *v)
        .filter(|v| v.is_finite())
        .map_or(0, |v| v as i64)
}

/// Send note on/offs and controllers for MIDI clips overlapping the block.
pub(crate) fn schedule_notes(s: &Session, t: &Track, pos: Samples, frames: usize, inst: &mut PluginSlot, held: &mut HeldNotes, channel: Option<u8>) {
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = pos.saturating_sub(offset);
    let end = pos.saturating_add(frames as i64);
    for clip in t.clips() {
        if clip.muted {
            continue;
        }
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        if !clip.range().overlaps(&Range { start: pos, end }) {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        // MIDI Real-Time Properties (non-destructive velocity/transpose/duration/delay).
        let rtp = |name: &str, d: f64| Key::new(format_args!("rtp.{}.{name}", t.id.0)).map_or(d, |k| s.edit.value(k.as_str(), d));
        let (dv, dp, dur, delay) = if s.edit.values.is_empty() {
            (0i64, 0i64, 1.0f64, 0i64)
        } else {
            (
                rtp("velocity", 0.0).clamp(-127.0, 127.0) as i64,
                rtp("transpose", 0.0).clamp(-127.0, 127.0) as i64,
                rtp("duration", 100.0).clamp(1.0, 1000.0) / 100.0,
                rtp("delay", 0.0).clamp(-1e7, 1e7) as i64,
            )
        };
        // The block in clip ticks, one tick wider on each side so rounding never drops an event.
        let near = s.tempo.samples_to_ticks(pos, sr).saturating_sub(base).saturating_sub(1)
            ..=s.tempo.samples_to_ticks(end, sr).saturating_sub(base).saturating_add(1);
        for n0 in &sequence.notes {
            if n0.channel >= 16 {
                continue;
            }
            let mut n = *n0;
            n.channel = channel.unwrap_or(n.channel);
            n.velocity = (i64::from(n.velocity) + dv).clamp(1, 127) as u8;
            n.pitch = (i64::from(n.pitch) + dp).clamp(0, 127) as u8;
            n.length = ((n.length as f64 * dur) as i64).max(1);
            n.start = n.start.saturating_add(delay).max(0);
            // A note whose on and off both fall outside the block sends nothing: skip it before
            // converting its positions, so a long sequence costs a comparison per note per block.
            if n.start > *near.end() || n.start.saturating_add(n.length) < *near.start() {
                continue;
            }
            let on = s.tempo.tick_to_samples(base.saturating_add(n.start), sr);
            let off = s.tempo.tick_to_samples(base.saturating_add(n.start).saturating_add(n.length), sr).min(clip.end());
            if on >= clip.end() {
                continue;
            }
            if on >= pos && on < end {
                inst.plugin.note_on_channel(usize::try_from(on - pos).unwrap_or(0), n.channel, n.pitch, n.velocity);
                if let Some(h) = held.get_mut(usize::from(n.channel)).and_then(|keys| keys.get_mut(usize::from(n.pitch))) {
                    *h = true;
                }
            }
            if off >= pos && off < end {
                inst.plugin.note_off_channel(usize::try_from(off - pos).unwrap_or(0), n.channel, n.pitch);
                if let Some(h) = held.get_mut(usize::from(n.channel)).and_then(|keys| keys.get_mut(usize::from(n.pitch))) {
                    *h = false;
                }
            }
        }
        // Events before tick 0 are what a start trim hid: they don't play.
        for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 && c.channel < 16) {
            let tick = c.tick.saturating_add(delay).max(0);
            if !near.contains(&tick) {
                continue;
            }
            let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
            if at >= pos && at < end && at < clip.end() {
                inst.plugin.midi(usize::try_from(at - pos).unwrap_or(0), ctrl_message(channel.unwrap_or(c.channel), c.kind, c.value, dp));
            }
        }
    }
}

/// A controller event as a MIDI 1.0 message on its channel, with `transpose`
/// applied to a poly-pressure key.
fn ctrl_message(channel: u8, kind: CtrlKind, value: i32, transpose: i64) -> [u8; 3] {
    let v7 = value.clamp(0, 127) as u8;
    let mut message = match kind {
        CtrlKind::Cc(n) => [0xB0, n & 0x7F, v7],
        CtrlKind::PitchBend => {
            let v = value.saturating_add(8192).clamp(0, 16383);
            [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
        }
        CtrlKind::ChannelPressure => [0xD0, v7, 0],
        CtrlKind::PolyPressure(k) => [0xA0, (i64::from(k) + transpose).clamp(0, 127) as u8, v7],
        CtrlKind::Program => [0xC0, v7, 0],
    };
    message[0] |= channel & 0x0F;
    message
}

/// Before an instrument plays from `pos`: reset its controllers, then send each one the last value
/// it had on this track before `pos`.
pub(crate) fn chase_controllers(s: &Session, t: &Track, pos: Samples, inst: &mut PluginSlot, channel: Option<u8>) {
    for ch in channel.map_or(0..16, |ch| ch..ch.saturating_add(1)) {
        inst.plugin.midi(0, [0xB0 | ch, 121, 0]);
    }
    let pos = pos.saturating_sub(midi_offset(s, t));
    const BEND: usize = 128;
    const PRESSURE: usize = 129;
    const PROGRAM: usize = 130;
    let mut last = [[(i64::MIN, 0usize, 0i32); 131]; 16];
    let mut order = 0usize;
    let sr = s.sample_rate;
    for clip in t.clips() {
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        if clip.muted || clip.start >= pos {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        let until = pos.min(clip.end());
        for c in sequence.ctrls.iter().filter(|c| c.tick >= 0 && c.channel < 16) {
            // Match the scheduler's clip/event traversal order when messages share a timestamp.
            order = order.saturating_add(1);
            let slot = match c.kind {
                CtrlKind::Cc(n) => usize::from(n & 0x7F),
                CtrlKind::PitchBend => BEND,
                CtrlKind::ChannelPressure => PRESSURE,
                CtrlKind::Program => PROGRAM,
                CtrlKind::PolyPressure(_) => continue,
            };
            let at = s.tempo.tick_to_samples(base.saturating_add(c.tick), sr);
            if at < until
                && let Some(l) = last.get_mut(usize::from(channel.unwrap_or(c.channel))).and_then(|controllers| controllers.get_mut(slot))
                && at >= l.0
            {
                *l = (at, order, c.value);
            }
        }
    }
    for (channel, controllers) in last.iter().enumerate() {
        let reset_at = controllers.get(121).map_or((i64::MIN, 0), |c| (c.0, c.1));
        for (slot, &(at, order, value)) in controllers.iter().enumerate() {
            // The opening reset already establishes defaults. Replaying an old reset here could
            // clear a later expression/modulation value whose slot happens to precede 121.
            if slot == 121 {
                continue;
            }
            let kind = match slot {
                BEND => CtrlKind::PitchBend,
                PRESSURE => CtrlKind::ChannelPressure,
                PROGRAM => CtrlKind::Program,
                n => CtrlKind::Cc(u8::try_from(n).unwrap_or(0)),
            };
            if at != i64::MIN && !((at, order) < reset_at && reset_by_cc121(kind)) {
                inst.plugin.midi(0, ctrl_message(u8::try_from(channel).unwrap_or(0), kind, value, 0));
            }
        }
    }
}

/// RP-015 leaves bank, level/pan, sound/effect settings, channel modes and parameter data intact.
/// Earlier resettable values must not undo a recorded channel reset when seeking.
fn reset_by_cc121(kind: CtrlKind) -> bool {
    !matches!(kind, CtrlKind::Program | CtrlKind::Cc(0 | 6 | 7 | 10 | 32 | 38 | 39 | 42 | 70..=79 | 91..=97 | 120..=127))
}

/// Session audio shared with a sample-playing instrument.
struct PoolSample(Arc<SourceAudio>);

impl SampleSource for PoolSample {
    fn sample_rate(&self) -> f32 {
        self.0.buffer.sample_rate as f32
    }
    fn num_channels(&self) -> usize {
        self.0.buffer.channels.len()
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.0.channel(ch)
    }
}

/// Hands a strip's instrument the audio of its insert's sample source when it changed (a new
/// sample, a cleared one, or a new instance). Missing media plays nothing.
pub(crate) fn sync_sample(strip: &mut Strip, want: Option<&soundcraft_model::Insert>, s: &Session) {
    let audio = want.and_then(|i| i.sample).and_then(|id| s.pool.get(id));
    let same = match (strip.sample.as_ref(), audio) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    if let Some(inst) = strip.instrument.as_mut() {
        let shared = audio.map(|a| Arc::new(PoolSample(Arc::clone(a))) as Arc<dyn SampleSource>);
        inst.plugin.set_sample(shared);
    }
    strip.sample = audio.cloned();
}
