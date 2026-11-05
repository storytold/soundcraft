//! Clips: windows onto audio sources or MIDI sequences, placed on a playlist.

use crate::{ClipId, MAX_NAME_CHARS, SourceId, truncate_chars};
use soundcraft_midi::Sequence;
use soundcraft_time::{MAX_POSITION, Range, Samples};

/// Fade curve shapes (Pro Tools offers standard, equal-power, S-curve and custom presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum FadeShape {
    Linear,
    #[default]
    EqualPower,
    SCurve,
    Exponential,
    Logarithmic,
}

impl FadeShape {
    pub const ALL: [FadeShape; 5] = [FadeShape::Linear, FadeShape::EqualPower, FadeShape::SCurve, FadeShape::Exponential, FadeShape::Logarithmic];

    /// Fade-in gain at `t` ∈ [0,1] (fade-outs use `gain(1 - t)`).
    pub fn gain(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        match self {
            FadeShape::Linear => t,
            FadeShape::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            FadeShape::SCurve => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
            FadeShape::Exponential => t * t * t,
            FadeShape::Logarithmic => 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FadeShape::Linear => "Linear",
            FadeShape::EqualPower => "Equal Power",
            FadeShape::SCurve => "S-Curve",
            FadeShape::Exponential => "Exponential",
            FadeShape::Logarithmic => "Logarithmic",
        }
    }

    pub fn from_id(s: &str) -> Option<FadeShape> {
        let s = s.to_ascii_lowercase().replace(['-', '_', ' '], "");
        FadeShape::ALL.into_iter().find(|f| f.label().to_ascii_lowercase().replace(['-', ' '], "") == s)
    }
}

/// A fade at a clip boundary, `len` samples long.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Fade {
    pub len: Samples,
    pub shape: FadeShape,
    /// Continuous bend (-1..1); zero preserves the original shape exactly.
    #[serde(default)]
    pub curvature: f32,
}

impl Fade {
    /// The envelope used by both rendering and audio, with fixed zero/unity endpoints.
    pub fn gain(self, t: f32) -> f32 {
        let base = self.shape.gain(t);
        let curvature = if self.curvature.is_finite() { self.curvature.clamp(-1.0, 1.0) } else { 0.0 };
        if curvature == 0.0 || base <= 0.0 || base >= 1.0 {
            return base;
        }
        let weighted = base * (curvature * 4.0).exp2();
        weighted / (weighted + 1.0 - base)
    }

    /// Invert the bend at the midpoint so a vertical drag follows the pointer.
    pub fn curvature_for_midpoint_gain(self, gain: f32) -> f32 {
        let gain = if gain.is_finite() { gain.clamp(0.001, 0.999) } else { self.gain(0.5) };
        let base = self.shape.gain(0.5);
        (((gain / (1.0 - gain)).log2() - (base / (1.0 - base)).log2()) / 4.0).clamp(-1.0, 1.0)
    }

    pub fn sanitize(&mut self) {
        self.curvature = if self.curvature.is_finite() { self.curvature.clamp(-1.0, 1.0) } else { 0.0 };
    }
}

/// What a clip plays.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClipContent {
    /// Audio from `source`, starting `offset` samples into the file.
    Audio { source: SourceId, offset: Samples },
    /// MIDI notes, with ticks relative to the clip start.
    Midi { sequence: Sequence },
    /// Picture from the movie `source` ([`crate::VideoSource`], `Session::videos`), starting
    /// `offset` samples into the movie. Video clips carry no audio.
    Video { source: SourceId, offset: Samples },
}

/// A clip on a playlist.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    /// Timeline start (samples).
    pub start: Samples,
    /// Length (samples, > 0).
    pub length: Samples,
    pub content: ClipContent,
    /// Static clip gain in dB.
    #[serde(default)]
    pub gain_db: f32,
    /// Clip gain envelope breakpoints: (offset from clip start, dB). Empty = flat `gain_db`.
    #[serde(default)]
    pub gain_env: Vec<(Samples, f32)>,
    #[serde(default)]
    pub fade_in: Fade,
    #[serde(default)]
    pub fade_out: Fade,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub edit_locked: bool,
    #[serde(default)]
    pub time_locked: bool,
    #[serde(default)]
    pub rating: u8,
    /// Sync point offset from clip start.
    #[serde(default)]
    pub sync_point: Samples,
    /// Optional colour override (RGB).
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// The key in [`crate::Session::auto_names`] of the names that editing gives the clips it
    /// separates from this one (Clip List › Auto Rename). `None` keeps the clip's name.
    #[serde(default)]
    pub auto_name: Option<u64>,
    /// Elastic / Vari-speed ratio (1.0 = none) — applied by rendered processing.
    #[serde(default = "one")]
    pub stretch: f64,
    /// Reversed playback (AudioSuite Reverse renders into a new source; this is for previews).
    #[serde(default)]
    pub group: Option<u64>,
    /// Marks the clips made by `clip.loop` (the origin and its copies share one id), so
    /// `clip.unloop` still recognises the loop after the source's content is edited.
    #[serde(default)]
    pub loop_id: Option<u64>,
}

fn one() -> f64 {
    1.0
}

impl Clip {
    pub fn audio(id: ClipId, name: impl Into<String>, source: SourceId, offset: Samples, start: Samples, length: Samples) -> Clip {
        Clip {
            id,
            name: name.into(),
            start,
            length: length.max(1),
            content: ClipContent::Audio { source, offset },
            gain_db: 0.0,
            gain_env: Vec::new(),
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            muted: false,
            edit_locked: false,
            time_locked: false,
            rating: 0,
            sync_point: 0,
            color: None,
            auto_name: None,
            stretch: 1.0,
            group: None,
            loop_id: None,
        }
    }

    pub fn midi(id: ClipId, name: impl Into<String>, start: Samples, length: Samples, sequence: Sequence) -> Clip {
        let mut c = Clip::audio(id, name, SourceId(0), 0, start, length);
        c.content = ClipContent::Midi { sequence };
        c
    }

    /// A clip showing the movie `source` from `offset` samples into it.
    pub fn video(id: ClipId, name: impl Into<String>, source: SourceId, offset: Samples, start: Samples, length: Samples) -> Clip {
        let mut c = Clip::audio(id, name, SourceId(0), 0, start, length);
        c.content = ClipContent::Video { source, offset };
        c
    }

    pub fn end(&self) -> Samples {
        self.start.saturating_add(self.length)
    }

    pub fn range(&self) -> Range {
        Range { start: self.start, end: self.end() }
    }

    pub fn is_audio(&self) -> bool {
        matches!(self.content, ClipContent::Audio { .. })
    }

    pub fn is_video(&self) -> bool {
        matches!(self.content, ClipContent::Video { .. })
    }

    /// The audio source this clip plays (None for MIDI and video clips).
    pub fn source(&self) -> Option<SourceId> {
        match self.content {
            ClipContent::Audio { source, .. } => Some(source),
            ClipContent::Midi { .. } | ClipContent::Video { .. } => None,
        }
    }

    /// The movie this clip shows (video clips only).
    pub fn video_source(&self) -> Option<SourceId> {
        match self.content {
            ClipContent::Video { source, .. } => Some(source),
            ClipContent::Audio { .. } | ClipContent::Midi { .. } => None,
        }
    }

    /// Offset into the audio file or movie (0 for MIDI).
    pub fn source_offset(&self) -> Samples {
        match self.content {
            ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => offset,
            ClipContent::Midi { .. } => 0,
        }
    }

    /// Gain (linear) at `rel` samples from the clip start: clip gain × envelope × fades.
    pub fn gain_at(&self, rel: Samples) -> f32 {
        self.gain_at_with_fades(rel, self.fade_in, self.fade_out)
    }

    /// Evaluate temporary fades while retaining clip gain and its envelope.
    /// UI previews can use this without modifying the clip or rebuilding media.
    pub fn gain_at_with_fades(&self, rel: Samples, fade_in: Fade, fade_out: Fade) -> f32 {
        let mut db = self.gain_db;
        if !self.gain_env.is_empty() {
            db += env_value(&self.gain_env, rel);
        }
        let mut g = db_to_gain(db);
        if fade_in.len > 0 && rel < fade_in.len {
            g *= fade_in.gain(rel as f32 / fade_in.len as f32);
        }
        let from_end = self.length.saturating_sub(rel);
        if fade_out.len > 0 && from_end <= fade_out.len {
            g *= fade_out.gain(from_end as f32 / fade_out.len as f32);
        }
        g
    }

    /// Gain envelope value in dB at `rel` samples from the clip start (0 without an envelope).
    pub fn env_db_at(&self, rel: Samples) -> f32 {
        env_value(&self.gain_env, rel)
    }

    /// Move the start edge to `new_start` keeping material anchored (trim start).
    pub fn trim_start_to(&mut self, new_start: Samples) {
        // #907: these are public APIs, so they have to survive arguments a command would never
        // pass (commands clamp to `MAX_POSITION` first). `Samples` is `i64`, so an extreme
        // `new_start` against an extreme `start` wrapped in release and panicked in debug.
        // Never so far back that the length would pass `MAX_POSITION`: trimming the start keeps the end.
        let end = self.end();
        let new_start = new_start.max(end.saturating_sub(MAX_POSITION)).clamp(-MAX_POSITION, MAX_POSITION).min(end.saturating_sub(1));
        let delta = new_start.saturating_sub(self.start);
        // The mixer plays a stretched audio clip from `offset + rel / stretch`.
        let stretch = if self.stretch.is_finite() && self.stretch > 0.0 { self.stretch } else { 1.0 };
        match &mut self.content {
            ClipContent::Audio { offset, .. } if (stretch - 1.0).abs() >= 1e-9 => {
                *offset = offset.saturating_add((delta as f64 / stretch).round() as Samples);
            }
            ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => {
                *offset = offset.saturating_add(delta);
            }
            ClipContent::Midi { .. } => {}
        }
        // MIDI note ticks are re-anchored by the engine, which knows the tempo map.
        self.length = self.length.saturating_sub(delta).clamp(1, MAX_POSITION);
        self.start = new_start;
        if self.sync_point != 0 {
            self.sync_point = self.sync_point.saturating_sub(delta);
        }
        self.drop_stray_sync();
        self.clamp_fades();
        // Keep the gain at the cut when earlier breakpoints are dropped.
        if self.gain_env.iter().any(|&(t, _)| t < delta) && self.gain_env.iter().all(|&(t, _)| t != delta) {
            self.gain_env.insert(0, (delta, env_value(&self.gain_env, delta)));
        }
        self.gain_env.retain(|&(t, _)| t >= delta);
        self.gain_env.iter_mut().for_each(|(t, _)| *t = t.saturating_sub(delta));
    }

    /// Move the end edge to `new_end`.
    pub fn trim_end_to(&mut self, new_end: Samples) {
        // #907: saturating, like `trim_start_to`.
        let new_end = new_end.clamp(-MAX_POSITION, MAX_POSITION).max(self.start.saturating_add(1));
        self.length = new_end.saturating_sub(self.start).clamp(1, MAX_POSITION);
        self.drop_stray_sync();
        self.clamp_fades();
    }

    /// Keep both fades within the clip.
    pub fn clamp_fades(&mut self) {
        self.fade_in.len = self.fade_in.len.clamp(0, self.length);
        self.fade_out.len = self.fade_out.len.clamp(0, self.length);
    }

    /// A sync point (0 means none) that a trim left outside the clip no longer marks anything.
    fn drop_stray_sync(&mut self) {
        if !(0..self.length).contains(&self.sync_point) {
            self.sync_point = 0;
        }
    }

    /// Repair a clip read from a file: positions and lengths within ±[`MAX_POSITION`], as commands
    /// keep theirs, fades within the clip, a name that can be drawn, and a usable gain and stretch.
    pub fn sanitize(&mut self) {
        bound(&mut self.start);
        self.length = self.length.clamp(1, MAX_POSITION);
        bound(&mut self.sync_point);
        self.gain_env.iter_mut().for_each(|(at, _)| bound(at));
        match &mut self.content {
            ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => bound(offset),
            ClipContent::Midi { sequence } => bound_sequence(sequence),
        }
        self.clamp_fades();
        self.fade_in.sanitize();
        self.fade_out.sanitize();
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        if !self.gain_db.is_finite() {
            self.gain_db = 0.0;
        }
        if !self.stretch.is_finite() || self.stretch <= 0.0 {
            self.stretch = 1.0;
        }
    }
}

pub(crate) fn bound(at: &mut Samples) {
    *at = (*at).clamp(-MAX_POSITION, MAX_POSITION);
}

pub(crate) fn bound_sequence(s: &mut Sequence) {
    for n in &mut s.notes {
        bound(&mut n.start);
        n.length = n.length.clamp(1, MAX_POSITION);
    }
    s.ctrls.iter_mut().for_each(|c| bound(&mut c.tick));
}

fn env_value(env: &[(Samples, f32)], at: Samples) -> f32 {
    let mut prev: Option<(Samples, f32)> = None;
    for &(t, v) in env {
        if t >= at {
            return match prev {
                Some((pt, pv)) if t > pt => {
                    let elapsed = i128::from(at).saturating_sub(i128::from(pt));
                    let span = i128::from(t).saturating_sub(i128::from(pt));
                    pv + (v - pv) * (elapsed as f32 / span as f32)
                }
                _ => v,
            };
        }
        prev = Some((t, v));
    }
    prev.map_or(0.0, |(_, v)| v)
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= -144.0 { 0.0 } else { 10f32.powf(db.min(48.0) / 20.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_shapes_hit_endpoints() {
        for s in FadeShape::ALL {
            assert!(s.gain(0.0).abs() < 1e-6, "{s:?}");
            assert!((s.gain(1.0) - 1.0).abs() < 1e-6, "{s:?}");
            assert_eq!(s.gain(f32::NAN), 0.0);
            assert_eq!(FadeShape::from_id(s.label()), Some(s));
        }
    }

    #[test]
    fn fade_curvature_is_bounded_monotonic_and_inverts_at_the_midpoint() {
        for shape in FadeShape::ALL {
            let original = Fade { len: 1000, shape, curvature: 0.0 };
            for i in 0..=128 {
                let t = i as f32 / 128.0;
                assert_eq!(original.gain(t), shape.gain(t), "legacy curves must be unchanged");
            }
            for curvature in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                let fade = Fade { curvature, ..original };
                assert_eq!(fade.gain(0.0), 0.0);
                assert_eq!(fade.gain(1.0), 1.0);
                let mut previous = 0.0;
                for i in 0..=128 {
                    let gain = fade.gain(i as f32 / 128.0);
                    assert!(gain.is_finite() && (0.0..=1.0).contains(&gain));
                    assert!(gain >= previous, "{shape:?}, {curvature}");
                    previous = gain;
                }
                assert!((fade.curvature_for_midpoint_gain(fade.gain(0.5)) - curvature).abs() < 1e-5);
            }
            assert!((Fade { curvature: 0.5, ..original }).gain(0.5) > original.gain(0.5));
            assert!((Fade { curvature: -0.5, ..original }).gain(0.5) < original.gain(0.5));
        }
    }

    #[test]
    fn fade_curvature_loads_legacy_data_and_sanitizes_hostile_values() {
        let fade: Fade = serde_json::from_str(r#"{"len":100,"shape":"SCurve"}"#).unwrap();
        assert_eq!(fade.curvature, 0.0);
        for curvature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -10.0, 10.0] {
            let mut fade = Fade { curvature, ..fade };
            assert!(fade.gain(0.5).is_finite());
            fade.sanitize();
            assert!(fade.curvature.is_finite() && (-1.0..=1.0).contains(&fade.curvature));
            let roundtrip: Fade = serde_json::from_str(&serde_json::to_string(&fade).unwrap()).unwrap();
            assert_eq!(roundtrip, fade);
        }
    }

    #[test]
    fn temporary_fades_keep_clip_gain_and_envelope_without_mutating_the_clip() {
        let mut clip = Clip::audio(ClipId(1), "preview", SourceId(1), 0, 0, 1000);
        clip.gain_db = -6.0;
        clip.gain_env = vec![(0, 0.0), (1000, 6.0)];
        clip.fade_in = Fade { len: 100, shape: FadeShape::SCurve, curvature: 0.25 };
        clip.fade_out = Fade { len: 100, shape: FadeShape::Logarithmic, curvature: -0.1 };
        let before = clip.clone();
        let fade_in = Fade { len: 1000, shape: FadeShape::Linear, curvature: 0.5 };
        let fade_out = Fade { len: 1000, shape: FadeShape::Linear, curvature: -0.5 };
        assert!((clip.gain_at_with_fades(500, fade_in, fade_out) - db_to_gain(-3.0) * 0.16).abs() < 1e-6);
        let mut committed = clip.clone();
        committed.fade_in = fade_in;
        committed.fade_out = fade_out;
        for rel in [0, 50, 500, 999, 1000] {
            assert_eq!(clip.gain_at_with_fades(rel, fade_in, fade_out), committed.gain_at(rel));
        }
        for rel in [i64::MIN, i64::MAX] {
            assert!(clip.gain_at(rel).is_finite());
            assert!(clip.gain_at_with_fades(rel, fade_in, fade_out).is_finite());
        }
        assert_eq!(clip, before);
        clip.gain_env = vec![(i64::MIN, 0.0), (i64::MAX, 6.0)];
        assert!((clip.gain_at(500) - db_to_gain(-3.0)).abs() < 1e-6);
        assert!((clip.gain_at_with_fades(500, fade_in, fade_out) - db_to_gain(-3.0) * 0.16).abs() < 1e-6);
    }

    #[test]
    fn gain_with_fades() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 0, 100, 1000);
        c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        assert!(c.gain_at(0).abs() < 1e-6);
        assert!((c.gain_at(50) - 0.5).abs() < 1e-6);
        assert!((c.gain_at(500) - 1.0).abs() < 1e-6);
        assert!((c.gain_at(950) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn gain_envelope_interpolates() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 0, 0, 1000);
        c.gain_env = vec![(0, 0.0), (1000, -20.0)];
        assert!((c.gain_at(500) - db_to_gain(-10.0)).abs() < 1e-4);
    }

    #[test]
    fn trim_start_keeps_gain_envelope_at_cut() {
        for (env, cut, expected) in [
            (vec![(0, 0.0), (1000, -20.0)], 500, vec![(0, -10.0), (500, -20.0)]),
            (vec![(0, 0.0), (100, -20.0)], 500, vec![(0, -20.0)]),
            (vec![(0, 0.0), (500, -10.0), (1000, -20.0)], 500, vec![(0, -10.0), (500, -20.0)]),
            (vec![], 500, vec![]),
            (vec![(100, -3.0)], 0, vec![(100, -3.0)]),
            (vec![(100, -3.0)], -100, vec![(200, -3.0)]),
        ] {
            let mut clip = Clip::audio(ClipId(1), "a", SourceId(1), 100, 0, 1000);
            clip.gain_env = env;
            let gain_at_cut = clip.gain_at(cut);
            clip.trim_start_to(cut);
            assert_eq!(clip.gain_env, expected);
            if cut > 0 {
                assert!((clip.gain_at(0) - gain_at_cut).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn trims_keep_material_anchored() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 1000, 100, 1000);
        c.trim_start_to(300);
        assert_eq!((c.start, c.length, c.source_offset()), (300, 800, 1200));
        c.trim_end_to(500);
        assert_eq!(c.length, 200);
        c.trim_end_to(-50);
        assert_eq!(c.length, 1);
        // Regression: trimming the start must not leave a fade-out longer than the clip.
        let mut c = Clip::audio(ClipId(2), "b", SourceId(1), 0, 0, 1000);
        c.fade_out = Fade { len: 800, shape: FadeShape::Linear, curvature: 0.0 };
        c.trim_start_to(900);
        assert!(c.fade_out.len <= c.length);
    }

    /// #907: `trim_start_to` / `trim_end_to` are public APIs, so they have to survive arguments a
    /// command would never pass. `Samples` is `i64`, so an extreme `new_start` against an extreme
    /// `start` wrapped in release and panicked in debug. Every position stays within
    /// `±MAX_POSITION` and every length within `1..=MAX_POSITION`.
    #[test]
    fn trims_saturate_on_extreme_arguments() {
        for &new_start in &[Samples::MIN, -(MAX_POSITION * 2), -1, 0, MAX_POSITION, MAX_POSITION * 2, Samples::MAX] {
            let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 1000, MAX_POSITION / 2, 1000);
            c.trim_start_to(new_start);
            assert!((-(MAX_POSITION as i128)..=(MAX_POSITION as i128)).contains(&(c.start as i128)), "start {new_start}: {}", c.start);
            assert!((1..=MAX_POSITION).contains(&c.length), "start {new_start}: length {}", c.length);
            assert_eq!(c.end(), MAX_POSITION / 2 + 1000, "start {new_start} moved the end");
        }
        for &new_end in &[Samples::MIN, -(MAX_POSITION * 2), -1, 0, MAX_POSITION, MAX_POSITION * 2, Samples::MAX] {
            let mut c = Clip::audio(ClipId(2), "b", SourceId(1), 1000, -(MAX_POSITION / 2), 1000);
            c.trim_end_to(new_end);
            assert!((1..=MAX_POSITION).contains(&c.length), "end {new_end}: length {}", c.length);
            assert!(c.end() - c.start == c.length, "end {new_end}: start {} end {}", c.start, c.end());
        }
    }

    #[test]
    fn trim_start_of_stretched_audio_advances_offset_by_source_time() {
        // The mixer plays source `offset + rel / stretch`, so a split at timeline 1000 of a 2x clip
        // must start the right half at source sample 500.
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 0, 0, 2000);
        c.stretch = 2.0;
        c.trim_start_to(1000);
        assert_eq!(c.source_offset(), 500);
        let mut c = Clip::audio(ClipId(2), "b", SourceId(1), 0, 0, 2000);
        c.stretch = 0.5;
        c.trim_start_to(1000);
        assert_eq!(c.source_offset(), 2000);
        let mut c = Clip::audio(ClipId(3), "c", SourceId(1), 0, 0, 2000);
        c.trim_start_to(1000);
        assert_eq!(c.source_offset(), 1000);
    }
}
