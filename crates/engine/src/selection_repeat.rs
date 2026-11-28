//! Immutable, bounded arrangement duplication. General clipboard editing is independent.

use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{AutomationLane, AutomationPoint, Clip, ClipContent, ClipId, EditMode, Session, TrackId};
use soundcraft_time::{Range, TimeFormat};
use std::collections::{BTreeMap, BTreeSet};

const MAX_TRACKS: usize = 4096;
const MAX_CLIPS: usize = 100_000;
const MAX_POINTS: usize = 1_000_000;
const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_METADATA: usize = 65_536;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Duplicate,
    Repeat,
    Extend,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Duplicate => "edit.duplicate",
            Self::Repeat => "edit.repeat",
            Self::Extend => "edit.duplicate_extend",
        }
    }
}

fn unsupported(message: &str) -> EngineError {
    EngineError::Unsupported(format!("selection repeat: {message}"))
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).filter(|v| (0..=cmd::MAX_POSITION).contains(v)).ok_or_else(|| unsupported("position exceeds the supported timeline"))
}

#[derive(Default)]
struct Budget {
    clips: usize,
    points: usize,
    bytes: usize,
    metadata: usize,
}

impl Budget {
    fn charge(&mut self, clips: usize, points: usize, bytes: usize, metadata: usize, copies: usize) -> Result<()> {
        self.clips = self
            .clips
            .checked_add(clips.checked_mul(copies).ok_or_else(|| unsupported("clip count overflow"))?)
            .filter(|n| *n <= MAX_CLIPS)
            .ok_or_else(|| unsupported("clip plan exceeds 100000 clips"))?;
        self.points = self
            .points
            .checked_add(points.checked_mul(copies).ok_or_else(|| unsupported("event count overflow"))?)
            .filter(|n| *n <= MAX_POINTS)
            .ok_or_else(|| unsupported("plan exceeds 1000000 automation/MIDI/envelope events"))?;
        self.bytes = self
            .bytes
            .checked_add(bytes.checked_mul(copies).ok_or_else(|| unsupported("plan size overflow"))?)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(|| unsupported("plan exceeds 128 MiB of document payload"))?;
        self.metadata = self
            .metadata
            .checked_add(metadata.checked_mul(copies).ok_or_else(|| unsupported("metadata count overflow"))?)
            .filter(|n| *n <= MAX_METADATA)
            .ok_or_else(|| unsupported("resulting metadata exceeds 65536 entries"))?;
        Ok(())
    }
}

fn metadata_suffix(key: &str, id: ClipId) -> Option<String> {
    for family in ["clip_fx", "elastic"] {
        let prefix = format!("{family}.{}.", id.0);
        if let Some(suffix) = key.strip_prefix(&prefix) {
            return Some(format!("{family}.{{id}}.{suffix}"));
        }
    }
    for family in ["clip.bpm", "clip.len0", "clip.pitch", "clip_fx.bypass"] {
        if key == format!("{family}.{}", id.0) {
            return Some(format!("{family}.{{id}}"));
        }
    }
    None
}

fn metadata_owner(key: &str) -> Option<u64> {
    for prefix in ["clip_fx.bypass.", "clip.bpm.", "clip.len0.", "clip.pitch."] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return rest.parse().ok();
        }
    }
    for prefix in ["clip_fx.", "elastic."] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return rest.split_once('.').and_then(|(id, _)| id.parse().ok());
        }
    }
    None
}

fn clip_cost(s: &Session, c: &Clip) -> Result<(usize, usize, usize)> {
    let mut points = c.gain_env.len();
    let mut bytes = std::mem::size_of::<Clip>().checked_add(c.name.len()).ok_or_else(|| unsupported("clip size overflow"))?;
    if let ClipContent::Midi { sequence } = &c.content {
        points = points
            .checked_add(sequence.notes.len())
            .and_then(|n| n.checked_add(sequence.ctrls.len()))
            .ok_or_else(|| unsupported("MIDI count overflow"))?;
    }
    if let Some(seq) = s.midi_originals.get(&c.id.0) {
        points = points
            .checked_add(seq.notes.len())
            .and_then(|n| n.checked_add(seq.ctrls.len()))
            .ok_or_else(|| unsupported("performance count overflow"))?;
    }
    bytes = bytes
        .checked_add(points.checked_mul(64).ok_or_else(|| unsupported("event size overflow"))?)
        .ok_or_else(|| unsupported("clip size overflow"))?;
    let mut metadata = 0;
    for key in s.edit.values.keys().chain(s.edit.flags.iter()) {
        if metadata_suffix(key, c.id).is_some() {
            metadata += 1;
            bytes = bytes.checked_add(key.len().saturating_add(128)).ok_or_else(|| unsupported("metadata size overflow"))?;
        }
    }
    Ok((points, bytes, metadata))
}

fn check_sequence(sequence: &soundcraft_midi::Sequence, ticks: i64) -> Result<()> {
    if sequence.notes.iter().any(|n| {
        n.start < 0
            || n.length <= 0
            || n.start.checked_add(n.length).is_none_or(|end| end > ticks)
            || n.pitch > 127
            || !(1..=127).contains(&n.velocity)
            || n.release_velocity > 127
            || n.channel > 15
    }) || sequence.ctrls.iter().any(|c| {
        c.tick < 0
            || c.tick >= ticks
            || c.channel > 15
            || match c.kind {
                soundcraft_midi::CtrlKind::Cc(number) => number > 127 || !(0..=127).contains(&c.value),
                soundcraft_midi::CtrlKind::PitchBend => !(-8192..=8191).contains(&c.value),
                soundcraft_midi::CtrlKind::PolyPressure(pitch) => pitch > 127 || !(0..=127).contains(&c.value),
                _ => !(0..=127).contains(&c.value),
            }
    }) {
        return Err(unsupported("MIDI content crosses its clip boundary or contains invalid events"));
    }
    Ok(())
}

fn check_clip(s: &Session, c: &Clip) -> Result<()> {
    if c.start < 0
        || c.length <= 0
        || c.start.checked_add(c.length).is_none_or(|end| end > cmd::MAX_POSITION)
        || c.sync_point < 0
        || c.sync_point > c.length
        || !c.gain_db.is_finite()
        || !c.stretch.is_finite()
        || c.stretch <= 0.0
        || c.fade_in.len < 0
        || c.fade_in.len > c.length
        || !c.fade_in.curvature.is_finite()
        || !(-1.0..=1.0).contains(&c.fade_in.curvature)
        || c.fade_out.len < 0
        || c.fade_out.len > c.length
        || !c.fade_out.curvature.is_finite()
        || !(-1.0..=1.0).contains(&c.fade_out.curvature)
        || c.gain_env.iter().any(|(at, value)| *at < 0 || *at > c.length || !value.is_finite())
        || c.gain_env.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.0 >= b.0))
    {
        return Err(unsupported("clip has invalid timing or processing"));
    }
    if c.edit_locked || c.time_locked || c.group.is_some() {
        return Err(unsupported("locked or grouped material requires a separate repeat policy"));
    }
    if c.loop_id.is_some() {
        return Err(unsupported("looped material requires a separate repeat policy"));
    }
    match &c.content {
        ClipContent::Audio { offset, .. } => {
            if *offset < 0 || offset.checked_add(c.length).is_none() {
                return Err(unsupported("audio source offset overflows"));
            }
        }
        ClipContent::Midi { sequence } => {
            let start = s.tempo.samples_to_ticks(c.start, s.sample_rate);
            let end = s.tempo.samples_to_ticks(c.end(), s.sample_rate);
            let ticks = end.checked_sub(start).filter(|n| *n > 0).ok_or_else(|| unsupported("MIDI clip has no positive tick span"))?;
            check_sequence(sequence, ticks)?;
            if let Some(original) = s.midi_originals.get(&c.id.0) {
                check_sequence(original, ticks)?;
            }
        }
        ClipContent::Video { .. } => return Err(unsupported("video repetition is not supported by this planner")),
    }
    for (key, value) in &s.edit.values {
        if metadata_suffix(key, c.id).is_some() && !value.is_finite() {
            return Err(unsupported("clip metadata contains a nonfinite value"));
        }
    }
    Ok(())
}

fn check_fragment(s: &Session, c: &Clip, range: Range) -> Result<()> {
    check_clip(s, c)?;
    if range == c.range() {
        return Ok(());
    }
    if !c.is_audio()
        || c.stretch != 1.0
        || c.fade_in.len != 0
        || c.fade_out.len != 0
        || !c.gain_env.is_empty()
        || s.midi_originals.contains_key(&c.id.0)
        || s.edit.values.keys().chain(s.edit.flags.iter()).any(|key| metadata_suffix(key, c.id).is_some())
    {
        return Err(unsupported("partial MIDI or processed clip fragments cannot be repeated safely"));
    }
    Ok(())
}

fn fragment(c: &Clip, range: Range, start: i64, id: ClipId) -> Result<Clip> {
    let relative = range.start.checked_sub(c.start).filter(|n| *n >= 0).ok_or_else(|| unsupported("fragment offset overflow"))?;
    let mut out = c.clone();
    out.id = id;
    if let ClipContent::Audio { offset, .. } = &mut out.content {
        *offset = offset.checked_add(relative).ok_or_else(|| unsupported("fragment source offset overflow"))?;
    }
    out.start = start;
    out.length = range.len();
    out.sync_point = c.sync_point.checked_sub(relative).ok_or_else(|| unsupported("fragment sync point overflow"))?.clamp(0, out.length);
    Ok(out)
}

struct Ids {
    next: u64,
}

impl Ids {
    fn take(&mut self) -> Result<ClipId> {
        let id = self.next;
        self.next = id.checked_add(1).ok_or_else(|| unsupported("clip id space exhausted"))?;
        Ok(ClipId(id))
    }
}

struct TrackPlan {
    id: TrackId,
    clips: Vec<Clip>,
    automation: Option<Vec<AutomationLane>>,
}

struct Plan {
    tracks: Vec<TrackPlan>,
    mappings: Vec<(ClipId, ClipId)>,
    removed: BTreeSet<ClipId>,
    values: Vec<(String, f64)>,
    flags: Vec<String>,
    performances: Vec<(u64, soundcraft_midi::Sequence)>,
    next_id: u64,
    selection: Range,
    selected_clips: Vec<ClipId>,
    copies: usize,
    count: usize,
    destination: Range,
    musical: bool,
}

fn count(kind: Kind, p: &Value) -> Result<usize> {
    if !matches!(kind, Kind::Repeat) {
        return Ok(1);
    }
    let Some(value) = p.get("count") else { return Ok(1) };
    let number = value
        .as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && (1.0..=999.0).contains(n))
        .ok_or_else(|| cmd::bad(kind.id(), "count must be an integer from 1 through 999"))?;
    Ok(number as usize)
}

fn boundaries(s: &Session, p: &Value, range: Range, count: usize) -> Result<(Vec<i64>, bool)> {
    let musical = match p.get("timebase") {
        Some(Value::String(v)) if v == "samples" => false,
        Some(Value::String(v)) if v == "ticks" => true,
        Some(_) => return Err(cmd::bad("selection repeat", "timebase must be samples or ticks")),
        None => {
            s.edit.main_counter == TimeFormat::BarsBeats
                || ["start", "end"].iter().any(|key| p.get(key).and_then(Value::as_str).is_some_and(|v| v.starts_with("bars") || v.contains('|')))
        }
    };
    let mut positions = Vec::with_capacity(count + 1);
    if !musical {
        for k in 0..=count {
            let delta = i64::try_from(k).ok().and_then(|k| range.len().checked_mul(k)).ok_or_else(|| unsupported("repeat duration overflow"))?;
            positions.push(add(range.end, delta)?);
        }
    } else {
        if s.tempo.tempos().len() != 1 {
            return Err(unsupported("musical repeat across tempo changes needs material remapping"));
        }
        let start = s.tempo.samples_to_ticks(range.start, s.sample_rate);
        let end = s.tempo.samples_to_ticks(range.end, s.sample_rate);
        let span = end.checked_sub(start).filter(|n| *n > 0).ok_or_else(|| unsupported("selection has no positive musical duration"))?;
        if s.tempo.tick_to_samples(start, s.sample_rate) != range.start || s.tempo.tick_to_samples(end, s.sample_rate) != range.end {
            return Err(unsupported("musical selection boundaries do not round-trip exactly"));
        }
        for k in 0..=count {
            let tick = i64::try_from(k)
                .ok()
                .and_then(|k| span.checked_mul(k))
                .and_then(|delta| end.checked_add(delta))
                .ok_or_else(|| unsupported("musical duration overflow"))?;
            let at = s.tempo.tick_to_samples(tick, s.sample_rate);
            if !(0..=cmd::MAX_POSITION).contains(&at) || positions.last().is_some_and(|previous| at.checked_sub(*previous) != Some(range.len())) {
                return Err(unsupported("musical iterations change sample width; remapping is not yet supported"));
            }
            positions.push(at);
        }
    }
    Ok((positions, musical))
}

fn validate_tempo(s: &Session) -> Result<()> {
    if !(8_000..=768_000).contains(&s.sample_rate.hz())
        || s.tempo.tempos().len().saturating_add(s.tempo.meters().len()) > MAX_METADATA
        || s.tempo.meters().is_empty()
        || s.tempo.meters().first().is_none_or(|m| m.tick != 0)
        || s.tempo.meters().iter().any(|m| {
            m.tick < 0 || m.tick > cmd::MAX_POSITION || !(1..=64).contains(&m.numerator) || ![1, 2, 4, 8, 16, 32, 64].contains(&m.denominator)
        })
        || s.tempo.meters().windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.tick >= b.tick))
        || s.tempo.tempos().is_empty()
        || s.tempo.tempos().first().is_none_or(|t| t.tick != 0)
        || s.tempo.tempos().iter().any(|t| t.tick < 0 || t.tick > cmd::MAX_POSITION || !t.bpm.is_finite() || !(5.0..=1000.0).contains(&t.bpm))
        || s.tempo.tempos().windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.tick >= b.tick))
    {
        return Err(unsupported("invalid tempo map"));
    }
    Ok(())
}

fn shuffle_limit(s: &Session, track: TrackId, at: i64, length: i64) -> Result<i64> {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    if clips.iter().filter(|c| c.start <= at && at < c.end()).count() > 1 {
        return Err(unsupported(crate::edit::IN_CROSSFADE));
    }
    if crate::edit::shuffle_room(s, track, at) < length {
        return Err(unsupported(crate::edit::NO_ROOM));
    }
    Ok(clips.iter().filter(|c| crate::edit::locked(c) && c.end() > at).map(|c| c.start).min().unwrap_or(i64::MAX))
}

fn lane_plan(lane: &AutomationLane, source: Range, destination: Range, count: usize, shuffle: bool, limit: i64) -> Result<AutomationLane> {
    if lane.points.is_empty() {
        return Ok(lane.clone());
    }
    let default = lane.param.range().2;
    let mut points = BTreeMap::new();
    for p in &lane.points {
        if p.at < destination.start {
            points.insert(p.at, p.value);
        } else if shuffle {
            points.insert(if p.at < limit { add(p.at, destination.len())? } else { p.at }, p.value);
        } else if p.at >= destination.end {
            points.insert(p.at, p.value);
        }
    }
    if destination.start > 0 {
        points.insert(destination.start - 1, lane.value_at(destination.start - 1, default));
    }
    for k in 0..count {
        let delta = source
            .len()
            .checked_mul(i64::try_from(k).map_err(|_| unsupported("iteration overflow"))?)
            .ok_or_else(|| unsupported("iteration overflow"))?;
        let at = add(destination.start, delta)?;
        points.insert(at, lane.value_at(source.start, default));
        for p in lane.points.iter().filter(|p| p.at > source.start && p.at < source.end - 1) {
            points.insert(add(at, p.at - source.start)?, p.value);
        }
        points.insert(add(at, source.len() - 1)?, lane.value_at(source.end - 1, default));
    }
    points.insert(destination.end, lane.value_at(if shuffle && destination.end < limit { destination.start } else { destination.end }, default));
    if shuffle && limit != i64::MAX {
        points.insert(limit, lane.value_at(limit, default));
    }
    Ok(AutomationLane {
        param: lane.param.clone(),
        enabled: lane.enabled,
        points: points.into_iter().map(|(at, value)| AutomationPoint { at, value }).collect(),
    })
}

fn prepare(e: &Engine, p: &Value, kind: Kind) -> Result<Plan> {
    let s = e.session();
    if !p.is_object() {
        return Err(cmd::bad(kind.id(), "parameters must be an object"));
    }
    if p.get("tracks").and_then(Value::as_array).is_some_and(|a| a.len() > MAX_TRACKS) || s.edit.selected_tracks.len() > MAX_TRACKS {
        return Err(unsupported("too many targets"));
    }
    if p.get("tracks").is_some_and(|v| !v.is_array()) || ["indices", "notes", "clip", "clips"].iter().any(|key| p.get(key).is_some()) {
        return Err(cmd::bad(kind.id(), "use an arrangement track/time range, not a note/clip-index selection"));
    }
    let count = count(kind, p)?;
    validate_tempo(s)?;
    for key in ["start", "end", "length"] {
        if let Some(text) = p.get(key).and_then(Value::as_str) {
            if text.len() > 256 {
                return Err(cmd::bad(kind.id(), "position text exceeds 256 bytes"));
            }
            let body =
                text.split_once(':').filter(|(prefix, _)| soundcraft_time::TimeFormat::from_id(prefix).is_some()).map_or(text, |(_, body)| body);
            if body.split(['|', '.', ' ']).filter_map(|part| part.parse::<i64>().ok()).any(|n| !(-cmd::MAX_POSITION..=cmd::MAX_POSITION).contains(&n))
            {
                return Err(cmd::bad(kind.id(), "position component exceeds the supported bound"));
            }
        }
    }
    let range = cmd::range_param(e, kind.id(), p)?;
    if range.is_empty() || range.start < 0 || range.end > cmd::MAX_POSITION {
        return Err(cmd::bad(kind.id(), "selection must be a nonempty supported nonnegative range"));
    }
    let (positions, musical) = boundaries(s, p, range, count)?;
    let final_end = positions.last().copied().ok_or_else(|| unsupported("no destination"))?;
    let destination = Range::new(range.end, final_end);
    let shuffle = s.edit.edit_mode == EditMode::Shuffle;
    let follow = s.edit.automation_follows_edit;
    let mut targets = cmd::tracks_required(e, kind.id(), p)?;
    targets.sort();
    targets.dedup();
    if targets.len() > MAX_TRACKS || s.edit.values.len().saturating_add(s.edit.flags.len()) > MAX_METADATA {
        return Err(unsupported("target or metadata limit exceeded"));
    }
    let mut all_ids = BTreeSet::new();
    let mut max_clip_id = 0;
    let mut max_other_id = 0;
    if s.edit.selected_clips.len() > MAX_CLIPS || s.midi_originals.len() > MAX_CLIPS {
        return Err(unsupported("selected clip or original performance inventory exceeds 100000 entries"));
    }
    if s.tracks.len() > MAX_TRACKS
        || s.tracks.iter().map(|t| t.playlists.len()).try_fold(0usize, |a, n| a.checked_add(n)).is_none_or(|n| n > MAX_CLIPS)
    {
        return Err(unsupported("document has too many tracks/playlists"));
    }
    for t in &s.tracks {
        max_other_id = max_other_id.max(t.id.0);
        for playlist in &t.playlists {
            for c in &playlist.clips {
                if all_ids.len() >= MAX_CLIPS || !all_ids.insert(c.id) || c.id.0 == 0 {
                    return Err(unsupported("too many clips or ambiguous clip identities"));
                }
                max_clip_id = max_clip_id.max(c.id.0).max(c.loop_id.unwrap_or(0));
            }
        }
    }
    if s.sources.len().saturating_add(s.videos.len()).saturating_add(s.busses.len()).saturating_add(s.markers.len()).saturating_add(s.groups.len())
        > MAX_CLIPS
    {
        return Err(unsupported("document identity inventory exceeds 100000 entries"));
    }
    if s.edit.selected_clips.iter().any(|id| !all_ids.contains(id)) {
        return Err(unsupported("selected clip identity is not present in the document"));
    }
    for id in s
        .sources
        .iter()
        .map(|x| x.id.0)
        .chain(s.videos.iter().map(|x| x.id.0))
        .chain(s.busses.iter().map(|x| x.id.0))
        .chain(s.markers.iter().map(|x| x.id.0))
        .chain(s.groups.iter().map(|x| x.id.0))
    {
        max_other_id = max_other_id.max(id);
    }
    for id in s.edit.values.keys().chain(s.edit.flags.iter()).filter_map(|key| metadata_owner(key)).chain(s.midi_originals.keys().copied()) {
        max_clip_id = max_clip_id.max(id);
    }
    if s.next_id == 0 || s.next_id <= max_clip_id {
        return Err(unsupported("next clip identity collides with the document"));
    }
    let first_id = s.next_id.max(max_other_id.checked_add(1).ok_or_else(|| unsupported("document identity space exhausted"))?);
    let scans = targets
        .iter()
        .filter_map(|id| s.track(*id))
        .map(|t| t.clips().len())
        .try_fold(0usize, |a, n| a.checked_add(n))
        .and_then(|n| n.checked_mul(s.edit.values.len().saturating_add(s.edit.flags.len())));
    if scans.is_none_or(|n| n > 2_000_000) {
        return Err(unsupported("clip metadata scan exceeds the work limit"));
    }
    let mut budget = Budget { metadata: s.edit.values.len() + s.edit.flags.len(), ..Budget::default() };
    let mut shuffle_limits = BTreeMap::new();
    // Cost the entire plan before cloning clip sequences, envelopes or metadata.
    for id in &targets {
        let track = s.track(*id).ok_or_else(|| unsupported("target disappeared"))?;
        if !matches!(track.kind, soundcraft_model::TrackKind::Audio | soundcraft_model::TrackKind::Midi | soundcraft_model::TrackKind::Instrument)
            || !matches!(track.view.as_str(), "waveform" | "blocks" | "clips" | "notes")
        {
            return Err(unsupported("only audio/MIDI master-view arrangement selections are supported"));
        }
        if s.edit.tool == soundcraft_model::Tool::Grabber && track.kind.is_midi() {
            return Err(unsupported("MIDI Grabber note selection requires a separate merge policy"));
        }
        if track.kind.is_midi() && s.tempo.tempos().len() != 1 {
            return Err(unsupported("MIDI repeat across tempo changes requires event remapping"));
        }
        let playlist = track.playlist().ok_or_else(|| unsupported("target has no active playlist"))?;
        let limit = if shuffle { shuffle_limit(s, *id, destination.start, destination.len())? } else { i64::MAX };
        shuffle_limits.insert(*id, limit);
        for c in &playlist.clips {
            let end = c
                .start
                .checked_add(c.length)
                .filter(|end| c.start >= 0 && c.length > 0 && *end <= cmd::MAX_POSITION)
                .ok_or_else(|| unsupported("invalid destination clip timing"))?;
            let (points, bytes, metadata) = clip_cost(s, c)?;
            budget.charge(2, points.saturating_mul(2), bytes.saturating_mul(2), metadata.saturating_mul(2), 1)?;
            if let Some(piece) = c.range().intersect(&range) {
                check_fragment(s, c, piece)?;
                budget.charge(1, points, bytes, metadata, count)?;
            }
            let touched = if shuffle { end > destination.start && c.start < limit } else { c.range().overlaps(&destination) };
            if touched {
                check_clip(s, c)?;
                if shuffle && c.start >= destination.start {
                    add(c.start, destination.len())?;
                    add(end, destination.len())?;
                } else if c.start < destination.start || (!shuffle && end > destination.end) {
                    if c.start < destination.start {
                        check_fragment(s, c, Range::new(c.start, destination.start))?;
                    }
                    if shuffle || end > destination.end {
                        let start = if shuffle { destination.start } else { destination.end };
                        check_fragment(s, c, Range::new(start, end))?;
                        if shuffle {
                            add(end, destination.len())?;
                        }
                    }
                }
            }
        }
        if follow {
            for lane in &track.automation {
                if matches!(lane.param, soundcraft_model::AutoParam::Pan(index) if index > 1)
                    || matches!(&lane.param, soundcraft_model::AutoParam::Plugin { param, .. } if param.len() > 4096)
                    || (!matches!(lane.param, soundcraft_model::AutoParam::Mute | soundcraft_model::AutoParam::SendMute(_))
                        && lane.points.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| !(b.value - a.value).is_finite())))
                {
                    return Err(unsupported("automation parameter or interpolation exceeds the supported bounds"));
                }
                if lane.points.iter().any(|p| p.at < 0 || p.at > cmd::MAX_POSITION || !p.value.is_finite())
                    || lane.points.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.at >= b.at))
                {
                    return Err(unsupported("automation points must be finite and uniquely ordered"));
                }
                if shuffle {
                    for point in lane.points.iter().filter(|p| p.at >= destination.start && p.at < limit) {
                        if add(point.at, destination.len())? >= limit {
                            return Err(unsupported("automation would cross a locked clip boundary"));
                        }
                    }
                }
                let new_points = lane
                    .points
                    .len()
                    .checked_add(3)
                    .and_then(|n| n.checked_mul(count + 1))
                    .ok_or_else(|| unsupported("automation count overflow"))?;
                budget.charge(0, new_points, new_points.saturating_mul(64).saturating_add(lane.param.label().len()), 0, 1)?;
            }
        }
    }
    if budget.clips.checked_mul(s.edit.values.len().saturating_add(s.edit.flags.len())).is_none_or(|n| n > 2_000_000) {
        return Err(unsupported("resulting metadata scan exceeds the work limit"));
    }
    first_id
        .checked_add(u64::try_from(budget.clips).map_err(|_| unsupported("clip id count overflow"))?)
        .ok_or_else(|| unsupported("clip id reservation overflows"))?;
    let mut ids = Ids { next: first_id };
    let mut plan = Plan {
        tracks: Vec::new(),
        mappings: Vec::new(),
        removed: BTreeSet::new(),
        values: Vec::new(),
        flags: Vec::new(),
        performances: Vec::new(),
        next_id: first_id,
        selection: range,
        selected_clips: Vec::new(),
        copies: 0,
        count,
        destination,
        musical,
    };
    let mut copied_ids = Vec::new();
    for id in targets {
        let track = s.track(id).ok_or_else(|| unsupported("target disappeared"))?;
        let limit = shuffle_limits.get(&id).copied().ok_or_else(|| unsupported("target insertion boundary disappeared"))?;
        let mut clips = Vec::new();
        for c in track.clips() {
            if shuffle {
                if c.end() <= destination.start || c.start >= limit {
                    clips.push(c.clone());
                } else if c.start >= destination.start {
                    let mut moved = c.clone();
                    moved.start = add(c.start, destination.len())?;
                    check_clip(s, &moved)?;
                    clips.push(moved);
                } else {
                    clips.push(fragment(c, Range::new(c.start, destination.start), c.start, c.id)?);
                    let fresh = ids.take()?;
                    clips.push(fragment(c, Range::new(destination.start, c.end()), destination.end, fresh)?);
                    plan.mappings.push((c.id, fresh));
                }
            } else if !c.range().overlaps(&destination) {
                clips.push(c.clone());
            } else {
                let left = c.start < destination.start;
                let right = c.end() > destination.end;
                if left {
                    clips.push(fragment(c, Range::new(c.start, destination.start), c.start, c.id)?);
                }
                if right {
                    let retained = if left { ids.take()? } else { c.id };
                    clips.push(fragment(c, Range::new(destination.end, c.end()), destination.end, retained)?);
                    if retained != c.id {
                        plan.mappings.push((c.id, retained));
                    }
                }
                if !left && !right {
                    plan.removed.insert(c.id);
                }
            }
        }
        for c in track.clips() {
            let Some(piece) = c.range().intersect(&range) else { continue };
            for at in positions.iter().take(count) {
                let fresh = ids.take()?;
                let start = add(*at, piece.start - range.start)?;
                add(start, piece.len())?;
                let copied = fragment(c, piece, start, fresh)?;
                check_clip(s, &copied)?;
                if let Some(original) = s.midi_originals.get(&c.id.0) {
                    let ticks = s
                        .tempo
                        .samples_to_ticks(copied.end(), s.sample_rate)
                        .checked_sub(s.tempo.samples_to_ticks(copied.start, s.sample_rate))
                        .ok_or_else(|| unsupported("copied performance timing overflow"))?;
                    check_sequence(original, ticks)?;
                }
                clips.push(copied);
                plan.mappings.push((c.id, fresh));
                copied_ids.push(fresh);
                plan.copies += 1;
            }
        }
        clips.sort_by_key(|c| (c.start, c.id.0));
        let automation = if follow {
            Some(track.automation.iter().map(|l| lane_plan(l, range, destination, count, shuffle, limit)).collect::<Result<Vec<_>>>()?)
        } else {
            None
        };
        plan.tracks.push(TrackPlan { id, clips, automation });
    }
    for (old, new) in &plan.mappings {
        for (key, value) in &s.edit.values {
            if let Some(template) = metadata_suffix(key, *old) {
                plan.values.push((template.replacen("{id}", &new.0.to_string(), 1), *value));
            }
        }
        for key in &s.edit.flags {
            if let Some(template) = metadata_suffix(key, *old) {
                plan.flags.push(template.replacen("{id}", &new.0.to_string(), 1));
            }
        }
        if let Some(sequence) = s.midi_originals.get(&old.0) {
            plan.performances.push((new.0, sequence.clone()));
        }
    }
    if plan.values.len().saturating_add(plan.flags.len()).saturating_add(s.edit.values.len()).saturating_add(s.edit.flags.len()) > MAX_METADATA {
        return Err(unsupported("resulting clip metadata exceeds 65536 entries"));
    }
    plan.next_id = ids.next;
    plan.selection = match kind {
        Kind::Duplicate => destination,
        Kind::Repeat => s.edit.selection,
        Kind::Extend => Range::new(range.start, destination.end),
    };
    plan.selected_clips = match kind {
        Kind::Duplicate => copied_ids,
        Kind::Repeat => s.edit.selected_clips.iter().filter(|id| !plan.removed.contains(id)).copied().collect(),
        Kind::Extend => {
            let mut selected: BTreeSet<_> = s.edit.selected_clips.iter().filter(|id| !plan.removed.contains(id)).copied().collect();
            selected.extend(copied_ids);
            selected.into_iter().collect()
        }
    };
    Ok(plan)
}

pub(crate) fn run(e: &mut Engine, p: &Value, kind: Kind) -> Result<Value> {
    let plan = prepare(e, p, kind)?;
    let response = match kind {
        Kind::Repeat => json!({"repeats": plan.count, "clips": plan.copies, "destination": plan.destination, "musical": plan.musical}),
        _ => json!({"clips": plan.copies, "selection": plan.selection, "destination": plan.destination, "musical": plan.musical}),
    };
    let s = e.session_mut();
    for track in plan.tracks {
        if let Some(target) = s.track_mut(track.id) {
            if let Some(playlist) = target.playlist_mut() {
                playlist.clips = track.clips;
            }
            if let Some(lanes) = track.automation {
                target.automation = lanes;
            }
        }
    }
    s.edit.values.retain(|key, _| !plan.removed.iter().any(|id| metadata_suffix(key, *id).is_some()));
    s.edit.flags.retain(|key| !plan.removed.iter().any(|id| metadata_suffix(key, *id).is_some()));
    s.midi_originals.retain(|id, _| !plan.removed.contains(&ClipId(*id)));
    s.edit.values.extend(plan.values);
    s.edit.flags.extend(plan.flags);
    s.midi_originals.extend(plan.performances);
    s.next_id = plan.next_id;
    s.edit.selection = plan.selection;
    s.edit.selected_clips = plan.selected_clips;
    if s.edit.link_timeline_edit {
        s.edit.timeline_selection = plan.selection;
    }
    Ok(response)
}
