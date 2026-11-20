use super::db_to_gain;
use soundcraft_model::Session;
use soundcraft_time::Samples;

/// Blip length in seconds.
const LEN_S: f64 = 0.03;

pub fn render(s: &Session, pos: Samples, frames: usize, main: &mut [Vec<f32>]) {
    let sr = s.sample_rate;
    let rate = sr.as_f64();
    let len = (LEN_S * rate) as i64;
    let end = pos.saturating_add(frames as i64);
    let gain = f64::from(db_to_gain(s.edit.value("click.volume_db", -6.0).clamp(-60.0, 12.0) as f32));
    let accent = (note_hz(s.edit.value("click.accent_note", 84.0)), s.edit.value("click.accent_velocity", 127.0));
    let normal = (note_hz(s.edit.value("click.normal_note", 72.0)), s.edit.value("click.normal_velocity", 100.0));
    // Start one blip length early so a click that began in an earlier block still rings into this one.
    let t0 = s.tempo.samples_to_ticks(pos.saturating_sub(len), sr).max(0).saturating_sub(1);
    let mut tick = t0.saturating_sub(s.tempo.bar_beat_at_tick(t0).tick);
    // Blocks hold a few beats at most; the guard only protects against a malformed tempo map.
    for _ in 0..4096 {
        let at = s.tempo.tick_to_samples(tick, sr);
        if at >= end {
            break;
        }
        if at.saturating_add(len) > pos {
            let (hz, vel) = if s.tempo.bar_beat_at_tick(tick).beat == 1 { accent } else { normal };
            let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
            blip(main, at - pos, frames, hz, amp as f32, rate);
        }
        tick = tick.saturating_add(s.tempo.meter_at_tick(tick).ticks_per_beat());
    }
}

/// Adds one decaying sine blip whose first sample sits `rel` frames into the block (negative
/// when it began before the block) to every channel.
fn blip(main: &mut [Vec<f32>], rel: i64, frames: usize, hz: f64, amp: f32, rate: f64) {
    let len = (LEN_S * rate) as usize;
    let w = std::f64::consts::TAU * hz / rate;
    let first = usize::try_from(-rel).unwrap_or(0);
    for i in first..len {
        let Ok(at) = usize::try_from(rel + i as i64) else { continue };
        if at >= frames {
            break;
        }
        let env = (-6.0 * i as f64 / len as f64).exp() as f32;
        let v = ((w * i as f64).sin() as f32) * env * amp;
        for ch in main.iter_mut() {
            if let Some(d) = ch.get_mut(at) {
                *d += v;
            }
        }
    }
}

/// Click pitch for a MIDI note setting (clamped to the MIDI range).
fn note_hz(note: f64) -> f64 {
    soundcraft_dsp::midi_to_hz(note.clamp(0.0, 127.0))
}
