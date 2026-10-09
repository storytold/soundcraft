//! JSON views of the session for agents (so they can verify their work without screenshots).

use crate::Engine;
use serde_json::{Value, json};
use soundcraft_model::{Clip, ClipContent, ClipId, Track, TrackId};
use soundcraft_time::{TimeFormat, format_position};

fn clip_json(e: &Engine, c: &Clip) -> Value {
    let s = e.session();
    let fmt = |x| format_position(x, TimeFormat::MinSecs, s.sample_rate, &s.tempo, s.frame_rate, s.timecode_start);
    let mut v = json!({
        "id": c.id, "name": c.name, "start": c.start, "end": c.end(), "length": c.length,
        "start_time": fmt(c.start), "gain_db": c.gain_db, "muted": c.muted,
        "fade_in": c.fade_in.len, "fade_out": c.fade_out.len, "locked": c.edit_locked || c.time_locked,
    });
    match &c.content {
        ClipContent::Audio { source, offset } => {
            v["source"] = json!(source);
            v["offset"] = json!(offset);
        }
        ClipContent::Midi { sequence } => {
            v["notes"] = json!(sequence.notes.len());
        }
        ClipContent::Video { source, offset } => {
            v["video"] = json!(source);
            v["offset"] = json!(offset);
        }
    }
    if !c.gain_env.is_empty() {
        v["gain_envelope"] = json!(c.gain_env);
    }
    v
}

fn track_json(e: &Engine, t: &Track, full: bool) -> Value {
    let inserts: Vec<Value> = t
        .mixer
        .inserts
        .iter()
        .enumerate()
        .filter_map(|(i, x)| {
            x.as_ref().map(|x| json!({"slot": i, "plugin": x.plugin, "bypass": x.bypass, "params": if full { json!(x.params) } else { Value::Null }}))
        })
        .collect();
    let sends: Vec<Value> = t
        .mixer
        .sends
        .iter()
        .enumerate()
        .filter_map(|(i, x)| {
            x.as_ref().map(|x| json!({"slot": i, "target": x.target, "level_db": x.level_db, "pan": x.pan, "mute": x.mute, "pre_fader": x.pre_fader, "surround": x.surround}))
        })
        .collect();
    let mut v = json!({
        "id": t.id, "name": t.name, "kind": t.kind.id(), "format": t.format.label(),
        "volume_db": t.mixer.volume_db, "pan": t.mixer.pan, "surround": t.mixer.surround, "mute": t.mixer.mute, "solo": t.mixer.solo,
        "record_arm": t.mixer.record_arm, "input": t.mixer.input, "output": t.mixer.output,
        "automation_mode": t.mixer.automation_mode.label(), "inserts": inserts, "sends": sends,
        "clips": t.clips().iter().map(|c| clip_json(e, c)).collect::<Vec<_>>(),
        "playlist": t.playlist().map(|p| p.name.clone()), "playlists": t.playlists.len(),
        "hidden": t.hidden, "inactive": t.inactive, "view": t.view, "height": t.height.label(),
        "automation": t.automation.iter().filter(|l| !l.points.is_empty()).map(|l| json!({"param": l.param.label(), "points": if full { json!(l.points) } else { json!(l.points.len()) }})).collect::<Vec<_>>(),
    });
    if let Some(i) = &t.instrument {
        v["instrument"] = json!(i.plugin);
    }
    if !t.comments.is_empty() {
        v["comments"] = json!(t.comments);
    }
    v
}

pub fn session(e: &Engine, full: bool) -> Value {
    let s = e.session();
    json!({
        "name": s.name, "path": e.path, "dirty": e.is_dirty(), "sample_rate": s.sample_rate.hz(), "bit_depth": s.bit_depth,
        "frame_rate": s.frame_rate.label(), "tempo": s.tempo.tempos(), "meter": s.tempo.meters(), "main_format": s.main_format().label(),
        "length_samples": s.content_end(),
        "tracks": s.tracks.iter().map(|t| track_json(e, t, full)).collect::<Vec<_>>(),
        "busses": s.busses.iter().map(|b| json!({"id": b.id, "name": b.name, "format": b.format.label()})).collect::<Vec<_>>(),
        "markers": s.markers.iter().map(|m| json!({"id": m.id, "number": m.number, "name": m.name, "kind": m.kind, "start": m.start, "end": m.end})).collect::<Vec<_>>(),
        "groups": s.groups.iter().map(|g| json!({"id": g.id, "name": g.name, "letter": g.letter, "members": g.members, "active": g.active})).collect::<Vec<_>>(),
        "sources": s.sources.iter().map(|x| json!({
            "id": x.id, "name": x.name, "channels": x.channels, "frames": x.frames, "path": x.path,
            "loaded": s.pool.contains(x.id),
        })).collect::<Vec<_>>(),
        "selection": {"start": s.edit.selection.start, "end": s.edit.selection.end, "tracks": s.edit.selected_tracks, "clips": s.edit.selected_clips},
        "edit_mode": s.edit.edit_mode, "tool": s.edit.tool, "grid": s.edit.grid.label(), "nudge": s.edit.nudge.label(),
        "zoom": {"samples_per_px": s.edit.zoom.samples_per_px, "scroll": s.edit.zoom.scroll},
        "transport": e.transport,
        "undo": e.undo_label(), "redo": e.redo_label(),
    })
}

pub fn track(e: &Engine, t: TrackId) -> Option<Value> {
    e.session().track(t).map(|t| track_json(e, t, true))
}

pub fn clip(e: &Engine, id: ClipId) -> Option<Value> {
    e.session().find_clip(id).map(|(t, c)| {
        let mut v = clip_json(e, c);
        v["track"] = json!(t);
        if let ClipContent::Midi { sequence } = &c.content {
            v["note_list"] = json!(sequence.notes);
        }
        v
    })
}

/// Plain-text session report (File › Export › Session Info as Text).
pub fn session_text(e: &Engine) -> String {
    let s = e.session();
    let fmt = |x| format_position(x, TimeFormat::MinSecs, s.sample_rate, &s.tempo, s.frame_rate, s.timecode_start);
    let mut out = String::new();
    out.push_str(&format!("SESSION NAME:\t{}\nSAMPLE RATE:\t{}\nBIT DEPTH:\t{:?}\nTIMECODE FORMAT:\t{}\n# OF AUDIO TRACKS:\t{}\n# OF AUDIO CLIPS:\t{}\n# OF AUDIO FILES:\t{}\n\n",
        s.name, s.sample_rate.hz(), s.bit_depth, s.frame_rate.label(),
        s.tracks.iter().filter(|t| t.kind == soundcraft_model::TrackKind::Audio).count(),
        s.tracks.iter().map(|t| t.clips().len()).sum::<usize>(), s.sources.len()));
    out.push_str("F I L E S  I N  S E S S I O N\nFilename\tLocation\n");
    for src in &s.sources {
        out.push_str(&format!("{}\t{}\n", src.name, src.path));
    }
    out.push_str("\nT R A C K  L I S T I N G\n");
    for t in &s.tracks {
        out.push_str(&format!(
            "TRACK NAME:\t{}\nCOMMENTS:\t{}\nSTATE:\t{}{}{}\n",
            t.name,
            t.comments,
            if t.inactive { "Inactive " } else { "" },
            if t.mixer.mute { "Muted " } else { "" },
            if t.mixer.solo { "Solo" } else { "" }
        ));
        out.push_str("CHANNEL\tEVENT\tCLIP NAME\tSTART TIME\tEND TIME\tDURATION\tSTATE\n");
        for (i, c) in t.clips().iter().enumerate() {
            out.push_str(&format!(
                "1\t{}\t{}\t{}\t{}\t{}\t{}\n",
                i + 1,
                c.name,
                fmt(c.start),
                fmt(c.end()),
                fmt(c.length),
                if c.muted { "Muted" } else { "Unmuted" }
            ));
        }
        out.push('\n');
    }
    out.push_str("M A R K E R S  L I S T I N G\n#\tLOCATION\tNAME\n");
    for m in &s.markers {
        out.push_str(&format!("{}\t{}\t{}\n", m.number, fmt(m.start), m.name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sources_report_whether_audio_is_loaded() {
        let mut e = crate::demo::demo_engine();
        let before = session(&e, false);
        let sources = before["sources"].as_array().expect("sources");
        assert!(!sources.is_empty());
        assert!(sources.iter().all(|s| s["loaded"] == true), "{sources:?}");

        let id = e.session().sources[0].id;
        assert!(e.session_mut().pool.remove(id).is_some());
        let after = session(&e, false);
        let entry = after["sources"].as_array().unwrap().iter().find(|s| s["id"] == json!(id)).expect("source still listed");
        assert_eq!(entry["loaded"], false, "{entry}");
        assert_eq!(entry["path"], before["sources"][0]["path"], "metadata stays even when unloaded");
    }
}
