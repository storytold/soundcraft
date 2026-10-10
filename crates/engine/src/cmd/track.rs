//! Track menu commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ChannelFormat, Group, GroupId, Playlist, Route, TrackHeight, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.new",
            "New...",
            ["Track"],
            Some("Cmd+Shift+N"),
            "{count?: 1, format?: mono|stereo|5.1…, kind?: audio|aux|master|midi|instrument|vca|folder, name?, timebase?: samples|ticks, instrument?: plugin id}",
            always,
            new_track
        ),
        cmd!("track.group", "Group...", ["Track"], None, "{name?, tracks?, edit?: true, mix?: true}", has_selection, group),
        cmd!("track.bus_folder", "Create Bus Folder", ["Track"], Some("Cmd+G"), "{tracks?, name?}", has_selection, move_to_folder),
        cmd!("track.ungroup", "Delete Group", [], None, "{group: id|name}", always, delete_group),
        cmd!("track.group_toggle", "Toggle Group Active", [], None, "{group: id|name}", always, toggle_group),
        cmd!(
            "track.duplicate",
            "Duplicate...",
            ["Track"],
            Some("Alt+Shift+D"),
            "{tracks?, playlists?: true, automation?: true, inserts?: true, sends?: true}",
            has_selection,
            duplicate
        ),
        cmd!("track.split_into_mono", "Split into Mono", ["Track"], None, "{tracks?}", has_selection, split_mono),
        cmd!("track.make_inactive", "Make Inactive", ["Track"], None, "{tracks?, inactive?: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.make_inactive",
            |t, v| t.inactive = v,
            |t| t.inactive
        )),
        cmd!(
            "track.move_to_new_folder",
            "Move to New Folder...",
            ["Track"],
            Some("Cmd+Alt+Shift+N"),
            "{tracks?, name?}",
            has_selection,
            move_to_folder
        ),
        cmd!(
            "track.change_width",
            "Change Track Width",
            ["Track", "Change Track Width"],
            None,
            "{tracks?, format: mono|stereo|…}",
            has_selection,
            change_width
        ),
        cmd!("track.delete", "Delete", ["Track"], None, "{tracks?}", has_selection, delete),
        cmd!("track.rename", "Rename Track", [], None, "{track, name}", has_tracks, rename),
        cmd!("track.comments", "Track Comments", [], None, "{track, comments}", has_tracks, comments),
        cmd!("track.color", "Track Color", [], None, "{tracks?, color: [r,g,b] | palette index}", has_selection, color),
        cmd!("track.height", "Track Height", [], None, "{tracks?, height: micro|mini|small|medium|large|jumbo|extreme}", has_selection, height),
        cmd!("track.view", "Track View", [], None, "{tracks?, view: waveform|blocks|notes|volume|pan|mute|<automation id>}", has_selection, view),
        cmd!("track.hide", "Hide Track", [], None, "{tracks?, hidden?: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.hide",
            |t, v| t.hidden = v,
            |t| t.hidden
        )),
        cmd!("track.pin", "Pin Track", [], None, "{tracks?}", has_selection, |e, p| flag(e, p, "track.pin", |t, v| t.pinned = v, |t| t.pinned)),
        cmd!("track.move", "Move Track", [], None, "{track, to: index}", has_tracks, move_track),
        cmd!("track.folder_assign", "Move into Bus Folder", [], None, "{tracks?, folder: track id|name}", has_tracks, folder_assign),
        cmd!("track.freeze", "Freeze", ["Track"], None, "{tracks?}", has_selection, |e, p| flag(
            e,
            p,
            "track.freeze",
            |t, v| t.frozen = v,
            |t| t.frozen
        )),
        cmd!("track.commit", "Commit...", ["Track"], Some("Alt+Shift+C"), "{tracks?}", has_selection, commit),
        cmd!("track.bounce", "Bounce...", ["Track"], Some("Cmd+Alt+Shift+B"), "{tracks?}", has_selection, commit),
        cmd!("track.bypass_inserts", "All", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, bypass_inserts),
        cmd!("track.mute_sends", "All", ["Track", "Mute Sends"], None, "{tracks?, mute?: bool}", has_selection, mute_sends),
        cmd!("track.set_record_to_input_only", "Set Record Tracks to Input Only", ["Track"], Some("Alt+K"), "{}", always, |e, _| {
            let s = e.session_mut();
            let mut n = 0;
            for t in &mut s.tracks {
                if t.mixer.record_arm {
                    t.mixer.input_monitor = true;
                    n += 1;
                }
            }
            Ok(json!({"tracks": n}))
        }),
        cmd!(noundo "track.scroll_to", "Scroll to Track...", ["Track"], Some("Cmd+Alt+F"), "{track}", has_tracks, |e, p| {
            let t = track_param(e, "track.scroll_to", p, "track")?.ok_or_else(|| bad("track.scroll_to", "`track` required"))?;
            let s = e.session_mut();
            s.edit.selected_tracks = vec![t];
            Ok(json!({"track": t}))
        }),
        cmd!("track.clear_clip_indicators", "Clear All Clip Indicators", ["Track"], Some("Alt+C"), "{}", always, |_, _| Ok(json!({"cleared": true}))),
        cmd!("track.clear_trim_automation", "Clear Trim Automation", ["Track"], None, "{tracks?}", has_selection, |_, _| Ok(json!({}))),
        cmd!("track.create_click", "Create Click Track", ["Track"], None, "{}", always, create_click),
        cmd!("track.playlist_new", "New Playlist", [], None, "{track?, name?}", has_selection, |e, p| new_playlist(e, p, false)),
        cmd!("track.playlist_duplicate", "Duplicate Playlist", [], None, "{track?, name?}", has_selection, |e, p| new_playlist(e, p, true)),
        cmd!("track.playlist_select", "Select Playlist", [], None, "{track?, index | name}", has_selection, select_playlist),
        cmd!("track.playlist_delete_unused", "Delete Unused Playlists", [], None, "{track?}", has_selection, |e, p| {
            let tracks = tracks_required(e, "track.playlist_delete_unused", p)?;
            let s = e.session_mut();
            let mut n = 0;
            for t in tracks {
                if let Some(tr) = s.track_mut(t)
                    && let Some(active) = tr.playlists.get(tr.active_playlist).cloned()
                {
                    n += tr.playlists.len() - 1;
                    tr.playlists = vec![active];
                    tr.active_playlist = 0;
                }
            }
            Ok(json!({"deleted": n}))
        }),
        cmd!(noundo "track.folder_toggle", "Open/Close Folder", [], None, "{track, open?: bool}", has_tracks, |e, p| {
            let t = track_param(e, "track.folder_toggle", p, "track")?.ok_or_else(|| bad("track.folder_toggle", "`track` required"))?;
            let v = p.get("open").and_then(Value::as_bool);
            let s = e.session_mut();
            let tr = s.track_mut(t).ok_or_else(|| bad("track.folder_toggle", "no track"))?;
            tr.folder_open = v.unwrap_or(!tr.folder_open);
            Ok(json!({"open": tr.folder_open}))
        }),
        cmd!("track.vca", "Assign to VCA", [], None, "{tracks?, vca: VCA track id|name | null}", has_selection, |e, p| {
            let tracks = tracks_required(e, "track.vca", p)?;
            let vca = track_param(e, "track.vca", p, "vca")?;
            if let Some(v) = vca
                && e.session().track(v).is_none_or(|t| t.kind != TrackKind::Vca)
            {
                return Err(bad("track.vca", "`vca` must be a VCA Master track"));
            }
            let s = e.session_mut();
            for t in &tracks {
                if let Some(tr) = s.track_mut(*t) {
                    tr.mixer.vca = vca.map(|v| v.0);
                }
            }
            Ok(json!({"vca": vca}))
        }),
        cmd!(
            "track.playlist_promote",
            "Promote to Main Playlist",
            [],
            None,
            "{track, playlist: index, start?, end?} (copy that range from an alternate playlist into the active one)",
            has_tracks,
            |e, p| {
                let t = track_param(e, "track.playlist_promote", p, "track")?.ok_or_else(|| bad("track.playlist_promote", "`track` required"))?;
                let idx =
                    p.get("playlist").and_then(Value::as_u64).ok_or_else(|| bad("track.playlist_promote", "`playlist` index required"))? as usize;
                let r = range_param(e, "track.playlist_promote", p)?;
                if r.is_empty() {
                    return Err(bad("track.playlist_promote", "give `start` and `end`"));
                }
                let s = e.session_mut();
                let (src, active) = {
                    let tr = s.track(t).ok_or_else(|| bad("track.playlist_promote", "no track"))?;
                    (tr.playlists.get(idx).cloned().ok_or_else(|| bad("track.playlist_promote", "no such playlist"))?, tr.active_playlist)
                };
                if idx == active {
                    return Err(bad("track.playlist_promote", "that playlist is already active"));
                }
                // Copy the clips of the source playlist inside the range (via a temporary active switch).
                if let Some(tr) = s.track_mut(t) {
                    tr.active_playlist = idx;
                }
                let clips = crate::edit::copy_range(s, t, r);
                if let Some(tr) = s.track_mut(t) {
                    tr.active_playlist = active;
                }
                let _ = src;
                let ids = crate::edit::paste(s, t, r.start, &clips, r.len(), false);
                Ok(json!({"clips": ids}))
            }
        ),
        cmd!("track.input", "Track Input", [], None, "{tracks?, input: none|bus name|hardware name}", has_selection, |e, p| route(e, p, true)),
        cmd!("track.output", "Track Output", [], None, "{tracks?, output: main|none|bus name}", has_selection, |e, p| route(e, p, false)),
        cmd!("track.timebase", "Track Timebase", [], None, "{tracks?, ticks: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.timebase",
            |t, v| t.ticks_timebase = v,
            |t| t.ticks_timebase
        )),
        cmd!(
            "track.elastic",
            "Elastic Audio",
            [],
            None,
            "{tracks?, algorithm?: polyphonic|rhythmic|monophonic|varispeed|x-form|none}",
            has_selection,
            |e, p| {
                let tracks = tracks_required(e, "track.elastic", p)?;
                let alg = str_param(p, "algorithm").filter(|a| *a != "none").map(str::to_string);
                let s = e.session_mut();
                for t in tracks {
                    if let Some(tr) = s.track_mut(t) {
                        tr.elastic = alg.clone();
                    }
                }
                Ok(json!({"algorithm": alg}))
            }
        ),
    ]
}

fn new_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let count = i64_or(p, "count", 1).clamp(1, 128);
    let kind = match str_param(p, "kind") {
        Some(k) => TrackKind::from_id(k).ok_or_else(|| bad("track.new", format!("unknown kind `{k}`")))?,
        None => TrackKind::Audio,
    };
    let default_fmt = if matches!(kind, TrackKind::Master | TrackKind::Aux | TrackKind::Instrument | TrackKind::Folder) {
        ChannelFormat::Stereo
    } else {
        ChannelFormat::Mono
    };
    let format = match str_param(p, "format") {
        Some(f) => ChannelFormat::from_id(f).ok_or_else(|| bad("track.new", format!("unknown format `{f}`")))?,
        None => default_fmt,
    };
    let name = str_param(p, "name").map(str::to_string);
    let ticks = str_param(p, "timebase").map(|t| t == "ticks");
    let instrument = str_param(p, "instrument").map(str::to_string);
    let s = e.session_mut();
    let mut ids = Vec::new();
    for _ in 0..count {
        let id = s.add_track(kind, format, name.as_deref());
        if let Some(t) = s.track_mut(id) {
            if let Some(tb) = ticks {
                t.ticks_timebase = tb;
            }
            if kind == TrackKind::Instrument {
                t.instrument = Some(soundcraft_model::Insert::new(instrument.clone().unwrap_or_else(|| "subtractive_synth".into())));
            }
        }
        ids.push(id);
    }
    s.edit.selected_tracks = ids.clone();
    Ok(json!({"tracks": ids}))
}

fn group(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.group", p)?;
    let s = e.session_mut();
    let id = GroupId(s.alloc());
    let letter = (b'a'..=b'z').map(char::from).find(|c| !s.groups.iter().any(|g| g.letter == *c)).unwrap_or('z');
    let name = str_param(p, "name").map_or_else(|| format!("Group {}", s.groups.len() + 1), str::to_string);
    let color = s.next_track_color();
    s.groups.push(Group {
        id,
        name,
        letter,
        members: tracks,
        edit: bool_or(p, "edit", true),
        mix: bool_or(p, "mix", true),
        active: true,
        color,
        attributes: vec!["volume".into(), "mute".into(), "solo".into(), "record".into()],
    });
    Ok(json!({"group": id}))
}

fn find_group(e: &Engine, p: &Value, cmd: &str) -> Result<GroupId> {
    let s = e.session();
    let g = match (p.get("group").and_then(Value::as_u64), str_param(p, "group")) {
        (Some(n), _) => s.groups.iter().find(|g| g.id.0 == n),
        (_, Some(name)) => s.groups.iter().find(|g| g.name == name || g.letter.to_string() == name),
        _ => None,
    };
    g.map(|g| g.id).ok_or_else(|| bad(cmd, "unknown `group`"))
}

fn delete_group(e: &mut Engine, p: &Value) -> Result<Value> {
    let g = find_group(e, p, "track.ungroup")?;
    e.session_mut().groups.retain(|x| x.id != g);
    Ok(json!({}))
}

fn toggle_group(e: &mut Engine, p: &Value) -> Result<Value> {
    let g = find_group(e, p, "track.group_toggle")?;
    let s = e.session_mut();
    let mut active = false;
    if let Some(gr) = s.groups.iter_mut().find(|x| x.id == g) {
        gr.active = !gr.active;
        active = gr.active;
    }
    Ok(json!({"active": active}))
}

fn duplicate(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.duplicate", p)?;
    let (pl, auto, ins, snd) =
        (bool_or(p, "playlists", true), bool_or(p, "automation", true), bool_or(p, "inserts", true), bool_or(p, "sends", true));
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(idx) = s.track_index(t) else { continue };
        let Some(mut nt) = s.tracks.get(idx).cloned() else { continue };
        nt.id = soundcraft_model::TrackId(s.alloc());
        nt.name = s.unique_track_name(&format!("{}.dup", nt.name));
        if !pl {
            for p in &mut nt.playlists {
                p.clips.clear();
            }
        } else {
            for p in &mut nt.playlists {
                for c in &mut p.clips {
                    c.id = s.new_clip_id();
                }
            }
        }
        if !auto {
            nt.automation.clear();
        }
        if !ins {
            nt.mixer.inserts.iter_mut().for_each(|i| *i = None);
        }
        if !snd {
            nt.mixer.sends.iter_mut().for_each(|i| *i = None);
        }
        out.push(nt.id);
        s.tracks.insert((idx + 1).min(s.tracks.len()), nt);
    }
    s.edit.selected_tracks = out.clone();
    Ok(json!({"tracks": out}))
}

fn split_mono(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.split_into_mono", p)?;
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(src) = s.track(t).cloned() else { continue };
        if src.channels() < 2 || src.kind != TrackKind::Audio {
            continue;
        }
        for ch in 0..src.channels() {
            let suffix = match (src.channels(), ch) {
                (2, 0) => "L".to_string(),
                (2, 1) => "R".to_string(),
                _ => format!("{}", ch + 1),
            };
            let id = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("{}.{suffix}", src.name)));
            let mut clips = src.clips().to_vec();
            for c in &mut clips {
                c.id = s.new_clip_id();
                // Mono tracks read channel 0 of the source; record the channel in the name for now.
                c.name = format!("{}.{suffix}", c.name);
            }
            if let Some(tr) = s.track_mut(id) {
                tr.mixer.pan = vec![if ch == 0 { -1.0 } else { 1.0 }];
                tr.comments = format!("source_channel={ch}");
                if let Some(pl) = tr.playlist_mut() {
                    pl.clips = clips;
                }
            }
            out.push(id);
        }
    }
    Ok(json!({"tracks": out}))
}

fn flag(
    e: &mut Engine,
    p: &Value,
    cmd: &str,
    set: fn(&mut soundcraft_model::Track, bool),
    get: fn(&soundcraft_model::Track) -> bool,
) -> Result<Value> {
    let tracks = tracks_required(e, cmd, p)?;
    let explicit = p.get("inactive").or_else(|| p.get("hidden")).or_else(|| p.get("value")).or_else(|| p.get("ticks")).and_then(Value::as_bool);
    let s = e.session_mut();
    let target = explicit.unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(get)));
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            set(tr, target);
        }
    }
    Ok(json!({"value": target, "tracks": tracks}))
}

fn move_to_folder(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.move_to_new_folder", p)?;
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    let first_idx = tracks.iter().filter_map(|t| s.track_index(*t)).min().unwrap_or(0);
    let fid = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some(name.as_deref().unwrap_or("Folder")));
    // Place the folder above its first member.
    if let Some(i) = s.track_index(fid) {
        let f = s.tracks.remove(i);
        s.tracks.insert(first_idx.min(s.tracks.len()), f);
    }
    // A routing folder sums its members through its own bus.
    let fname = s.track(fid).map(|x| x.name.clone()).unwrap_or_else(|| "Folder".into());
    let bus = s.add_bus(&format!("{fname} Bus"), ChannelFormat::Stereo);
    if let Some(f) = s.track_mut(fid) {
        f.mixer.input = Route::Bus(bus);
    }
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.folder = Some(fid);
            tr.mixer.output = Route::Bus(bus);
        }
    }
    Ok(json!({"folder": fid, "bus": bus}))
}

fn folder_assign(e: &mut Engine, p: &Value) -> Result<Value> {
    let folder = track_param(e, "track.folder_assign", p, "folder")?.ok_or_else(|| bad("track.folder_assign", "`folder` required"))?;
    let tracks = tracks_required(e, "track.folder_assign", p)?;
    let s = e.session_mut();
    let target = s.track(folder).ok_or_else(|| bad("track.folder_assign", "folder not found"))?;
    if target.kind != TrackKind::Folder {
        return Err(bad("track.folder_assign", "target is not a routing folder"));
    }
    let Route::Bus(bus) = target.mixer.input else {
        return Err(bad("track.folder_assign", "folder has no input bus"));
    };
    for id in &tracks {
        let tr = s.track(*id).ok_or_else(|| bad("track.folder_assign", "source track not found"))?;
        if matches!(tr.kind, TrackKind::Folder | TrackKind::Master | TrackKind::Vca | TrackKind::Video) {
            return Err(bad("track.folder_assign", "only audio, instrument, MIDI and aux tracks can join a bus folder"));
        }
    }
    let mut moved = Vec::new();
    for id in &tracks {
        if let Some(i) = s.track_index(*id) {
            moved.push(s.tracks.remove(i));
        }
    }
    let at = s.tracks.iter().enumerate().filter(|(_, t)| t.id == folder || t.folder == Some(folder))
        .map(|(i, _)| i + 1).max().ok_or_else(|| bad("track.folder_assign", "folder not found"))?;
    for (offset, mut tr) in moved.into_iter().enumerate() {
        tr.folder = Some(folder);
        tr.mixer.output = Route::Bus(bus);
        s.tracks.insert(at + offset, tr);
    }
    Ok(json!({"folder": folder, "tracks": tracks}))
}

fn change_width(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.change_width", p)?;
    let f = str_param(p, "format").and_then(ChannelFormat::from_id).ok_or_else(|| bad("track.change_width", "`format` required"))?;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.format = f;
            tr.mixer.pan = soundcraft_model::Mixer::new(f).pan;
        }
    }
    Ok(json!({"format": f}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.delete", p)?;
    let s = e.session_mut();
    s.tracks.retain(|t| !tracks.contains(&t.id));
    s.edit.selected_tracks.retain(|t| !tracks.contains(t));
    for g in &mut s.groups {
        g.members.retain(|t| !tracks.contains(t));
    }
    for t in &mut s.tracks {
        if t.folder.is_some_and(|f| tracks.contains(&f)) {
            t.folder = None;
        }
    }
    Ok(json!({"deleted": tracks.len()}))
}

fn rename(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.rename", p, "track")?.ok_or_else(|| bad("track.rename", "`track` required"))?;
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("track.rename", "`name` required"))?;
    let s = e.session_mut();
    if s.tracks.iter().any(|x| x.name == name && x.id != t) {
        return Err(bad("track.rename", format!("a track named `{name}` already exists")));
    }
    if let Some(tr) = s.track_mut(t) {
        tr.name = name.chars().take(64).collect();
    }
    Ok(json!({"name": name}))
}

fn comments(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.comments", p, "track")?.ok_or_else(|| bad("track.comments", "`track` required"))?;
    let c = str_param(p, "comments").unwrap_or_default().to_string();
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.comments = c;
    }
    Ok(json!({}))
}

fn color(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.color", p)?;
    let c = match p.get("color") {
        Some(Value::Array(a)) if a.len() == 3 => {
            let v: Vec<u8> = a.iter().map(|x| x.as_u64().unwrap_or(0).min(255) as u8).collect();
            [v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0), v.get(2).copied().unwrap_or(0)]
        }
        Some(v) if v.is_u64() => soundcraft_model::TRACK_COLORS.get(v.as_u64().unwrap_or(0) as usize % 16).copied().unwrap_or([128, 128, 128]),
        _ => return Err(bad("track.color", "`color` must be [r,g,b] or a palette index")),
    };
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.color = c;
        }
    }
    Ok(json!({"color": c}))
}

fn height(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.height", p)?;
    let h = str_param(p, "height").and_then(TrackHeight::from_id).ok_or_else(|| bad("track.height", "unknown height"))?;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.height = h;
        }
    }
    Ok(json!({"height": h}))
}

fn view(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.view", p)?;
    let v = str_param(p, "view").ok_or_else(|| bad("track.view", "`view` required"))?.to_string();
    let ok = matches!(v.as_str(), "waveform" | "blocks" | "notes" | "regions" | "clip_gain" | "playlists")
        || soundcraft_model::AutoParam::parse(&v).is_some();
    if !ok {
        return Err(bad("track.view", format!("unknown view `{v}`")));
    }
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.view = v.clone();
        }
    }
    Ok(json!({"view": v}))
}

fn move_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.move", p, "track")?.ok_or_else(|| bad("track.move", "`track` required"))?;
    let to = i64_or(p, "to", 0).max(0) as usize;
    let s = e.session_mut();
    if let Some(i) = s.track_index(t) {
        let tr = s.tracks.remove(i);
        s.tracks.insert(to.min(s.tracks.len()), tr);
    }
    Ok(json!({}))
}

fn commit(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.commit", p)?;
    let mut out = Vec::new();
    for t in tracks {
        if let Some(id) = crate::io::commit_track(e, t)? {
            out.push(id);
        }
    }
    Ok(json!({"tracks": out}))
}

fn bypass_inserts(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.bypass_inserts", p)?;
    let s = e.session_mut();
    let target = p
        .get("bypass")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(|tr| tr.mixer.inserts.iter().flatten().all(|i| i.bypass))));
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for i in tr.mixer.inserts.iter_mut().flatten() {
                i.bypass = target;
            }
        }
    }
    Ok(json!({"bypass": target}))
}

fn mute_sends(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.mute_sends", p)?;
    let s = e.session_mut();
    let target = p.get("mute").and_then(Value::as_bool).unwrap_or(true);
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for snd in tr.mixer.sends.iter_mut().flatten() {
                snd.mute = target;
            }
        }
    }
    Ok(json!({"mute": target}))
}

fn create_click(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session_mut();
    let id = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Click"));
    if let Some(t) = s.track_mut(id) {
        t.comments = "click".into();
    }
    s.edit.click = true;
    Ok(json!({"track": id}))
}

fn new_playlist(e: &mut Engine, p: &Value, dup: bool) -> Result<Value> {
    let tracks = tracks_required(e, "track.playlist_new", p)?;
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    for t in &tracks {
        let mut clips = Vec::new();
        if dup && let Some(tr) = s.track(*t) {
            clips = tr.clips().to_vec();
        }
        for c in &mut clips {
            c.id = s.new_clip_id();
        }
        if let Some(tr) = s.track_mut(*t) {
            let n = name.clone().unwrap_or_else(|| format!("{}.{:02}", tr.name, tr.playlists.len() + 1));
            tr.playlists.push(Playlist { name: n, clips });
            tr.active_playlist = tr.playlists.len() - 1;
        }
    }
    Ok(json!({"tracks": tracks}))
}

fn select_playlist(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.playlist_select", p)?;
    let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let i = idx.or_else(|| name.as_ref().and_then(|n| tr.playlists.iter().position(|p| &p.name == n)));
            match i {
                Some(i) if i < tr.playlists.len() => tr.active_playlist = i,
                _ => return Err(bad("track.playlist_select", "no such playlist")),
            }
        }
    }
    Ok(json!({}))
}

pub(crate) fn parse_route(s: &soundcraft_model::Session, v: &str) -> Route {
    match v.to_ascii_lowercase().as_str() {
        "" | "none" | "no input" | "no output" => Route::None,
        "main" | "out 1-2" | "master" => Route::Main,
        _ => s.bus_by_name(v).map_or_else(|| Route::Hardware(v.to_string()), |b| Route::Bus(b.id)),
    }
}

fn route(e: &mut Engine, p: &Value, input: bool) -> Result<Value> {
    let key = if input { "input" } else { "output" };
    let tracks = tracks_required(e, "track.route", p)?;
    let v = str_param(p, key).ok_or_else(|| bad("track.route", format!("`{key}` required")))?.to_string();
    let create_bus = bool_or(p, "create_bus", true);
    let s = e.session_mut();
    let mut r = parse_route(s, &v);
    if let Route::Hardware(name) = &r
        && create_bus
        && !name.to_ascii_lowercase().starts_with("in ")
    {
        let fmt = tracks
            .first()
            .and_then(|t| s.track(*t))
            .map_or(ChannelFormat::Stereo, |t| if t.kind == TrackKind::Aux { t.format } else { ChannelFormat::Stereo });
        r = Route::Bus(s.add_bus(&name.clone(), fmt));
    }
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            if input {
                tr.mixer.input = r.clone();
            } else {
                tr.mixer.output = r.clone();
            }
        }
    }
    Ok(json!({key: r}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comp_from_alternate_playlist() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").map(|t| t.id.0).unwrap();
        // Duplicate the playlist, then clear a range on the new active playlist and promote it back from the first.
        e.execute("track.playlist_duplicate", &json!({"track": kick})).unwrap();
        e.execute("edit.clear", &json!({"tracks": [kick], "start": {"seconds": 10.0}, "end": {"seconds": 12.0}})).unwrap();
        let gap = |e: &Engine| {
            e.session().track(soundcraft_model::TrackId(kick)).unwrap().playlist().unwrap().clip_at(e.session().sample_rate.samples(11.0)).is_none()
        };
        assert!(gap(&e));
        e.execute("track.playlist_promote", &json!({"track": kick, "playlist": 0, "start": {"seconds": 10.0}, "end": {"seconds": 12.0}})).unwrap();
        assert!(!gap(&e));
        assert!(e.execute("track.playlist_promote", &json!({"track": kick, "playlist": 1, "start": 0, "end": 10})).is_err());
    }

    #[test]
    fn folder_routes_members_through_its_bus() {
        let mut e = crate::demo::demo_engine();
        let r = e.execute("track.move_to_new_folder", &json!({"tracks": ["Kick", "Snare"], "name": "Drums"})).unwrap();
        assert!(r["bus"].is_u64());
        let s = e.session();
        let kick = s.track_by_name("Kick").unwrap();
        assert!(matches!(kick.mixer.output, Route::Bus(_)));
        assert!(s.tracks.iter().any(|t| t.kind == TrackKind::Folder && t.mixer.input == kick.mixer.output));
    }

    #[test]
    fn bus_folder_accepts_track_and_routes_through_its_fader() {
        let mut e = crate::demo::demo_engine();
        let r = e.execute("track.bus_folder", &json!({"tracks": ["Kick", "Snare"], "name": "Drums"})).unwrap();
        let fid = r["folder"].as_u64().unwrap();
        e.execute("track.folder_assign", &json!({"tracks": ["Bass"], "folder": fid})).unwrap();
        let s = e.session();
        let folder = s.track(soundcraft_model::TrackId(fid)).unwrap();
        let bass = s.track_by_name("Bass").unwrap();
        assert_eq!(bass.folder, Some(folder.id));
        assert_eq!(bass.mixer.output, folder.mixer.input);
        assert_eq!(folder.mixer.output, Route::Main);
        assert!(e.execute("mix.volume", &json!({"track": fid, "db": -9.0})).is_ok());
        assert_eq!(e.session().track(folder.id).unwrap().mixer.volume_db, -9.0);
    }
}
