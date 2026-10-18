//! Bounded whole-clip capture and placement plans. No media is rendered or rewritten.

use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{
    ChannelFormat, Clip, ClipContent, ClipDefinition, ClipDefinitionId, ClipId, ClipMetadata, ClipTemplate, ElasticOriginal, MAX_NAME_CHARS, Session,
    TrackId, TrackKind,
};
use soundcraft_time::MAX_POSITION;
use std::collections::BTreeSet;

fn bad(command: &str, message: impl Into<String>) -> EngineError {
    cmd::bad(command, message)
}

#[derive(Default)]
struct Budget {
    points: usize,
    bytes: usize,
    metadata: usize,
}

impl Budget {
    fn charge(total: &mut usize, amount: usize, limit: usize) -> Result<()> {
        *total =
            total.checked_add(amount).filter(|n| *n <= limit).ok_or_else(|| bad("clip.definitions", "reusable clip storage exceeds its budget"))?;
        Ok(())
    }
    fn sequence(&mut self, sequence: &soundcraft_midi::Sequence) -> Result<()> {
        let count = sequence.notes.len().checked_add(sequence.ctrls.len()).ok_or_else(|| bad("clip.definitions", "event count overflow"))?;
        Self::charge(&mut self.points, count, ClipDefinition::MAX_POINTS)?;
        Self::charge(&mut self.bytes, count.saturating_mul(64), ClipDefinition::MAX_BYTES)
    }
    fn template(&mut self, content: &ClipContent, points: usize, name: &str) -> Result<()> {
        Self::charge(&mut self.points, points, ClipDefinition::MAX_POINTS)?;
        Self::charge(&mut self.bytes, name.len().saturating_add(512).saturating_add(points.saturating_mul(16)), ClipDefinition::MAX_BYTES)?;
        if let ClipContent::Midi { sequence } = content {
            self.sequence(sequence)?;
        }
        Ok(())
    }
    fn effect(&mut self, key: &str) -> Result<()> {
        Self::charge(&mut self.metadata, 1, ClipDefinition::MAX_METADATA)?;
        Self::charge(&mut self.bytes, key.len().saturating_add(64), ClipDefinition::MAX_BYTES)
    }
    fn definition(&mut self, definition: &ClipDefinition) -> Result<()> {
        self.template(&definition.template.content, definition.template.gain_env.len(), &definition.name)?;
        if let Some(original) = &definition.midi_original {
            self.sequence(original)?;
        }
        for key in definition.metadata.effects.keys() {
            self.effect(key)?;
        }
        Ok(())
    }
}

struct Ids {
    next: u64,
}
impl Ids {
    fn take(&mut self) -> Result<u64> {
        let id = self.next;
        self.next = id.checked_add(1).filter(|next| *next < u64::MAX).ok_or_else(|| bad("clip.definitions", "session id space is exhausted"))?;
        Ok(id)
    }
}

fn inventory(session: &Session) -> Result<(Ids, Budget)> {
    session.validate_definitions().map_err(|error| bad("clip.definitions", error.to_string()))?;
    if session.tracks.len() > 4096
        || session.edit.values.len().saturating_add(session.edit.flags.len()) > ClipDefinition::MAX_METADATA
        || session.midi_originals.len() > ClipDefinition::MAX_COUNT
        || session.pool.len() > ClipDefinition::MAX_COUNT
        || session.tracks.iter().try_fold(0usize, |n, t| n.checked_add(t.playlists.len())).is_none_or(|n| n > ClipDefinition::MAX_COUNT)
    {
        return Err(bad("clip.definitions", "session exceeds capture planning limits"));
    }
    let mut seen = BTreeSet::new();
    let mut maximum = 0;
    let ids = session
        .tracks
        .iter()
        .map(|t| t.id.0)
        .chain(session.all_clips().map(|c| c.id.0))
        .chain(session.sources.iter().map(|s| s.id.0))
        .chain(session.videos.iter().map(|v| v.id.0))
        .chain(session.busses.iter().map(|b| b.id.0))
        .chain(session.groups.iter().map(|g| g.id.0))
        .chain(session.markers.iter().map(|m| m.id.0))
        .chain(session.clip_definitions.iter().map(|d| d.id.0));
    for (index, id) in ids.enumerate() {
        if index >= ClipDefinition::MAX_COUNT * 8 || id == 0 || !seen.insert(id) {
            return Err(bad("clip.definitions", "invalid or excessive session id namespace"));
        }
        maximum = maximum.max(id);
    }
    for id in session
        .all_clips()
        .flat_map(|c| [c.group, c.loop_id])
        .flatten()
        .chain(session.midi_originals.keys().copied())
        .chain(session.pool.ids().map(|id| id.0))
    {
        maximum = maximum.max(id);
    }
    // Reserve stale per-clip owners too: a repaired counter must not inherit their effects.
    for key in session.edit.values.keys().chain(&session.edit.flags) {
        if let Some(id) = metadata_owner(key) {
            maximum = maximum.max(id);
        }
    }
    let next = session.next_id.max(maximum.checked_add(1).ok_or_else(|| bad("clip.definitions", "session id space is exhausted"))?).max(1);
    let mut budget = Budget::default();
    for definition in &session.clip_definitions {
        budget.definition(definition)?;
    }
    Ok((Ids { next }, budget))
}

fn metadata_owner(key: &str) -> Option<u64> {
    ["clip.bpm.", "clip.len0.", "clip.pitch.", "clip_fx.bypass."]
        .into_iter()
        .find_map(|prefix| key.strip_prefix(prefix))
        .or_else(|| ["clip_fx.", "elastic."].into_iter().find_map(|prefix| key.strip_prefix(prefix)?.split_once('.').map(|(id, _)| id)))?
        .parse()
        .ok()
}

fn metadata(session: &Session, clip: &Clip, budget: &mut Budget) -> Result<ClipMetadata> {
    let value = |family: &str| session.edit.values.get(&format!("{family}.{}", clip.id.0)).copied();
    let elastic = |suffix: &str| session.edit.values.get(&format!("elastic.{}.{suffix}", clip.id.0)).copied();
    let mut output = ClipMetadata {
        effects_bypassed: session.edit.flag(&format!("clip_fx.bypass.{}", clip.id.0)),
        elastic_ratio: elastic("ratio"),
        source_bpm: value("clip.bpm"),
        original_length: value("clip.len0"),
        pitch: value("clip.pitch"),
        ..ClipMetadata::default()
    };
    output.elastic_original = match (elastic("src"), elastic("off"), elastic("len")) {
        (None, None, None) => None,
        (Some(source), Some(offset), Some(length))
            if source.is_finite()
                && source.fract() == 0.0
                && (1.0..(1u64 << 53) as f64).contains(&source)
                && offset.is_finite()
                && offset.fract() == 0.0
                && (0.0..=MAX_POSITION as f64).contains(&offset)
                && length.is_finite()
                && length.fract() == 0.0
                && (1.0..=MAX_POSITION as f64).contains(&length) =>
        {
            Some(ElasticOriginal { source: soundcraft_model::SourceId(source as u64), offset: offset as i64, length: length as i64 })
        }
        _ => return Err(bad("clip.capture", "invalid elastic original metadata")),
    };
    let prefix = format!("clip_fx.{}.", clip.id.0);
    for (key, value) in session.edit.values.range(prefix.clone()..) {
        let Some(suffix) = key.strip_prefix(&prefix) else { break };
        if !ClipMetadata::is_effect_key(suffix) || !value.is_finite() {
            return Err(bad("clip.capture", "invalid clip effect metadata"));
        }
        budget.effect(suffix)?;
        output.effects.insert(suffix.to_owned(), *value);
    }
    Ok(output)
}

fn instance_metadata(session: &mut Session, id: ClipId, metadata: &ClipMetadata) {
    for (suffix, value) in &metadata.effects {
        session.edit.values.insert(format!("clip_fx.{}.{suffix}", id.0), *value);
    }
    session.edit.set_flag(&format!("clip_fx.bypass.{}", id.0), metadata.effects_bypassed);
    for (family, value) in [("clip.bpm", metadata.source_bpm), ("clip.len0", metadata.original_length), ("clip.pitch", metadata.pitch)] {
        if let Some(value) = value {
            session.edit.values.insert(format!("{family}.{}", id.0), value);
        }
    }
    if let Some(ratio) = metadata.elastic_ratio {
        session.edit.values.insert(format!("elastic.{}.ratio", id.0), ratio);
    }
    if let Some(original) = &metadata.elastic_original {
        for (suffix, value) in [("src", original.source.0 as f64), ("off", original.offset as f64), ("len", original.length as f64)] {
            session.edit.values.insert(format!("elastic.{}.{suffix}", id.0), value);
        }
    }
}

fn name(params: &Value, fallback: &str) -> Result<String> {
    let name = match params.get("name") {
        None => fallback,
        Some(Value::String(name)) => name,
        _ => return Err(bad("clip.capture", "name must be a string")),
    };
    if name.is_empty() || name.len() > MAX_NAME_CHARS * 4 || name.chars().count() > MAX_NAME_CHARS || name.contains('\0') {
        return Err(bad("clip.capture", "name must be a nonempty string of at most 256 characters"));
    }
    Ok(name.to_owned())
}

/// Capture whole named clips, or whole clips in the edit selection, leaving the timeline intact.
pub fn capture(engine: &mut Engine, params: &Value) -> Result<Value> {
    let session = engine.session();
    let (mut ids, mut budget) = inventory(session)?;
    if params.get("clip").is_some() && params.get("clips").is_some() {
        return Err(bad("clip.capture", "pass clip or clips, not both"));
    }
    if ["track", "tracks", "start", "end"].iter().any(|key| params.get(key).is_some()) {
        return Err(bad("clip.capture", "pass clip ids, or use the current whole-clip edit selection"));
    }
    let explicit = params.get("clip").is_some() || params.get("clips").is_some();
    let selected = if let Some(array) = params.get("clips") {
        let array = array
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= ClipDefinition::MAX_COUNT)
            .ok_or_else(|| bad("clip.capture", "clips must be a nonempty bounded array of ids"))?;
        array
            .iter()
            .map(|v| v.as_u64().filter(|id| *id != 0).map(ClipId).ok_or_else(|| bad("clip.capture", "invalid clip id")))
            .collect::<Result<Vec<_>>>()?
    } else if let Some(value) = params.get("clip") {
        vec![ClipId(value.as_u64().filter(|id| *id != 0).ok_or_else(|| bad("clip.capture", "invalid clip id"))?)]
    } else {
        if session.edit.selected_clips.len() > ClipDefinition::MAX_COUNT {
            return Err(bad("clip.capture", "too many selected clips"));
        }
        session.edit.selected_clips.clone()
    };
    let mut chosen = BTreeSet::new();
    for id in selected {
        if !chosen.insert(id) || session.find_clip(id).is_none() {
            return Err(bad("clip.capture", "duplicate or unknown selected clip"));
        }
    }
    let range = session.edit.selection;
    if !explicit && !range.is_empty() && (range.start < 0 || range.end > MAX_POSITION || range.end <= range.start) {
        return Err(bad("clip.capture", "invalid edit range"));
    }
    let tracks: BTreeSet<_> = session.edit.selected_tracks.iter().copied().collect();
    if chosen.is_empty() && (tracks.is_empty() || range.is_empty()) {
        return Err(bad("clip.capture", "select whole clips, or select tracks and a whole-clip range"));
    }
    if chosen.is_empty() && tracks.iter().any(|id| session.track(*id).is_none()) {
        return Err(bad("clip.capture", "unknown selected track"));
    }
    let mut definitions = Vec::new();
    let mut response = Vec::new();
    for track in &session.tracks {
        for clip in track.clips() {
            let selected = if chosen.is_empty() { tracks.contains(&track.id) && clip.range().overlaps(&range) } else { chosen.contains(&clip.id) };
            if !selected {
                continue;
            }
            if !explicit && !range.is_empty() && (range.start > clip.start || range.end < clip.end()) {
                return Err(bad("clip.capture", "partial capture is unsupported; select the whole clip"));
            }
            if clip.start < 0 || clip.start.checked_add(clip.length).is_none_or(|end| end > MAX_POSITION) {
                return Err(bad("clip.capture", "invalid clip timeline geometry"));
            }
            let name = name(params, &clip.name)?;
            budget.template(&clip.content, clip.gain_env.len(), &name)?;
            let metadata = metadata(session, clip, &mut budget)?;
            let midi_original = session.midi_originals.get(&clip.id.0);
            if let Some(original) = midi_original {
                budget.sequence(original)?;
            }
            let definition = ClipDefinition {
                id: ClipDefinitionId(ids.take()?),
                name,
                template: ClipTemplate::from(clip),
                metadata,
                midi_original: midi_original.cloned(),
            };
            definition.validate_media(session, false).map_err(|why| bad("clip.capture", why))?;
            if session.clip_definitions.len().saturating_add(definitions.len()).saturating_add(1) > ClipDefinition::MAX_COUNT {
                return Err(bad("clip.capture", "too many reusable definitions"));
            }
            response.push(json!({"id":definition.id,"track":track.id,"clip":clip.id,"length":clip.length}));
            definitions.push(definition);
        }
    }
    if definitions.is_empty() {
        return Err(bad("clip.capture", "selection contains no whole clips"));
    }
    let count = definitions.len();
    let session = engine.session_mut();
    session.clip_definitions.extend(definitions);
    session.next_id = ids.next;
    Ok(json!({"definitions":response,"count":count}))
}

/// Place a fresh instance at the exact position, without trimming or shifting other material.
pub fn place(engine: &mut Engine, params: &Value) -> Result<Value> {
    let session = engine.session();
    let (mut ids, mut budget) = inventory(session)?;
    let id = ClipDefinitionId(
        params
            .get("definition")
            .and_then(Value::as_u64)
            .filter(|id| *id != 0)
            .ok_or_else(|| bad("clip.place_definition", "definition id required"))?,
    );
    let definition = session.clip_definition(id).ok_or_else(|| bad("clip.place_definition", "unknown reusable definition"))?;
    let at = cmd::position_param(engine, "clip.place_definition", params, "at")?.unwrap_or(session.edit.selection.start);
    if !(0..=MAX_POSITION).contains(&at) || at.checked_add(definition.template.length).is_none_or(|end| end > MAX_POSITION) {
        return Err(bad("clip.place_definition", "destination position or end is out of range"));
    }
    definition.validate_media(session, true).map_err(|why| bad("clip.place_definition", why))?;
    let kind = match definition.template.content {
        ClipContent::Audio { .. } => TrackKind::Audio,
        ClipContent::Midi { .. } => TrackKind::Midi,
        ClipContent::Video { .. } => TrackKind::Video,
    };
    let track = cmd::track_param(engine, "clip.place_definition", params, "track")?;
    if let Some(track) = track {
        let target = session.track(track).ok_or_else(|| bad("clip.place_definition", "unknown destination track"))?;
        if target.kind != kind && !(kind == TrackKind::Midi && target.kind == TrackKind::Instrument) {
            return Err(bad("clip.place_definition", "definition and destination track kinds disagree"));
        }
        let playlist = target.playlist().ok_or_else(|| bad("clip.place_definition", "destination has no active playlist"))?;
        let end = at + definition.template.length;
        if playlist.clips.iter().any(|clip| clip.start < end && clip.end() > at) {
            return Err(bad("clip.place_definition", "destination overlaps an existing clip; use an empty range or a new track"));
        }
    } else if session.tracks.len() >= 4096 {
        return Err(bad("clip.place_definition", "new destination exceeds the track limit"));
    }
    let format = match definition.template.content {
        ClipContent::Audio { source, .. } if track.is_none() => {
            let channels = usize::from(session.source(source).ok_or_else(|| bad("clip.place_definition", "audio source unavailable"))?.channels);
            ChannelFormat::ALL
                .into_iter()
                .find(|f| f.channels() == channels)
                .ok_or_else(|| bad("clip.place_definition", "audio width needs an explicit compatible destination track"))?
        }
        _ => ChannelFormat::Mono,
    };
    budget.definition(definition)?;
    let metadata_values = definition
        .metadata
        .effects
        .len()
        .saturating_add(
            [definition.metadata.source_bpm, definition.metadata.original_length, definition.metadata.pitch, definition.metadata.elastic_ratio]
                .into_iter()
                .flatten()
                .count(),
        )
        .saturating_add(if definition.metadata.elastic_original.is_some() { 3 } else { 0 })
        .saturating_add(usize::from(definition.metadata.effects_bypassed));
    if session.edit.values.len().saturating_add(session.edit.flags.len()).saturating_add(metadata_values) > ClipDefinition::MAX_METADATA {
        return Err(bad("clip.place_definition", "placed metadata exceeds the session budget"));
    }
    let new_track = if track.is_none() { Some(TrackId(ids.take()?)) } else { None };
    let clip_id = ClipId(ids.take()?);
    let clip = definition.template.instantiate(clip_id, &definition.name, at);
    let metadata = definition.metadata.clone();
    let original = definition.midi_original.clone();
    let name = definition.name.clone();
    let length = clip.length;
    let destination = track.or(new_track).ok_or_else(|| bad("clip.place_definition", "missing destination plan"))?;
    let session = engine.session_mut();
    if let Some(id) = new_track {
        session.next_id = id.0;
        session.add_track(kind, format, Some(&name));
    }
    if let Some(playlist) = session.track_mut(destination).and_then(|track| track.playlist_mut()) {
        playlist.clips.push(clip);
        playlist.sort();
    }
    instance_metadata(session, clip_id, &metadata);
    if let Some(original) = original {
        session.midi_originals.insert(clip_id.0, original);
    }
    session.next_id = ids.next;
    Ok(json!({"definition":id,"clip":clip_id,"track":destination,"at":at,"length":length}))
}

/// Remove only reusable definitions. Existing clips and source files remain intact.
pub fn clear(engine: &mut Engine, params: &Value) -> Result<Value> {
    let values = params
        .get("definitions")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= ClipDefinition::MAX_COUNT)
        .ok_or_else(|| bad("clip.clear_definitions", "definitions must be a nonempty bounded array of ids"))?;
    let mut ids = BTreeSet::new();
    for value in values {
        let id = ClipDefinitionId(value.as_u64().filter(|id| *id != 0).ok_or_else(|| bad("clip.clear_definitions", "invalid definition id"))?);
        if !ids.insert(id) || engine.session().clip_definition(id).is_none() {
            return Err(bad("clip.clear_definitions", "duplicate or unknown definition id"));
        }
    }
    engine.session_mut().clip_definitions.retain(|definition| !ids.contains(&definition.id));
    Ok(json!({"definitions":ids.len()}))
}
