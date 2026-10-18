//! Explicit, persisted Master Fader insert order; legacy sessions retain their signal order.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{MasterInsertOrder, TrackKind};

const ID: &str = "mix.master_insert_order";
const MAX_TARGETS: usize = 4096;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!("mix.master_insert_order", "Set Master Insert Order", [], None, "{track?|tracks?, order: pre_fader|post_fader}", has_selection, set)]
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let object = p.as_object().ok_or_else(|| bad(ID, "parameters must be an object"))?;
    if let Some(key) = object.keys().find(|key| !matches!(key.as_str(), "track" | "tracks" | "order")) {
        return Err(bad(ID, format!("unknown parameter `{key}`")));
    }
    if p.get("track").is_some() && p.get("tracks").is_some() {
        return Err(bad(ID, "use either `track` or `tracks`, not both"));
    }
    if let Some(value) = p.get("tracks") {
        let tracks = value.as_array().ok_or_else(|| bad(ID, "tracks must be an array of track IDs or names"))?;
        if tracks.len() > MAX_TARGETS {
            return Err(bad(ID, "at most 4096 Master tracks may be targeted"));
        }
    }
    let order = match p.get("order").and_then(Value::as_str) {
        Some("pre_fader") => MasterInsertOrder::PreFader,
        Some("post_fader") => MasterInsertOrder::PostFader,
        _ => return Err(bad(ID, "order must be pre_fader or post_fader")),
    };
    let mut tracks = tracks_required(e, ID, p)?;
    if tracks.len() > MAX_TARGETS {
        return Err(bad(ID, "at most 4096 Master tracks may be targeted"));
    }
    tracks.sort();
    tracks.dedup();
    let mut changed = 0;
    for id in &tracks {
        let track = e.session().track(*id).ok_or_else(|| bad(ID, "target track is missing"))?;
        if track.kind != TrackKind::Master {
            return Err(bad(ID, "insert order can only be set on Master tracks"));
        }
        changed += usize::from(track.mixer.master_insert_order != order);
    }
    if changed > 0 {
        let session = e.session_mut();
        for id in &tracks {
            if let Some(track) = session.track_mut(*id) {
                track.mixer.master_insert_order = order;
            }
        }
    }
    Ok(json!({"order": order, "tracks": tracks, "changed": changed}))
}
