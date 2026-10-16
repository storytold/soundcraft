//! Create and update ordinary MIDI clips from bounded arithmetic step recipes.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::groove::{Config, generate};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, GrooveRecipe, Insert, TrackKind};
use soundcraft_time::MAX_POSITION;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "groove.create",
            "Create Groove",
            [],
            None,
            "{name?, track?, start?, config?} — creates MIDI and its recipe; omitted track creates a Drum Synth track",
            always,
            create
        ),
        cmd!(
            "groove.update",
            "Apply Groove",
            [],
            None,
            "{clip, config?: partial configuration} — replaces an unmodified generated MIDI clip",
            always,
            update
        ),
        cmd!("groove.detach", "Make Ordinary MIDI", [], None, "{clip} — removes the recipe without changing notes", always, detach),
    ]
}

fn configuration(base: Config, p: &Value, id: &str) -> Result<Config> {
    let mut value = serde_json::to_value(base).map_err(|err| bad(id, err.to_string()))?;
    if let Some(patch) = p.get("config") {
        let patch = patch.as_object().ok_or_else(|| bad(id, "config must be an object"))?;
        let fields = value.as_object_mut().ok_or_else(|| bad(id, "invalid configuration"))?;
        for (key, value) in patch {
            if !fields.contains_key(key) {
                return Err(bad(id, format!("unknown groove field: {key}")));
            }
            fields.insert(key.clone(), value.clone());
        }
    }
    let config: Config = serde_json::from_value(value).map_err(|err| bad(id, err.to_string()))?;
    config.validate().map_err(|err| bad(id, err))?;
    Ok(config)
}

fn range(e: &Engine, start: Samples, config: &Config, id: &str) -> Result<Range> {
    if !(0..=MAX_POSITION).contains(&start) {
        return Err(bad(id, "invalid start position"));
    }
    let s = e.session();
    let tick = s.tempo.samples_to_ticks(start, s.sample_rate);
    let duration = config.length_ticks().map_err(|err| bad(id, err))?;
    let end_tick = tick.checked_add(duration).ok_or_else(|| bad(id, "pattern is too long"))?;
    // Keep the clip's exact start sample when editing an existing region.
    let end = s.tempo.tick_to_samples(end_tick, s.sample_rate);
    if end <= start || end > MAX_POSITION {
        return Err(bad(id, "invalid pattern duration"));
    }
    Ok(Range::new(start, end))
}

fn check_target(e: &Engine, track: TrackId, range: Range, except: Option<ClipId>, id: &str) -> Result<()> {
    let t = e.session().track(track).ok_or_else(|| bad(id, "track does not exist"))?;
    if !t.kind.is_midi() {
        return Err(bad(id, "choose an instrument or MIDI track"));
    }
    if t.clips().iter().any(|c| Some(c.id) != except && c.range().overlaps(&range)) {
        return Err(bad(id, "pattern overlaps another clip"));
    }
    Ok(())
}

fn create(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "groove.create";
    let config = configuration(Config::default(), p, id)?;
    let sequence = generate(&config).map_err(|err| bad(id, err))?;
    let start = position_param(e, id, p, "start")?.unwrap_or(e.session().edit.selection.start);
    let range = range(e, start, &config, id)?;
    let target = track_param(e, id, p, "track")?;
    if let Some(target) = target {
        check_target(e, target, range, None, id)?;
    }
    let name = match p.get("name") {
        None => "Groove",
        Some(v) => v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(id, "name must be nonempty text"))?.trim(),
    };
    let mut name = name.to_string();
    soundcraft_model::truncate_chars(&mut name, soundcraft_model::MAX_NAME_CHARS);
    let s = e.session_mut();
    let track = target.unwrap_or_else(|| {
        let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
        if let Some(t) = s.track_mut(track) {
            t.instrument = Some(Insert::new("drum_synth"));
            t.ticks_timebase = true;
        }
        track
    });
    let cid = s.new_clip_id();
    let mut clip = Clip::midi(cid, name, range.start, range.len(), sequence);
    clip.groove = Some(GrooveRecipe { version: 1, config });
    crate::edit::place_clip(s, track, clip);
    s.edit.selected_tracks = vec![track];
    s.edit.selected_clips = vec![cid];
    Ok(json!({"track":track, "clip":cid, "start":range.start, "end":range.end}))
}

fn update(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "groove.update";
    let cid = clip_id(p, id)?;
    let (track, clip) = e.session().find_clip(cid).ok_or_else(|| bad(id, "clip does not exist"))?;
    let recipe = clip.groove.as_ref().ok_or_else(|| bad(id, "clip has no groove recipe"))?;
    if recipe.version != 1 {
        return Err(bad(id, "unsupported groove recipe version"));
    }
    if clip.edit_locked || clip.time_locked {
        return Err(bad(id, "clip is locked"));
    }
    let old = generate(&recipe.config).map_err(|err| bad(id, err))?;
    if clip.content != (ClipContent::Midi { sequence: old }) || range(e, clip.start, &recipe.config, id)?.len() != clip.length {
        return Err(bad(id, "clip was edited outside Groove Builder; make it ordinary MIDI to preserve those edits"));
    }
    let config = configuration(recipe.config.clone(), p, id)?;
    let range = range(e, clip.start, &config, id)?;
    check_target(e, track, range, Some(cid), id)?;
    let sequence = generate(&config).map_err(|err| bad(id, err))?;
    let clip = e.session_mut().find_clip_mut(cid).ok_or_else(|| bad(id, "clip does not exist"))?;
    clip.length = range.len();
    clip.content = ClipContent::Midi { sequence };
    clip.groove = Some(GrooveRecipe { version: 1, config });
    clip.clamp_fades();
    Ok(json!({"clip":cid, "start":range.start, "end":range.end}))
}

fn detach(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "groove.detach";
    let cid = clip_id(p, id)?;
    let clip = e.session_mut().find_clip_mut(cid).ok_or_else(|| bad(id, "clip does not exist"))?;
    if clip.edit_locked {
        return Err(bad(id, "clip is locked"));
    }
    if clip.groove.take().is_none() {
        return Err(bad(id, "clip has no groove recipe"));
    }
    Ok(json!({"clip":cid}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(v: &Value) -> ClipId {
        ClipId(v["clip"].as_u64().unwrap())
    }

    #[test]
    fn creation_is_one_undo_and_invalid_input_leaves_no_track() {
        let mut e = Engine::default();
        let before = e.session().clone();
        for p in [json!({"config":{"steps":0}}), json!({"config":{"unknown":1}}), json!({"name":""}), json!({"start":-1})] {
            assert!(e.execute("groove.create", &p).is_err());
            assert_eq!(&before, e.session());
        }
        e.execute("groove.create", &json!({})).unwrap();
        assert_eq!(e.undo_history().len(), 1);
        assert_eq!(e.session().tracks[0].instrument.as_ref().unwrap().plugin, "drum_synth");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(&before, e.session());
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
    }

    #[test]
    fn recipes_roundtrip_and_updates_are_undoable() {
        let mut e = Engine::default();
        let v = e.execute("groove.create", &json!({"start":48000})).unwrap();
        let original = e.session().find_clip(cid(&v)).unwrap().1.clone();
        let serialized = serde_json::to_string(&original).unwrap();
        let reopened: Clip = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reopened, original);
        e.execute("groove.update", &json!({"clip":v["clip"], "config":{"velocity":50}})).unwrap();
        assert_ne!(e.session().find_clip(cid(&v)).unwrap().1.content, original.content);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().find_clip(cid(&v)).unwrap().1, &original);
        e.execute("groove.detach", &json!({"clip":v["clip"]})).unwrap();
        let detached = e.session().find_clip(cid(&v)).unwrap().1;
        assert!(detached.groove.is_none());
        assert_eq!(detached.content, original.content);
    }

    #[test]
    fn saved_session_reopens_regenerates_and_renders_without_external_media() {
        let mut e = Engine::default();
        let v = e.execute("groove.create", &json!({"config":{"repeats":1,"swing":30,"ending":4}})).unwrap();
        let original = e.session().find_clip(cid(&v)).unwrap().1.clone();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("soundcraft-groove-{}-{stamp}.scraft", std::process::id()));
        e.execute("session.save", &json!({"path":path})).unwrap();
        let mut reopened = Engine::default();
        reopened.execute("session.open", &json!({"path":path})).unwrap();
        reopened.execute("groove.update", &json!({"clip":v["clip"]})).unwrap();
        assert_eq!(reopened.session().find_clip(cid(&v)).unwrap().1, &original);
        let audio = soundcraft_mix::render_range(reopened.session(), original.range(), 256);
        assert!(audio.iter().flatten().all(|sample| sample.is_finite()));
        assert!(audio.iter().flatten().any(|sample| sample.abs() > 0.01));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn overlaps_manual_edits_and_locks_cannot_be_overwritten() {
        let mut e = Engine::default();
        let v = e.execute("groove.create", &json!({})).unwrap();
        let original = e.session().clone();
        assert!(e.execute("groove.create", &json!({"track":v["track"], "start":0})).is_err());
        assert_eq!(e.session(), &original);
        e.execute("midi.note_add", &json!({"clip":v["clip"], "pitch":60,"start_ticks":10})).unwrap();
        let edited = e.session().clone();
        assert!(e.execute("groove.update", &json!({"clip":v["clip"]})).is_err());
        assert_eq!(e.session(), &edited);
        e.execute("edit.undo", &json!({})).unwrap();
        e.session_mut().find_clip_mut(cid(&v)).unwrap().edit_locked = true;
        assert!(e.execute("groove.update", &json!({"clip":v["clip"]})).is_err());
        assert!(e.execute("groove.detach", &json!({"clip":v["clip"]})).is_err());
    }
}
