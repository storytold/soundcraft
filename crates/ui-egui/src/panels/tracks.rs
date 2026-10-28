use super::{
    Align2, Color32, Rect, Sense, SoundApp, Stroke, Tokens, Ui, bold, gutter_scroll, json, panel_header, pos2, regular, rgb, vec2,
    with_pending_offset,
};

/// Drag a Tracks list row to reorder the tracks: a line marks the gap it will land in.
fn drag_track(app: &mut SoundApp, ui: &Ui, row: Rect, id: u64, dropped: bool) {
    let s = app.engine.session();
    let (Some(from), Some(p)) = (s.track_index(soundcraft_model::TrackId(id)), ui.ctx().pointer_latest_pos()) else { return };
    let spacing = ui.spacing().item_spacing.y;
    let pitch = row.height() + spacing;
    let gap = (from as f32 + ((p.y - row.center().y) / pitch).floor() + 1.0).clamp(0.0, s.tracks.len() as f32);
    if dropped {
        let to = gap as usize;
        let to = if to > from { to - 1 } else { to };
        if to != from {
            let _ = app.run("track.move", json!({"track": id, "to": to}));
        }
    } else {
        let y = row.min.y + (gap - from as f32) * pitch - spacing / 2.0;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("track_list_reorder")))
            .line_segment([pos2(row.min.x, y), pos2(row.max.x, y)], Stroke::new(2.0, Tokens::current().accent));
    }
}

/// The strip down the Tracks panel's right edge: the lists' scroll bars, and a clear line between
/// the panel and the window beside it.
const GUTTER_W: f32 = 10.0;

/// Tracks and Groups panels: headers, track and group lists, and their scrollbars in the panel gutter.
pub fn tracks_and_groups(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let gutter = full.with_min_x(full.max.x - GUTTER_W);
    ui.painter().rect_filled(gutter, 0.0, t.panel_bg.lerp_to_gamma(t.text_dim, 0.15));
    ui.painter().vline(gutter.min.x + 0.5, full.y_range(), Stroke::new(1.0, t.border));
    ui.painter().vline(gutter.max.x - 0.5, full.y_range(), Stroke::new(1.0, t.border));
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(full.with_max_x(gutter.min.x)));
    let total = ui.available_height();
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 22.0));
    panel_header(ui, "TRACKS");
    let plus = Rect::from_center_size(pos2(header.max.x - 13.0, header.center().y), vec2(18.0, 18.0));
    let tip = format!("New Tracks… ({})", crate::shortcuts::shortcut_label("Cmd+Shift+N", ui.ctx().os().is_mac()));
    if crate::widgets::icon_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(plus)), plus.size(), "plus", false, &tip).clicked() {
        let _ = app.dialogs.open_for_command(&app.engine, "track.new");
    }
    let tracks: Vec<(u64, String, bool, [u8; 3], bool)> = {
        let s = app.engine.session();
        s.tracks.iter().map(|x| (x.id.0, x.name.clone(), !x.hidden, x.color, s.edit.selected_tracks.contains(&x.id))).collect()
    };
    let hidden = egui::scroll_area::ScrollBarVisibility::AlwaysHidden;
    let lists = with_pending_offset(egui::ScrollArea::vertical().id_salt("tracks_scroll"), ui, "tracks_scroll")
        .max_height(total * 0.62)
        .auto_shrink([false, false])
        .scroll_bar_visibility(hidden);
    let out = lists.show(ui, |ui| {
        for (id, name, shown, color, sel) in tracks {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, sel, &name));
            if sel {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            }
            let dot = Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(8.0, 8.0));
            let dresp = ui.interact(dot.expand(3.0), ui.scope_id().with(("vis", id)), Sense::click());
            ui.painter().circle(dot.center(), 3.5, if shown { t.visible_dot } else { Color32::TRANSPARENT }, Stroke::new(1.0, t.text_dim));
            if dresp.clicked() {
                let _ = app.run("track.hide", json!({"tracks": [id], "hidden": shown}));
            }
            ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 20.0, r.min.y + 3.0), vec2(4.0, 11.0)), 0.0, rgb(color));
            ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 30.0, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                regular(11.5),
                if shown { t.text } else { t.text_dim },
            );
            if resp.clicked() {
                crate::edit_window::click_track(app, soundcraft_model::TrackId(id), ui.input(|i| i.modifiers), |_, _| true);
            }
            if resp.dragged() || resp.drag_stopped() {
                drag_track(app, ui, r, id, resp.drag_stopped());
            }
        }
    });
    gutter_scroll(ui, gutter, &out);
    ui.add_space(4.0);
    panel_header(ui, "GROUPS");
    let groups: Vec<(u64, char, String, bool, [u8; 3])> =
        app.engine.session().groups.iter().map(|g| (g.id.0, g.letter, g.name.clone(), g.active, g.color)).collect();
    let groups_area = with_pending_offset(egui::ScrollArea::vertical().id_salt("groups_scroll"), ui, "groups_scroll");
    let out = groups_area.auto_shrink([false, false]).scroll_bar_visibility(hidden).show(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, "<ALL>", regular(11.5).clone(), t.text);
        let all = ui.interact(r, ui.scope_id().with("group_all"), Sense::click());
        all.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "<ALL>"));
        if all.double_clicked() {
            let _ = app.run("edit.select_all", json!({}));
        }
        all.context_menu(|ui| {
            if ui.button("New Group…").clicked() {
                let _ = app.dialogs.open_for_command(&app.engine, "track.group");
                ui.close();
            }
        });
        for (id, letter, name, active, color) in groups {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, active, &name));
            if active {
                ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 2.0), vec2(14.0, 13.0)), 1.0, rgb(color));
            }
            ui.painter().text(
                pos2(r.min.x + 11.0, r.center().y),
                Align2::CENTER_CENTER,
                letter.to_string(),
                bold(10.0),
                if active { t.text_dark } else { t.text_dim },
            );
            ui.painter().with_clip_rect(r).text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, &name, regular(11.5), t.text);
            if resp.clicked() {
                let _ = app.run("track.group_toggle", json!({"group": id}));
            }
            let members: Vec<u64> =
                app.engine.session().groups.iter().find(|g| g.id.0 == id).map(|g| g.members.iter().map(|m| m.0).collect()).unwrap_or_default();
            if resp.double_clicked() {
                let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
            }
            resp.context_menu(|ui| {
                if ui.button("Select Tracks in Group").clicked() {
                    let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
                    ui.close();
                }
                if ui.button(if active { "Disable" } else { "Enable" }).clicked() {
                    let _ = app.run("track.group_toggle", json!({"group": id}));
                    ui.close();
                }
                if ui.button("Modify Group…").clicked() {
                    let _ = app.dialogs.open_modify_group(&app.engine, soundcraft_model::GroupId(id));
                    ui.close();
                }
                if ui.button("Delete Group").clicked() {
                    let _ = app.run("track.ungroup", json!({"group": id}));
                    ui.close();
                }
            });
        }
    });
    gutter_scroll(ui, gutter, &out);
}
