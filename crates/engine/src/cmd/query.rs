//! Read-only queries for agents.
use super::{CommandInfo, CommandSpec, always, bad, command_specs, has_tracks, position_param, str_param, track_param};

use crate::cmd;
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "session.inspect", "Inspect Session", [], None, "{detail?: summary|full}", always, |e, p| Ok(crate::inspect::session(e, str_param(p, "detail") == Some("full")))),
        cmd!(query "track.inspect", "Inspect Track", [], None, "{track}", has_tracks, |e, p| {
            let t = track_param(e, "track.inspect", p, "track")?.ok_or_else(|| bad("track.inspect", "`track` required"))?;
            crate::inspect::track(e, t).ok_or_else(|| bad("track.inspect", "no such track"))
        }),
        cmd!(query "clip.inspect", "Inspect Clip", [], None, "{clip}", always, |e, p| {
            let id = p.get("clip").and_then(Value::as_u64).ok_or_else(|| bad("clip.inspect", "`clip` id required"))?;
            crate::inspect::clip(e, soundcraft_model::ClipId(id)).ok_or_else(|| bad("clip.inspect", "no such clip"))
        }),
        cmd!(query "engine.commands", "List Commands", [], None, "{filter?}", always, |e, p| {
            let f = str_param(p, "filter").map(str::to_ascii_lowercase);
            let v: Vec<CommandInfo> = command_specs()
                .iter()
                .filter(|c| f.as_ref().is_none_or(|f| c.id.contains(f.as_str()) || c.label.to_ascii_lowercase().contains(f.as_str())))
                .map(|c| c.info(e))
                .collect();
            Ok(json!(v))
        }),
        cmd!(query "engine.history", "Undo History", ["Window"], None, "{}", always, |e, _| Ok(json!({"undo": e.undo_history(), "can_redo": e.can_redo()}))),
        cmd!(query "engine.plugins", "List Plugins", [], None, "{}", always, |_, _| {
            let v: Vec<_> = soundcraft_dsp::plugins().iter().map(|p| json!({"id": p.id, "name": p.name, "category": p.category, "instrument": p.is_instrument, "params": p.params})).collect();
            Ok(json!(v))
        }),
        cmd!(query "engine.parity", "Parity Report", [], None, "{}", always, |_, _| Ok(crate::catalog::parity_json())),
        cmd!(query "session.format_time", "Format Time", [], None, "{at: samples, format?: bars_beats|min_secs|timecode|feet_frames|samples}", always, |e, p| {
            let at = position_param(e, "session.format_time", p, "at")?.unwrap_or(0);
            let s = e.session();
            let f = str_param(p, "format").and_then(soundcraft_time::TimeFormat::from_id).unwrap_or(s.edit.main_counter);
            Ok(json!({"text": s.format_position(at, f)}))
        }),
    ]
}
