use super::clips::Key;
use super::live_midi::LiveNotes;
use super::strip::{Block, PluginSlot, Strip};
use super::{ClipContent, CtrlKind, Range, SampleSource, Samples, Session, SourceAudio, Track};
use std::sync::Arc;

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

/// A late Clock packet can put an attack and its release less than a millisecond
/// apart. Preserve the score duration from the delivered attack, after the release arrives.
pub(crate) struct LateRelease {
    pitch: u8,
    earliest: Samples,
    released: bool,
}

/// Send note on/offs and controllers for MIDI clips overlapping the block.
///
/// `live` holds the notes played live on the same instrument. Instruments end notes by pitch, so
/// a clip note-off is not sent while a live key holds that pitch: the key's own release ends it.
/// A note-off held back that way is told to `live`: when the key goes up earlier in the same
/// block, the note still lasts until the clip's note-off.
pub(crate) fn schedule_notes(
    s: &Session,
    t: &Track,
    mut blk: Block,
    inst: &mut PluginSlot,
    held: &mut Vec<u8>,
    live: &mut LiveNotes,
    late: &mut Vec<LateRelease>,
) {
    if blk.frames == 0 {
        return;
    }
    if blk.delivery == super::ClockDelivery::Predicted && blk.score.start < blk.pos && blk.pos < blk.score.end {
        // Opening the presentation window can release a whole note and its note-off
        // together. Keep their existing ordered offsets for this strip when the held
        // interval contains MIDI. Empty backlog leaves new attacks on their samples.
        let overdue = Block { score: Range { start: blk.score.start, end: blk.pos }, ..blk };
        if clip_events::<true>(s, t, overdue, inst, held, live, late) {
            blk.delivery = super::ClockDelivery::Wire;
        }
    }
    if blk.score.start < blk.score.end {
        clip_events::<false>(s, t, blk, inst, held, live, late);
    }
    let end = blk.pos.saturating_add(blk.frames as i64);
    late.retain(|note| {
        if note.released && note.earliest < end {
            release_clip_note(inst, held, live, note.pitch, note.earliest.saturating_sub(blk.pos).max(0) as usize);
            return false;
        }
        true
    });
}

// Detection uses the exact same clip boundaries, realtime properties and MIDI offsets
// as dispatch. The probe returns at the first event without changing voices or controls.
fn clip_events<const DETECT: bool>(
    s: &Session,
    t: &Track,
    blk: Block,
    inst: &mut PluginSlot,
    held: &mut Vec<u8>,
    live: &mut LiveNotes,
    late: &mut Vec<LateRelease>,
) -> bool {
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = blk.score.start.saturating_sub(offset);
    let end = blk.score.end.saturating_sub(offset);
    let note_start = blk.note_start.saturating_sub(offset);
    let scan_start = note_start.min(pos);
    // Process the requested boundary first even when a replacement note is stored
    // earlier in a different clip. Ordinary blocks need just the main pass.
    let compensated = note_start < pos;
    let mut deferred = 0u128;
    for &boundary in if compensated { &[true, false][..] } else { &[false][..] } {
        for clip in t.clips() {
            if clip.muted {
                continue;
            }
            let ClipContent::Midi { sequence } = &clip.content else { continue };
            if !clip.range().overlaps(&Range { start: scan_start, end }) {
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
            let near = s.tempo.samples_to_ticks(scan_start, sr).saturating_sub(base).saturating_sub(1)
                ..=s.tempo.samples_to_ticks(end, sr).saturating_sub(base).saturating_add(1);
            for n0 in &sequence.notes {
                let mut n = *n0;
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
                if on >= clip.end() || (compensated && (on == note_start) != boundary) {
                    continue;
                }
                // Clock compensation can put the first device buffer after the requested onset.
                // Deliver due attacks at frame zero without chasing notes from before the locate.
                let attack = on >= note_start && on < end && (on == note_start || off >= pos);
                if attack {
                    if DETECT {
                        return true;
                    }
                    let at = blk.score_offset(on.saturating_sub(pos));
                    let bit = 1u128 << n.pitch;
                    if deferred & bit != 0 {
                        release_clip_note(inst, held, live, n.pitch, at.min(1));
                        deferred &= !bit;
                    }
                    if let Some(index) = late.iter().position(|note| note.pitch == n.pitch) {
                        let previous = late.swap_remove(index);
                        if previous.released {
                            let release = previous.earliest.saturating_sub(blk.pos).max(0) as usize;
                            release_clip_note(inst, held, live, n.pitch, at.min(release));
                        }
                    }
                    // An attack delivered after its time, from a coalesced packet or from the
                    // backlog a wire-held block catches up on, keeps its score duration.
                    let backlog = blk.delivery == super::ClockDelivery::Coalesced || blk.score.start < blk.pos;
                    if backlog && on.saturating_add(offset) < blk.pos && off > on {
                        let earliest = blk.pos.saturating_add(at as i64).saturating_add(off.saturating_sub(on));
                        late.push(LateRelease { pitch: n.pitch, earliest, released: false });
                    }
                    inst.plugin.note_on(at, n.pitch, n.velocity);
                    if !held.contains(&n.pitch) && held.len() < 128 {
                        held.push(n.pitch);
                    }
                }
                if off < end && (off >= pos || attack) {
                    if DETECT {
                        return true;
                    }
                    if let Some(index) = late.iter().position(|note| note.pitch == n.pitch) {
                        let earliest = late[index].earliest;
                        if earliest >= blk.pos.saturating_add(blk.frames as i64) {
                            late[index].released = true;
                        } else {
                            late.swap_remove(index);
                            let at = blk.score_offset(off.saturating_sub(pos)).max(earliest.saturating_sub(blk.pos).max(0) as usize);
                            release_clip_note(inst, held, live, n.pitch, at);
                        }
                    } else if attack && on < pos && off <= pos {
                        deferred |= 1u128 << n.pitch;
                    } else {
                        release_clip_note(inst, held, live, n.pitch, blk.score_offset(off.saturating_sub(pos)));
                    }
                }
            }
            if boundary {
                continue;
            }
            // Events before tick 0 are what a start trim hid: they don't play.
            for c in sequence.ctrls.iter().filter(|c| c.tick >= 0) {
                let tick = c.tick.saturating_add(delay).max(0);
                if !near.contains(&tick) {
                    continue;
                }
                let at = s.tempo.tick_to_samples(base.saturating_add(tick), sr);
                if at >= pos && at < end && at < clip.end() {
                    if DETECT {
                        return true;
                    }
                    inst.plugin.midi(blk.score_offset(at.saturating_sub(pos)), ctrl_message(c.kind, c.value, dp));
                }
            }
        }
    }
    while deferred != 0 {
        let pitch = deferred.trailing_zeros() as u8;
        release_clip_note(inst, held, live, pitch, 1.min(blk.frames - 1));
        deferred &= deferred - 1;
    }
    false
}

fn release_clip_note(inst: &mut PluginSlot, held: &mut Vec<u8>, live: &mut LiveNotes, pitch: u8, at: usize) {
    if live.holds(pitch) {
        live.clip_off_held_back(pitch, at);
    } else {
        inst.plugin.note_off(at, pitch);
    }
    held.retain(|p| *p != pitch);
}

/// A controller event as a MIDI 1.0 message on channel 1 (where the notes go), with `transpose`
/// applied to a poly-pressure key.
fn ctrl_message(kind: CtrlKind, value: i32, transpose: i64) -> [u8; 3] {
    let v7 = value.clamp(0, 127) as u8;
    match kind {
        CtrlKind::Cc(n) => [0xB0, n & 0x7F, v7],
        CtrlKind::PitchBend => {
            let v = value.saturating_add(8192).clamp(0, 16383);
            [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
        }
        CtrlKind::ChannelPressure => [0xD0, v7, 0],
        CtrlKind::PolyPressure(k) => [0xA0, (i64::from(k) + transpose).clamp(0, 127) as u8, v7],
        CtrlKind::Program => [0xC0, v7, 0],
    }
}

/// Before an instrument plays from `pos`: reset its controllers, then send each one the last value
/// it had on this track before `pos`. Returns the control changes that were given a value, bit
/// `n` for CC `n`.
pub(crate) fn chase_controllers(s: &Session, t: &Track, pos: Samples, inst: &mut PluginSlot) -> u128 {
    inst.plugin.midi(0, [0xB0, 121, 0]);
    let pos = pos.saturating_sub(midi_offset(s, t));
    const BEND: usize = 128;
    const PRESSURE: usize = 129;
    const PROGRAM: usize = 130;
    // MIDI Real-Time Properties: the delay moves the track's controllers as it does when they
    // are played (`schedule_notes`), so a value is in the past here when it was already sent.
    let delay = if s.edit.values.is_empty() {
        0i64
    } else {
        Key::new(format_args!("rtp.{}.delay", t.id.0)).map_or(0.0, |k| s.edit.value(k.as_str(), 0.0)).clamp(-1e7, 1e7) as i64
    };
    let mut last = [(i64::MIN, 0i32); 131];
    let mut sent = 0u128;
    let sr = s.sample_rate;
    for clip in t.clips() {
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        if clip.muted || clip.start >= pos {
            continue;
        }
        let base = s.tempo.samples_to_ticks(clip.start, sr);
        let until = pos.min(clip.end());
        for c in sequence.ctrls.iter().filter(|c| c.tick >= 0) {
            let slot = match c.kind {
                CtrlKind::Cc(n) => usize::from(n & 0x7F),
                CtrlKind::PitchBend => BEND,
                CtrlKind::ChannelPressure => PRESSURE,
                CtrlKind::Program => PROGRAM,
                CtrlKind::PolyPressure(_) => continue,
            };
            let at = s.tempo.tick_to_samples(base.saturating_add(c.tick.saturating_add(delay).max(0)), sr);
            if at < until
                && let Some(l) = last.get_mut(slot)
                && at >= l.0
            {
                *l = (at, c.value);
            }
        }
    }
    for (slot, &(at, value)) in last.iter().enumerate() {
        let kind = match slot {
            BEND => CtrlKind::PitchBend,
            PRESSURE => CtrlKind::ChannelPressure,
            PROGRAM => CtrlKind::Program,
            n => CtrlKind::Cc(u8::try_from(n).unwrap_or(0)),
        };
        if at != i64::MIN {
            inst.plugin.midi(0, ctrl_message(kind, value, 0));
            if slot < 128 {
                sent |= 1u128 << slot;
            }
        }
    }
    sent
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
