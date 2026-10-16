//! Reusable whole-clip snapshots, independent of every timeline instance.

use crate::{Clip, ClipContent, ClipDefinitionId, ClipId, Fade, MAX_NAME_CHARS, Session, SourceId};
use soundcraft_midi::{CtrlKind, Sequence};
use soundcraft_time::{MAX_POSITION, Samples};
use std::collections::BTreeMap;

/// The unwarped material retained by a rendered Elastic clip.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ElasticOriginal {
    pub source: SourceId,
    pub offset: Samples,
    pub length: Samples,
}

/// Per-clip settings with no reference to the old timeline owner's id.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClipMetadata {
    #[serde(default)]
    pub effects: BTreeMap<String, f64>,
    #[serde(default)]
    pub effects_bypassed: bool,
    #[serde(default)]
    pub elastic_original: Option<ElasticOriginal>,
    #[serde(default)]
    pub elastic_ratio: Option<f64>,
    #[serde(default)]
    pub source_bpm: Option<f64>,
    #[serde(default)]
    pub original_length: Option<f64>,
    #[serde(default)]
    pub pitch: Option<f64>,
}

impl ClipMetadata {
    /// The public clip-effects command accepts arbitrary bounded parameter suffixes.
    pub fn is_effect_key(key: &str) -> bool {
        key.len() <= 256 && !key.contains('\0')
    }
}

/// Sample-based clip geometry; MIDI events retain their relative tick positions.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClipTemplate {
    pub content: ClipContent,
    pub length: Samples,
    pub gain_db: f32,
    pub gain_env: Vec<(Samples, f32)>,
    pub fade_in: Fade,
    pub fade_out: Fade,
    pub muted: bool,
    pub rating: u8,
    pub sync_point: Samples,
    pub color: Option<[u8; 3]>,
    pub stretch: f64,
}

impl From<&Clip> for ClipTemplate {
    fn from(clip: &Clip) -> Self {
        Self {
            content: clip.content.clone(),
            length: clip.length,
            gain_db: clip.gain_db,
            gain_env: clip.gain_env.clone(),
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
            muted: clip.muted,
            rating: clip.rating,
            sync_point: clip.sync_point,
            color: clip.color,
            stretch: clip.stretch,
        }
    }
}

impl ClipTemplate {
    /// Fresh instances are unlocked and do not join the original group or loop.
    pub fn instantiate(&self, id: ClipId, name: &str, at: Samples) -> Clip {
        let mut clip = Clip::audio(id, name, SourceId(0), 0, at, self.length);
        clip.content = self.content.clone();
        clip.gain_db = self.gain_db;
        clip.gain_env = self.gain_env.clone();
        clip.fade_in = self.fade_in;
        clip.fade_out = self.fade_out;
        clip.muted = self.muted;
        clip.rating = self.rating;
        clip.sync_point = self.sync_point;
        clip.color = self.color;
        clip.stretch = self.stretch;
        clip
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClipDefinition {
    pub id: ClipDefinitionId,
    pub name: String,
    pub template: ClipTemplate,
    #[serde(default)]
    pub metadata: ClipMetadata,
    #[serde(default)]
    pub midi_original: Option<Sequence>,
}

impl ClipDefinition {
    pub const MAX_COUNT: usize = 100_000;
    pub const MAX_POINTS: usize = 1_000_000;
    pub const MAX_METADATA: usize = 65_536;
    pub const MAX_BYTES: usize = 128 * 1024 * 1024;

    /// Also retains the media needed by Remove Warp after all instances are gone.
    pub fn audio_sources(&self) -> impl Iterator<Item = SourceId> + '_ {
        let source = match self.template.content {
            ClipContent::Audio { source, .. } => Some(source),
            _ => None,
        };
        source.into_iter().chain(self.metadata.elastic_original.as_ref().map(|original| original.source))
    }

    pub fn video_source(&self) -> Option<SourceId> {
        match self.template.content {
            ClipContent::Video { source, .. } => Some(source),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let t = &self.template;
        if self.id.0 == 0
            || self.id.0 >= u64::MAX - 1
            || self.name.is_empty()
            || self.name.len() > MAX_NAME_CHARS * 4
            || self.name.chars().count() > MAX_NAME_CHARS
            || self.name.contains('\0')
            || !(1..=MAX_POSITION).contains(&t.length)
            || !t.gain_db.is_finite()
            || !t.stretch.is_finite()
            || t.stretch <= 0.0
            || t.rating > 5
            || !(0..=t.length).contains(&t.sync_point)
            || t.gain_env.len() > Self::MAX_POINTS
            // Loaded curves and right trims retain interpolation anchors outside the window.
            || t.gain_env.iter().any(|&(at, value)| !(-MAX_POSITION..=MAX_POSITION).contains(&at) || !value.is_finite())
            || t.gain_env.windows(2).any(|p| p.first().zip(p.get(1)).is_some_and(|(a, b)| a.0 >= b.0))
            || [t.fade_in, t.fade_out]
                .iter()
                .any(|fade| !(0..=t.length).contains(&fade.len) || !fade.curvature.is_finite() || !(-1.0..=1.0).contains(&fade.curvature))
        {
            return Err("invalid reusable clip geometry or gain");
        }
        match &t.content {
            ClipContent::Audio { source, offset } | ClipContent::Video { source, offset } if source.0 != 0 && (0..=MAX_POSITION).contains(offset) => {
            }
            ClipContent::Midi { sequence } => validate_sequence(sequence)?,
            _ => return Err("invalid reusable media reference"),
        }
        if let Some(original) = &self.midi_original {
            if !matches!(t.content, ClipContent::Midi { .. }) {
                return Err("saved performance requires a MIDI definition");
            }
            validate_sequence(original)?;
        }
        let m = &self.metadata;
        if m.effects.len() > Self::MAX_METADATA
            || m.effects.iter().any(|(key, value)| !ClipMetadata::is_effect_key(key) || !value.is_finite())
            || [m.elastic_ratio, m.source_bpm, m.original_length, m.pitch].into_iter().flatten().any(|v| !v.is_finite())
            || m.elastic_ratio.is_some_and(|v| v <= 0.0)
        {
            return Err("invalid reusable clip metadata");
        }
        if let Some(original) = &m.elastic_original
            && (!matches!(t.content, ClipContent::Audio { .. })
                || original.source.0 == 0
                || original.source.0 >= (1u64 << 53)
                || !(0..=MAX_POSITION).contains(&original.offset)
                || !(1..=MAX_POSITION).contains(&original.length)
                || original.offset.checked_add(original.length).is_none_or(|end| end > MAX_POSITION))
        {
            return Err("invalid reusable elastic original");
        }
        Ok(())
    }

    /// Validate declarations, and any decoded data that is currently available.
    pub fn validate_media(&self, session: &Session, require_loaded: bool) -> Result<(), &'static str> {
        self.validate()?;
        if soundcraft_time::SampleRate::new(session.sample_rate.hz()).is_err() {
            return Err("invalid session sample rate");
        }
        match self.template.content {
            ClipContent::Audio { source, offset } => {
                let last = offset as f64 + (self.template.length - 1) as f64 / self.template.stretch;
                validate_audio(session, source, offset, last, require_loaded)?;
            }
            ClipContent::Video { source, offset } => {
                let video = session.video(source).ok_or("movie source metadata is unavailable")?;
                if !video.duration.is_finite() || video.duration < 0.0 || !video.frame_rate.is_finite() || video.frame_rate < 0.0 {
                    return Err("invalid movie source metadata");
                }
                let last = offset.checked_add(self.template.length - 1).ok_or("movie window overflow")?;
                let extent = video.duration * session.sample_rate.as_f64();
                if last > MAX_POSITION || !extent.is_finite() || (extent > 0.0 && last as f64 >= extent) {
                    return Err("reusable clip exceeds its movie source");
                }
            }
            ClipContent::Midi { .. } => {}
        }
        if let Some(original) = &self.metadata.elastic_original {
            let last = original.offset.checked_add(original.length - 1).ok_or("elastic original overflow")?;
            validate_audio(session, original.source, original.offset, last as f64, require_loaded)?;
        }
        Ok(())
    }
}

fn validate_audio(session: &Session, source: SourceId, offset: Samples, last: f64, require_loaded: bool) -> Result<(), &'static str> {
    let media = session.source(source).ok_or("audio source metadata is unavailable")?;
    if media.channels == 0
        || media.channels > 64
        || media.sample_rate != session.sample_rate.hz()
        || media.frames == 0
        || media.frames > MAX_POSITION as u64
        || !last.is_finite()
        || last < offset as f64
        || last >= media.frames as f64
        || last > MAX_POSITION as f64
    {
        return Err("reusable clip exceeds valid audio source geometry");
    }
    if let Some(audio) = session.pool.get(source) {
        if audio.buffer.sample_rate != media.sample_rate
            || audio.buffer.channels.len() != usize::from(media.channels)
            || audio.buffer.channels.iter().any(|channel| channel.len() as u64 != media.frames)
        {
            return Err("decoded audio differs from source metadata");
        }
    } else if require_loaded {
        return Err("decoded audio is unavailable; restore its media before placement");
    }
    Ok(())
}

pub(crate) fn validate_sequence(sequence: &Sequence) -> Result<(), &'static str> {
    if sequence.notes.len().checked_add(sequence.ctrls.len()).is_none_or(|n| n > ClipDefinition::MAX_POINTS)
        || sequence.notes.iter().any(|n| {
            n.start < 0
                || n.length <= 0
                || n.start.checked_add(n.length).is_none_or(|end| end > MAX_POSITION)
                || n.pitch > 127
                || n.channel > 15
                || !(1..=127).contains(&n.velocity)
                || n.release_velocity > 127
        })
        || sequence.ctrls.iter().any(|c| {
            !(0..=MAX_POSITION).contains(&c.tick)
                || c.channel > 15
                || match c.kind {
                    CtrlKind::Cc(n) => n > 127 || !(0..=127).contains(&c.value),
                    CtrlKind::PolyPressure(n) => n > 127 || !(0..=127).contains(&c.value),
                    CtrlKind::PitchBend => !(-8192..=8191).contains(&c.value),
                    _ => !(0..=127).contains(&c.value),
                }
        })
        || sequence.notes.windows(2).any(|p| p.first().zip(p.get(1)).is_some_and(|(a, b)| a.start > b.start))
        || sequence.ctrls.windows(2).any(|p| p.first().zip(p.get(1)).is_some_and(|(a, b)| a.tick > b.tick))
    {
        return Err("invalid reusable MIDI sequence");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelFormat, FadeShape, Source, TrackKind};
    use soundcraft_audio_io::FileFormat;

    fn session() -> Session {
        let mut session = Session::default();
        session.sources.push(Source {
            id: SourceId(1),
            name: "tone".into(),
            path: "tone.wav".into(),
            channels: 1,
            frames: 256,
            sample_rate: 48_000,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        session.clip_definitions.push(ClipDefinition {
            id: ClipDefinitionId(2),
            name: "Reusable".into(),
            template: ClipTemplate::from(&Clip::audio(ClipId(3), "original", SourceId(1), 5, 100, 64)),
            metadata: ClipMetadata::default(),
            midi_original: None,
        });
        session
    }

    #[test]
    fn whole_template_retains_settings_and_clears_instance_identity() {
        let mut clip = Clip::audio(ClipId(9), "Original", SourceId(1), 7, 100, 80);
        clip.gain_db = -3.0;
        clip.gain_env = vec![(0, -1.0), (79, 2.0)];
        clip.fade_in = Fade { len: 16, shape: FadeShape::SCurve, curvature: 0.35 };
        clip.fade_out = Fade { len: 20, shape: FadeShape::EqualPower, curvature: -0.4 };
        clip.muted = true;
        clip.rating = 4;
        clip.sync_point = 17;
        clip.color = Some([1, 2, 3]);
        clip.stretch = 1.5;
        clip.edit_locked = true;
        clip.time_locked = true;
        clip.group = Some(88);
        clip.loop_id = Some(89);
        let template = ClipTemplate::from(&clip);
        let placed = template.instantiate(ClipId(20), "Reusable", 300);
        assert_eq!(ClipTemplate::from(&placed), template);
        assert_eq!((placed.id, placed.name.as_str(), placed.start), (ClipId(20), "Reusable", 300));
        assert!(!placed.edit_locked && !placed.time_locked);
        assert_eq!((placed.group, placed.loop_id), (None, None));
    }

    #[test]
    fn definition_sessions_write_v2_and_survive_without_timeline_clips() {
        let session = session();
        assert_eq!(session.format_version, Session::FORMAT_VERSION);
        let text = session.to_json().unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap()["format_version"], 2);
        let back = Session::from_json(&text).unwrap();
        assert_eq!(back.format_version, 2);
        assert_eq!(back.clip_definitions, session.clip_definitions);
        assert_eq!(back.next_id, 3);
        assert_eq!(back.all_clips().count(), 0);
    }

    #[test]
    fn v1_without_definitions_loads_and_v1_with_definitions_is_rejected() {
        let ordinary = Session::default();
        let mut value = serde_json::to_value(&ordinary).unwrap();
        value.as_object_mut().unwrap().remove("clip_definitions");
        assert!(Session::from_json(&value.to_string()).unwrap().clip_definitions.is_empty());
        assert_eq!(serde_json::from_str::<serde_json::Value>(&ordinary.to_json().unwrap()).unwrap()["format_version"], 1);
        let mut value = serde_json::to_value(session()).unwrap();
        assert!(Session::from_json(&value.to_string()).is_err());
        value["format_version"] = 3.into();
        assert!(matches!(Session::from_json(&value.to_string()), Err(crate::SessionError::Version(3))));
    }

    #[test]
    fn colliding_definition_ids_include_alternate_clips_and_group_links() {
        for reserved in [10u64, 11, 12] {
            let mut session = session();
            let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            let mut clip = Clip::audio(ClipId(10), "other", SourceId(1), 0, 0, 16);
            clip.group = Some(11);
            clip.loop_id = Some(12);
            session.track_mut(track).unwrap().playlists.push(crate::Playlist { name: "alternate".into(), clips: vec![clip] });
            session.clip_definitions[0].id = ClipDefinitionId(reserved);
            assert!(session.validate_definitions().is_err());
        }
        let mut session = session();
        session.clip_definitions.push(session.clip_definitions[0].clone());
        assert!(session.to_json().is_err());
    }

    #[test]
    fn invalid_definition_geometry_is_rejected_before_sanitization() {
        let original = session();
        for variant in 0..6 {
            let mut session = original.clone();
            let definition = &mut session.clip_definitions[0];
            match variant {
                0 => definition.template.length = 0,
                1 => definition.template.fade_in.curvature = 2.0,
                2 => definition.template.gain_env = vec![(5, 0.0), (4, 1.0)],
                3 => definition.template.stretch = f64::INFINITY,
                4 => definition.name = "x".repeat(MAX_NAME_CHARS + 1),
                _ => {
                    definition.metadata.effects.insert("gain".into(), f64::NAN);
                }
            }
            assert!(session.validate_definitions().is_err(), "variant {variant}");
        }
        let mut value = serde_json::to_value(original).unwrap();
        value["format_version"] = 2.into();
        value["clip_definitions"][0]["template"]["length"] = 0.into();
        assert!(Session::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn definitions_validate_retained_sources_and_saved_midi_data() {
        let mut session = session();
        session.clip_definitions[0].metadata.elastic_original = Some(ElasticOriginal { source: SourceId(8), offset: 0, length: 64 });
        assert!(session.validate_definitions().is_err());
        session.clip_definitions[0].metadata.elastic_original = Some(ElasticOriginal { source: SourceId(1), offset: 200, length: 64 });
        assert!(session.validate_definitions().is_err());
        session.clip_definitions[0].metadata.elastic_original = None;
        session.clip_definitions[0].midi_original = Some(Sequence::default());
        assert!(session.validate_definitions().is_err());
        session.clip_definitions[0].template.content = ClipContent::Midi {
            sequence: Sequence {
                notes: vec![],
                ctrls: vec![soundcraft_midi::CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::PitchBend, value: 8192 }],
            },
        };
        assert!(session.validate_definitions().is_err());
    }

    #[test]
    fn exhausted_v2_allocator_is_rejected_and_loaded_definitions_reserve_ids() {
        let original = session();
        for variant in 0..3 {
            let mut value = serde_json::to_value(&original).unwrap();
            value["format_version"] = 2.into();
            match variant {
                0 => value["clip_definitions"][0]["id"] = u64::MAX.into(),
                1 => value["next_id"] = u64::MAX.into(),
                _ => value["sources"][0]["id"] = u64::MAX.into(),
            }
            assert!(Session::from_json(&value.to_string()).is_err());
        }
        let mut loaded = Session::from_json(&original.to_json().unwrap()).unwrap();
        let definition = loaded.clip_definitions[0].id;
        let track = loaded.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        assert!(track.0 > definition.0);
        assert!(loaded.validate_definitions().is_ok());
    }

    #[test]
    fn signed_gain_envelope_anchors_survive_round_trip_with_exact_bounds() {
        let mut stored = session();
        let curve = vec![(-16, -12.0), (80, 0.0)];
        stored.clip_definitions[0].template.gain_env = curve.clone();
        let loaded = Session::from_json(&stored.to_json().unwrap()).unwrap();
        assert_eq!(loaded.clip_definitions[0].template.gain_env, curve);
        assert_eq!(loaded.clip_definitions[0].template.length, 64);

        let endpoints = vec![(-MAX_POSITION, -12.0), (MAX_POSITION, 0.0)];
        stored.clip_definitions[0].template.gain_env = endpoints.clone();
        let text = stored.to_json().unwrap();
        assert_eq!(Session::from_json(&text).unwrap().clip_definitions[0].template.gain_env, endpoints);
        for outside in [-MAX_POSITION - 1, MAX_POSITION + 1] {
            let mut invalid = stored.clone();
            invalid.clip_definitions[0].template.gain_env = vec![(outside, -1.0)];
            assert!(invalid.to_json().is_err());
            let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
            value["clip_definitions"][0]["template"]["gain_env"] = serde_json::json!([[outside, -1.0]]);
            assert!(Session::from_json(&value.to_string()).is_err());
        }
    }
}
