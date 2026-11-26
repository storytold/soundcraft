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
        params: "{track, slot: 0..9 | \"a\"..\"j\" | \"instrument\", state?: base64 | null, params?: instrument parameter values} → {bytes}. Not undoable and not journaled (states can be large).",
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

fn insert_state(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, CMD, p, "track")?.ok_or_else(|| bad(CMD, "`track` required"))?;
    let slot = slot_param(p)?;
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(b64::decode(s).ok_or_else(|| bad(CMD, "`state` is not valid base64 (or is too large)"))?),
        Some(_) => return Err(bad(CMD, "`state` must be a base64 string or null")),
    };
    let n = state.as_ref().map_or(0, Vec::len);
    // Plugins report changes that leave their state as it was: don't modify the session for those.
    let stored = e.session().track(t).and_then(|tr| match slot {
        Some(i) => tr.mixer.inserts.get(i).and_then(Option::as_ref),
        None => tr.instrument.as_ref(),
    });
    let ins = stored.ok_or_else(|| bad(CMD, "empty slot"))?;
    let mut values = Vec::new();
    if let Some(params) = p.get("params") {
        if slot.is_some() {
            return Err(bad(CMD, "editor parameter snapshots require the instrument slot"));
        }
        let obj = params.as_object().ok_or_else(|| bad(CMD, "`params` must be an object"))?;
        let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(CMD, "unknown instrument"))?;
        for pi in info.params {
            if let Some(v) = obj.get(pi.id) {
                let value = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(CMD, "expected a numeric parameter value"))?;
                values.push((pi.id.to_string(), pi.clamp(value as f32)));
            }
        }
        if values.len() != obj.len() {
            return Err(bad(CMD, "snapshot contains an unknown instrument parameter"));
        }
    }
    let write_state = p.get("state").is_some() || p.get("params").is_none();
    let state_changed = write_state && ins.state_bytes().as_deref() != state.as_deref().filter(|b| !b.is_empty());
    if !state_changed && values.iter().all(|(id, v)| ins.params.get(id) == Some(v)) {
        return Ok(json!({"bytes": n}));
    }
    let s = e.session_mut();
    let tr = s.track_mut(t).ok_or_else(|| bad(CMD, "no track"))?;
    let ins = match slot {
        Some(i) => tr.mixer.inserts.get_mut(i).and_then(Option::as_mut),
        None => tr.instrument.as_mut(),
    }
    .ok_or_else(|| bad(CMD, "empty slot"))?;
    if state_changed {
        match state {
            Some(bytes) => ins.set_state_bytes(&bytes),
            None => ins.state = None,
        }
    }
    ins.params.extend(values);
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
    fn native_instrument_values_roundtrip_with_the_patch_and_without_a_state_reader() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = e.session().tracks[0].id;
        let state = soundcraft_model::b64::encode(b"native patch");
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 500.0})).unwrap();
        let snapshot = json!({"track": t.0, "slot": "instrument", "state": state, "params": {"cutoff": 200.0}});
        e.execute("mix.insert_state", &snapshot).unwrap();
        let rev = e.revision;
        e.execute("mix.insert_state", &snapshot).unwrap();
        assert_eq!(e.revision, rev, "an identical capture does not dirty the session");
        e.execute("mix.insert_state", &json!({"track": t.0, "slot": "instrument", "params": {"cutoff": 300.0}})).unwrap();
        let path = std::env::temp_dir().join(format!("soundcraft-instrument-snapshot-{}.scraft", std::process::id()));
        e.execute("session.save", &json!({"path": path})).unwrap();
        let mut reopened = Engine::default();
        reopened.execute("session.open", &json!({"path": path})).unwrap();
        std::fs::remove_file(&path).unwrap();
        let instrument = reopened.session().track(t).unwrap().instrument.as_ref().unwrap();
        assert_eq!(instrument.params["cutoff"], 300.0, "host values seen by the first block and subsequent relative edits");
        assert_eq!(instrument.state_bytes().as_deref(), Some(&b"native patch"[..]));
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
