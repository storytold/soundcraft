//! Reusable whole-clip definition commands; capture itself retains its existing menu entry.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "clip.definitions", "Reusable Clip Definitions", [], None, "{} — lists detached whole-clip snapshots", always, |e,_| {
            e.session().validate_definitions().map_err(|error| bad("clip.definitions",error.to_string()))?;
            Ok(json!({"definitions":e.session().clip_definitions}))
        }),
        cmd!(
            "clip.place_definition",
            "Place Reusable Clip",
            [],
            None,
            "{definition: id, track?: id|name, at?: position} — places a fresh whole clip in an empty destination; default: a new track",
            always,
            crate::clip_capture::place
        ),
        cmd!(
            "clip.clear_definitions",
            "Remove Reusable Clips",
            [],
            None,
            "{definitions: [id]} — removes definitions; existing clips and media remain",
            always,
            crate::clip_capture::clear
        ),
    ]
}
