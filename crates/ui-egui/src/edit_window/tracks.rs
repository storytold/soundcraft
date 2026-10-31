use super::automation::draw_automation;
use super::clips::{ClipLane, clip_name_height, draw_clip, draw_live_take, draw_strip_silence, grid_lines};
use super::header::{header_column, track_header};
use super::input::lane_interaction;
use super::layout::{Column, HEADER_W, LANE_H, STRIP_W, lane_rect, sample_at, x_of};
use super::{
    Align2, AutoParam, ClipContent, Pos2, Range, Rect, Samples, Sense, Session, SoundApp, Stroke, Tokens, Track, TrackHeight, TrackId, TrackKind, Ui,
    json, ops, pos2, regular, rgb, vec2,
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
    let auto_view = AutoParam::parse(&track.view)
        .or_else(|| matches!(track.kind, TrackKind::Master | TrackKind::Aux | TrackKind::Vca).then_some(AutoParam::Volume));
    let sorted = track.clips().is_sorted_by_key(|c| c.start);
    if let Some(point) = ui.ctx().pointer_hover_pos().filter(|point| cursor_is_in_lane(*point, lane, tl)) {
        let at = sample_at(&s, tl, point.x).max(0);
        app.cursor_readout = Some((at, cursor_value(&s, track, auto_view.as_ref(), at, point.y, lane)));
    }
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

/// True when a pointer is both over the lane and inside the timeline viewport.
fn cursor_is_in_lane(point: Pos2, lane: Rect, timeline: Rect) -> bool {
    lane.contains(point) && timeline.contains(point)
}

/// Formats the value beneath the edit cursor for the current track lane.
fn cursor_value(s: &Session, track: &Track, automation: Option<&AutoParam>, at: Samples, pointer_y: f32, lane_rect: Rect) -> String {
    if let Some(param) = automation {
        let default = automation_base(track, param);
        let value = track.lane(param).map_or(default, |lane| lane.value_at(at, default));
        return match param {
            AutoParam::Volume | AutoParam::SendLevel(_) => format!("{value:.1} dB"),
            AutoParam::Mute | AutoParam::SendMute(_) => {
                if value >= 0.5 {
                    "muted".into()
                } else {
                    "open".into()
                }
            }
            _ => format!("{value:.3}"),
        };
    }
    if track.kind == TrackKind::Audio {
        if !track.clips().iter().any(|clip| clip.is_audio() && clip.range().contains(at)) {
            return "—".into();
        }
        let body_top = lane_rect.min.y + 1.0 + clip_name_height(s, lane_rect);
        let body_bottom = lane_rect.max.y - 1.0;
        let amplitude = cursor_audio_amplitude(pointer_y, body_top, body_bottom, track.channels(), s.edit.zoom.waveform_zoom);
        return match s.bit_depth {
            soundcraft_model::BitDepthSetting::Int16 => format!("{:.0}", amplitude * f32::from(i16::MAX)),
            soundcraft_model::BitDepthSetting::Int24 => format!("{:.0}", amplitude * 8_388_607.0),
            soundcraft_model::BitDepthSetting::Float32 => format!("{amplitude:.6}"),
        };
    }
    let Some(clip) = track.clips().iter().rev().find(|clip| clip.range().contains(at)) else { return "—".into() };
    match &clip.content {
        ClipContent::Audio { .. } => "—".into(),
        ClipContent::Midi { sequence } => {
            let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
            let tick = s.tempo.samples_to_ticks(at, s.sample_rate).saturating_sub(base);
            let name_h = clip_name_height(s, lane_rect);
            let body = Rect::from_min_max(pos2(lane_rect.min.x, lane_rect.min.y + 1.0 + name_h), pos2(lane_rect.max.x, lane_rect.max.y - 1.0));
            let (lo, hi) = sequence.notes.iter().fold((127u8, 0u8), |(lo, hi), note| (lo.min(note.pitch), hi.max(note.pitch)));
            let (lo, hi) = if lo > hi { (48, 72) } else { (lo.saturating_sub(2), hi.saturating_add(2)) };
            let span = f32::from(hi.saturating_sub(lo).max(1));
            sequence
                .notes
                .iter()
                .rev()
                .find(|note| {
                    let y = body.max.y - 3.0 - (f32::from(note.pitch.saturating_sub(lo)) / span) * (body.height() - 6.0);
                    let height = ((body.height() - 6.0) / span).clamp(1.5, 6.0);
                    note.start <= tick && note.start.saturating_add(note.length) > tick && (pointer_y - y).abs() <= height * 0.5
                })
                .map_or_else(|| "—".into(), |note| midi_note_name(note.pitch))
        }
        ClipContent::Video { .. } => "picture".into(),
    }
}

/// Maps a vertical pointer position to the audio amplitude shown by the waveform.
fn cursor_audio_amplitude(pointer_y: f32, body_top: f32, body_bottom: f32, channels: usize, waveform_zoom: f32) -> f32 {
    let channels = channels.max(1);
    let channel_height = (body_bottom - body_top).max(1.0) / channels as f32;
    let channel = ((pointer_y - body_top) / channel_height).floor().clamp(0.0, channels.saturating_sub(1) as f32);
    let center = body_top + (channel + 0.5) * channel_height;
    ((center - pointer_y) / (channel_height * 0.46) / waveform_zoom.max(f32::EPSILON)).clamp(-1.0, 1.0)
}

/// Returns the static mixer value used when an automation lane has no point.
fn automation_base(track: &Track, param: &AutoParam) -> f32 {
    match param {
        AutoParam::Volume => track.mixer.volume_db,
        AutoParam::Pan(index) => track.mixer.pan.get(usize::from(*index)).copied().unwrap_or(0.0),
        AutoParam::Mute => {
            if track.mixer.mute {
                1.0
            } else {
                0.0
            }
        }
        AutoParam::SendLevel(index) => track.mixer.sends.get(usize::from(*index)).and_then(Option::as_ref).map_or(-144.0, |send| send.level_db),
        AutoParam::SendPan(index) => track.mixer.sends.get(usize::from(*index)).and_then(Option::as_ref).map_or(0.0, |send| send.pan),
        AutoParam::SendMute(index) => {
            track.mixer.sends.get(usize::from(*index)).and_then(Option::as_ref).map_or(0.0, |send| if send.mute { 1.0 } else { 0.0 })
        }
        AutoParam::Plugin { slot, param } => track
            .mixer
            .inserts
            .get(usize::from(*slot))
            .and_then(Option::as_ref)
            .and_then(|insert| {
                insert
                    .params
                    .get(param)
                    .copied()
                    .or_else(|| soundcraft_dsp::plugin_info(&insert.plugin).and_then(|info| info.param(param)).map(|info| info.default))
            })
            .unwrap_or(0.0),
    }
}

/// Formats a MIDI pitch as a note name and octave.
fn midi_note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
    let name = NAMES.get(usize::from(pitch % 12)).copied().unwrap_or("C");
    format!("{name}{} ({pitch})", i16::from(pitch) / 12 - 1)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipped row cannot claim the ruler or header area outside the timeline.
    #[test]
    fn cursor_readout_requires_the_timeline_and_lane_intersection() {
        let lane = Rect::from_min_max(pos2(0.0, 20.0), pos2(300.0, 80.0));
        let timeline = Rect::from_min_max(pos2(100.0, 0.0), pos2(300.0, 200.0));
        assert!(!cursor_is_in_lane(pos2(50.0, 40.0), lane, timeline));
        assert!(cursor_is_in_lane(pos2(150.0, 40.0), lane, timeline));
        assert!(!cursor_is_in_lane(pos2(150.0, 10.0), lane, timeline));
    }

    /// Stereo waveform values use the center of the channel beneath the pointer.
    #[test]
    fn audio_cursor_value_follows_the_vertical_position_in_each_channel() {
        assert_eq!(cursor_audio_amplitude(35.0, 10.0, 110.0, 2, 1.0), 0.0);
        assert_eq!(cursor_audio_amplitude(85.0, 10.0, 110.0, 2, 1.0), 0.0);
        assert_eq!(cursor_audio_amplitude(12.0, 10.0, 110.0, 2, 1.0), 1.0);
        assert_eq!(cursor_audio_amplitude(58.0, 10.0, 110.0, 2, 1.0), -1.0);
        assert!((cursor_audio_amplitude(23.5, 10.0, 110.0, 2, 2.0) - 0.25).abs() < f32::EPSILON);
    }
}
