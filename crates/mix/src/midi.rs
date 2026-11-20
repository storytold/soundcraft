use super::clips::Key;
use super::instances::is_third_party;
use super::live_midi::LiveNotes;
use super::strip::{PluginSlot, Strip};
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

/// Send note on/offs and controllers for MIDI clips overlapping the block.
///
/// Live MIDI uses the instrument's first channel. The clip and a live key may share
/// a pitch there (or on a single-part instrument), so defer that clip's release.
pub(crate) fn schedule_notes(
    s: &Session,
    t: &Track,
    pos: Samples,
    frames: usize,
    inst: &mut PluginSlot,
    held: &mut [u128; 16],
    live: &mut LiveNotes,
) {
    let multichannel = is_third_party(&inst.id);
    let sr = s.sample_rate;
    // MIDI Track Offsets: the track's notes sound `offset` samples later (earlier if negative), so
    // the block looks that far back into the clips.
    let offset = midi_offset(s, t);
    let pos = pos.saturating_sub(offset);
    let end = pos.saturating_add(frames as i64);
    // Offsets can arrive out of order when clips overlap. Keep the final event by
    // sample for the block-end bitmap, with a later traversal event winning ties.
    let mut latest = [[0usize; 128]; 16];
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
            let held_channel = if multichannel { usize::from(n.channel) } else { 0 };
            if on >= pos && on < end {
                let at = usize::try_from(on - pos).unwrap_or(0);
                inst.plugin.note_on_channel(at, n.channel, n.pitch, n.velocity);
                update_clip_held(&mut latest, held, held_channel, n.pitch, at, true);
            }
            if off >= pos && off < end {
                let at = usize::try_from(off - pos).unwrap_or(0);
                if held_channel == 0 && live.holds(n.pitch) {
                    live.clip_off_held_back(n.pitch, at);
                } else {
                    inst.plugin.note_off_channel(at, n.channel, n.pitch);
                }
                update_clip_held(&mut latest, held, held_channel, n.pitch, at, false);
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
                inst.plugin.midi(usize::try_from(at - pos).unwrap_or(0), ctrl_message(c.channel, c.kind, c.value, dp));
            }
        }
    }
}

fn update_clip_held(latest: &mut [[usize; 128]; 16], held: &mut [u128; 16], channel: usize, pitch: u8, offset: usize, sounding: bool) {
    let Some(previous) = latest.get_mut(channel).and_then(|keys| keys.get_mut(usize::from(pitch))) else { return };
    let Some(held) = held.get_mut(channel) else { return };
    if offset >= *previous {
        *previous = offset;
        let bit = 1u128 << pitch;
        *held = (*held & !bit) | if sounding { bit } else { 0 };
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

const BEND: usize = 128;
const PRESSURE: usize = 129;
const PROGRAM: usize = 130;
const PROGRAM_BANK_MSB: usize = 131;
const PROGRAM_BANK_LSB: usize = 132;
const CHASE_PER_CHANNEL: usize = PROGRAM_BANK_LSB + 1;
const CHASE_SLOTS: usize = 16 * CHASE_PER_CHANNEL;

#[derive(Clone, Copy, Default)]
pub(crate) struct ChaseEvent {
    time: i64,
    order: usize,
    message: [u8; 3],
}

/// Reset controllers, then replay the latest values and recorded resets in playback order.
/// Shared or omni receivers therefore hear the latest event regardless of its MIDI channel.
pub(crate) fn chase_controllers(s: &Session, t: &Track, pos: Samples, inst: &mut PluginSlot, last: &mut Vec<ChaseEvent>, channels: u16) -> u128 {
    let multichannel = is_third_party(&inst.id);
    let reset = if multichannel { channels } else { channels & 1 };
    for channel in (0..16u8).filter(|channel| reset & (1 << channel) != 0) {
        inst.plugin.midi(0, [0xB0 | channel, 121, 0]);
    }
    let pos = pos.saturating_sub(midi_offset(s, t));
    last.clear();
    last.resize_with(CHASE_SLOTS, ChaseEvent::default);
    // MIDI Real-Time Properties: the delay moves the track's controllers as it does when they
    // are played (`schedule_notes`), so a value is in the past here when it was already sent.
    let delay = if s.edit.values.is_empty() {
        0i64
    } else {
        Key::new(format_args!("rtp.{}.delay", t.id.0)).map_or(0.0, |k| s.edit.value(k.as_str(), 0.0)).clamp(-1e7, 1e7) as i64
    };
    let mut sent = 0u128;
    let sr = s.sample_rate;
    let candidates = || {
        t.clips()
            .iter()
            .filter_map(|clip| {
                let ClipContent::Midi { sequence } = &clip.content else { return None };
                if clip.muted || clip.start >= pos {
                    return None;
                }
                let base = s.tempo.samples_to_ticks(clip.start, sr);
                let until = pos.min(clip.end());
                Some(sequence.ctrls.iter().filter_map(move |c| {
                    if c.tick < 0 || c.channel >= 16 || (multichannel && channels & (1 << c.channel) == 0) {
                        return None;
                    }
                    let at = s.tempo.tick_to_samples(base.saturating_add(c.tick.saturating_add(delay).max(0)), sr);
                    (at < until).then_some((c, at))
                }))
            })
            .flatten()
            .enumerate()
    };
    for (order, (c, at)) in candidates() {
        let slot = match c.kind {
            CtrlKind::Cc(n) => usize::from(n & 0x7F),
            CtrlKind::PitchBend => BEND,
            CtrlKind::ChannelPressure => PRESSURE,
            CtrlKind::Program => PROGRAM,
            CtrlKind::PolyPressure(_) => continue,
        };
        let index = usize::from(c.channel) * CHASE_PER_CHANNEL + slot;
        if let Some(event) = last.get_mut(index)
            && (event.order == 0 || at >= event.time)
        {
            *event = ChaseEvent { time: at, order: order.saturating_add(1), message: ctrl_message(c.channel, c.kind, c.value, 0) };
        }
    }
    // A newer Bank Select stays pending for the next Program Change. Recover the older
    // selection separately when it was overwritten, even when clips overlap in time.
    let mut needs_bank = [[false; 2]; 16];
    for (needed, slots) in needs_bank.iter_mut().zip(last.as_chunks::<CHASE_PER_CHANNEL>().0) {
        let Some(program) = slots.get(PROGRAM).filter(|program| program.order != 0) else { continue };
        for (needed, cc) in needed.iter_mut().zip([0, 32]) {
            *needed = slots.get(cc).is_some_and(|bank| bank.order != 0 && (bank.time, bank.order) > (program.time, program.order));
        }
    }
    if needs_bank.iter().flatten().any(|&needed| needed) {
        for (order, (c, at)) in candidates() {
            let (component, slot) = match c.kind {
                CtrlKind::Cc(0) => (0, PROGRAM_BANK_MSB),
                CtrlKind::Cc(32) => (1, PROGRAM_BANK_LSB),
                _ => continue,
            };
            let channel = usize::from(c.channel);
            if !needs_bank.get(channel).and_then(|parts| parts.get(component)).copied().unwrap_or(false) {
                continue;
            }
            let base = channel * CHASE_PER_CHANNEL;
            let Some(program) = last.get(base + PROGRAM).copied() else { continue };
            let order = order.saturating_add(1);
            if (at, order) < (program.time, program.order)
                && let Some(bank) = last.get_mut(base + slot)
                && (bank.order == 0 || at >= bank.time)
            {
                *bank = ChaseEvent { time: at, order, message: ctrl_message(c.channel, c.kind, c.value, 0) };
            }
        }
    }
    last.retain(|event| event.order != 0);
    last.sort_unstable_by_key(|event| (event.time, event.order));
    for event in last {
        inst.plugin.midi(0, event.message);
        let [status, cc, _] = event.message;
        if status & 0xF0 == 0xB0 && (status & 0x0F == 0 || !multichannel) {
            sent |= 1u128 << cc;
        }
    }
    sent
}

pub(crate) fn chase_scratch() -> Vec<ChaseEvent> {
    Vec::with_capacity(CHASE_SLOTS)
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
