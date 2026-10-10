//! Edit menu commands.

use super::*;
use crate::edit as ops;
use crate::{Clipboard, cmd};
use serde_json::json;
use soundcraft_model::{EditMode, FadeShape, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{}", can_undo, |e, _| Ok(json!({"undone": e.undo()}))),
        cmd!(noundo "edit.redo", "Redo", ["Edit"], Some("Cmd+Shift+Z"), "{}", can_redo, |e, _| Ok(json!({"redone": e.redo()}))),
        cmd!("edit.restore_last_selection", "Restore Last Selection", ["Edit"], Some("Cmd+Alt+Z"), "{}", always, restore_selection),
        cmd!("edit.cut", "Cut", ["Edit"], Some("Cmd+X"), "{tracks?, start?, end?}", has_range, |e, p| cut_copy(e, p, true, "edit.cut")),
        cmd!(noundo "edit.copy", "Copy", ["Edit"], Some("Cmd+C"), "{tracks?, start?, end?}", has_range, |e, p| cut_copy(e, p, false, "edit.copy")),
        cmd!("edit.paste", "Paste", ["Edit"], Some("Cmd+V"), "{tracks?, at?}", has_clipboard, paste),
        cmd!("edit.clear", "Clear", ["Edit"], Some("Cmd+B"), "{tracks?, start?, end?}", has_range, clear),
        cmd!("edit.cut_clip_gain", "Clip Gain", ["Edit", "Cut Special"], Some("Cmd+Ctrl+X"), "{}", has_range, |e, p| reset_clip_gain(
            e,
            p,
            "edit.cut_clip_gain"
        )),
        cmd!("edit.clear_clip_gain", "Clip Gain", ["Edit", "Clear Special"], Some("Cmd+Ctrl+B"), "{}", has_range, |e, p| reset_clip_gain(
            e,
            p,
            "edit.clear_clip_gain"
        )),
        cmd!("edit.cut_all_automation", "All Automation", ["Edit", "Cut Special"], None, "{tracks?, start?, end?}", has_range, |e, p| {
            super::edit_more::cut_all_automation(e, p)
        }),
        cmd!("edit.clear_all_automation", "All Automation", ["Edit", "Clear Special"], None, "{}", has_range, |e, p| clear_automation(e, p, None)),
        cmd!("edit.clear_pan_automation", "Pan Automation", ["Edit", "Clear Special"], None, "{}", has_range, |e, p| clear_automation(
            e,
            p,
            Some("pan")
        )),
        cmd!("edit.clear_plugin_automation", "Plugin Automation", ["Edit", "Clear Special"], None, "{}", has_range, |e, p| clear_automation(
            e,
            p,
            Some("plugin")
        )),
        cmd!(
            "edit.paste_repeat_to_fill",
            "Repeat to Fill Selection",
            ["Edit", "Paste Special"],
            Some("Cmd+Alt+V"),
            "{}",
            has_clipboard,
            repeat_to_fill
        ),
        cmd!(noundo "edit.select_all", "Select All", ["Edit"], Some("Cmd+A"), "{}", always, select_all),
        cmd!(noundo "edit.select", "Set Edit Selection", [], None, "{tracks?: [id|name], start?, end?, clips?: [id], exact?: bool (ignore edit groups)}", always, select),
        cmd!(noundo "edit.select_none", "Deselect All", [], None, "{}", always, |e, _| {
            let s = e.session_mut();
            s.edit.selected_tracks.clear();
            s.edit.selected_clips.clear();
            s.edit.selection.end = s.edit.selection.start;
            Ok(json!({}))
        }),
        cmd!(noundo "edit.selection_to_timeline", "Change Timeline to Match Edit", ["Edit", "Selection"], Some("Alt+Shift+5"), "{}", always, |e, _| {
            let s = e.session_mut();
            s.edit.timeline_selection = s.edit.selection;
            Ok(json!({}))
        }),
        cmd!(noundo "edit.timeline_to_selection", "Change Edit to Match Timeline", ["Edit", "Selection"], Some("Alt+Shift+6"), "{}", always, |e, _| {
            let s = e.session_mut();
            s.edit.selection = s.edit.timeline_selection;
            Ok(json!({}))
        }),
        cmd!(noundo "edit.move_selection_left", "Move Edit Left", ["Edit", "Selection"], Some("Cmd+Alt+L"), "{}", always, |e, _| shift_selection(e, -1)),
        cmd!(noundo "edit.move_selection_right", "Move Edit Right", ["Edit", "Selection"], Some("Cmd+Alt+'"), "{}", always, |e, _| shift_selection(e, 1)),
        cmd!(noundo "edit.halve_selection", "Halve Edit", ["Edit", "Selection"], None, "{}", always, |e, _| scale_selection(e, 0.5)),
        cmd!(noundo "edit.double_selection", "Double Edit", ["Edit", "Selection"], None, "{}", always, |e, _| scale_selection(e, 2.0)),
        cmd!(noundo "edit.extend_selection_up", "Extend Edit Up", ["Edit", "Selection"], None, "{}", has_tracks, |e, _| extend_tracks(e, -1)),
        cmd!(noundo "edit.extend_selection_down", "Extend Edit Down", ["Edit", "Selection"], None, "{}", has_tracks, |e, _| extend_tracks(e, 1)),
        cmd!(noundo "edit.remove_selection_top", "Remove Edit from Top", ["Edit", "Selection"], None, "{}", has_tracks, |e, _| {
            let s = e.session_mut();
            if !s.edit.selected_tracks.is_empty() {
                s.edit.selected_tracks.remove(0);
            }
            Ok(json!({}))
        }),
        cmd!(noundo "edit.remove_selection_bottom", "Remove Edit from Bottom", ["Edit", "Selection"], None, "{}", has_tracks, |e, _| {
            e.session_mut().edit.selected_tracks.pop();
            Ok(json!({}))
        }),
        cmd!("edit.duplicate", "Duplicate", ["Edit"], Some("Cmd+D"), "{tracks?, start?, end?}", has_range, duplicate),
        cmd!("edit.repeat", "Repeat...", ["Edit"], Some("Alt+R"), "{count: n}", has_range, repeat),
        cmd!("edit.shift", "Shift...", ["Edit"], Some("Alt+H"), "{by: samples|{seconds}, later?: bool}", has_range, shift),
        cmd!("edit.insert_silence", "Insert Silence", ["Edit"], Some("Cmd+Shift+E"), "{tracks?, start?, end?}", has_range, insert_silence),
        cmd!("edit.separate", "At Selection", ["Edit", "Separate"], Some("Cmd+E"), "{tracks?, at?|start/end}", has_selection, separate),
        cmd!("edit.separate_on_grid", "On Grid", ["Edit", "Separate"], None, "{tracks?, start?, end?}", has_range, separate_on_grid),
        cmd!("edit.separate_at_transients", "At Transients", ["Edit", "Separate"], None, "{sensitivity?: 0..1}", has_range, separate_at_transients),
        cmd!("edit.heal", "Heal Separation", ["Edit"], Some("Cmd+H"), "{tracks?, start?, end?}", has_selection, heal),
        cmd!("edit.trim_to_selection", "To Selection", ["Edit", "Trim"], Some("Cmd+T"), "{tracks?, start?, end?}", has_range, trim_to_selection),
        cmd!("edit.trim_start_to_insertion", "Start to Insertion", ["Edit", "Trim"], Some("Alt+Shift+7"), "{tracks?, at?}", has_selection, |e, p| {
            trim_edge(e, p, true)
        }),
        cmd!("edit.trim_end_to_insertion", "End to Insertion", ["Edit", "Trim"], Some("Alt+Shift+8"), "{tracks?, at?}", has_selection, |e, p| {
            trim_edge(e, p, false)
        }),
        cmd!("edit.trim_to_file_boundaries", "To File Boundaries", ["Edit", "Trim"], None, "{clips?}", has_selection, trim_to_file),
        cmd!("edit.trim_to_fill_selection", "To Fill Selection", ["Edit", "Trim"], None, "{}", has_range, trim_to_fill),
        cmd!("edit.mute_clips", "Mute", ["Edit"], Some("Cmd+M"), "{clips?}", has_selection, mute_clips),
        cmd!("edit.copy_to_new_playlist", "New Playlist", ["Edit", "Copy Selection to..."], None, "{}", has_range, copy_to_new_playlist),
        cmd!(
            "edit.strip_silence",
            "Strip Silence",
            ["Edit"],
            Some("Cmd+U"),
            "{threshold_db?: -48, min_length_ms?: 50, pad_before_ms?: 5, pad_after_ms?: 20}",
            has_range,
            strip_silence
        ),
        cmd!(
            "edit.fades_create",
            "Create...",
            ["Edit", "Fades"],
            Some("Cmd+F"),
            "{shape?: linear|equal power|s-curve|exponential|logarithmic, tracks?, start?, end?}",
            has_range,
            fades_create
        ),
        cmd!("edit.fades_delete", "Delete", ["Edit", "Fades"], None, "{tracks?, start?, end?}", has_selection, fades_delete),
        cmd!("edit.fade_to_start", "Fade to Start", ["Edit", "Fades"], Some("Cmd+Ctrl+D"), "{}", has_selection, |e, p| fade_to(e, p, true)),
        cmd!("edit.fade_to_end", "Fade to End", ["Edit", "Fades"], Some("Cmd+Ctrl+G"), "{}", has_selection, |e, p| fade_to(e, p, false)),
        cmd!("edit.nudge", "Nudge", [], Some("Plus/Minus"), "{direction: 1|-1, count?: n}", has_selection, nudge),
        cmd!(noundo "edit.mode", "Edit Mode", [], Some("F1-F4"), "{mode: shuffle|slip|spot|grid|relative_grid}", always, |e, p| {
            let m = str_param(p, "mode").and_then(EditMode::from_id).ok_or_else(|| bad("edit.mode", "mode must be shuffle|slip|spot|grid|relative_grid"))?;
            e.session_mut().edit.edit_mode = m;
            Ok(json!({"mode": m}))
        }),
        cmd!(noundo "edit.tool", "Edit Tool", [], Some("F5-F10"), "{tool: zoom|trim|selector|grabber|scrubber|pencil|smart}", always, |e, p| {
            let t = str_param(p, "tool").and_then(soundcraft_model::Tool::from_id).ok_or_else(|| bad("edit.tool", "unknown tool"))?;
            e.session_mut().edit.tool = t;
            Ok(json!({"tool": t}))
        }),
        cmd!(
            "edit.move_clips",
            "Move Clips",
            [],
            None,
            "{clips?: [id], by?: samples, to?: position (first clip start), track?: target}",
            has_selection,
            move_clips
        ),
        cmd!("edit.consolidate", "Consolidate", ["Edit"], Some("Alt+Shift+3"), "{tracks?, start?, end?}", has_range, consolidate),
    ]
}

fn shuffle(s: &Session) -> bool {
    s.edit.edit_mode == EditMode::Shuffle
}

fn restore_selection(e: &mut Engine, _: &Value) -> Result<Value> {
    Ok(json!({"selection": e.session().edit.selection}))
}

fn cut_copy(e: &mut Engine, p: &Value, cut: bool, id: &str) -> Result<Value> {
    let tracks = tracks_required(e, id, p)?;
    let range = range_param(e, id, p)?;
    if range.is_empty() {
        return Err(bad(id, "selection is empty"));
    }
    let mut cb = Clipboard { length: range.len(), ..Clipboard::default() };
    let follow = e.session().edit.automation_follows_edit;
    for t in &tracks {
        cb.tracks.push(ops::copy_range(e.session(), *t, range));
        let lanes = e
            .session()
            .track(*t)
            .map(|tr| {
                tr.automation
                    .iter()
                    .map(|l| {
                        let mut l2 = l.clone();
                        l2.points = l
                            .points
                            .iter()
                            .filter(|p| range.contains(p.at))
                            .map(|p| soundcraft_model::AutomationPoint { at: p.at - range.start, value: p.value })
                            .collect();
                        l2
                    })
                    .collect()
            })
            .unwrap_or_default();
        cb.automation.push(lanes);
    }
    cb.markers = e
        .session()
        .markers
        .iter()
        .filter(|m| m.kind == soundcraft_model::MarkerKind::Marker && range.contains(m.start))
        .map(|m| {
            let mut m = m.clone();
            m.end -= m.start;
            m.start -= range.start;
            m.end += m.start;
            m
        })
        .collect();
    if cut {
        let sh = shuffle(e.session());
        let s = e.session_mut();
        for t in &tracks {
            ops::clear_range(s, *t, range, sh);
            if follow && let Some(tr) = s.track_mut(*t) {
                for l in &mut tr.automation {
                    l.clear_range(range.start, range.end);
                    if sh {
                        l.shift_from(range.end, -range.len());
                    }
                }
            }
        }
        s.edit.selection = soundcraft_time::Range::point(range.start);
    }
    let n: usize = cb.tracks.iter().map(Vec::len).sum();
    e.clipboard = cb;
    Ok(json!({"clips": n, "length": range.len()}))
}

fn paste(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.paste", p)?;
    let at = position_param(e, "edit.paste", p, "at")?.unwrap_or(e.session().edit.selection.start);
    let cb = e.clipboard.clone();
    let sh = shuffle(e.session());
    let follow = e.session().edit.automation_follows_edit;
    let s = e.session_mut();
    let mut pasted = 0;
    for (i, t) in tracks.iter().enumerate() {
        let Some(clips) = cb.tracks.get(i % cb.tracks.len().max(1)) else { continue };
        pasted += ops::paste(s, *t, at, clips, cb.length, sh).len();
        if follow && let (Some(lanes), Some(tr)) = (cb.automation.get(i % cb.automation.len().max(1)), s.track_mut(*t)) {
            for l in lanes {
                let dst = tr.lane_mut(&l.param);
                if !l.points.is_empty() {
                    dst.clear_range(at, at + cb.length);
                }
                for pt in &l.points {
                    dst.set_point(pt.at + at, pt.value);
                }
            }
        }
    }
    s.edit.selection = soundcraft_time::Range::new(at, at + cb.length);
    Ok(json!({"pasted": pasted}))
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.clear", p)?;
    let range = range_param(e, "edit.clear", p)?;
    let ids = clip_ids_param(e, p);
    let sh = shuffle(e.session());
    let s = e.session_mut();
    let mut n = 0;
    if range.is_empty() && !ids.is_empty() {
        for t in &mut s.tracks {
            if let Some(pl) = t.playlist_mut() {
                let before = pl.clips.len();
                pl.clips.retain(|c| !ids.contains(&c.id));
                n += before - pl.clips.len();
            }
        }
        s.edit.selected_clips.clear();
    } else {
        for t in &tracks {
            n += ops::clear_range(s, *t, range, sh).len();
        }
    }
    Ok(json!({"removed": n}))
}

fn reset_clip_gain(e: &mut Engine, p: &Value, _id: &str) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.gain_db = 0.0;
            c.gain_env.clear();
        }
    }
    Ok(json!({"clips": ids.len()}))
}

fn clear_automation(e: &mut Engine, p: &Value, which: Option<&str>) -> Result<Value> {
    let tracks = tracks_required(e, "edit.clear_automation", p)?;
    let r = range_param(e, "edit.clear_automation", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                let hit = match which {
                    None => true,
                    Some("pan") => matches!(l.param, soundcraft_model::AutoParam::Pan(_)),
                    Some(_) => matches!(l.param, soundcraft_model::AutoParam::Plugin { .. }),
                };
                if hit {
                    n += l.clear_range(r.start, r.end);
                }
            }
        }
    }
    Ok(json!({"points_removed": n}))
}

fn repeat_to_fill(e: &mut Engine, p: &Value) -> Result<Value> {
    let range = range_param(e, "edit.paste_repeat_to_fill", p)?;
    let len = e.clipboard.length.max(1);
    let tracks = tracks_required(e, "edit.paste_repeat_to_fill", p)?;
    let cb = e.clipboard.clone();
    let s = e.session_mut();
    let mut at = range.start;
    let mut n = 0;
    while at < range.end && n < 10_000 {
        for (i, t) in tracks.iter().enumerate() {
            if let Some(clips) = cb.tracks.get(i % cb.tracks.len().max(1)) {
                let trimmed: Vec<_> = clips
                    .iter()
                    .filter(|c| at + c.start < range.end)
                    .map(|c| {
                        let mut c = c.clone();
                        if at + c.end() > range.end {
                            c.trim_end_to(range.end - at);
                        }
                        c
                    })
                    .collect();
                ops::paste(s, *t, at, &trimmed, (range.end - at).min(len), false);
            }
        }
        at += len;
        n += 1;
    }
    Ok(json!({"repeats": n}))
}

fn select_all(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session_mut();
    s.edit.selected_tracks = s.tracks.iter().filter(|t| !t.hidden).map(|t| t.id).collect();
    let end = s.content_end();
    s.edit.selection = soundcraft_time::Range::new(0, end);
    Ok(json!({"tracks": s.edit.selected_tracks.len(), "end": end}))
}

fn select(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = if p.get("tracks").is_some() || p.get("track").is_some() { Some(tracks_param(e, "edit.select", p)?) } else { None };
    let st = position_param(e, "edit.select", p, "start")?;
    let en = position_param(e, "edit.select", p, "end")?;
    let clips =
        p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(soundcraft_model::ClipId).collect::<Vec<_>>());
    let exact = bool_or(p, "exact", false);
    let s = e.session_mut();
    if let Some(mut t) = tracks {
        // Active edit groups select all their members together.
        if !exact {
            let extra: Vec<soundcraft_model::TrackId> =
                s.groups.iter().filter(|g| g.active && g.edit && g.members.iter().any(|m| t.contains(m))).flat_map(|g| g.members.clone()).collect();
            for m in extra {
                if !t.contains(&m) {
                    t.push(m);
                }
            }
            let order: Vec<soundcraft_model::TrackId> = s.tracks.iter().map(|x| x.id).collect();
            t.sort_by_key(|x| order.iter().position(|o| o == x).unwrap_or(usize::MAX));
        }
        s.edit.selected_tracks = t;
    }
    match (st, en) {
        (Some(a), Some(b)) => s.edit.selection = soundcraft_time::Range::new(a.max(0), b.max(0)),
        (Some(a), None) => s.edit.selection = soundcraft_time::Range::point(a.max(0)),
        _ => {}
    }
    if let Some(c) = clips {
        // Selecting clips also selects their range and tracks (Grabber click).
        let mut range: Option<soundcraft_time::Range> = None;
        let mut trs = Vec::new();
        for id in &c {
            if let Some((tid, clip)) = s.find_clip(*id) {
                range = Some(range.map_or(clip.range(), |r| soundcraft_time::Range::new(r.start.min(clip.start), r.end.max(clip.end()))));
                if !trs.contains(&tid) {
                    trs.push(tid);
                }
            }
        }
        if let Some(r) = range {
            s.edit.selection = r;
            s.edit.selected_tracks = trs;
        }
        s.edit.selected_clips = c;
    } else if st.is_some() {
        s.edit.selected_clips.clear();
    }
    if s.edit.link_timeline_edit {
        s.edit.timeline_selection = s.edit.selection;
    }
    Ok(json!({"selection": s.edit.selection, "tracks": s.edit.selected_tracks, "clips": s.edit.selected_clips}))
}

fn shift_selection(e: &mut Engine, dir: i64) -> Result<Value> {
    let s = e.session_mut();
    let len = s.edit.selection.len().max(1);
    s.edit.selection = s.edit.selection.shifted(dir * len);
    if s.edit.selection.start < 0 {
        s.edit.selection = s.edit.selection.shifted(-s.edit.selection.start);
    }
    Ok(json!({"selection": s.edit.selection}))
}

fn scale_selection(e: &mut Engine, f: f64) -> Result<Value> {
    let s = e.session_mut();
    let len = soundcraft_time::to_samples(s.edit.selection.len() as f64 * f);
    s.edit.selection.end = s.edit.selection.start.saturating_add(len);
    Ok(json!({"selection": s.edit.selection}))
}

fn extend_tracks(e: &mut Engine, dir: i64) -> Result<Value> {
    let s = e.session_mut();
    let idxs: Vec<usize> = s.edit.selected_tracks.iter().filter_map(|t| s.track_index(*t)).collect();
    let next = if dir < 0 { idxs.iter().min().and_then(|i| i.checked_sub(1)) } else { idxs.iter().max().map(|i| i + 1) };
    if let Some(t) = next.and_then(|i| s.tracks.get(i)).map(|t| t.id) {
        if dir < 0 {
            s.edit.selected_tracks.insert(0, t);
        } else {
            s.edit.selected_tracks.push(t);
        }
    } else if idxs.is_empty()
        && let Some(t) = s.tracks.first()
    {
        s.edit.selected_tracks.push(t.id);
    }
    Ok(json!({"tracks": s.edit.selected_tracks}))
}

fn duplicate(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.duplicate", p)?;
    let range = range_param(e, "edit.duplicate", p)?;
    if range.is_empty() {
        return Err(bad("edit.duplicate", "selection is empty"));
    }
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let clips = ops::copy_range(s, *t, range);
        n += ops::paste(s, *t, range.end, &clips, range.len(), false).len();
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                let pts: Vec<_> = l.points.iter().filter(|p| range.contains(p.at)).copied().collect();
                for pt in pts {
                    l.set_point(pt.at + range.len(), pt.value);
                }
            }
        }
    }
    s.edit.selection = range.shifted(range.len());
    Ok(json!({"clips": n}))
}

fn repeat(e: &mut Engine, p: &Value) -> Result<Value> {
    let count = i64_or(p, "count", 1).clamp(1, 999);
    let tracks = tracks_required(e, "edit.repeat", p)?;
    let range = range_param(e, "edit.repeat", p)?;
    if range.is_empty() {
        return Err(bad("edit.repeat", "selection is empty"));
    }
    let s = e.session_mut();
    for t in &tracks {
        let clips = ops::copy_range(s, *t, range);
        for k in 1..=count {
            ops::paste(s, *t, range.start + range.len() * k, &clips, range.len(), false);
        }
    }
    Ok(json!({"repeats": count}))
}

fn shift(e: &mut Engine, p: &Value) -> Result<Value> {
    let by = position_param(e, "edit.shift", p, "by")?.ok_or_else(|| bad("edit.shift", "`by` is required"))?;
    let later = bool_or(p, "later", true);
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let n = ops::move_clips(s, &ids, if later { by } else { -by }, None);
    Ok(json!({"moved": n}))
}

fn insert_silence(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.insert_silence", p)?;
    let range = range_param(e, "edit.insert_silence", p)?;
    let s = e.session_mut();
    for t in &tracks {
        ops::insert_silence(s, *t, range);
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                l.shift_from(range.start, range.len());
            }
        }
    }
    Ok(json!({"inserted": range.len()}))
}

fn separate(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.separate", p)?;
    let points: Vec<i64> = if let Some(at) = position_param(e, "edit.separate", p, "at")? {
        vec![at]
    } else {
        let r = range_param(e, "edit.separate", p)?;
        if r.is_empty() { vec![r.start] } else { vec![r.start, r.end] }
    };
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        for at in &points {
            n += ops::separate_at(s, *t, *at);
        }
    }
    Ok(json!({"separated": n}))
}

fn separate_on_grid(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.separate_on_grid", p)?;
    let r = range_param(e, "edit.separate_on_grid", p)?;
    let s = e.session_mut();
    let lines = s.edit.grid.lines(r.start, r.end, s.sample_rate, &s.tempo, s.frame_rate, 10_000);
    let mut n = 0;
    for t in &tracks {
        for at in &lines {
            n += ops::separate_at(s, *t, *at);
        }
    }
    Ok(json!({"separated": n}))
}

fn separate_at_transients(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.separate_at_transients", p)?;
    let r = range_param(e, "edit.separate_at_transients", p)?;
    let sens = f32_or(p, "sensitivity", 0.5).clamp(0.0, 1.0);
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let hits = crate::io::transients_in(s, *t, r, sens);
        for at in hits {
            n += ops::separate_at(s, *t, at);
        }
    }
    Ok(json!({"separated": n}))
}

fn heal(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.heal", p)?;
    let mut r = range_param(e, "edit.heal", p)?;
    if r.is_empty() {
        r = soundcraft_time::Range::new(r.start - 1, r.start + 1);
    }
    let s = e.session_mut();
    let n: usize = tracks.iter().map(|t| ops::heal(s, *t, r)).sum();
    Ok(json!({"healed": n}))
}

fn trim_to_selection(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.trim_to_selection", p)?;
    let r = range_param(e, "edit.trim_to_selection", p)?;
    let s = e.session_mut();
    let n: usize = tracks.iter().map(|t| ops::trim_to(s, *t, r)).sum();
    Ok(json!({"trimmed": n}))
}

fn trim_edge(e: &mut Engine, p: &Value, start: bool) -> Result<Value> {
    let tracks = tracks_required(e, "edit.trim", p)?;
    let at = position_param(e, "edit.trim", p, "at")?.unwrap_or(e.session().edit.selection.start);
    let s = e.session_mut();
    let n: usize = tracks.iter().map(|t| if start { ops::trim_start_to(s, *t, at) } else { ops::trim_end_to(s, *t, at) }).sum();
    Ok(json!({"trimmed": n}))
}

fn trim_to_file(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        let frames =
            s.find_clip(id).and_then(|(_, c)| c.source()).and_then(|src| s.source(src)).map(|src| i64::try_from(src.frames).unwrap_or(i64::MAX));
        if let (Some(frames), Some(c)) = (frames, s.find_clip_mut(id)) {
            let off = c.source_offset();
            let new_start = c.start - off;
            c.trim_start_to(new_start);
            c.trim_end_to(new_start + frames);
            n += 1;
        }
    }
    Ok(json!({"trimmed": n}))
}

fn trim_to_fill(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = range_param(e, "edit.trim_to_fill_selection", p)?;
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        let frames =
            s.find_clip(id).and_then(|(_, c)| c.source()).and_then(|src| s.source(src)).map(|src| i64::try_from(src.frames).unwrap_or(i64::MAX));
        if let (Some(frames), Some(c)) = (frames, s.find_clip_mut(id)) {
            let file_start = c.start - c.source_offset();
            c.trim_start_to(r.start.max(file_start));
            c.trim_end_to(r.end.min(file_start + frames));
            n += 1;
        }
    }
    Ok(json!({"trimmed": n}))
}

fn mute_clips(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let any_unmuted = ids.iter().any(|id| s.find_clip(*id).is_some_and(|(_, c)| !c.muted));
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.muted = any_unmuted;
        }
    }
    Ok(json!({"muted": any_unmuted, "clips": ids.len()}))
}

fn copy_to_new_playlist(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.copy_to_new_playlist", p)?;
    let r = range_param(e, "edit.copy_to_new_playlist", p)?;
    let s = e.session_mut();
    for t in &tracks {
        let clips = ops::copy_range(s, *t, r);
        let mut fresh = Vec::new();
        for mut c in clips {
            c.id = s.new_clip_id();
            c.start += r.start;
            fresh.push(c);
        }
        if let Some(tr) = s.track_mut(*t) {
            let name = format!("{}.{:02}", tr.name, tr.playlists.len() + 1);
            tr.playlists.push(soundcraft_model::Playlist { name, clips: fresh });
            tr.active_playlist = tr.playlists.len() - 1;
        }
    }
    Ok(json!({"tracks": tracks.len()}))
}

fn strip_silence(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.strip_silence", p)?;
    let r = range_param(e, "edit.strip_silence", p)?;
    let thr = f32_or(p, "threshold_db", -48.0).clamp(-96.0, 0.0);
    let sr = e.session().sample_rate;
    let ms = |k: &str, d: f64| sr.samples(f64_or(p, k, d).clamp(0.0, 10_000.0) / 1000.0);
    let (min_len, pre, post) = (ms("min_length_ms", 50.0), ms("pad_before_ms", 5.0), ms("pad_after_ms", 20.0));
    let s = e.session_mut();
    let mut kept = 0;
    for t in &tracks {
        kept += crate::io::strip_silence(s, *t, r, thr, min_len, pre, post);
    }
    Ok(json!({"clips": kept}))
}

fn fades_create(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.fades_create", p)?;
    let r = range_param(e, "edit.fades_create", p)?;
    let shape = str_param(p, "shape")
        .map_or(Some(FadeShape::EqualPower), FadeShape::from_id)
        .ok_or_else(|| bad("edit.fades_create", "unknown fade shape"))?;
    let s = e.session_mut();
    let n: usize = tracks.iter().map(|t| ops::create_fades(s, *t, r, shape)).sum();
    if n == 0 {
        return Err(bad("edit.fades_create", "the selection must cover a clip start, end or the boundary between two clips"));
    }
    Ok(json!({"fades": n}))
}

fn fades_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.fades_delete", p)?;
    let r = range_param(e, "edit.fades_delete", p)?;
    let s = e.session_mut();
    let n: usize = tracks.iter().map(|t| ops::delete_fades(s, *t, r)).sum();
    Ok(json!({"deleted": n}))
}

fn fade_to(e: &mut Engine, p: &Value, to_start: bool) -> Result<Value> {
    let tracks = tracks_required(e, "edit.fade_to", p)?;
    let at = e.session().edit.selection.start;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let range = s.track(*t).and_then(|tr| {
            tr.clips()
                .iter()
                .find(|c| c.range().contains(at))
                .map(|c| if to_start { soundcraft_time::Range::new(c.start, at) } else { soundcraft_time::Range::new(at, c.end()) })
        });
        if let Some(r) = range {
            n += ops::create_fades(s, *t, r, FadeShape::EqualPower);
        }
    }
    Ok(json!({"fades": n}))
}

fn nudge(e: &mut Engine, p: &Value) -> Result<Value> {
    let dir = i64_or(p, "direction", 1).signum();
    let count = i64_or(p, "count", 1).clamp(1, 1000);
    let s = e.session();
    let step = s.edit.nudge.step_samples(s.edit.selection.start, s.sample_rate, &s.tempo, s.frame_rate) * count * if dir == 0 { 1 } else { dir };
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let n = if ids.is_empty() { 0 } else { ops::move_clips(s, &ids, step, None) };
    s.edit.selection = s.edit.selection.shifted(step);
    if s.edit.selection.start < 0 {
        s.edit.selection = s.edit.selection.shifted(-s.edit.selection.start);
    }
    Ok(json!({"moved": n, "by": step}))
}

fn move_clips(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("edit.move_clips", "no clips"));
    }
    let to_track = track_param(e, "edit.move_clips", p, "track")?;
    let first = ids.iter().filter_map(|id| e.session().find_clip(*id).map(|(_, c)| c.start)).min().unwrap_or(0);
    let delta = match (position_param(e, "edit.move_clips", p, "by")?, position_param(e, "edit.move_clips", p, "to")?) {
        (Some(by), _) => by,
        (None, Some(to)) => to - first,
        _ => 0,
    };
    // Grid mode snaps the destination.
    let s = e.session();
    let delta = if s.edit.edit_mode.is_grid() && p.get("to").is_some() {
        s.edit.grid.snap(first + delta, s.sample_rate, &s.tempo, s.frame_rate) - first
    } else {
        delta
    };
    let s = e.session_mut();
    let n = ops::move_clips(s, &ids, delta, to_track);
    s.edit.selected_clips = ids;
    Ok(json!({"moved": n, "by": delta}))
}

fn consolidate(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.consolidate", p)?;
    let r = range_param(e, "edit.consolidate", p)?;
    if r.is_empty() {
        return Err(bad("edit.consolidate", "selection is empty"));
    }
    let mut n = 0;
    for t in tracks {
        if crate::io::consolidate(e, t, r)? {
            n += 1;
        }
    }
    Ok(json!({"consolidated": n}))
}
