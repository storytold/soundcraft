//! Atomic, bounded batch track naming. Preview and Apply use the same planner.

use super::*;
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};

const APPLY: &str = "track.batch_rename";
const PREVIEW: &str = "track.batch_rename_preview";
const MAX_TARGETS: usize = 4096;
const MAX_TEXT_BYTES: usize = 256;
const MAX_WORK_BYTES: usize = 1024;
const MAX_NAME_CHARS: usize = 64;

pub fn specs() -> Vec<CommandSpec> {
    let params = "{tracks?: ids|names[], track?: id|name, expected_revision?: u64, replace?: {clear?: false, find?: string, with?: string}, trim?: {beginning?: 0, end?: 0}, add?: {prefix?: string, suffix?: string}, numbering?: {position?: beginning|end, start?: 1, increment?: 1, places?: 1, separator?: string}} (session track order; 4096 targets; names <=64 Unicode scalars)";
    vec![
        CommandSpec {
            id: APPLY,
            label: "Batch Rename Tracks",
            menu: &["Track"],
            shortcut: Some("Alt+Shift+R"),
            params,
            enabled: has_selection,
            run: apply,
            journal: true,
            undoable: true,
            view: false,
        },
        CommandSpec {
            id: PREVIEW,
            label: "Preview Batch Track Rename",
            menu: &[],
            shortcut: None,
            params,
            enabled: has_selection,
            run: preview,
            journal: false,
            undoable: false,
            view: false,
        },
    ]
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Params {
    tracks: Option<Vec<Value>>,
    track: Option<Value>,
    expected_revision: Option<u64>,
    replace: Option<Replace>,
    trim: Option<Trim>,
    add: Option<Add>,
    numbering: Option<Numbering>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Replace {
    clear: bool,
    find: String,
    with: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Trim {
    beginning: u64,
    end: u64,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Add {
    prefix: String,
    suffix: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Position {
    #[default]
    Beginning,
    End,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Numbering {
    position: Position,
    start: u64,
    increment: u64,
    places: usize,
    separator: String,
}

impl Default for Numbering {
    fn default() -> Self {
        Self { position: Position::Beginning, start: 1, increment: 1, places: 1, separator: String::new() }
    }
}

struct Row {
    id: TrackId,
    old: String,
    name: String,
    error: Option<String>,
}

struct Plan {
    rows: Vec<Row>,
    revision: u64,
}

impl Plan {
    fn valid(&self) -> bool {
        self.rows.iter().all(|r| r.error.is_none())
    }

    fn value(&self) -> Value {
        json!({
            "revision": self.revision,
            "valid": self.valid(),
            "changed": self.rows.iter().filter(|r| r.error.is_none() && r.name != r.old).count(),
            "rows": self.rows.iter().map(|r| json!({"track": r.id.0, "old": r.old, "name": r.name, "error": r.error})).collect::<Vec<_>>()
        })
    }
}

fn plan(e: &Engine, p: &Value, cmd: &str) -> Result<Plan> {
    // Bound the array before deserializing/cloning it.
    if let Some(v) = p.get("tracks") {
        let tracks = v.as_array().ok_or_else(|| bad(cmd, "`tracks` must be an array"))?;
        if tracks.len() > MAX_TARGETS {
            return Err(bad(cmd, "at most 4096 tracks can be renamed together"));
        }
        for target in tracks {
            validate_target(target).map_err(|err| bad(cmd, err))?;
        }
    }
    if let Some(target) = p.get("track") {
        validate_target(target).map_err(|err| bad(cmd, err))?;
    }
    for (stage, keys) in [("replace", &["find", "with"][..]), ("add", &["prefix", "suffix"][..]), ("numbering", &["separator"][..])] {
        for key in keys {
            if let Some(text) = p.get(stage).and_then(|v| v.get(*key)).and_then(Value::as_str) {
                text_bound(text).map_err(|err| bad(cmd, err))?;
            }
        }
    }
    let params = Params::deserialize(p).map_err(|err| bad(cmd, err.to_string()))?;
    if params.expected_revision.is_some_and(|r| r != e.revision) {
        return Err(bad(cmd, "session changed; preview the names again before applying"));
    }
    if params.tracks.is_some() && params.track.is_some() {
        return Err(bad(cmd, "pass either `track` or `tracks`, not both"));
    }
    validate_stages(&params).map_err(|err| bad(cmd, err))?;
    if params.tracks.is_none() && params.track.is_none() {
        let s = e.session();
        if s.edit.selected_tracks.len() > MAX_TARGETS
            || s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).take(MAX_TARGETS + 1).count() > MAX_TARGETS
        {
            return Err(bad(cmd, "at most 4096 tracks can be renamed together"));
        }
    }
    let targets = tracks_required(e, cmd, p)?;
    if targets.len() > MAX_TARGETS {
        return Err(bad(cmd, "at most 4096 tracks can be renamed together"));
    }
    let mut ids = HashSet::with_capacity(targets.len());
    for id in targets {
        if !ids.insert(id) {
            return Err(bad(cmd, "a track can appear only once in a batch"));
        }
    }
    let mut rows = Vec::with_capacity(ids.len());
    for track in e.session().tracks.iter().filter(|t| ids.contains(&t.id)) {
        if rows.len() >= ids.len() {
            return Err(bad(cmd, "target track IDs are duplicated in the session"));
        }
        let result = transform(&track.name, rows.len(), &params);
        let (name, error) = match result {
            Ok(name) => {
                let error = if name.is_empty() {
                    Some("name must not be empty".into())
                } else if name.chars().count() > MAX_NAME_CHARS {
                    Some("name exceeds 64 Unicode scalars; shorten it before applying".into())
                } else {
                    None
                };
                (name, error)
            }
            Err(err) => (String::new(), Some(err)),
        };
        // A hostile persisted name is not copied into the preview unboundedly.
        let old = track.name.chars().take(MAX_WORK_BYTES).collect();
        rows.push(Row { id: track.id, old, name, error });
    }
    if rows.len() != ids.len() {
        return Err(bad(cmd, "one or more target tracks no longer exist"));
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for row in &rows {
        if row.error.is_none() {
            *counts.entry(&row.name).or_default() += 1;
        }
    }
    // Store only collisions, not a second index of every unselected track.
    let reserved: HashSet<&str> =
        e.session().tracks.iter().filter(|t| !ids.contains(&t.id)).map(|t| t.name.as_str()).filter(|name| counts.contains_key(name)).collect();
    let errors: Vec<_> = rows
        .iter()
        .map(|r| {
            if reserved.contains(r.name.as_str()) {
                Some("name belongs to a track outside this batch".to_string())
            } else if counts.get(r.name.as_str()).copied().unwrap_or_default() > 1 {
                Some("multiple tracks would have this name".to_string())
            } else {
                None
            }
        })
        .collect();
    for (row, error) in rows.iter_mut().zip(errors) {
        if row.error.is_none() {
            row.error = error;
        }
    }
    Ok(Plan { rows, revision: e.revision })
}

fn validate_target(target: &Value) -> std::result::Result<(), String> {
    if target.as_u64().is_some() || target.as_str().is_some_and(|s| s.len() <= MAX_WORK_BYTES) {
        Ok(())
    } else {
        Err("targets must be integer track IDs or names of at most 1024 UTF-8 bytes".into())
    }
}

fn validate_stages(p: &Params) -> std::result::Result<(), String> {
    if let Some(r) = &p.replace {
        text_bound(&r.find)?;
        text_bound(&r.with)?;
        if !r.clear && r.find.is_empty() {
            return Err("Replace needs nonempty Find text or Clear Existing Name".into());
        }
    }
    if let Some(t) = &p.trim
        && (t.beginning > MAX_WORK_BYTES as u64 || t.end > MAX_WORK_BYTES as u64)
    {
        return Err("trim counts must be integers from 0 to 1024".into());
    }
    if let Some(a) = &p.add {
        text_bound(&a.prefix)?;
        text_bound(&a.suffix)?;
    }
    if let Some(n) = &p.numbering {
        if !(1..=20).contains(&n.places) {
            return Err("numbering places must be an integer from 1 to 20".into());
        }
        if n.separator.len() > 4 || n.separator.chars().count() > 1 {
            return Err("numbering separator must be empty or one Unicode scalar".into());
        }
    }
    Ok(())
}

fn text_bound(text: &str) -> std::result::Result<(), String> {
    if text.len() > MAX_TEXT_BYTES { Err("Find, replacement, prefix and suffix text are limited to 256 UTF-8 bytes each".into()) } else { Ok(()) }
}

fn append(out: &mut String, text: &str) -> std::result::Result<(), String> {
    if out.len().saturating_add(text.len()) > MAX_WORK_BYTES {
        return Err("intermediate names are limited to 1024 UTF-8 bytes".into());
    }
    out.push_str(text);
    Ok(())
}

fn transform(old: &str, index: usize, p: &Params) -> std::result::Result<String, String> {
    if old.len() > MAX_WORK_BYTES {
        return Err("existing name exceeds the 1024-byte working limit".into());
    }
    let mut name = old.to_string();
    if let Some(r) = &p.replace {
        if r.clear {
            name.clear();
        } else {
            let mut out = String::new();
            let mut rest = name.as_str();
            while let Some(at) = rest.find(&r.find) {
                append(&mut out, rest.get(..at).unwrap_or_default())?;
                append(&mut out, &r.with)?;
                rest = rest.get(at.saturating_add(r.find.len())..).unwrap_or_default();
            }
            append(&mut out, rest)?;
            name = out;
        }
    }
    if let Some(t) = &p.trim {
        let chars = name.chars().count();
        let begin = usize::try_from(t.beginning).map_err(|_| "trim count is too large")?.min(chars);
        let end = usize::try_from(t.end).map_err(|_| "trim count is too large")?;
        name = name.chars().skip(begin).take(chars.saturating_sub(begin).saturating_sub(end)).collect();
    }
    if let Some(a) = &p.add {
        let mut out = String::new();
        append(&mut out, &a.prefix)?;
        append(&mut out, &name)?;
        append(&mut out, &a.suffix)?;
        name = out;
    }
    if let Some(n) = &p.numbering {
        let index = u64::try_from(index).map_err(|_| "too many numbering steps")?;
        let value = n.increment.checked_mul(index).and_then(|v| n.start.checked_add(v)).ok_or("numbering exceeds u64")?;
        let digits = format!("{value:0width$}", width = n.places);
        let mut out = String::new();
        let separator = if name.is_empty() { "" } else { &n.separator };
        match n.position {
            Position::Beginning => {
                append(&mut out, &digits)?;
                append(&mut out, separator)?;
                append(&mut out, &name)?;
            }
            Position::End => {
                append(&mut out, &name)?;
                append(&mut out, separator)?;
                append(&mut out, &digits)?;
            }
        }
        name = out;
    }
    Ok(name.trim().to_string())
}

fn preview(e: &mut Engine, p: &Value) -> Result<Value> {
    Ok(plan(e, p, PREVIEW)?.value())
}

fn apply(e: &mut Engine, p: &Value) -> Result<Value> {
    let plan = plan(e, p, APPLY)?;
    if !plan.valid() {
        let row = plan.rows.iter().find(|r| r.error.is_some());
        return Err(bad(APPLY, row.and_then(|r| r.error.as_deref()).unwrap_or("invalid names")));
    }
    let value = plan.value();
    if plan.rows.iter().any(|r| r.old != r.name) {
        let names: HashMap<TrackId, String> = plan.rows.into_iter().map(|r| (r.id, r.name)).collect();
        for track in &mut e.session_mut().tracks {
            if let Some(name) = names.get(&track.id) {
                track.name.clone_from(name);
            }
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Route, Session, TrackKind};
    use std::sync::Arc;

    fn engine(names: &[&str]) -> Engine {
        let mut s = Session::default();
        for name in names {
            s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        }
        s.edit.selected_tracks = s.tracks.iter().map(|t| t.id).collect();
        Engine::new(s)
    }

    fn names(e: &Engine) -> Vec<&str> {
        e.session().tracks.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn batch_rename_stage_order_and_unicode() {
        let mut e = engine(&["xé_Kick_z", "x猫_Snare_z"]);
        let p = json!({"replace":{"find":"_","with":"-"},"trim":{"beginning":2,"end":2},"add":{"prefix":"Drum","suffix":"!"},"numbering":{"start":7,"increment":2,"places":3,"separator":"·"}});
        e.execute(APPLY, &p).unwrap();
        assert_eq!(names(&e), vec!["007·Drum-Kick!", "009·Drum-Snare!"]);
    }

    #[test]
    fn batch_rename_swap_is_one_undo_and_preserves_every_other_field() {
        let mut e = engine(&["2", "1"]);
        let folder = e.session().tracks[0].id;
        let mut session = e.session().clone();
        session.tracks[1].folder = Some(folder);
        session.tracks[1].mixer.output = Route::Bus(soundcraft_model::BusId(91));
        session.tracks[1].mixer.vca = Some(37);
        session.tracks[0].comments = "unchanged".into();
        e = Engine::new(session.clone());
        e.execute(APPLY, &json!({"replace":{"clear":true},"numbering":{}})).unwrap();
        let mut expected = session.clone();
        expected.tracks[0].name = "1".into();
        expected.tracks[1].name = "2".into();
        assert_eq!(e.session(), &expected);
        assert_eq!(e.undo_label(), Some("Batch Rename Tracks"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session(), &session);
        assert!(e.undo_label().is_none());
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(e.session(), &expected);
    }

    #[test]
    fn batch_rename_explicit_targets_follow_session_order() {
        let mut e = engine(&["A", "B", "C"]);
        let ids: Vec<_> = e.session().tracks.iter().map(|t| t.id.0).collect();
        e.execute(APPLY, &json!({"tracks":[ids[2],"A"],"replace":{"clear":true},"numbering":{"start":3}})).unwrap();
        assert_eq!(names(&e), vec!["3", "B", "4"]);
    }

    #[test]
    fn batch_rename_invalid_final_sets_never_mutate() {
        for p in [
            json!({"replace":{"clear":true},"add":{"prefix":"same"}}),
            json!({"tracks":["A"],"replace":{"clear":true},"add":{"prefix":"B"}}),
            json!({"trim":{"beginning":1024}}),
            json!({"add":{"suffix":"é".repeat(64)}}),
            json!({"numbering":{"start":u64::MAX,"increment":1}}),
        ] {
            let mut e = engine(&["A", "B"]);
            let before = e.session_arc();
            let rev = e.revision;
            let preview = e.execute(PREVIEW, &p).unwrap();
            assert_eq!(preview["valid"], false);
            assert!(e.execute(APPLY, &p).is_err(), "{p}");
            assert!(Arc::ptr_eq(&before, &e.session_arc()));
            assert_eq!(e.revision, rev);
            assert!(!e.is_dirty());
            assert!(e.undo_label().is_none());
        }
    }

    #[test]
    fn batch_rename_preview_and_noop_leave_history_and_dirty_unchanged() {
        let mut e = engine(&["A", "B"]);
        let before = e.session_arc();
        let rev = e.revision;
        let p = json!({"replace":{"find":"absent","with":"value"}});
        assert_eq!(e.execute(PREVIEW, &p).unwrap()["changed"], 0);
        assert!(e.journal.is_empty());
        assert_eq!(e.execute(APPLY, &p).unwrap()["changed"], 0);
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert_eq!(e.revision, rev);
        assert!(!e.is_dirty());
        assert!(e.undo_label().is_none());
    }

    #[test]
    fn batch_rename_noop_keeps_an_existing_redo_step() {
        let mut e = engine(&["A"]);
        e.execute(APPLY, &json!({"add":{"suffix":"!"}})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        let before = e.session_arc();
        let redo = e.redo_label().map(str::to_string);
        let dirty = e.is_dirty();
        e.execute(PREVIEW, &json!({})).unwrap();
        e.execute(APPLY, &json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert_eq!(e.redo_label(), redo.as_deref());
        assert_eq!(e.is_dirty(), dirty);
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(names(&e), vec!["A!"]);
    }

    #[test]
    fn batch_rename_rejects_hostile_types_duplicates_and_bounds() {
        let mut e = engine(&["A", "B"]);
        for p in [
            json!({"tracks":"A"}),
            json!({"tracks":["A","A"]}),
            json!({"tracks":["missing"]}),
            json!({"tracks":["A"],"track":"B"}),
            json!({"trim":{"beginning":1.5}}),
            json!({"trim":{"end":-1}}),
            json!({"trim":{"end":1025}}),
            json!({"replace":{"find":""}}),
            json!({"replace":{"clear":"yes"}}),
            json!({"add":{"prefix":"a".repeat(257)}}),
            json!({"numbering":{"places":21}}),
            json!({"numbering":{"places":0}}),
            json!({"numbering":{"increment":-1}}),
            json!({"numbering":{"separator":"ab"}}),
            json!({"unknown":true}),
            json!({"tracks":vec!["A";4097]}),
        ] {
            assert!(e.execute(APPLY, &p).is_err(), "{p}");
            assert!(!e.is_dirty());
            assert!(e.undo_label().is_none());
        }
    }

    #[test]
    fn batch_rename_rejects_stale_revision_even_with_reused_ids() {
        let mut e = engine(&["A"]);
        let rev = e.revision;
        let new = engine(&["Other"]).session().clone();
        e.replace_session(new);
        let p = json!({"tracks":[e.session().tracks[0].id.0],"expected_revision":rev,"add":{"suffix":"!"}});
        assert!(e.execute(APPLY, &p).is_err());
        assert_eq!(names(&e), vec!["Other"]);
    }

    #[test]
    fn batch_rename_end_numbering_and_no_truncation_collision() {
        let mut e = engine(&["猫é", "other"]);
        e.execute(APPLY, &json!({"track":"猫é","trim":{"beginning":1},"numbering":{"position":"end","start":0,"separator":"-"}})).unwrap();
        assert_eq!(names(&e), vec!["é-0", "other"]);
        let long = "x".repeat(64);
        let mut e = engine(&[&long, "x"]);
        assert!(e.execute(APPLY, &json!({"track":"x","add":{"suffix":"x".repeat(64)}})).is_err());
        assert_eq!(names(&e), vec![long.as_str(), "x"]);
    }

    #[test]
    fn batch_rename_cycle_and_case_sensitive_names() {
        let mut e = engine(&["3", "1", "2"]);
        e.execute(APPLY, &json!({"replace":{"clear":true},"numbering":{}})).unwrap();
        assert_eq!(names(&e), vec!["1", "2", "3"]);
        let mut e = engine(&["Kick", "kick"]);
        let id = e.session().tracks[1].id.0;
        e.execute(APPLY, &json!({"tracks":[id],"add":{"suffix":"!"}})).unwrap();
        assert_eq!(names(&e), vec!["Kick", "kick!"]);
    }

    #[test]
    fn batch_rename_bounds_intermediate_expansion_and_existing_names() {
        let mut e = engine(&[&"a".repeat(64)]);
        let p = json!({"replace":{"find":"a","with":"b".repeat(256)},"trim":{"end":1024}});
        assert_eq!(e.execute(PREVIEW, &p).unwrap()["valid"], false);
        assert!(e.execute(APPLY, &p).is_err());
        let mut e = engine(&[&"a".repeat(2048)]);
        let v = e.execute(PREVIEW, &json!({"replace":{"clear":true},"numbering":{}})).unwrap();
        assert_eq!(v["valid"], false);
        assert_eq!(v["rows"][0]["old"].as_str().unwrap().len(), 1024);
    }

    #[test]
    fn batch_rename_bounds_selection_and_rejects_duplicate_document_ids() {
        let mut s = engine(&["A"]).session().clone();
        s.edit.selected_tracks = vec![s.tracks[0].id; 4097];
        let mut e = Engine::new(s);
        assert!(e.execute(PREVIEW, &json!({})).is_err());
        let mut s = engine(&["A"]).session().clone();
        s.tracks.push(s.tracks[0].clone());
        let id = s.tracks[0].id.0;
        let mut e = Engine::new(s);
        assert!(e.execute(APPLY, &json!({"tracks":[id],"add":{"suffix":"!"}})).is_err());
        assert!(!e.is_dirty());
        assert!(e.undo_label().is_none());
    }
}
