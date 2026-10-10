//! More Edit menu commands: Cut/Copy/Clear Special variants, Paste Special, selection helpers,
//! Space Clips, Snap to, Copy/Move Selection to playlists, TCE, and automation coalesce/trim/glide.

use super::more_util::{clip_db, render_stretched, replace_clip, set_clip_env, target_playlist};
use super::*;
use crate::edit as ops;
use crate::{Clipboard, TransportRequest, cmd};
use serde_json::json;
use soundcraft_model::{AutoParam, AutomationLane, Clip, ClipContent, ClipId, EditMode, Playlist, Session};
use soundcraft_time::{Range, Samples};

#[derive(Clone, Copy, PartialEq)]
enum Lanes {
    All,
    Pan,
    Plugin,
}

impl Lanes {
    fn hit(self, p: &AutoParam) -> bool {
        match self {
            Lanes::All => true,
            Lanes::Pan => matches!(p, AutoParam::Pan(_)),
            Lanes::Plugin => matches!(p, AutoParam::Plugin { .. }),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dest {
    Target,
    New,
    Duplicate,
}

fn has_clip_gain_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.clip_gain.is_empty() { Err("no clip gain has been copied".into()) } else { Ok(()) }
}

fn has_clip_effects_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.clip_effects.is_empty() { Err("no clip effects have been copied".into()) } else { Ok(()) }
}

fn has_automation_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.automation.iter().flatten().any(|l| !l.points.is_empty()) { Ok(()) } else { Err("no automation has been copied".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "edit.cut_pan_automation",
            "Pan Automation",
            ["Edit", "Cut Special"],
            None,
            "{tracks?, start?, end?} — cuts pan automation only to the clipboard",
            has_range,
            |e, p| auto_copy(e, p, Lanes::Pan, true, "edit.cut_pan_automation")
        ),
        cmd!(
            "edit.cut_plugin_automation",
            "Plugin Automation",
            ["Edit", "Cut Special"],
            None,
            "{tracks?, start?, end?} — cuts plug-in automation only",
            has_range,
            |e, p| auto_copy(e, p, Lanes::Plugin, true, "edit.cut_plugin_automation")
        ),
        cmd!(noundo "edit.copy_all_automation", "All Automation", ["Edit", "Copy Special"], None, "{tracks?, start?, end?} — copies automation only (no clips)", has_range, |e, p| auto_copy(e, p, Lanes::All, false, "edit.copy_all_automation")),
        cmd!(noundo "edit.copy_pan_automation", "Pan Automation", ["Edit", "Copy Special"], None, "{tracks?, start?, end?}", has_range, |e, p| auto_copy(e, p, Lanes::Pan, false, "edit.copy_pan_automation")),
        cmd!(noundo "edit.copy_plugin_automation", "Plugin Automation", ["Edit", "Copy Special"], None, "{tracks?, start?, end?}", has_range, |e, p| auto_copy(e, p, Lanes::Plugin, false, "edit.copy_plugin_automation")),
        cmd!(noundo "edit.copy_clip_gain", "Clip Gain", ["Edit", "Copy Special"], None, "{tracks?, start?, end?} — copies the clip gain curve under the selection", has_range, copy_clip_gain),
        cmd!(
            "edit.paste_clip_gain",
            "Paste Clip Gain",
            [],
            None,
            "{tracks?, at?} — pastes copied clip gain onto the clips under the paste range",
            has_clip_gain_clipboard,
            paste_clip_gain
        ),
        cmd!(noundo "edit.copy_clip_effects", "Clip Effects", ["Edit", "Copy Special"], None, "{clips?} — copies the first clip's clip-effect settings", has_selection, |e, p| clip_effects(e, p, true, false, "edit.copy_clip_effects")),
        cmd!(
            "edit.cut_clip_effects",
            "Clip Effects",
            ["Edit", "Cut Special"],
            None,
            "{clips?} — copies then removes clip-effect settings",
            has_selection,
            |e, p| clip_effects(e, p, true, true, "edit.cut_clip_effects")
        ),
        cmd!(
            "edit.clear_clip_effects",
            "Clip Effects",
            ["Edit", "Clear Special"],
            None,
            "{clips?} — removes clip-effect settings (and bypass)",
            has_selection,
            |e, p| clip_effects(e, p, false, true, "edit.clear_clip_effects")
        ),
        cmd!("edit.paste_clip_effects", "Paste Clip Effects", [], None, "{clips?}", has_clip_effects_clipboard, paste_clip_effects),
        cmd!(
            "edit.paste_merge_midi",
            "Merge MIDI",
            ["Edit", "Paste Special"],
            None,
            "{tracks?, at?} — merges copied MIDI notes into existing MIDI clips instead of replacing them",
            has_clipboard,
            paste_merge_midi
        ),
        cmd!(
            "edit.paste_merge_markers",
            "Merge Markers",
            ["Edit", "Paste Special"],
            None,
            "{at?} — adds the copied markers at the insertion point, skipping any that already exist there",
            has_marker_clipboard,
            paste_merge_markers
        ),
        cmd!(
            "edit.paste_to_current_automation",
            "To Current Automation Type",
            ["Edit", "Paste Special"],
            None,
            "{tracks?, at?, param?} — pastes copied automation into the shown lane (track view) or `param`, rescaled to its range",
            has_automation_clipboard,
            paste_to_current_automation
        ),
        cmd!(noundo "edit.play_timeline", "Play Timeline", ["Edit", "Selection"], None, "{} — plays the timeline selection", always, |e, _| {
            let s = e.session();
            let r = if s.edit.timeline_selection.is_empty() { s.edit.selection } else { s.edit.timeline_selection };
            e.transport_requests.push(TransportRequest::Locate(r.start));
            e.transport_requests.push(TransportRequest::Play);
            Ok(json!({"start": r.start, "end": r.end}))
        }),
        cmd!(
            "edit.duplicate_extend",
            "Duplicate and Extend Edit",
            ["Edit", "Selection"],
            None,
            "{tracks?, start?, end?} — duplicates the selection and extends the selection over the copy",
            has_range,
            duplicate_extend
        ),
        cmd!(noundo "edit.extend_up_to_members", "Extend Edit Up to Members", ["Edit", "Selection"], None, "{} — adds the parent folder of each selected member and its members", has_selection, |e, _| extend_members(e, true)),
        cmd!(noundo "edit.extend_down_to_members", "Extend Edit Down to Members", ["Edit", "Selection"], None, "{} — adds the members of selected folder tracks (recursively)", has_selection, |e, _| extend_members(e, false)),
        cmd!(noundo "edit.marker_lane_move_up", "Move Edit Selection To Marker Lane Above", ["Edit", "Selection"], None, "{} — the selection's marker ruler lane (1..5) moves up", always, |e, _| marker_lane(e, -1, false)),
        cmd!(noundo "edit.marker_lane_move_down", "Move Edit Selection To Marker Lane Below", ["Edit", "Selection"], None, "{}", always, |e, _| marker_lane(e, 1, false)),
        cmd!(noundo "edit.marker_lane_extend_up", "Extend Edit Selection To Marker Lane Above", ["Edit", "Selection"], None, "{} — adds the marker ruler lane above to the selection", always, |e, _| marker_lane(e, -1, true)),
        cmd!(noundo "edit.marker_lane_extend_down", "Extend Edit Selection To Marker Lane Below", ["Edit", "Selection"], None, "{}", always, |e, _| marker_lane(e, 1, true)),
        cmd!(
            "edit.space_clips",
            "Space Clips…",
            ["Edit"],
            None,
            "{clips?, gap?: samples|{seconds} (default 0), mode?: end_to_start|start_to_start}",
            has_selection,
            space_clips
        ),
        cmd!(
            "edit.snap_next",
            "Next",
            ["Edit", "Snap to"],
            None,
            "{clips?} — moves each clip so its end meets the start of the next clip",
            has_selection,
            |e, p| snap(e, p, true)
        ),
        cmd!(
            "edit.snap_previous",
            "Previous",
            ["Edit", "Snap to"],
            None,
            "{clips?} — moves each clip so its start meets the end of the previous clip",
            has_selection,
            |e, p| snap(e, p, false)
        ),
        cmd!(
            "edit.copy_to_target_playlist",
            "Target Playlist",
            ["Edit", "Copy Selection to..."],
            None,
            "{tracks?, start?, end?} — copies the selection into each track's target playlist",
            has_range,
            |e, p| to_playlist(e, p, Dest::Target, false, "edit.copy_to_target_playlist")
        ),
        cmd!(
            "edit.copy_to_duplicate_playlist",
            "Duplicate of Main Playlist",
            ["Edit", "Copy Selection to..."],
            None,
            "{tracks?, start?, end?}",
            has_range,
            |e, p| to_playlist(e, p, Dest::Duplicate, false, "edit.copy_to_duplicate_playlist")
        ),
        cmd!(
            "edit.move_to_target_playlist",
            "Target Playlist",
            ["Edit", "Move Selection to..."],
            None,
            "{tracks?, start?, end?} — copies to the target playlist and clears it from the main",
            has_range,
            |e, p| to_playlist(e, p, Dest::Target, true, "edit.move_to_target_playlist")
        ),
        cmd!("edit.move_to_new_playlist", "New Playlist", ["Edit", "Move Selection to..."], None, "{tracks?, start?, end?}", has_range, |e, p| {
            to_playlist(e, p, Dest::New, true, "edit.move_to_new_playlist")
        }),
        cmd!(
            "edit.move_to_duplicate_playlist",
            "Duplicate of Main Playlist",
            ["Edit", "Move Selection to..."],
            None,
            "{tracks?, start?, end?}",
            has_range,
            |e, p| to_playlist(e, p, Dest::Duplicate, true, "edit.move_to_duplicate_playlist")
        ),
        cmd!(
            "edit.tce_to_timeline",
            "TCE Edit to Timeline Selection",
            ["Edit"],
            None,
            "{clip?, start?, end?} — time-stretches the clip to fill the timeline selection (pitch preserved)",
            has_selection,
            tce
        ),
        cmd!(
            "edit.automation_coalesce_volume_to_clip_gain",
            "Coalesce Volume Automation to Clip Gain",
            ["Edit", "Automation"],
            None,
            "{tracks?} — folds the whole volume curve into clip gain; the fader goes to 0 dB",
            has_selection,
            coalesce_volume_to_clip_gain
        ),
        cmd!(
            "edit.automation_coalesce_clip_gain_to_volume",
            "Coalesce Clip Gain to Volume Automation",
            ["Edit", "Automation"],
            None,
            "{tracks?} — folds all clip gain into volume automation",
            has_selection,
            coalesce_clip_gain_to_volume
        ),
        cmd!(
            "edit.automation_trim_to_all",
            "Trim to All Enabled",
            ["Edit", "Automation"],
            None,
            "{tracks?, start?, end?} — offsets every enabled lane in the selection so it starts at the current control value",
            has_range,
            |e, p| trim_glide(e, p, false, false, "edit.automation_trim_to_all")
        ),
        cmd!(
            "edit.automation_glide_to_all",
            "Glide to All Enabled",
            ["Edit", "Automation"],
            None,
            "{tracks?, start?, end?} — ramps every enabled lane from its value at the start to the current value at the end",
            has_range,
            |e, p| trim_glide(e, p, true, false, "edit.automation_glide_to_all")
        ),
        cmd!("edit.automation_glide_pan", "Glide Pan Only", ["Edit", "Automation"], None, "{tracks?, start?, end?}", has_range, |e, p| trim_glide(
            e,
            p,
            true,
            true,
            "edit.automation_glide_pan"
        )),
    ]
}

fn shuffle(s: &Session) -> bool {
    s.edit.edit_mode == EditMode::Shuffle
}

fn nonempty_range(e: &Engine, p: &Value, id: &str) -> Result<Range> {
    let r = range_param(e, id, p)?;
    if r.is_empty() { Err(bad(id, "the selection is empty")) } else { Ok(r) }
}

pub(super) fn cut_all_automation(e: &mut Engine, p: &Value) -> Result<Value> {
    auto_copy(e, p, Lanes::All, true, "edit.cut_all_automation")
}

fn auto_copy(e: &mut Engine, p: &Value, which: Lanes, cut: bool, id: &str) -> Result<Value> {
    let tracks = tracks_required(e, id, p)?;
    let r = nonempty_range(e, p, id)?;
    let mut cb = Clipboard { length: r.len(), ..Clipboard::default() };
    let mut lanes_n = 0;
    for t in &tracks {
        cb.tracks.push(Vec::new());
        let lanes: Vec<AutomationLane> = e
            .session()
            .track(*t)
            .map(|tr| {
                tr.automation
                    .iter()
                    .filter(|l| which.hit(&l.param) && !l.points.is_empty())
                    .map(|l| {
                        let mut out = AutomationLane::new(l.param.clone());
                        out.enabled = l.enabled;
                        let (_, _, def) = l.param.range();
                        out.set_point(0, l.value_at(r.start, def));
                        for pt in l.points.iter().filter(|pt| pt.at > r.start && pt.at < r.end) {
                            out.set_point(pt.at - r.start, pt.value);
                        }
                        out.set_point(r.len() - 1, l.value_at(r.end - 1, def));
                        out
                    })
                    .collect()
            })
            .unwrap_or_default();
        lanes_n += lanes.len();
        cb.automation.push(lanes);
    }
    if cut {
        let s = e.session_mut();
        for t in &tracks {
            if let Some(tr) = s.track_mut(*t) {
                for l in tr.automation.iter_mut().filter(|l| which.hit(&l.param)) {
                    l.clear_range(r.start, r.end);
                }
            }
        }
    }
    e.clipboard = cb;
    Ok(json!({"lanes": lanes_n, "length": r.len()}))
}

fn copy_clip_gain(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.copy_clip_gain";
    let tracks = tracks_required(e, id, p)?;
    let r = nonempty_range(e, p, id)?;
    let s = e.session();
    let mut out = Vec::new();
    for t in &tracks {
        let mut env: Vec<(Samples, f32)> = Vec::new();
        for c in s.track(*t).map(|tr| tr.clips()).unwrap_or(&[]) {
            let Some(i) = c.range().intersect(&r) else { continue };
            env.push((i.start - r.start, clip_db(c, i.start - c.start)));
            for &(o, _) in &c.gain_env {
                let at = c.start + o;
                if at > i.start && at < i.end - 1 {
                    env.push((at - r.start, clip_db(c, o)));
                }
            }
            env.push((i.end - 1 - r.start, clip_db(c, i.end - 1 - c.start)));
        }
        env.sort_by_key(|x| x.0);
        env.dedup_by_key(|x| x.0);
        out.push(env);
    }
    let n = out.iter().map(Vec::len).sum::<usize>();
    e.clipboard = Clipboard { length: r.len(), clip_gain: out, ..Clipboard::default() };
    Ok(json!({"points": n, "length": r.len()}))
}

fn paste_clip_gain(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.paste_clip_gain";
    let tracks = tracks_required(e, id, p)?;
    let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start);
    let cb = e.clipboard.clone();
    let paste = Range::new(at, at.saturating_add(cb.length.max(1)));
    let s = e.session_mut();
    let mut n = 0;
    for (i, t) in tracks.iter().enumerate() {
        let Some(env) = cb.clip_gain.get(i % cb.clip_gain.len().max(1)) else { continue };
        let Some(pl) = s.track_mut(*t).and_then(|tr| tr.playlist_mut()) else { continue };
        for c in pl.clips.iter_mut().filter(|c| c.range().overlaps(&paste)) {
            let Some(ov) = c.range().intersect(&paste) else { continue };
            let (a, b) = (ov.start - c.start, ov.end - c.start);
            let mut pts: Vec<(Samples, f32)> = Vec::new();
            // Keep the existing curve outside the paste range, anchored at its edges.
            let mut old: Vec<(Samples, f32)> = c.gain_env.iter().map(|&(o, _)| (o, clip_db(c, o))).collect();
            old.push((0, clip_db(c, 0)));
            old.push((c.length, clip_db(c, c.length)));
            pts.extend(old.into_iter().filter(|(o, _)| *o < a || *o > b));
            if a > 0 {
                pts.push((a - 1, clip_db(c, a - 1)));
            }
            if b < c.length {
                pts.push((b, clip_db(c, b)));
            }
            for &(o, v) in env {
                let rel = at + o - c.start;
                if rel >= a && rel < b {
                    pts.push((rel, v));
                }
            }
            set_clip_env(c, pts);
            n += 1;
        }
    }
    Ok(json!({"clips": n}))
}

fn fx_prefix(c: ClipId) -> String {
    format!("clip_fx.{}.", c.0)
}

fn clip_effects(e: &mut Engine, p: &Value, copy: bool, remove: bool, id: &str) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let first = *ids.first().ok_or_else(|| bad(id, "select a clip"))?;
    if copy {
        let pre = fx_prefix(first);
        let fx: Vec<(String, f64)> =
            e.session().edit.values.iter().filter_map(|(k, v)| k.strip_prefix(&pre).map(|name| (name.to_string(), *v))).collect();
        if fx.is_empty() {
            return Err(bad(id, "the clip has no clip effects"));
        }
        e.clipboard = Clipboard { clip_effects: fx, ..Clipboard::default() };
    }
    let mut removed = 0;
    if remove {
        let s = e.session_mut();
        for c in &ids {
            let pre = fx_prefix(*c);
            let before = s.edit.values.len();
            s.edit.values.retain(|k, _| !k.starts_with(&pre));
            removed += before - s.edit.values.len();
            s.edit.set_flag(&format!("clip_fx.bypass.{}", c.0), false);
        }
    }
    Ok(json!({"clips": ids.len(), "copied": e.clipboard.clip_effects.len(), "removed": removed}))
}

fn paste_clip_effects(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("edit.paste_clip_effects", "select a clip"));
    }
    let fx = e.clipboard.clip_effects.clone();
    let s = e.session_mut();
    for c in &ids {
        let pre = fx_prefix(*c);
        s.edit.values.retain(|k, _| !k.starts_with(&pre));
        for (k, v) in &fx {
            s.edit.values.insert(format!("{pre}{k}"), *v);
        }
    }
    Ok(json!({"clips": ids.len()}))
}

fn paste_merge_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.paste_merge_midi";
    let tracks = tracks_required(e, id, p)?;
    let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
    let cb = e.clipboard.clone();
    let s = e.session_mut();
    let tempo = s.tempo.clone();
    let sr = s.sample_rate;
    let mut merged = 0;
    let mut placed = 0;
    for (i, t) in tracks.iter().enumerate() {
        let Some(clips) = cb.tracks.get(i % cb.tracks.len().max(1)) else { continue };
        for c in clips {
            let pos = at.saturating_add(c.start);
            let ClipContent::Midi { sequence } = &c.content else {
                let mut nc = c.clone();
                nc.id = s.new_clip_id();
                nc.start = pos;
                ops::place_clip(s, *t, nc);
                placed += 1;
                continue;
            };
            let target = s.track(*t).and_then(|tr| {
                let host = tr.clips().iter().find(|x| matches!(x.content, ClipContent::Midi { .. }) && x.range().contains(pos))?;
                let next_start = tr.clips().iter().filter(|x| x.start > host.start && x.id != host.id).map(|x| x.start).min();
                Some((host.id, host.start, next_start))
            });
            match target {
                Some((hid, hstart, next_start)) => {
                    let diff = tempo.samples_to_ticks(pos, sr) - tempo.samples_to_ticks(hstart, sr);
                    let want_end = pos.saturating_add(c.length);
                    let limit = next_start.unwrap_or(Samples::MAX);
                    if let Some(h) = s.find_clip_mut(hid)
                        && let ClipContent::Midi { sequence: hs } = &mut h.content
                    {
                        for n in &sequence.notes {
                            let mut n = *n;
                            n.start = n.start.saturating_add(diff);
                            hs.notes.push(n);
                        }
                        for cc in &sequence.ctrls {
                            let mut cc = *cc;
                            cc.tick = cc.tick.saturating_add(diff);
                            hs.ctrls.push(cc);
                        }
                        hs.sort();
                        if want_end > h.end() {
                            h.trim_end_to(want_end.min(limit));
                        }
                        merged += 1;
                    }
                }
                None => {
                    let mut nc = c.clone();
                    nc.id = s.new_clip_id();
                    nc.start = pos;
                    ops::place_clip(s, *t, nc);
                    placed += 1;
                }
            }
        }
    }
    Ok(json!({"merged": merged, "placed": placed}))
}

fn map_value(v: f32, from: &AutoParam, to: &AutoParam) -> f32 {
    let (a0, a1, _) = from.range();
    let (b0, b1, _) = to.range();
    let finite = |x: f32, y: f32| x > f32::MIN && y < f32::MAX && y > x;
    if from == to || !finite(a0, a1) || !finite(b0, b1) {
        return v;
    }
    b0 + (v - a0) / (a1 - a0) * (b1 - b0)
}

fn paste_to_current_automation(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.paste_to_current_automation";
    let tracks = tracks_required(e, id, p)?;
    let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
    let forced = match str_param(p, "param") {
        Some(x) => Some(AutoParam::parse(x).ok_or_else(|| bad(id, format!("unknown automation param `{x}`")))?),
        None => None,
    };
    let cb = e.clipboard.clone();
    let s = e.session_mut();
    let mut written = 0;
    for (i, t) in tracks.iter().enumerate() {
        let Some(lanes) = cb.automation.get(i % cb.automation.len().max(1)) else { continue };
        let Some(src) = lanes.iter().max_by_key(|l| l.points.len()).filter(|l| !l.points.is_empty()) else { continue };
        let Some(tr) = s.track_mut(*t) else { continue };
        let dst = forced.clone().unwrap_or_else(|| AutoParam::parse(&tr.view).unwrap_or(AutoParam::Volume));
        let lane = tr.lane_mut(&dst);
        lane.clear_range(at, at.saturating_add(cb.length));
        for pt in &src.points {
            lane.set_point(at.saturating_add(pt.at), map_value(pt.value, &src.param, &dst));
            written += 1;
        }
    }
    Ok(json!({"points": written}))
}

fn duplicate_extend(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.duplicate_extend";
    let tracks = tracks_required(e, id, p)?;
    let r = nonempty_range(e, p, id)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let clips = ops::copy_range(s, *t, r);
        n += ops::paste(s, *t, r.end, &clips, r.len(), false).len();
        if let Some(tr) = s.track_mut(*t) {
            for l in &mut tr.automation {
                let pts: Vec<_> = l.points.iter().filter(|pt| r.contains(pt.at)).copied().collect();
                for pt in pts {
                    l.set_point(pt.at.saturating_add(r.len()), pt.value);
                }
            }
        }
    }
    s.edit.selection = Range::new(r.start, r.end.saturating_add(r.len()));
    if s.edit.link_timeline_edit {
        s.edit.timeline_selection = s.edit.selection;
    }
    Ok(json!({"clips": n, "selection": s.edit.selection}))
}

fn extend_members(e: &mut Engine, up: bool) -> Result<Value> {
    let s = e.session_mut();
    let mut set: std::collections::BTreeSet<u64> = s.edit.selected_tracks.iter().map(|t| t.0).collect();
    let mut roots: Vec<u64> = Vec::new();
    for t in &s.edit.selected_tracks {
        let Some(tr) = s.track(*t) else { continue };
        if up {
            if let Some(f) = tr.folder {
                set.insert(f.0);
                roots.push(f.0);
            }
        } else if tr.is_folder() {
            roots.push(tr.id.0);
        }
    }
    // Add members (recursively, bounded by the track count).
    let mut guard = 0;
    while let Some(f) = roots.pop() {
        guard += 1;
        if guard > s.tracks.len().saturating_mul(2) + 8 {
            break;
        }
        for tr in s.tracks.iter().filter(|x| x.folder.is_some_and(|p| p.0 == f)) {
            if set.insert(tr.id.0) && tr.is_folder() {
                roots.push(tr.id.0);
            }
        }
    }
    s.edit.selected_tracks = s.tracks.iter().filter(|t| set.contains(&t.id.0)).map(|t| t.id).collect();
    Ok(json!({"tracks": s.edit.selected_tracks}))
}

/// Marker ruler lanes (1..5) in the edit selection, kept in `selection.marker_lane.top/bottom`.
fn marker_lane(e: &mut Engine, dir: i64, extend: bool) -> Result<Value> {
    let s = e.session_mut();
    let top = s.edit.value("selection.marker_lane.top", 1.0).clamp(1.0, 5.0) as i64;
    let bottom = (s.edit.value("selection.marker_lane.bottom", top as f64).clamp(1.0, 5.0) as i64).max(top);
    let (top, bottom) = match (extend, dir < 0) {
        (false, true) => ((top - 1).max(1), (top - 1).max(1)),
        (false, false) => ((bottom + 1).min(5), (bottom + 1).min(5)),
        (true, true) => ((top - 1).max(1), bottom),
        (true, false) => (top, (bottom + 1).min(5)),
    };
    s.edit.values.insert("selection.marker_lane.top".into(), top as f64);
    s.edit.values.insert("selection.marker_lane.bottom".into(), bottom as f64);
    let r = s.edit.selection;
    let markers: Vec<u32> = s
        .markers
        .iter()
        .filter(|m| (top..=bottom).contains(&i64::from(m.ruler)) && (r.contains(m.start) || m.start == r.start))
        .map(|m| m.number)
        .collect();
    Ok(json!({"lanes": [top, bottom], "markers": markers}))
}

fn space_clips(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.space_clips";
    let ids = clip_ids_param(e, p);
    let gap = position_param(e, id, p, "gap")?.unwrap_or(0);
    let s2s = match str_param(p, "mode") {
        None | Some("end_to_start") => false,
        Some("start_to_start") => true,
        Some(m) => return Err(bad(id, format!("unknown mode `{m}`"))),
    };
    if ids.len() < 2 {
        return Err(bad(id, "select two or more clips"));
    }
    let s = e.session_mut();
    let track_ids: Vec<_> = s.tracks.iter().map(|t| t.id).collect();
    let mut moved = 0;
    for t in track_ids {
        let mut sel: Vec<Clip> = s
            .track(t)
            .map_or_else(Vec::new, |tr| tr.clips().iter().filter(|c| ids.contains(&c.id) && !c.time_locked && !c.edit_locked).cloned().collect());
        if sel.len() < 2 {
            continue;
        }
        sel.sort_by_key(|c| (c.start, c.id.0));
        if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
            pl.clips.retain(|c| !sel.iter().any(|x| x.id == c.id));
        }
        let mut cur = sel.first().map_or(0, |c| c.start);
        for mut c in sel {
            c.start = cur.max(0);
            cur = if s2s { c.start.saturating_add(gap) } else { c.end().saturating_add(gap) };
            ops::place_clip(s, t, c);
            moved += 1;
        }
    }
    Ok(json!({"clips": moved, "gap": gap}))
}

fn snap(e: &mut Engine, p: &Value, next: bool) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut order: Vec<(ClipId, Samples)> = ids.iter().filter_map(|id| s.find_clip(*id).map(|(_, c)| (*id, c.start))).collect();
    // Snap the clip nearest the destination first so neighbours stay put.
    order.sort_by_key(|x| if next { -x.1 } else { x.1 });
    let mut moved = 0;
    for (id, _) in order {
        let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        // Selected neighbours are real destinations. Ignoring them made Snap to Next
        // pile every selected clip onto the same later clip, and Snap to Previous
        // leave the gap between the selected clips in place.
        let others = s.track(t).map(|tr| tr.clips().iter().filter(|x| x.id != id).cloned().collect::<Vec<_>>()).unwrap_or_default();
        let delta = if next {
            others.iter().filter(|x| x.start >= c.end()).map(|x| x.start).min().map(|st| st - c.end())
        } else {
            others.iter().filter(|x| x.end() <= c.start).map(Clip::end).max().map(|en| en - c.start)
        };
        if let Some(d) = delta.filter(|d| *d != 0) {
            moved += ops::move_clips(s, &[id], d, None);
        }
    }
    Ok(json!({"moved": moved}))
}

fn to_playlist(e: &mut Engine, p: &Value, dest: Dest, mv: bool, id: &str) -> Result<Value> {
    let tracks = tracks_required(e, id, p)?;
    let r = nonempty_range(e, p, id)?;
    let sh = shuffle(e.session());
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in &tracks {
        if s.track(*t).is_none_or(|tr| !tr.kind.has_playlist()) {
            continue;
        }
        let idx = match dest {
            Dest::Target => {
                let i = target_playlist(s, *t).ok_or_else(|| bad(id, "designate a target playlist first (Track › Designate as Target Playlist)"))?;
                if s.track(*t).is_some_and(|tr| tr.active_playlist == i) {
                    return Err(bad(id, "the target playlist is the main playlist"));
                }
                i
            }
            Dest::New | Dest::Duplicate => {
                let mut clips: Vec<Clip> =
                    if dest == Dest::Duplicate { s.track(*t).map(|tr| tr.clips().to_vec()).unwrap_or_default() } else { Vec::new() };
                for c in &mut clips {
                    c.id = s.new_clip_id();
                }
                let Some(tr) = s.track_mut(*t) else { continue };
                let name = format!("{}.{:02}", tr.name, tr.playlists.len() + 1);
                tr.playlists.push(Playlist { name, clips });
                tr.playlists.len() - 1
            }
        };
        let clips = ops::copy_range(s, *t, r);
        let main = s.track(*t).map_or(0, |tr| tr.active_playlist);
        if let Some(tr) = s.track_mut(*t) {
            tr.active_playlist = idx;
        }
        for mut c in clips {
            c.id = s.new_clip_id();
            c.start = c.start.saturating_add(r.start);
            ops::place_clip(s, *t, c);
        }
        if let Some(tr) = s.track_mut(*t) {
            tr.active_playlist = main;
        }
        if mv {
            ops::clear_range(s, *t, r, sh);
        }
        out.push(json!({"track": t, "playlist": idx}));
    }
    Ok(json!({"tracks": out}))
}

fn tce(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.tce_to_timeline";
    let target = {
        let st = position_param(e, id, p, "start")?;
        let en = position_param(e, id, p, "end")?;
        match (st, en) {
            (Some(a), Some(b)) => Range::new(a, b),
            _ => e.session().edit.timeline_selection,
        }
    };
    if target.is_empty() || target.start < 0 {
        return Err(bad(id, "make a timeline selection first"));
    }
    let ids = clip_ids_param(e, p);
    let Some((t, c)) = ids.iter().find_map(|cid| e.session().find_clip(*cid).filter(|(_, c)| c.is_audio()).map(|(t, c)| (t, c.clone()))) else {
        return Err(bad(id, "select an audio clip"));
    };
    if c.time_locked || c.edit_locked {
        return Err(bad(id, "the clip is locked"));
    }
    let s = e.session_mut();
    let Some(src) = render_stretched(s, id, &c, target.len())? else { return Err(bad(id, "the clip's audio is not loaded")) };
    let ratio = target.len() as f64 / c.length.max(1) as f64;
    let scale = |x: Samples| soundcraft_time::to_samples(x as f64 * ratio);
    let mut nc = c.clone();
    nc.content = ClipContent::Audio { source: src, offset: 0 };
    nc.start = target.start;
    nc.length = target.len();
    nc.stretch = 1.0;
    nc.gain_env = c.gain_env.iter().map(|&(o, v)| (scale(o), v)).collect();
    nc.fade_in.len = scale(c.fade_in.len).clamp(0, nc.length);
    nc.fade_out.len = scale(c.fade_out.len).clamp(0, nc.length);
    nc.sync_point = scale(c.sync_point).clamp(0, nc.length);
    replace_clip(s, t, c.id, nc);
    s.edit.selected_clips = vec![c.id];
    Ok(json!({"clip": c.id, "source": src, "ratio": ratio}))
}

fn volume_at(lane: Option<&AutomationLane>, fader: f32, at: Samples) -> f32 {
    match lane {
        Some(l) if !l.points.is_empty() => l.value_at(at, fader),
        _ => fader,
    }
}

fn coalesce_volume_to_clip_gain(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.automation_coalesce_volume_to_clip_gain", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        if !tr.kind.has_playlist() || tr.kind.is_midi() {
            continue;
        }
        let lane = tr.lane(&AutoParam::Volume).cloned();
        let fader = tr.mixer.volume_db;
        let Some(pl) = tr.playlist_mut() else { continue };
        for c in pl.clips.iter_mut().filter(|c| c.is_audio()) {
            let mut offs: Vec<Samples> = vec![0, c.length];
            offs.extend(c.gain_env.iter().map(|x| x.0));
            if let Some(l) = &lane {
                offs.extend(l.points.iter().filter(|pt| c.range().contains(pt.at)).map(|pt| pt.at - c.start));
            }
            offs.sort_unstable();
            offs.dedup();
            let pts: Vec<(Samples, f32)> = offs.iter().map(|&o| (o, clip_db(c, o) + volume_at(lane.as_ref(), fader, c.start + o))).collect();
            set_clip_env(c, pts);
            n += 1;
        }
        tr.automation.retain(|l| l.param != AutoParam::Volume);
        tr.mixer.volume_db = 0.0;
    }
    Ok(json!({"clips": n}))
}

fn coalesce_clip_gain_to_volume(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "edit.automation_coalesce_clip_gain_to_volume", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let clips: Vec<Clip> = tr.clips().iter().filter(|c| c.is_audio() && (c.gain_db != 0.0 || !c.gain_env.is_empty())).cloned().collect();
        if clips.is_empty() {
            continue;
        }
        let lane = tr.lane(&AutoParam::Volume).cloned();
        let fader = tr.mixer.volume_db;
        let mut times: Vec<Samples> = lane.as_ref().map(|l| l.points.iter().map(|pt| pt.at).collect()).unwrap_or_default();
        for c in &clips {
            times.push(c.start);
            times.push(c.end() - 1);
            times.extend(c.gain_env.iter().map(|x| c.start + x.0));
        }
        times.sort_unstable();
        times.dedup();
        let gain_at = |at: Samples| clips.iter().find(|c| c.range().contains(at)).map_or(0.0, |c| clip_db(c, at - c.start));
        let pts: Vec<(Samples, f32)> = times.iter().map(|&at| (at, volume_at(lane.as_ref(), fader, at) + gain_at(at))).collect();
        let l = tr.lane_mut(&AutoParam::Volume);
        l.points.clear();
        for (at, v) in pts {
            l.set_point(at.max(0), v);
        }
        if let Some(pl) = tr.playlist_mut() {
            for c in pl.clips.iter_mut().filter(|c| clips.iter().any(|x| x.id == c.id)) {
                c.gain_db = 0.0;
                c.gain_env.clear();
                n += 1;
            }
        }
    }
    Ok(json!({"clips": n}))
}

fn trim_glide(e: &mut Engine, p: &Value, glide: bool, pan_only: bool, id: &str) -> Result<Value> {
    let tracks = tracks_required(e, id, p)?;
    let r = nonempty_range(e, p, id)?;
    let s = e.session_mut();
    let mut lanes = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let mut params: Vec<AutoParam> = tr
            .automation
            .iter()
            .filter(|l| l.enabled && !matches!(l.param, AutoParam::Mute | AutoParam::SendMute(_)))
            .filter(|l| !pan_only || matches!(l.param, AutoParam::Pan(_)))
            .map(|l| l.param.clone())
            .collect();
        if pan_only {
            for i in 0..tr.mixer.pan.len().min(2) {
                let pp = AutoParam::Pan(i as u8);
                if !params.contains(&pp) {
                    params.push(pp);
                }
            }
        }
        for param in params {
            let cur = super::mix::current_value(tr, &param);
            let (lo, hi, _) = param.range();
            let l = tr.lane_mut(&param);
            let before = l.value_at(r.start - 1, cur);
            let after = l.value_at(r.end, cur);
            let from = l.value_at(r.start, cur);
            if glide {
                l.clear_range(r.start, r.end.saturating_add(2));
                if r.start > 0 {
                    l.set_point(r.start - 1, before);
                }
                l.set_point(r.start, from);
                l.set_point(r.end, cur);
                l.set_point(r.end.saturating_add(1), after);
            } else {
                let delta = cur - from;
                let last = l.value_at(r.end - 1, cur);
                if r.start > 0 {
                    l.set_point(r.start - 1, before);
                }
                l.set_point(r.start, from);
                l.set_point(r.end - 1, last);
                l.set_point(r.end, after);
                for pt in l.points.iter_mut().filter(|pt| pt.at >= r.start && pt.at < r.end) {
                    pt.value = (pt.value + delta).clamp(lo, hi);
                }
            }
            lanes += 1;
        }
    }
    Ok(json!({"lanes": lanes}))
}

fn has_marker_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.markers.is_empty() { Err("no markers on the clipboard".into()) } else { Ok(()) }
}

fn paste_merge_markers(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "edit.paste_merge_markers";
    if e.clipboard.markers.is_empty() {
        return Err(bad(id, "no markers on the clipboard (copy a range that contains markers)"));
    }
    let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
    let copied = e.clipboard.markers.clone();
    let s = e.session_mut();
    let mut added = 0;
    for m in copied {
        let start = at.saturating_add(m.start);
        if s.markers.iter().any(|x| x.start == start && x.name == m.name && x.ruler == m.ruler) {
            continue;
        }
        let new = s.add_marker(&m.name, m.kind, start, start.saturating_add(m.end - m.start));
        if let Some(x) = s.markers.iter_mut().find(|x| x.id == new) {
            x.comments = m.comments;
            x.color = m.color;
            x.ruler = m.ruler;
        }
        added += 1;
    }
    Ok(json!({"added": added}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, SourceId, TrackKind};

    #[test]
    fn merge_markers_pastes_copied_markers_once() {
        let mut e = crate::demo::demo_engine();
        let first = e.session().markers.first().cloned().expect("demo has markers");
        let r = json!({"start": first.start.saturating_sub(10).max(0), "end": first.start + 10});
        e.execute("edit.select", &json!({"tracks": ["Kick"], "start": r["start"], "end": r["end"]})).unwrap();
        e.execute("edit.copy", &json!({})).unwrap();
        let before = e.session().markers.len();
        let at = 5_000_000;
        let out = e.execute("edit.paste_merge_markers", &json!({"at": at})).unwrap();
        assert_eq!(out["added"], 1);
        assert_eq!(e.session().markers.len(), before + 1);
        assert!(e.session().markers.iter().any(|m| m.name == first.name && m.start == at + first.start - r["start"].as_i64().unwrap()));
        // Pasting again at the same place adds nothing.
        assert_eq!(e.execute("edit.paste_merge_markers", &json!({"at": at})).unwrap()["added"], 0);
    }

    fn session_with_clips(starts: &[(Samples, Samples)]) -> (Engine, soundcraft_model::TrackId, Vec<ClipId>) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut ids = Vec::new();
        for (st, len) in starts {
            let id = s.new_clip_id();
            ops::place_clip(&mut s, t, Clip::audio(id, "c", SourceId(1), 0, *st, *len));
            ids.push(id);
        }
        s.edit.selected_tracks = vec![t];
        (Engine::new(s), t, ids)
    }

    fn starts(e: &Engine, t: soundcraft_model::TrackId) -> Vec<Samples> {
        e.session().track(t).map(|tr| tr.clips().iter().map(|c| c.start).collect()).unwrap_or_default()
    }

    #[test]
    fn space_clips_butts_with_gap() {
        let (mut e, t, ids) = session_with_clips(&[(0, 100), (500, 100), (2000, 50)]);
        e.execute("edit.space_clips", &json!({"clips": ids.iter().map(|c| c.0).collect::<Vec<_>>(), "gap": 10})).unwrap();
        assert_eq!(starts(&e, t), vec![0, 110, 220]);
        assert!(e.can_undo());
    }

    #[test]
    fn snap_next_and_previous() {
        let (mut e, t, ids) = session_with_clips(&[(0, 100), (300, 100), (1000, 100)]);
        e.execute("edit.snap_previous", &json!({"clip": ids[1].0})).unwrap();
        assert_eq!(starts(&e, t), vec![0, 100, 1000]);
        e.execute("edit.snap_next", &json!({"clip": ids[1].0})).unwrap();
        assert_eq!(starts(&e, t), vec![0, 900, 1000]);
    }

    #[test]
    fn snap_selected_neighbours_keeps_both_clips() {
        let (mut e, t, ids) = session_with_clips(&[(0, 100), (300, 100), (1000, 100)]);
        e.execute("edit.snap_next", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips().len(), 3);
        assert_eq!(starts(&e, t), vec![800, 900, 1000]);

        let (mut e, t, ids) = session_with_clips(&[(0, 100), (300, 100), (1000, 100)]);
        e.execute("edit.snap_previous", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
        assert_eq!(e.session().track(t).unwrap().clips().len(), 3);
        assert_eq!(starts(&e, t), vec![0, 100, 1000]);
    }

    #[test]
    fn cut_all_automation_can_be_pasted() {
        let (mut e, t, _) = session_with_clips(&[(0, 1000)]);
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 100, "value": -6.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 400, "value": -12.0})).unwrap();
        let cut = e.execute("edit.cut_all_automation", &json!({"start": 50, "end": 250})).unwrap();
        assert!(cut["lanes"].as_u64().unwrap() >= 1, "{cut}");
        assert!(e.clipboard.automation.iter().any(|lanes| !lanes.is_empty()));
        let lane = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().clone();
        assert!(lane.points.iter().all(|p| p.at != 100), "the cut point is gone: {:?}", lane.points);
        e.execute("edit.paste_to_current_automation", &json!({"at": 1000})).unwrap();
        let vol = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().clone();
        assert!(vol.points.iter().any(|p| p.at >= 1000 && (p.value + 6.0).abs() < 1e-2), "paste wrote nothing at -6 dB: {:?}", vol.points);
    }

    #[test]
    fn copy_and_paste_automation_only() {
        let (mut e, t, _) = session_with_clips(&[(0, 1000)]);
        e.execute("automation.set_point", &json!({"track": t.0, "param": "pan", "at": 100, "value": -1.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "pan", "at": 200, "value": 1.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 150, "value": -6.0})).unwrap();
        e.execute("edit.cut_pan_automation", &json!({"start": 50, "end": 250})).unwrap();
        assert_eq!(e.clipboard.automation[0].len(), 1, "only the pan lane is cut");
        assert!(e.session().track(t).unwrap().lane(&AutoParam::Pan(0)).unwrap().points.is_empty());
        assert_eq!(e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().points.len(), 1);
        // Paste pan into the volume lane, rescaled from -1..1 to -144..12.
        e.execute("track.view", &json!({"view": "volume"})).unwrap();
        e.execute("edit.paste_to_current_automation", &json!({"at": 1000})).unwrap();
        let vol = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().clone();
        assert!((vol.value_at(1150, 0.0) - 12.0).abs() < 1e-3, "{:?}", vol.points);
        assert!((vol.value_at(1050, 0.0) + 144.0).abs() < 1e-3);
    }

    #[test]
    fn clip_gain_copy_paste() {
        let (mut e, t, ids) = session_with_clips(&[(0, 1000), (2000, 1000)]);
        e.execute("clip.gain", &json!({"clip": ids[0].0, "db": -6.0})).unwrap();
        e.execute("edit.copy_clip_gain", &json!({"start": 0, "end": 1000})).unwrap();
        e.execute("edit.paste_clip_gain", &json!({"at": 2000})).unwrap();
        let c = e.session().track(t).unwrap().clips()[1].clone();
        assert!((clip_db(&c, 500) + 6.0).abs() < 1e-3, "{c:?}");
    }

    #[test]
    fn move_to_new_playlist_clears_main() {
        let (mut e, t, _) = session_with_clips(&[(0, 1000)]);
        e.execute("edit.move_to_new_playlist", &json!({"start": 200, "end": 400})).unwrap();
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.playlists.len(), 2);
        assert_eq!(tr.active_playlist, 0);
        assert_eq!(tr.playlists[1].clips.len(), 1);
        assert_eq!(tr.playlists[1].clips[0].range(), Range::new(200, 400));
        assert!(tr.clips().iter().all(|c| !c.range().overlaps(&Range::new(200, 400))));
        // Target playlist needs a designation first.
        assert!(e.execute("edit.copy_to_target_playlist", &json!({"start": 0, "end": 100})).is_err());
        e.execute("track.playlist_select", &json!({"index": 1})).unwrap();
        e.execute("track.designate_target_playlist", &json!({})).unwrap();
        e.execute("track.playlist_select", &json!({"index": 0})).unwrap();
        e.execute("edit.copy_to_target_playlist", &json!({"start": 0, "end": 100})).unwrap();
        assert_eq!(e.session().track(t).unwrap().playlists[1].clips.len(), 2);
    }

    #[test]
    fn coalesce_volume_into_clip_gain_and_back() {
        let (mut e, t, _) = session_with_clips(&[(0, 1000)]);
        e.execute("automation.set_point", &json!({"track": t.0, "at": 0, "value": -10.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "at": 1000, "value": 0.0})).unwrap();
        e.execute("edit.automation_coalesce_volume_to_clip_gain", &json!({})).unwrap();
        let c = e.session().track(t).unwrap().clips()[0].clone();
        assert!((clip_db(&c, 500) + 5.0).abs() < 1e-3);
        assert!(e.session().track(t).unwrap().lane(&AutoParam::Volume).is_none());
        e.execute("edit.automation_coalesce_clip_gain_to_volume", &json!({})).unwrap();
        let tr = e.session().track(t).unwrap();
        assert!((tr.lane(&AutoParam::Volume).unwrap().value_at(500, 0.0) + 5.0).abs() < 0.1);
        assert_eq!(tr.clips()[0].gain_db, 0.0);
    }

    #[test]
    fn glide_and_trim() {
        let (mut e, t, _) = session_with_clips(&[(0, 1000)]);
        e.execute("automation.set_point", &json!({"track": t.0, "at": 0, "value": -10.0})).unwrap();
        e.execute("mix.volume", &json!({"db": 0.0})).unwrap();
        e.execute("edit.automation_glide_to_all", &json!({"start": 100, "end": 300})).unwrap();
        let l = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().clone();
        assert!((l.value_at(200, 0.0) + 5.0).abs() < 0.1);
        assert!((l.value_at(600, 0.0) + 10.0).abs() < 1e-3, "after the glide the old curve resumes");
        e.execute("edit.automation_trim_to_all", &json!({"start": 400, "end": 500})).unwrap();
        let l = e.session().track(t).unwrap().lane(&AutoParam::Volume).unwrap().clone();
        assert!((l.value_at(450, 0.0)).abs() < 1e-3);
        assert!((l.value_at(700, 0.0) + 10.0).abs() < 1e-3);
    }

    #[test]
    fn merge_midi_keeps_existing_notes() {
        let mut e = crate::demo::demo_engine();
        let keys = e.session().track_by_name("Keys").unwrap().id;
        let clip = e.session().track(keys).unwrap().clips()[0].clone();
        let before = match &clip.content {
            ClipContent::Midi { sequence } => sequence.notes.len(),
            _ => 0,
        };
        let r = Range::new(clip.start, clip.start + 48_000);
        e.execute("edit.select", &json!({"tracks": [keys.0], "start": r.start, "end": r.end})).unwrap();
        e.execute("edit.copy", &json!({})).unwrap();
        let copied = match &e.clipboard.tracks[0][0].content {
            ClipContent::Midi { sequence } => sequence.notes.len(),
            _ => 0,
        };
        e.execute("edit.paste_merge_midi", &json!({"at": clip.start + 96_000})).unwrap();
        let after = match &e.session().track(keys).unwrap().clips()[0].content {
            ClipContent::Midi { sequence } => sequence.notes.len(),
            _ => 0,
        };
        assert_eq!(after, before + copied);
    }

    #[test]
    fn tce_fits_timeline() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let tone: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let src = crate::io::add_source(&mut s, "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        ops::place_clip(&mut s, t, Clip::audio(id, "tone", src, 0, 0, 24_000));
        s.edit.selected_tracks = vec![t];
        s.edit.selected_clips = vec![id];
        s.edit.timeline_selection = Range::new(1000, 31_000);
        let mut e = Engine::new(s);
        let r = e.execute("edit.tce_to_timeline", &json!({})).unwrap();
        assert!((r["ratio"].as_f64().unwrap() - 1.25).abs() < 1e-9);
        let nc = e.session().find_clip(id).unwrap().1.clone();
        assert_eq!((nc.start, nc.length), (1000, 30_000));
        let nsrc = nc.source().unwrap();
        assert_ne!(nsrc, src);
        assert_eq!(e.session().source(nsrc).unwrap().frames, 30_000);
        assert!(e.session().pool.get(nsrc).unwrap().buffer.channels[0].iter().any(|v| v.abs() > 0.1));
    }

    #[test]
    fn extend_to_members() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2})).unwrap();
        e.execute("track.move_to_new_folder", &json!({})).unwrap();
        let folder = e.session().tracks.iter().find(|t| t.is_folder()).unwrap().id;
        e.execute("edit.select", &json!({"tracks": [folder.0]})).unwrap();
        e.execute("edit.extend_down_to_members", &json!({})).unwrap();
        assert_eq!(e.session().edit.selected_tracks.len(), 3);
        let member = e.session().tracks.iter().find(|t| t.folder == Some(folder)).unwrap().id;
        e.execute("edit.select", &json!({"tracks": [member.0]})).unwrap();
        e.execute("edit.extend_up_to_members", &json!({})).unwrap();
        assert_eq!(e.session().edit.selected_tracks.len(), 3);
    }

    #[test]
    fn marker_lane_selection() {
        let mut e = Engine::default();
        e.execute("edit.marker_lane_move_down", &json!({})).unwrap();
        e.execute("edit.marker_lane_extend_down", &json!({})).unwrap();
        let r = e.execute("edit.marker_lane_extend_up", &json!({})).unwrap();
        assert_eq!(r["lanes"], json!([1, 3]));
        let r = e.execute("edit.marker_lane_move_up", &json!({})).unwrap();
        assert_eq!(r["lanes"], json!([1, 1]));
    }

    #[test]
    fn duplicate_and_extend() {
        let (mut e, t, _) = session_with_clips(&[(0, 100)]);
        e.execute("edit.duplicate_extend", &json!({"start": 0, "end": 100})).unwrap();
        assert_eq!(starts(&e, t), vec![0, 100]);
        assert_eq!(e.session().edit.selection, Range::new(0, 200));
    }
}
