use super::db_to_gain;
use soundcraft_model::Session;

/// Blip length in seconds.
const LEN_S: f64 = 0.03;

pub(crate) struct Tail {
    frames_rendered: i64,
    hz: f64,
    amp: f32,
}

pub(crate) fn render(s: &Session, block: super::strip::Block, main: &mut [Vec<f32>], tails: &mut Vec<Tail>, clocked: bool) {
    let frames = block.frames;
    let pos = block.score.start;
    let sr = s.sample_rate;
    let rate = sr.as_f64();
    let len = (LEN_S * rate) as i64;
    tails.retain_mut(|tail| {
        blip(main, -tail.frames_rendered, frames, tail.hz, tail.amp, rate);
        tail.frames_rendered += frames as i64;
        tail.frames_rendered < len
    });
    let end = block.score.end;
    let gain = f64::from(db_to_gain(s.edit.value("click.volume_db", -6.0).clamp(-60.0, 12.0) as f32));
    let accent = (note_hz(s.edit.value("click.accent_note", 84.0)), s.edit.value("click.accent_velocity", 127.0));
    let normal = (note_hz(s.edit.value("click.normal_note", 72.0)), s.edit.value("click.normal_velocity", 100.0));
    let from = if clocked { pos } else { pos.saturating_sub(len) };
    let t0 = s.tempo.samples_to_ticks(from, sr).max(0).saturating_sub(1);
    let mut tick = t0.saturating_sub(s.tempo.bar_beat_at_tick(t0).tick);
    // Blocks hold a few beats at most; the guard only protects against a malformed tempo map.
    for _ in 0..4096 {
        let at = s.tempo.tick_to_samples(tick, sr);
        if at >= end {
            break;
        }
        if at >= from {
            let (hz, vel) = if s.tempo.bar_beat_at_tick(tick).beat == 1 { accent } else { normal };
            let amp = gain * (vel.clamp(1.0, 127.0) / 127.0);
            let offset = if clocked { block.score_offset(at - pos) as i64 } else { at - pos };
            blip(main, offset, frames, hz, amp as f32, rate);
            let frames_rendered = frames as i64 - offset;
            if clocked && frames_rendered < len && tails.len() < tails.capacity() {
                tails.push(Tail { frames_rendered, hz, amp: amp as f32 });
            }
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

#[cfg(test)]
mod tests {
    use crate::MixEngine;
    use soundcraft_model::Session;
    use soundcraft_time::SampleRate;

    #[test]
    fn clock_limit_defers_a_click_and_plays_its_tail_once() {
        let mut song = Session { sample_rate: SampleRate::HZ_48000, ..Default::default() };
        song.edit.click = true;
        let beat = song.tempo.tick_to_samples(soundcraft_time::TICKS_PER_QUARTER, song.sample_rate);
        let mut reference = MixEngine::new(48000., 2048);
        reference.metronome = true;
        let mut expected = vec![vec![0.; 2048]; 2];
        reference.render(&song, beat, 2048, &mut expected);
        assert!(expected[0].iter().any(|x| x.abs() > 0.01));
        let mut mix = MixEngine::new(48000., 128);
        mix.metronome = true;
        let mut block = vec![vec![0.; 128]; 2];
        let mut pcm = Vec::new();
        for i in 0..20 {
            let limit = soundcraft_time::TICKS_PER_QUARTER - i64::from(i < 3);
            mix.render_with_clock_limit(&song, beat + i * 128, 128, &mut block, Some(limit));
            pcm.extend_from_slice(&block[0]);
        }
        assert!(pcm[..384].iter().all(|x| *x == 0.));
        assert_eq!(&pcm[384..384 + 2048], &expected[0]);
    }
}
