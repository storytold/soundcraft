//! Third-party plugin state in the session (`mix.insert_state`): the UI reads each live plugin's
//! state right before saving and stores it on its insert (base64), from where the mixer restores
//! it into new instances.
use super::{CommandSpec, bad, has_tracks, track_param};
use crate::{Engine, Result};

use serde_json::{Value, json};
use soundcraft_model::{INSERT_SLOTS, b64};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "mix.insert_state",
        label: "Store Plugin State",
        menu: &[],
        shortcut: None,
        params: "{track, slot: 0..9 | \"a\"..\"j\" | \"instrument\", state: base64 | null, params?: {id: number}} → {bytes}. The state is not undoable and not journaled (states can be large). `params`, when a value changes, is stored in that same edit as one undo step and is not journaled.",
        enabled: has_tracks,
        run: insert_state,
        journal: false,
        undoable: false,
        view: false,
    }]
}

const CMD: &str = "mix.insert_state";

/// `None` = the instrument slot.
fn slot_param(p: &Value) -> Result<Option<usize>> {
    let v = p.get("slot").ok_or_else(|| bad(CMD, "`slot` required"))?;
    if let Some(n) = v.as_u64() {
        return usize::try_from(n).ok().filter(|&n| n < INSERT_SLOTS).map(Some).ok_or_else(|| bad(CMD, "slot out of range"));
    }
    let s = v.as_str().map(|s| s.trim().to_ascii_lowercase()).ok_or_else(|| bad(CMD, "`slot` must be a number or a letter"))?;
    if s == "instrument" {
        return Ok(None);
    }
    match s.as_bytes() {
        [c @ b'a'..=b'z'] if usize::from(c - b'a') < INSERT_SLOTS => Ok(Some(usize::from(c - b'a'))),
        _ => s.parse::<usize>().ok().filter(|&n| n < INSERT_SLOTS).map(Some).ok_or_else(|| bad(CMD, "slot out of range")),
    }
}

fn stored_insert(e: &Engine, track: soundcraft_model::TrackId, slot: Option<usize>) -> Option<&soundcraft_model::Insert> {
    let tr = e.session().track(track)?;
    match slot {
        Some(i) => tr.mixer.inserts.get(i).and_then(Option::as_ref),
        None => tr.instrument.as_ref(),
    }
}

fn insert_mut(s: &mut soundcraft_model::Session, track: soundcraft_model::TrackId, slot: Option<usize>) -> Option<&mut soundcraft_model::Insert> {
    let tr = s.track_mut(track)?;
    match slot {
        Some(i) => tr.mixer.inserts.get_mut(i).and_then(Option::as_mut),
        None => tr.instrument.as_mut(),
    }
}

fn state_matches(ins: &soundcraft_model::Insert, state: Option<&[u8]>) -> bool {
    ins.state_bytes().as_deref() == state.filter(|b| !b.is_empty())
}

/// Optional `params` on `mix.insert_state`. Absent or empty stores nothing. Keys are parameter
/// ids; values are finite numbers, clamped the same way as `mix.insert_params`.
fn param_writes(e: &Engine, track: soundcraft_model::TrackId, slot: Option<usize>, p: &Value) -> Result<Vec<(String, f32)>> {
    let Some(raw) = p.get("params") else { return Ok(Vec::new()) };
    let obj = raw.as_object().ok_or_else(|| bad(CMD, "`params` must be an object"))?;
    if obj.is_empty() {
        return Ok(Vec::new());
    }
    let ins = stored_insert(e, track, slot).ok_or_else(|| bad(CMD, "empty slot"))?;
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(CMD, "unknown plugin"))?;
    let mut unknown = Vec::new();
    let mut non_numeric = Vec::new();
    for (k, v) in obj {
        if !info.params.iter().any(|x| x.id == k.as_str()) {
            unknown.push(k.as_str());
        } else if v.as_f64().is_none_or(|f| !f.is_finite()) {
            non_numeric.push(k.as_str());
        }
    }
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(bad(CMD, format!("`{}` has no parameter(s): {}", info.id, unknown.join(", "))));
    }
    if !non_numeric.is_empty() {
        non_numeric.sort_unstable();
        return Err(bad(CMD, format!("parameter(s) need a numeric value: {}", non_numeric.join(", "))));
    }
    let mut out = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        let Some(pi) = info.params.iter().find(|x| x.id == k.as_str()) else { continue };
        let Some(f) = v.as_f64().filter(|f| f.is_finite()) else { continue };
        out.push((k.clone(), pi.clamp(f as f32)));
    }
    Ok(out)
}

fn insert_state(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, CMD, p, "track")?.ok_or_else(|| bad(CMD, "`track` required"))?;
    let slot = slot_param(p)?;
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(b64::decode(s).ok_or_else(|| bad(CMD, "`state` is not valid base64 (or is too large)"))?),
        Some(_) => return Err(bad(CMD, "`state` must be a base64 string or null")),
    };
    let n = state.as_ref().map_or(0, Vec::len);
    let writes = param_writes(e, t, slot, p)?;
    // Plugins report changes that leave their state as it was: don't modify the session for those.
    // A held parameter that actually moved is part of the same edit, so it does not publish a
    // second revision on its own.
    let (unchanged, params_changed) = {
        let Some(stored) = stored_insert(e, t, slot) else { return Err(bad(CMD, "empty slot")) };
        let params_changed = writes.iter().any(|(k, v)| stored.params.get(k).copied() != Some(*v));
        (state_matches(stored, state.as_deref()) && !params_changed, params_changed)
    };
    if unchanged {
        return Ok(json!({"bytes": n}));
    }
    let undo = params_changed.then(|| e.session_arc());
    {
        let s = e.session_mut();
        let ins = insert_mut(s, t, slot).ok_or_else(|| bad(CMD, "empty slot"))?;
        for (k, v) in &writes {
            ins.params.insert(k.clone(), *v);
        }
        match &state {
            Some(bytes) => ins.set_state_bytes(bytes),
            None => ins.state = None,
        }
    }
    if let Some(mut before) = undo {
        // State is not undoable. The snapshot was taken before this command, so it still has the
        // previous bytes; put the bytes just stored back, and let undo restore only the parameters.
        if let Some(ins) = insert_mut(std::sync::Arc::make_mut(&mut before), t, slot) {
            match &state {
                Some(bytes) => ins.set_state_bytes(bytes),
                None => ins.state = None,
            }
        }
        e.push_undo("Set Plugin Parameters", before);
    }
    Ok(json!({"bytes": n}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::b64;

    #[test]
    fn stores_and_clears_state_without_undo() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 2})).unwrap();
        let undo_before = e.can_undo();
        let blob = b64::encode(b"\x00\x01plugin state\xff");
        let r = e.execute("mix.insert_state", &json!({"track": t, "slot": "c", "state": blob})).unwrap();
        assert_eq!(r["bytes"], 15);
        let ins = e.session().tracks[0].mixer.inserts[2].clone().unwrap();
        assert_eq!(ins.state_bytes().unwrap(), b"\x00\x01plugin state\xff");
        assert_eq!(e.can_undo(), undo_before, "not undoable");
        assert!(e.journal.iter().all(|(id, _)| id != "mix.insert_state"), "not journaled");
        // Survives save/load.
        let dir = std::env::temp_dir().join(format!("soundcraft-insert-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.scraft");
        e.execute("session.save", &json!({"path": path.to_string_lossy()})).unwrap();
        let mut f = Engine::default();
        f.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(f.session().tracks[0].mixer.inserts[2].as_ref().unwrap().state, ins.state);
        let _ = std::fs::remove_dir_all(&dir);
        e.execute("mix.insert_state", &json!({"track": t, "slot": 2, "state": null})).unwrap();
        assert_eq!(e.session().tracks[0].mixer.inserts[2].as_ref().unwrap().state, None);
    }

    #[test]
    fn held_params_share_the_state_revision_and_one_undo_step() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let rev = e.revision;
        let undo = e.undo_history().len();
        let blob = b64::encode(&[9, 1]);
        e.execute("mix.insert_state", &json!({"track": t, "slot": "instrument", "state": blob, "params": {"cutoff": 44.0}})).unwrap();
        assert_eq!(e.revision, rev.wrapping_add(1), "the parameter and the state are one document revision");
        assert_eq!(e.undo_history().len(), undo + 1);
        assert_eq!(e.undo_history().last().map(String::as_str), Some("Set Plugin Parameters"));
        let ins = e.session().tracks[0].instrument.clone().unwrap();
        assert_eq!(ins.params.get("cutoff").copied(), Some(44.0));
        assert_eq!(ins.state_bytes().as_deref(), Some(&[9, 1][..]));
        assert!(e.journal.iter().all(|(id, _)| id != "mix.insert_state"), "not journaled");
        // The same bytes and the same value publish nothing more.
        let rev = e.revision;
        let undo = e.undo_history().len();
        e.execute("mix.insert_state", &json!({"track": t, "slot": "instrument", "state": blob, "params": {"cutoff": 44.0}})).unwrap();
        assert_eq!(e.revision, rev);
        assert_eq!(e.undo_history().len(), undo);
    }

    #[test]
    fn undo_restores_parameters_and_keeps_the_new_state() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let old_state = b64::encode(b"old-state");
        e.execute("mix.insert_state", &json!({"track": t, "slot": "instrument", "state": old_state})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t, "param": "cutoff", "value": 1000.0})).unwrap();
        let new_state = b64::encode(b"new-state");
        e.execute("mix.insert_state", &json!({"track": t, "slot": "instrument", "state": new_state, "params": {"cutoff": 44.0}})).unwrap();
        let ins = e.session().tracks[0].instrument.clone().unwrap();
        assert_eq!(ins.params.get("cutoff").copied(), Some(44.0));
        assert_eq!(ins.state_bytes().as_deref(), Some(&b"new-state"[..]));
        assert!(e.undo());
        let ins = e.session().tracks[0].instrument.clone().unwrap();
        assert_eq!(ins.params.get("cutoff").copied(), Some(1000.0), "undo restores the parameter");
        assert_eq!(ins.state_bytes().as_deref(), Some(&b"new-state"[..]), "undo keeps the new state");
        assert!(e.redo());
        let ins = e.session().tracks[0].instrument.clone().unwrap();
        assert_eq!(ins.params.get("cutoff").copied(), Some(44.0));
        assert_eq!(ins.state_bytes().as_deref(), Some(&b"new-state"[..]));

        // Clearing the state is the new state: undo restores the parameter and leaves it cleared.
        e.execute("track.new", &json!({})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        let a = audio.0;
        e.execute("mix.insert", &json!({"track": a, "plugin": "gain", "slot": 0})).unwrap();
        let blob = b64::encode(b"gain-state");
        e.execute("mix.insert_state", &json!({"track": a, "slot": 0, "state": blob})).unwrap();
        e.execute("mix.insert_param", &json!({"track": a, "slot": 0, "param": "gain", "value": -6.0})).unwrap();
        let before = e.session().track(audio).unwrap().mixer.inserts[0].as_ref().unwrap().params.get("gain").copied();
        e.execute("mix.insert_state", &json!({"track": a, "slot": 0, "state": null, "params": {"gain": -12.0}})).unwrap();
        assert_eq!(e.session().track(audio).unwrap().mixer.inserts[0].as_ref().unwrap().state, None);
        assert!(e.undo());
        let ins = e.session().track(audio).unwrap().mixer.inserts[0].as_ref().unwrap();
        assert_eq!(ins.params.get("gain").copied(), before);
        assert_eq!(ins.state, None, "undo does not bring the cleared state back");
    }

    #[test]
    fn storing_the_same_state_again_changes_nothing() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 0})).unwrap();
        let blob = b64::encode(b"same");
        e.execute("mix.insert_state", &json!({"track": t, "slot": 0, "state": blob})).unwrap();
        let rev = e.revision;
        e.execute("mix.insert_state", &json!({"track": t, "slot": 0, "state": blob})).unwrap();
        assert_eq!(e.revision, rev);
    }

    #[test]
    fn hostile_params_are_errors() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        e.execute("mix.insert", &json!({"track": t, "plugin": "gain", "slot": 0})).unwrap();
        for p in [
            json!({"track": t, "slot": 0, "state": "!!!"}),
            json!({"track": t, "slot": 0, "state": 5}),
            json!({"track": t, "slot": 1, "state": "AAAA"}),
            json!({"track": t, "slot": 99, "state": "AAAA"}),
            json!({"track": t, "slot": "instrument", "state": "AAAA"}),
            json!({"track": t, "slot": -1}),
            json!({"track": t}),
            json!({"track": 999, "slot": 0}),
            json!({"slot": 0}),
        ] {
            assert!(e.execute("mix.insert_state", &p).is_err(), "{p}");
        }
    }
}
