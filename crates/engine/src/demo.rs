//! A demo session whose audio is synthesised in code (no third-party media):
//! a 24-bar groove at 104 BPM with drums, bass, keys, pad, lead, a reverb aux and a master.

use crate::Engine;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{ChannelFormat, Clip, Fade, FadeShape, Insert, MarkerKind, Route, SendSlot, Session, TrackKind};
use soundcraft_time::{SampleRate, TICKS_PER_QUARTER};

const BPM: f64 = 104.0;
const BARS: usize = 24;

/// A small deterministic PRNG for noise.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

fn insert(id: &str) -> Insert {
    let mut i = Insert::new(id);
    if let Some(info) = soundcraft_dsp::plugin_info(id) {
        for p in info.params {
            i.params.insert(p.id.to_string(), p.default);
        }
    }
    i
}

fn beat_len(sr: f64) -> usize {
    (sr * 60.0 / BPM).round() as usize
}

fn kick(sr: f64, frames: usize, out: &mut [f32], at: usize, vel: f32) {
    let len = (sr * 0.45) as usize;
    let mut phase = 0.0f64;
    for i in 0..len.min(frames.saturating_sub(at)) {
        let t = i as f64 / sr;
        let f = 48.0 + 110.0 * (-t * 28.0).exp();
        phase += 2.0 * std::f64::consts::PI * f / sr;
        let env = (-t * 7.5).exp() as f32;
        let click = if i < 64 { (64 - i) as f32 / 64.0 * 0.3 } else { 0.0 };
        if let Some(o) = out.get_mut(at + i) {
            *o += ((phase.sin() as f32) * env + click) * vel * 0.9;
        }
    }
}

fn snare(sr: f64, frames: usize, out: &mut [f32], at: usize, vel: f32, rng: &mut Rng) {
    let len = (sr * 0.3) as usize;
    let mut lp = 0.0f32;
    for i in 0..len.min(frames.saturating_sub(at)) {
        let t = i as f64 / sr;
        let n = rng.next();
        lp += 0.55 * (n - lp);
        let body = ((2.0 * std::f64::consts::PI * 185.0 * t).sin() * (-t * 22.0).exp()) as f32;
        let env = (-t * 14.0).exp() as f32;
        if let Some(o) = out.get_mut(at + i) {
            *o += ((n - lp) * env * 0.8 + body * 0.5) * vel * 0.7;
        }
    }
}

fn hat(sr: f64, frames: usize, out: &mut [f32], at: usize, vel: f32, open: bool, rng: &mut Rng) {
    let len = (sr * if open { 0.35 } else { 0.06 }) as usize;
    let mut prev = 0.0f32;
    for i in 0..len.min(frames.saturating_sub(at)) {
        let t = i as f64 / sr;
        let n = rng.next();
        let hp = n - prev;
        prev = n;
        let env = (-t * if open { 9.0 } else { 70.0 }).exp() as f32;
        if let Some(o) = out.get_mut(at + i) {
            *o += hp * env * vel * 0.25;
        }
    }
}

fn drums(sr: f64, frames: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut k = vec![0.0; frames];
    let mut s = vec![0.0; frames];
    let mut h = vec![0.0; frames];
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let beat = beat_len(sr);
    for bar in 0..BARS {
        let b0 = bar * beat * 4;
        // Intro: hats only for 4 bars.
        let full = bar >= 4;
        if full {
            for (pos, vel) in [(0usize, 1.0f32), (2 * beat + beat / 2, 0.8), (3 * beat, 0.6)] {
                kick(sr, frames, &mut k, b0 + pos, vel);
            }
            for pos in [beat, 3 * beat] {
                snare(sr, frames, &mut s, b0 + pos, 1.0, &mut rng);
            }
            if bar % 4 == 3 {
                snare(sr, frames, &mut s, b0 + 3 * beat + beat / 2, 0.5, &mut rng);
                snare(sr, frames, &mut s, b0 + 3 * beat + 3 * beat / 4, 0.7, &mut rng);
            }
        }
        for i in 0..8 {
            let open = full && i == 7;
            hat(sr, frames, &mut h, b0 + i * beat / 2, if i % 2 == 0 { 1.0 } else { 0.6 }, open, &mut rng);
        }
    }
    (k, s, h)
}

/// Chord roots per bar (A minor, F, C, G progression), as MIDI notes.
const PROG: [[u8; 4]; 4] = [[57, 60, 64, 69], [53, 57, 60, 65], [48, 52, 55, 60], [55, 59, 62, 67]];

fn bass(sr: f64, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    let beat = beat_len(sr);
    let mut lp = 0.0f64;
    for bar in 4..BARS {
        let root = f64::from(PROG[bar % 4][0]) - 24.0;
        let pattern = [(0usize, 0.0f64, 1.5f64), (2, 0.0, 0.5), (3, 12.0, 0.5), (5, 0.0, 0.5), (6, 7.0, 1.0)];
        for (eighth, semi, beats) in pattern {
            let at = bar * beat * 4 + eighth * beat / 2;
            let len = (beats * beat as f64) as usize;
            let f = soundcraft_dsp::midi_to_hz(root + semi);
            let mut ph = 0.0f64;
            for i in 0..len {
                let Some(o) = out.get_mut(at + i) else { break };
                ph = (ph + f / sr) % 1.0;
                let saw = 2.0 * ph - 1.0;
                let t = i as f64 / sr;
                let cutoff = 0.02 + 0.12 * (-t * 9.0).exp();
                lp += cutoff * (saw - lp);
                let env = (1.0 - (-t * 200.0).exp()) * if i + 400 > len { (len - i) as f64 / 400.0 } else { 1.0 };
                *o += (lp * env * 0.55) as f32;
            }
        }
    }
    out
}

fn pad(sr: f64, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0; frames];
    let mut r = vec![0.0; frames];
    let bar_len = beat_len(sr) * 4;
    for bar in 0..BARS {
        let chord = PROG[bar % 4];
        let at = bar * bar_len;
        for (vi, n) in chord.iter().enumerate() {
            let f = soundcraft_dsp::midi_to_hz(f64::from(*n) + 12.0);
            for (side, buf) in [(-1.0f64, &mut l), (1.0, &mut r)] {
                let det = 1.0 + side * 0.0035 * (vi as f64 + 1.0);
                let mut ph = vi as f64 * 0.17;
                for i in 0..bar_len {
                    let Some(o) = buf.get_mut(at + i) else { break };
                    ph = (ph + f * det / sr) % 1.0;
                    let tri = 1.0 - 4.0 * (ph - 0.5).abs();
                    let t = i as f64 / bar_len as f64;
                    let env = (t * 8.0).min(1.0) * ((1.0 - t) * 6.0).min(1.0);
                    *o += (tri * env * 0.07) as f32;
                }
            }
        }
    }
    (l, r)
}

fn note(pitch: u8, start: i64, len: i64, vel: u8) -> Note {
    Note { pitch, velocity: vel, release_velocity: 64, channel: 0, start, length: len }
}

fn keys_sequence() -> Sequence {
    let q = TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for bar in 0..(BARS - 4) as i64 {
        let chord = PROG[(bar as usize + 4) % 4];
        for (k, offs) in [(0i64, 0i64), (1, q + q / 2), (2, 3 * q)] {
            let _ = k;
            for n in &chord[1..] {
                seq.notes.push(note(*n, bar * 4 * q + offs, q - 40, 92));
            }
        }
    }
    seq.sort();
    seq
}

fn lead_sequence() -> Sequence {
    let q = TICKS_PER_QUARTER;
    let e = q / 2;
    let phrase: [(u8, i64, i64); 10] =
        [(76, 0, 3), (74, 3, 1), (72, 4, 2), (74, 6, 2), (76, 8, 4), (79, 12, 2), (77, 14, 2), (76, 16, 6), (72, 22, 2), (74, 24, 8)];
    let mut seq = Sequence::default();
    for rep in 0..2i64 {
        for (p, st, ln) in phrase {
            seq.notes.push(note(p, rep * 32 * e + st * e, ln * e - 30, 100));
        }
    }
    seq.sort();
    seq
}

/// Build the demo session.
pub fn demo_session() -> Session {
    let sr = SampleRate::HZ_48000;
    let mut s = Session::new("Midnight Groove", sr);
    let _ = s.tempo.set_tempo(0, BPM);
    let srf = sr.as_f64();
    let bar = beat_len(srf) * 4;
    let total = bar * BARS + (srf * 2.0) as usize;
    let (k, sn, h) = drums(srf, total);
    let b = bass(srf, total);
    let (pl, pr) = pad(srf, total);
    let verb_bus = s.add_bus("Verb", ChannelFormat::Stereo);
    // Exact bar positions from the tempo map (bar index 0 = bar 1).
    let tempo = s.tempo.clone();
    let bar_at = move |n: usize| tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: n as i64 + 1, beat: 1, tick: 0 }, sr);
    let add_audio = |s: &mut Session, name: &str, chans: Vec<Vec<f32>>, start_bar: usize, color: Option<usize>| {
        let fmt = ChannelFormat::for_channels(chans.len());
        let buf = AudioBuffer { sample_rate: sr.hz(), channels: chans };
        let src = crate::io::add_source(s, name, buf, None, FileFormat::Wav);
        let t = s.add_track(TrackKind::Audio, fmt, Some(name));
        if let (Some(c), Some(tr)) = (color, s.track_mut(t)) {
            tr.color = soundcraft_model::TRACK_COLORS[c % 16];
        }
        let start = bar_at(start_bar);
        let len = total as i64 - start;
        let cid = s.new_clip_id();
        let mut clip = Clip::audio(cid, name, src, start, start, len - (srf * 1.5) as i64);
        clip.fade_out = Fade { len: (srf * 0.5) as i64, shape: FadeShape::SCurve };
        crate::edit::place_clip(s, t, clip);
        t
    };
    let tk = add_audio(&mut s, "Kick", vec![k], 4, Some(7));
    let ts = add_audio(&mut s, "Snare", vec![sn], 4, Some(6));
    let th = add_audio(&mut s, "Hats", vec![h], 0, Some(5));
    let tb = add_audio(&mut s, "Bass", vec![b], 4, Some(10));
    let tp = add_audio(&mut s, "Pad", vec![pl, pr], 0, Some(1));
    // Split the snare into verse/chorus clips so the playlist shows several clips.
    for at in [12, 20] {
        crate::edit::separate_at(&mut s, ts, bar_at(at));
        crate::edit::separate_at(&mut s, tk, bar_at(at));
    }
    for (i, at) in [8usize, 16].iter().enumerate() {
        crate::edit::separate_at(&mut s, tb, bar_at(*at));
        let _ = i;
    }
    // Instrument tracks with MIDI.
    let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let lead = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    for (t, seq, start_bar, bars, color) in [(keys, keys_sequence(), 4usize, BARS - 4, 2usize), (lead, lead_sequence(), 12, 8, 9)] {
        let start = bar_at(start_bar);
        let len = bar_at(start_bar + bars) - start;
        let cid = s.new_clip_id();
        let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
        crate::edit::place_clip(&mut s, t, Clip::midi(cid, name, start, len, seq));
        if let Some(tr) = s.track_mut(t) {
            tr.instrument = Some(insert("subtractive_synth"));
            tr.color = soundcraft_model::TRACK_COLORS[color];
            tr.mixer.volume_db = if t == keys { -9.0 } else { -12.0 };
        }
    }
    // Reverb aux.
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
    if let Some(tr) = s.track_mut(aux) {
        tr.mixer.input = Route::Bus(verb_bus);
        tr.mixer.inserts[0] = Some(insert("plate_reverb"));
        tr.mixer.volume_db = -4.0;
        tr.color = soundcraft_model::TRACK_COLORS[12];
    }
    for (t, lvl) in [(ts, -10.0f32), (tp, -8.0), (keys, -12.0), (lead, -6.0)] {
        if let Some(tr) = s.track_mut(t) {
            let mut snd = SendSlot::new(Route::Bus(verb_bus));
            snd.level_db = lvl;
            tr.mixer.sends[0] = Some(snd);
        }
    }
    // Mix moves.
    for (t, db, pan) in [(tk, -3.0f32, 0.0f32), (ts, -4.0, 0.0), (th, -10.0, 0.35), (tb, -5.0, 0.0), (tp, -8.0, 0.0)] {
        if let Some(tr) = s.track_mut(t) {
            tr.mixer.volume_db = db;
            if tr.mixer.pan.len() == 1 {
                tr.mixer.pan = vec![pan];
            }
        }
    }
    if let Some(tr) = s.track_mut(tk) {
        tr.mixer.inserts[0] = Some(insert("eq_7band"));
        tr.mixer.inserts[1] = Some(insert("compressor"));
    }
    if let Some(tr) = s.track_mut(tb) {
        tr.mixer.inserts[0] = Some(insert("compressor"));
        tr.mixer.inserts[1] = Some(insert("saturator"));
    }
    if let Some(tr) = s.track_mut(tp) {
        tr.mixer.inserts[0] = Some(insert("chorus"));
        // Pad swells in with volume automation.
        let l = tr.lane_mut(&soundcraft_model::AutoParam::Volume);
        l.set_point(0, -30.0);
        l.set_point((4 * bar) as i64, -8.0);
        l.set_point((20 * bar) as i64, -8.0);
        l.set_point((24 * bar) as i64, -24.0);
    }
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    if let Some(tr) = s.track_mut(m) {
        tr.mixer.inserts[0] = Some(insert("maximizer"));
    }
    // Markers.
    for (name, at) in [("Intro", 1i64), ("Verse", 5), ("Chorus", 13), ("Outro", 21)] {
        let pos = s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: at, beat: 1, tick: 0 }, sr);
        s.add_marker(name, MarkerKind::Marker, pos, pos);
    }
    s.edit.selected_tracks = vec![tk];
    s.edit.selection = soundcraft_time::Range::point((4 * bar) as i64);
    s.edit.zoom.samples_per_px = (total as f64 / 1400.0).max(1.0);
    s.edit.main_counter = soundcraft_time::TimeFormat::BarsBeats;
    s
}

/// The demo is deterministic; build it once per process and clone (sources are shared `Arc`s).
pub fn cached_demo_session() -> Session {
    static DEMO: std::sync::OnceLock<Session> = std::sync::OnceLock::new();
    DEMO.get_or_init(demo_session).clone()
}

pub fn demo_engine() -> Engine {
    Engine::new(cached_demo_session())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_builds_and_renders_sound() {
        let s = demo_session();
        assert!(s.tracks.len() >= 9);
        assert!(s.master().is_some());
        let sr = s.sample_rate;
        let r = soundcraft_time::Range::new(sr.samples(10.0), sr.samples(12.0));
        let out = soundcraft_mix::render_range(&s, r, 1024);
        let pk = out.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(pk > 0.05, "demo should be audible, peak {pk}");
        assert!(pk <= 1.5, "demo should not be wildly clipping, peak {pk}");
    }
}
