use super::automation::draw_automation;
use super::clips::{ClipLane, draw_clip, draw_live_take, draw_strip_silence, grid_lines};
use super::header::{header_column, track_header};
use super::input::lane_interaction;
use super::layout::{Column, HEADER_W, LANE_H, STRIP_W, lane_rect, sample_at, x_of};
use super::{
    Align2, AutoParam, Range, Rect, Sense, Session, SoundApp, Stroke, Tokens, Track, TrackHeight, TrackId, TrackKind, Ui, json, ops, pos2, regular,
    rgb, vec2,
};

pub(super) fn in_edit_window(s: &Session, t: &Track) -> bool {
    ops::shown(s, t)
}

/// Pro Tools' rule for track height: Alt reaches every shown track, Alt+Shift the selected ones too.
pub(crate) fn height_targets(s: &Session, m: egui::Modifiers, own: &[TrackId]) -> Vec<u64> {
    let extra = |t: &Track| if m.shift { s.edit.selected_tracks.contains(&t.id) } else { in_edit_window(s, t) };
    s.tracks.iter().filter(|t| own.contains(&t.id) || (m.alt && extra(t))).map(|t| t.id.0).collect()
}

/// How many folders this track sits in, for the name's indent (at most 6 shown).
pub(super) fn folder_depth(s: &Session, track: &Track) -> usize {
    let mut current = track.folder;
    let mut depth = 0;
    while let Some(folder) = current.and_then(|folder| s.track(folder))
        && depth < 6
    {
        depth += 1;
        current = folder.folder;
    }
    depth
}

/// Pro Tools' track-name click: Shift adds the tracks up to the nearest selected one, Cmd/Ctrl adds or removes one.
pub(crate) fn click_track(app: &mut SoundApp, id: TrackId, mods: egui::Modifiers, shown: impl Fn(&Session, &Track) -> bool) {
    let s = app.engine.session();
    let order: Vec<TrackId> = s.tracks.iter().filter(|t| shown(s, t)).map(|t| t.id).collect();
    let row = |t: &TrackId| order.iter().position(|o| o == t);
    let mut tracks = s.edit.selected_tracks.clone();
    if mods.command {
        if tracks.contains(&id) {
            tracks.retain(|t| *t != id);
        } else {
            tracks.push(id);
        }
    } else if mods.shift
        && let Some(a) = row(&id)
        && let Some(b) = tracks.iter().filter_map(row).min_by_key(|b| b.abs_diff(a))
    {
        let span = order.get(a.min(b)..=a.max(b)).unwrap_or_default();
        tracks.extend(span.iter().filter(|t| !tracks.contains(t)).copied().collect::<Vec<_>>());
    } else {
        tracks = vec![id];
    }
    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
    let _ = app.run("edit.select", json!({"tracks": ids}));
}

// ---- track rows ------------------------------------------------------------------------------

pub(super) fn track_row(app: &mut SoundApp, ui: &mut Ui, id: TrackId, row: Rect, tl: Rect, cols: &[Column]) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let Some(track) = s.track(id) else { return };
    let selected = s.edit.selected_tracks.contains(&id);
    let head = Rect::from_min_max(row.min, pos2(row.min.x + HEADER_W, row.max.y));
    let (head_bg, lane_bg) = crate::theme::track_backgrounds(track.color, selected, app.ui.track_tint);
    let mut x = head.max.x;
    for &(col, _, _, w) in cols {
        header_column(app, ui, track, col, Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height())), head_bg);
        x += w;
    }
    let main_h = track.height.points();
    let lane = lane_rect(track, row, tl);
    ui.painter().rect_filled(head, 0.0, head_bg);
    ui.painter().rect_filled(Rect::from_min_size(head.min, vec2(STRIP_W, head.height())), 0.0, rgb(track.color));
    ui.painter().line_segment([pos2(head.min.x, row.max.y + 0.5), pos2(tl.max.x, row.max.y + 0.5)], Stroke::new(1.0, t.border));
    ui.painter().hline(head.min.x + STRIP_W..=head.max.x, head.min.y + 0.5, Stroke::new(1.0, t.edge_light));
    track_header(app, ui, track, head, selected, head_bg);
    // Lane.
    ui.painter().vline(tl.min.x - 0.5, row.y_range(), Stroke::new(1.0, t.border));
    let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
    painter.rect_filled(lane, 0.0, lane_bg);
    let s = app.engine.session_arc();
    // Shown grid lines go over the clips; otherwise the bar lines stay behind them.
    if !s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let auto_view = AutoParam::parse(&track.view);
    let sorted = track.clips().is_sorted_by_key(|c| c.start);
    for clip in track.clips() {
        if !clip.range().overlaps(&view) {
            continue;
        }
        draw_clip(&ClipLane { app, painter: &painter, s: &s, track, sorted, tl, lane }, clip, auto_view.is_some());
    }
    draw_live_take(app, &painter, &s, track, tl, lane);
    crate::video_track::draw_lane(app, &painter, &s, track, tl, lane);
    if s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    if let Some(p) = &auto_view {
        draw_automation(&painter, &s, track, p, tl, lane);
    }
    if track.kind == TrackKind::Master || track.kind == TrackKind::Aux || track.kind == TrackKind::Vca {
        let p = auto_view.clone().unwrap_or(AutoParam::Volume);
        draw_automation(&painter, &s, track, &p, tl, lane);
    }
    // Selection highlight on this track.
    let sel = s.edit.selection;
    if selected && !sel.is_empty() {
        let x0 = x_of(&s, tl, sel.start).max(lane.min.x);
        let x1 = x_of(&s, tl, sel.end).min(lane.max.x);
        if x1 > x0 {
            painter.rect_filled(Rect::from_min_max(pos2(x0, lane.min.y), pos2(x1, lane.max.y)), 0.0, t.selection);
        }
    }
    let strip = crate::dialogs::strip_silence_rectangles(app, ui.ctx());
    if let Some((_, ranges)) = strip.iter().find(|(t, _)| *t == id) {
        draw_strip_silence(&painter, &s, tl, lane, ranges);
    }
    lane_interaction(app, ui, track, lane, tl);
    if track.view == "playlists" {
        playlist_lanes(app, ui, track, row, main_h, tl);
    }
}

/// Alternate playlists as lanes under the main one; clicking a clip promotes it (comping).
fn playlist_lanes(app: &mut SoundApp, ui: &mut Ui, track: &Track, row: Rect, main_h: f32, tl: Rect) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let mut y = row.min.y + main_h;
    for (i, pl) in track.playlists.iter().enumerate() {
        if i == track.active_playlist {
            continue;
        }
        let lane = Rect::from_min_max(pos2(tl.min.x, y), pos2(tl.max.x, y + LANE_H));
        let head = Rect::from_min_max(pos2(row.min.x + 12.0, y), pos2(tl.min.x, y + LANE_H));
        ui.painter().rect_filled(lane, 0.0, t.playlist_lane);
        ui.painter().text(pos2(head.min.x + 4.0, head.center().y), Align2::LEFT_CENTER, &pl.name, regular(10.5), t.text_dim);
        let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
        let sorted = track.clips().is_sorted_by_key(|c| c.start);
        let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
        for c in pl.clips.iter().filter(|c| c.range().overlaps(&view)) {
            draw_clip(&ClipLane { app, painter: &painter, s: &s, track, sorted, tl, lane }, c, false);
        }
        let resp = ui.interact(lane, ui.scope_id().with(("pl_lane", track.id.0, i)), Sense::click());
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let at = sample_at(&s, tl, p.x);
            let sel = s.edit.selection;
            let range = if !sel.is_empty() && sel.contains(at) {
                Some((sel.start, sel.end))
            } else {
                pl.clips.iter().find(|c| c.range().contains(at)).map(|c| (c.start, c.end()))
            };
            if let Some((a, b)) = range {
                let _ = app.run("track.playlist_promote", json!({"track": track.id.0, "playlist": i, "start": a, "end": b}));
            }
        }
        resp.on_hover_text("Click a take (or the selection) to promote it to the main playlist");
        y += LANE_H;
    }
}

pub(super) fn view_label(v: &str) -> String {
    match AutoParam::parse(v) {
        Some(p) => p.label(),
        None => v.to_string(),
    }
}

pub(super) fn view_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track) {
    let id = track.id.0;
    let mut opts: Vec<&str> = Vec::new();
    if track.kind.is_midi() {
        opts.extend(["notes", "regions", "blocks"]);
    } else if track.kind.has_playlist() {
        opts.extend(["waveform", "blocks", "clip_gain", "playlists"]);
    }
    opts.extend(["volume", "pan", "mute"]);
    for o in opts {
        if ui.selectable_label(track.view == o, view_label(o)).clicked() {
            let _ = app.run("track.view", json!({"track": id, "view": o}));
        }
    }
    for (slot, ins) in track.mixer.inserts.iter().enumerate() {
        let Some(ins) = ins else { continue };
        let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin) else { continue };
        ui.menu_button(info.name, |ui| {
            for p in info.params {
                let v = format!("plugin:{}:{}", (b'a' + slot as u8) as char, p.id);
                if ui.button(p.name).clicked() {
                    let _ = app.run("track.view", json!({"track": id, "view": v}));
                }
            }
        });
    }
}

pub(crate) fn track_context_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let tid = id.0;
    let items: [(&str, &str); 10] = [
        ("Rename...", "ui.rename_track"),
        ("Duplicate...", "track.duplicate"),
        ("Make Inactive", "track.make_inactive"),
        ("Hide", "track.hide"),
        ("Delete", "track.delete"),
        ("Split into Mono", "track.split_into_mono"),
        ("Commit...", "track.commit"),
        ("Freeze", "track.freeze"),
        ("Move to New Folder...", "track.move_to_new_folder"),
        ("Group...", "track.group"),
    ];
    let selection: Vec<u64> = {
        let selected = &app.engine.session().edit.selected_tracks;
        if selected.contains(&id) { selected.iter().map(|track| track.0).collect() } else { vec![tid] }
    };
    for (label, cmd) in items {
        if ui.button(label).clicked() {
            if cmd == "ui.rename_track" {
                let name = app.engine.session().track(id).map(|t| t.name.clone()).unwrap_or_default();
                app.dialogs.open_rename_track(id, &name);
            } else {
                let _ = app.run(cmd, json!({"tracks": selection}));
            }
        }
    }
    let folders: Vec<(u64, String)> = app
        .engine
        .session()
        .tracks
        .iter()
        .filter(|track| track.is_folder() && !selection.contains(&track.id.0))
        .map(|track| (track.id.0, track.name.clone()))
        .collect();
    if !folders.is_empty() {
        ui.menu_button("Move to Folder", |ui| {
            for (folder, name) in &folders {
                if ui.button(name).clicked() {
                    let _ = app.run("track.move_to_folder", json!({"tracks": selection, "folder": folder}));
                }
            }
        });
    }
    if app.engine.session().track(id).is_some_and(|track| track.folder.is_some()) && ui.button("Remove from Folder").clicked() {
        let _ = app.run("track.remove_from_folder", json!({"tracks": selection}));
    }
    // Only the track clicked: the command has one target, and the selection stays as it is.
    if ui.button("Scroll Into View").clicked() {
        let _ = app.run("view.scroll_into_view", json!({"track": id.0}));
    }
    ui.separator();
    ui.menu_button("Track Height", |ui| {
        for h in TrackHeight::ALL {
            if ui.button(h.label()).clicked() {
                let own: Vec<TrackId> = selection.iter().map(|&t| TrackId(t)).collect();
                let tracks = height_targets(app.engine.session(), ui.input(|i| i.modifiers), &own);
                let _ = app.run("track.height", json!({"tracks": tracks, "height": h.label()}));
            }
        }
    });
    ui.menu_button("Track Color", |ui| {
        for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 14.0), Sense::click());
            ui.painter().rect_filled(r, 2.0, rgb(*c));
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": selection, "color": i}));
            }
        }
    });
}
