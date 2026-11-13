//! Crossfade commands: slide a crossfade along its cut, or trim the edges of its zone.
use super::edit::{is_selected_crossfade, leave_selection_at};
use super::{CommandSpec, always, bad, clip_id, position_param};
use crate::edit as ops;
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "edit.move_crossfade",
            "Move Crossfade",
            [],
            None,
            "{clip: id of the later clip of the crossfade, by: samples} — slides the joint between the two clips, and the cut with it; the clips keep their audio in place, and the slide is held to the file each clip has; refused when a clip is edit-locked or stretched",
            always,
            move_crossfade
        ),
        cmd!(
            "edit.trim_crossfade",
            "Trim Crossfade",
            [],
            None,
            "{clip: id of the later clip of the crossfade, start?: samples, end?: samples} — moves the left edge (the later clip's start) and/or the right edge (the earlier clip's end) of the crossfade zone; the cut stays where it is and no edge crosses it, each edge is held to the file its clip has and each clip keeps a sample of its own; both fades become the new overlap, and an overlap of 0 leaves the clips butted at the cut; refused when a clip is edit-locked or stretched",
            always,
            trim_crossfade
        ),
    ]
}

fn move_crossfade(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "edit.move_crossfade")?;
    let by = position_param(e, "edit.move_crossfade", p, "by")?.ok_or_else(|| bad("edit.move_crossfade", "`by` required"))?;
    let by = ops::crossfade_slide(e.session(), id, by).ok_or_else(|| {
        bad("edit.move_crossfade", "needs the later clip of a crossfade between two audio clips that are neither edit-locked nor stretched")
    })?;
    // A slide that goes nowhere leaves the document, and the undo history, alone.
    if by != 0 {
        let selected = is_selected_crossfade(e, id);
        let cut = ops::crossfade_zone(e.session(), id).map_or(0, |(_, cut)| cut);
        ops::move_crossfade(e.session_mut(), id, by).ok_or_else(|| bad("edit.move_crossfade", "the crossfade can't move"))?;
        follow_selection(e, id, selected, cut.saturating_add(by));
    }
    Ok(json!({"by": by}))
}

fn trim_crossfade(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "edit.trim_crossfade")?;
    let start = position_param(e, "edit.trim_crossfade", p, "start")?;
    let end = position_param(e, "edit.trim_crossfade", p, "end")?;
    if start.is_none() && end.is_none() {
        return Err(bad("edit.trim_crossfade", "pass `start` and/or `end`"));
    }
    let refused =
        || bad("edit.trim_crossfade", "needs the later clip of a crossfade between two audio clips that are neither edit-locked nor stretched");
    let (zone, cut) = ops::crossfade_zone(e.session(), id).ok_or_else(refused)?;
    let (left, right) = ops::crossfade_trim(e.session(), id, start, end).ok_or_else(refused)?;
    // An edge that goes nowhere leaves the document, and the undo history, alone.
    if (zone.start, zone.end) != (left, right) {
        let selected = is_selected_crossfade(e, id);
        ops::trim_crossfade(e.session_mut(), id, start, end).ok_or_else(refused)?;
        follow_selection(e, id, selected, cut);
    }
    Ok(json!({"start": left, "end": right, "cut": cut}))
}

/// A crossfade that is the edit selection stays so when it slides or is trimmed: the selection takes its new
/// zone (else a plain range would be left on the old one, for Delete to clear). A trim that leaves the clips
/// butted ends the crossfade, and the selection with it: an insertion point at its `cut`.
fn follow_selection(e: &mut Engine, later: soundcraft_model::ClipId, was_selected: bool, cut: soundcraft_time::Samples) {
    if !was_selected {
        return;
    }
    let zone = ops::crossfade_zone(e.session(), later).map(|(zone, _)| zone);
    let s = e.session_mut();
    match zone {
        Some(zone) => {
            s.edit.selection = zone;
            if s.edit.link_timeline_edit {
                s.edit.timeline_selection = zone;
            }
        }
        None => leave_selection_at(s, cut),
    }
}
