//! Reusable interchange plan -> SoundCraft session. No processes, UI, or filesystem access.
//! Readers supply rational seconds. Endpoints are rounded independently (nearest, ties away
//! from zero), so adjacent edits share exactly the same boundary without cumulative drift.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
pub mod aaf;

use serde::{Deserialize, Serialize};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, Source, SourceId, Track, TrackId, TrackKind};
use soundcraft_time::{FrameRate, MAX_POSITION, SampleRate};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
#[error("interchange: {0}")]
pub struct ImportError(pub String);
type Result<T> = std::result::Result<T, ImportError>;
fn invalid(s: &str) -> ImportError {
    ImportError(s.into())
}

/// Rational seconds [numerator, denominator]. Both must be nonnegative, denominator > 0.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rational(pub i64, pub i64);
impl Rational {
    pub fn samples(self, hz: u32) -> Result<i64> {
        if self.0 < 0 || self.1 <= 0 {
            return Err(invalid("negative time or invalid denominator"));
        }
        let n = i128::from(self.0) * i128::from(hz);
        let d = i128::from(self.1);
        let v = (n + d / 2) / d;
        if v > i128::from(MAX_POSITION) {
            return Err(invalid("time exceeds SoundCraft limit"));
        }
        i64::try_from(v).map_err(|_| invalid("time overflow"))
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u32,
    pub name: String,
    pub sample_rate: u32,
    pub timecode_start: Rational,
    pub frame_rate: Option<TimecodeRate>,
    pub sources: Vec<Media>,
    pub tracks: Vec<ImportTrack>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimecodeRate {
    pub rate: Rational,
    pub drop: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub key: String,
    pub name: String,
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportTrack {
    pub name: String,
    pub channels: u16,
    pub clips: Vec<ImportClip>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportClip {
    pub name: String,
    pub source: String,
    pub start: Rational,
    pub end: Rational,
    pub offset: Rational,
}
#[derive(Debug, Serialize)]
pub struct HandleReport {
    pub clip: String,
    pub before: i64,
    pub after: i64,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub warnings: Vec<String>,
    pub handles: Vec<HandleReport>,
}
pub struct Imported {
    pub session: Session,
    pub report: Report,
}

fn layout(channels: u16) -> Result<ChannelFormat> {
    match channels {
        1 => Ok(ChannelFormat::Mono),
        2 => Ok(ChannelFormat::Stereo),
        _ => Err(invalid("only mono/stereo supported")),
    }
}
fn name(s: &str) -> Result<()> {
    if s.chars().count() > soundcraft_model::MAX_NAME_CHARS || s.is_empty() {
        return Err(invalid("name empty or too long for SoundCraft"));
    }
    Ok(())
}
fn frame_rate(tc: &TimecodeRate) -> Result<FrameRate> {
    let Rational(n, d) = tc.rate;
    if n <= 0 || d <= 0 {
        return Err(invalid("invalid frame rate"));
    }
    FrameRate::ALL
        .into_iter()
        .find(|r| {
            let (rn, rd) = r.rational();
            i128::from(n) * i128::from(rd) == i128::from(rn) * i128::from(d) && r.is_drop() == tc.drop
        })
        .ok_or_else(|| invalid("unsupported frame rate/drop combination"))
}
/// Build transactionally. On any error no partial session is returned. The caller must verify
/// media bytes against the metadata before using the plan; see the CLI for a bounded probe.
pub fn build_session(plan: Plan) -> Result<Imported> {
    if plan.schema_version != 1 {
        return Err(invalid("unsupported interchange schema"));
    }
    if plan.sources.len() > 100_000 || plan.tracks.len() > 100_000 || plan.tracks.iter().map(|t| t.clips.len()).sum::<usize>() > 100_000 {
        return Err(invalid("too many sources/tracks/clips"));
    }
    name(&plan.name)?;
    let sr = SampleRate::new(plan.sample_rate).map_err(|e| ImportError(e.to_string()))?;
    let mut s = Session::new(&plan.name, sr);
    s.timecode_start = plan.timecode_start.samples(sr.hz())?;
    if let Some(tc) = &plan.frame_rate {
        s.frame_rate = frame_rate(tc)?;
    }
    let mut ids = HashMap::new();
    for m in &plan.sources {
        name(&m.name)?;
        layout(m.channels)?;
        if m.sample_rate != sr.hz() || m.frames == 0 || m.frames > MAX_POSITION as u64 || m.path.is_empty() {
            return Err(invalid("invalid media metadata or mixed sample rates (resampling unsupported)"));
        }
        let id = SourceId(s.alloc());
        if ids.insert(m.key.as_str(), (id, m.frames, m.channels)).is_some() {
            return Err(invalid("duplicate source key"));
        }
        s.sources.push(Source {
            id,
            name: m.name.clone(),
            path: m.path.clone(),
            channels: m.channels,
            frames: m.frames,
            sample_rate: m.sample_rate,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
    }
    let mut report = Report { warnings: plan.warnings.clone(), handles: Vec::new() };
    for t in &plan.tracks {
        name(&t.name)?;
        let track_id = TrackId(s.alloc());
        // Construct directly: add_track intentionally renames duplicates, interchange must not.
        let mut track = Track::new(track_id, &t.name, TrackKind::Audio, layout(t.channels)?, s.next_track_color());
        let mut previous_end = 0;
        for c in &t.clips {
            name(&c.name)?;
            let (source, frames, channels) = ids.get(c.source.as_str()).ok_or_else(|| invalid("unknown source key"))?;
            if *channels != t.channels {
                return Err(invalid("clip/track layout mismatch"));
            }
            let start = c.start.samples(sr.hz())?;
            let end = c.end.samples(sr.hz())?;
            let offset = c.offset.samples(sr.hz())?;
            if end <= start || start < previous_end {
                return Err(invalid("zero/negative duration or overlapping/out-of-order clips"));
            }
            let length = end - start;
            let source_end = offset.checked_add(length).ok_or_else(|| invalid("source window overflow"))?;
            if source_end > *frames as i64 {
                return Err(invalid("clip extends beyond source; refusing truncated handles"));
            }
            let clip = Clip::audio(ClipId(s.alloc()), &c.name, *source, offset, start, length);
            track.playlist_mut().ok_or_else(|| invalid("missing playlist"))?.clips.push(clip);
            report.handles.push(HandleReport { clip: c.name.clone(), before: offset, after: *frames as i64 - source_end });
            previous_end = end;
        }
        s.tracks.push(track);
    }
    s.comments = "Imported by soundcraft-interchange schema v1; see accompanying import report for limitations.".into();
    Ok(Imported { session: s, report })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            schema_version: 1,
            name: "Test".into(),
            sample_rate: 48000,
            timecode_start: Rational(3600, 1),
            frame_rate: Some(TimecodeRate { rate: Rational(25, 1), drop: false }),
            warnings: vec![],
            sources: vec![Media { key: "s".into(), name: "src".into(), path: "a.wav".into(), channels: 1, frames: 480000, sample_rate: 48000 }],
            tracks: vec![ImportTrack {
                name: "DX".into(),
                channels: 1,
                clips: vec![ImportClip {
                    name: "cut".into(),
                    source: "s".into(),
                    start: Rational(1, 1),
                    end: Rational(3, 1),
                    offset: Rational(2, 1),
                }],
            }],
        }
    }
    #[test]
    fn positions_offsets_handles_and_roundtrip() {
        let result = build_session(plan()).unwrap();
        let c = &result.session.tracks[0].clips()[0];
        assert_eq!((c.start, c.length, c.source_offset()), (48000, 96000, 96000));
        assert_eq!((result.report.handles[0].before, result.report.handles[0].after), (96000, 288000));
        assert_eq!(result.session.timecode_start, 172800000);
        let back = Session::from_json(&result.session.to_json().unwrap()).unwrap();
        assert_eq!(back.tracks[0].clips()[0], *c);
    }
    #[test]
    fn adjacent_fractional_frame_boundaries_never_drift() {
        for n in 1..10000 {
            let a = Rational(n * 1001, 24000).samples(48000).unwrap();
            let b = Rational((n + 1) * 1001, 24000).samples(48000).unwrap();
            assert_eq!(b - a, 2002);
        }
        assert_eq!(Rational(1, 96000).samples(48000).unwrap(), 1);
    }
    #[test]
    fn fractional_sample_boundaries_use_absolute_endpoints() {
        let mut p = plan();
        p.sample_rate = 44100;
        p.sources[0].sample_rate = 44100;
        p.sources[0].frames = 4410000;
        p.tracks[0].clips = (0..1000)
            .map(|n| ImportClip {
                name: "frame".into(),
                source: "s".into(),
                start: Rational(n * 1001, 24000),
                end: Rational((n + 1) * 1001, 24000),
                offset: Rational(n * 1001, 24000),
            })
            .collect();
        let s = build_session(p).unwrap().session;
        let clips = s.tracks[0].clips();
        for pair in clips.windows(2) {
            assert_eq!(pair[0].end(), pair[1].start);
        }
        assert_eq!(clips[999].end(), 1839338);
        assert_eq!(clips.iter().map(|c| c.length).sum::<i64>(), 1839338);
    }
    #[test]
    fn invalid_windows_and_metadata_are_rejected() {
        let mut p = plan();
        p.tracks[0].clips[0].offset = Rational(9, 1);
        assert!(build_session(p).is_err());
        let mut p = plan();
        p.sources[0].sample_rate = 44100;
        assert!(build_session(p).is_err());
        assert!(Rational(1, 0).samples(48000).is_err());
        assert!(Rational(i64::MAX, 1).samples(48000).is_err());
        assert!(Rational(-1, 1).samples(48000).is_err());
    }
    #[test]
    fn stereo_names_and_order_are_preserved() {
        let mut p = plan();
        p.sources[0].channels = 2;
        p.tracks[0].channels = 2;
        p.tracks.push(ImportTrack { name: "DX".into(), channels: 1, clips: vec![] });
        let s = build_session(p).unwrap().session;
        assert_eq!(s.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["DX", "DX"]);
        assert_eq!(s.tracks[0].channels(), 2);
    }
}
