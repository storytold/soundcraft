use super::{
    Align2, Color32, Rect, Sense, SoundApp, Stroke, Tokens, Ui, bold, gutter_scroll, json, panel_header, pos2, regular, rgb, vec2,
    with_pending_offset,
};
use crate::theme::PANEL_GAP;
use soundcraft_model::{GroupId, TrackKind};

const TRACK_ROW_H: f32 = 12.0;
const VISIBILITY_W: f32 = 28.0;
const COLOR_W: f32 = 14.0;

struct TrackRow {
    id: u64,
    name: String,
    shown: bool,
    color: [u8; 3],
    selected: bool,
    kind: TrackKind,
    inactive: bool,
    depth: usize,
}

const GROUP_ROW_H: f32 = 13.0;
const GROUP_GUTTER_W: f32 = 14.0;
const GROUP_COLUMNS: [f32; 3] = [18.0, 29.0, 44.0];
const ALL_GROUP_COLOR: [u8; 3] = [109, 60, 222];

struct GroupRow {
    id: Option<GroupId>,
    letter: char,
    name: String,
    active: bool,
    color: [u8; 3],
}

fn paint_group_row(ui: &Ui, r: Rect, group: &GroupRow, t: Tokens) {
    let ink = if group.id.is_none() || group.active { t.text } else { t.text_dim };
    crate::icons::draw(
        ui.painter(),
        Rect::from_center_size(pos2(r.min.x + 9.0, r.center().y), vec2(6.0, 6.0)),
        if group.active { "group_active" } else { "group_inactive" },
        t.text_dim,
    );
    let swatch = Rect::from_center_size(pos2(r.min.x + 23.5, r.center().y), vec2(9.0, 10.0));
    ui.painter().rect_filled(swatch, 0.0, rgb(group.color));
    ui.painter().rect_stroke(swatch, 0.0, Stroke::new(0.75, t.group_border), egui::StrokeKind::Inside);
    ui.painter().text(pos2(r.min.x + 31.0, r.center().y), Align2::LEFT_CENTER, group.letter.to_string(), regular(10.5), ink);
    let format = egui::TextFormat {
        font_id: if group.id.is_none() { bold(11.5) } else { regular(11.5) },
        color: ink,
        italics: group.id.is_none(),
        ..Default::default()
    };
    let galley = ui.painter().layout_job(egui::text::LayoutJob::single_section(group.name.clone(), format));
    ui.painter().with_clip_rect(r).galley(pos2(r.min.x + 47.0, r.center().y - galley.size().y / 2.0), galley, ink);
}

fn track_list_face(r: Rect, fill: Color32, t: Tokens) -> egui::Shape {
    let rule = Stroke::new(0.75, fill.lerp_to_gamma(t.group_border, 0.45));
    egui::Shape::Vec(vec![
        egui::Shape::rect_filled(r, 0.0, fill),
        egui::Shape::line_segment([pos2(r.min.x + VISIBILITY_W, r.min.y), pos2(r.min.x + VISIBILITY_W, r.max.y)], rule),
        egui::Shape::line_segment([pos2(r.min.x + VISIBILITY_W + COLOR_W, r.min.y), pos2(r.min.x + VISIBILITY_W + COLOR_W, r.max.y)], rule),
    ])
}

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
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(full.with_max_x(gutter.min.x)));
    let total = ui.available_height();
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 22.0));
    panel_header(ui, "TRACKS");
    let plus = Rect::from_center_size(pos2(header.max.x - 13.0, header.center().y), vec2(18.0, 18.0));
    let tip = format!("New Tracks… ({})", crate::shortcuts::shortcut_label("Cmd+Shift+N", ui.ctx().os().is_mac()));
    if crate::widgets::icon_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(plus)), plus.size(), "plus", false, &tip).clicked() {
        let _ = app.dialogs.open_for_command(&app.engine, "track.new");
    }
    let tracks: Vec<TrackRow> = {
        let s = app.engine.session();
        s.tracks
            .iter()
            .map(|x| {
                let mut current = x.folder;
                let mut depth = 0;
                let mut seen = Vec::new();
                while let Some(folder_id) = current {
                    if seen.contains(&folder_id) {
                        break;
                    }
                    seen.push(folder_id);
                    let Some(folder) = s.track(folder_id) else {
                        break;
                    };
                    depth += 1;
                    current = folder.folder;
                }
                TrackRow {
                    id: x.id.0,
                    name: x.name.clone(),
                    shown: !x.hidden,
                    color: x.color,
                    selected: s.edit.selected_tracks.contains(&x.id),
                    kind: x.kind,
                    inactive: x.inactive,
                    depth,
                }
            })
            .collect()
    };
    let hidden = egui::scroll_area::ScrollBarVisibility::AlwaysHidden;
    let lists = with_pending_offset(egui::ScrollArea::vertical().id_salt("tracks_scroll"), ui, "tracks_scroll")
        .max_height(total * 0.62)
        .auto_shrink([false, false])
        .scroll_bar_visibility(hidden);
    let background = ui.painter().add(egui::Shape::Noop);
    let out = lists.show(ui, |ui| {
        // Contiguous rows: the same pitch is used by selection and drag-to-reorder.
        ui.spacing_mut().item_spacing.y = 0.0;
        for TrackRow { id, name, shown, color, selected: sel, kind, inactive, depth } in tracks {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), TRACK_ROW_H), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, sel, &name));
            if sel {
                ui.painter().add(track_list_face(r, t.track_list_selected, t));
                ui.painter().rect_stroke(r, 0.0, Stroke::new(0.5, t.track_list_selected.lerp_to_gamma(t.text, 0.2)), egui::StrokeKind::Inside);
            }
            if resp.has_focus() {
                ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            }
            let dot = Rect::from_center_size(pos2(r.min.x + VISIBILITY_W / 2.0, r.center().y), vec2(7.0, 7.0));
            let dresp = ui.interact(dot.expand(2.0), ui.scope_id().with(("vis", id)), Sense::click());
            dresp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, shown, format!("Show {name}")));
            let dot_ink = t.visible_dot.lerp_to_gamma(t.text_dim, 0.6);
            ui.painter().circle(dot.center(), 3.0, if shown { dot_ink } else { Color32::TRANSPARENT }, Stroke::new(0.5, dot_ink));
            if dresp.has_focus() {
                ui.painter().rect_stroke(dot.expand(1.5), 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            }
            if dresp.clicked() {
                let _ = app.run("track.hide", json!({"tracks": [id], "hidden": shown}));
            }
            dresp.on_hover_text(if shown { "Hide track" } else { "Show track" });
            let swatch = Rect::from_center_size(pos2(r.min.x + VISIBILITY_W + COLOR_W / 2.0, r.center().y), vec2(9.0, 10.0));
            ui.painter().rect_filled(swatch, 0.0, rgb(color));
            ui.painter().rect_stroke(swatch, 0.0, Stroke::new(0.75, t.group_border), egui::StrokeKind::Inside);
            let ink = match (sel, shown && !inactive) {
                (true, true) => t.track_list_selected_text,
                (true, false) => t.track_list_selected_text.lerp_to_gamma(t.track_list_selected, 0.35),
                (false, true) => t.text,
                (false, false) => t.text_dim,
            };
            let (icon, icon_ink) = match kind {
                TrackKind::Audio => ("track_audio", ink),
                TrackKind::Aux => ("track_aux", if shown && !inactive { t.meter_green } else { ink }),
                TrackKind::Master => ("track_master", ink),
                TrackKind::Midi => ("note", ink),
                TrackKind::Instrument => ("track_instrument", ink),
                TrackKind::Vca => ("track_vca", ink),
                TrackKind::Folder => ("track_folder", ink),
                TrackKind::Video => ("track_video", ink),
            };
            crate::icons::draw(ui.painter(), Rect::from_center_size(pos2(r.min.x + 49.0, r.center().y), vec2(12.0, 11.0)), icon, icon_ink);
            ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 59.0 + depth.min(8) as f32 * 10.0, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                regular(11.5),
                ink,
            );
            if resp.clicked() {
                crate::edit_window::click_track(app, soundcraft_model::TrackId(id), ui.input(|i| i.modifiers), |_, _| true);
            }
            if resp.dragged() || resp.drag_stopped() {
                drag_track(app, ui, r, id, resp.drag_stopped());
            }
        }
    });
    ui.painter().set(background, track_list_face(out.inner_rect, t.track_list_bg, t));
    gutter_scroll(ui, gutter, &out);
    let groups_top = out.inner_rect.max.y + PANEL_GAP;
    let gap = Rect::from_min_max(pos2(full.min.x, out.inner_rect.max.y), pos2(full.max.x, groups_top));
    ui.painter().rect_filled(gap, 0.0, Color32::BLACK);
    groups_panel(app, ui, Rect::from_min_max(pos2(full.min.x, groups_top), full.max));
}

fn groups_panel(app: &mut SoundApp, parent: &mut Ui, bounds: Rect) {
    let t = Tokens::current();
    let ui = &mut parent.new_child(egui::UiBuilder::new().max_rect(bounds));
    ui.spacing_mut().item_spacing.y = 0.0;
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().text(pos2(header.min.x + 8.0, header.center().y), Align2::LEFT_CENTER, "GROUPS", bold(11.5), t.header_text);
    ui.painter().hline(header.x_range(), header.max.y - 0.5, Stroke::new(1.0, t.border));
    let menu = Rect::from_center_size(pos2(header.max.x - 10.0, header.center().y), vec2(18.0, 18.0));
    let response = ui.interact(menu, ui.scope_id().with("groups_menu"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Groups menu"));
    let circle = if response.hovered() || response.has_focus() { t.text } else { t.text_dim.lerp_to_gamma(t.panel_bg2, 0.18) };
    ui.painter().circle_filled(menu.center(), 6.5, circle);
    crate::icons::draw(ui.painter(), menu.shrink(2.0), "triangle_down", t.panel_bg2);
    let response = response.on_hover_text("Group options");
    egui::Popup::menu(&response).show(|ui| group_menu(app, ui, None));

    let body = Rect::from_min_max(pos2(bounds.min.x, header.max.y), bounds.max);
    let gutter = body.with_min_x(body.max.x - GROUP_GUTTER_W);
    let content = body.with_max_x(gutter.min.x);
    ui.painter().rect_filled(content, 0.0, t.track_list_bg);
    let rule = Stroke::new(0.75, t.track_list_bg.lerp_to_gamma(t.group_border, 0.45));
    for x in GROUP_COLUMNS {
        ui.painter().vline(content.min.x + x, content.y_range(), rule);
    }
    ui.painter().rect_filled(gutter, 0.0, t.panel_bg);
    ui.painter().vline(gutter.min.x + 0.5, gutter.y_range(), Stroke::new(1.0, t.border));

    let groups: Vec<GroupRow> = std::iter::once(GroupRow { id: None, letter: '!', name: "<ALL>".into(), active: false, color: ALL_GROUP_COLOR })
        .chain(app.engine.session().groups.iter().map(|g| GroupRow {
            id: Some(g.id),
            letter: g.letter,
            name: g.name.clone(),
            active: g.active,
            color: g.color,
        }))
        .collect();
    let list_ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(content));
    let groups_area = with_pending_offset(egui::ScrollArea::vertical().id_salt("groups_scroll"), list_ui, "groups_scroll");
    let out =
        groups_area.auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(list_ui, |ui| {
            for group in groups {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), GROUP_ROW_H), Sense::click());
                resp.widget_info(|| match group.id {
                    Some(_) => egui::WidgetInfo::selected(egui::Role::CheckBox, true, group.active, &group.name),
                    None => egui::WidgetInfo::labeled(egui::Role::Button, true, &group.name),
                });
                if resp.has_focus() {
                    ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
                }
                paint_group_row(ui, r, &group, t);
                if resp.clicked()
                    && let Some(id) = group.id
                {
                    let _ = app.run("track.group_toggle", json!({"group": id.0}));
                }
                if resp.double_clicked() {
                    if let Some(id) = group.id {
                        let members: Vec<u64> = app
                            .engine
                            .session()
                            .groups
                            .iter()
                            .find(|g| g.id == id)
                            .map(|g| g.members.iter().map(|m| m.0).collect())
                            .unwrap_or_default();
                        let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
                    } else {
                        let _ = app.run("edit.select_all", json!({}));
                    }
                }
                resp.context_menu(|ui| group_menu(app, ui, group.id));
            }
        });
    gutter_scroll(ui, gutter.with_min_y(gutter.min.y + 15.0), &out);

    let focus = Rect::from_min_size(gutter.min + vec2(0.5, 0.5), vec2(GROUP_GUTTER_W - 1.0, 14.0));
    let response = ui.interact(focus, ui.scope_id().with("groups_keyboard_focus"), Sense::click());
    let active = app.engine.session().edit.keyboard_focus == "groups";
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, active, "Groups Keyboard Focus"));
    let face = if active {
        t.solo
    } else if response.hovered() {
        t.button_hi
    } else {
        t.panel_bg2
    };
    ui.painter().rect_filled(focus, 0.0, face);
    ui.painter().rect_stroke(focus, 0.0, Stroke::new(0.75, if response.has_focus() { t.accent } else { t.text_dim }), egui::StrokeKind::Inside);
    let ink = if active { t.text_dark } else { t.text };
    ui.painter().text(focus.min + vec2(1.5, 0.0), Align2::LEFT_TOP, "a", bold(8.5), ink);
    ui.painter().text(focus.max - vec2(1.5, 0.0), Align2::RIGHT_BOTTOM, "z", bold(8.5), ink);
    if response.clicked() {
        let _ = app.run("options.keyboard_focus", json!({"focus": if active { "none" } else { "groups" }}));
    }
    response.on_hover_text("Groups Keyboard Focus");
}

/// Shared commands for the header dropdown and group-row context menus.
fn group_menu(app: &mut SoundApp, ui: &mut Ui, id: Option<GroupId>) {
    if ui.button("New Group…").clicked() {
        let _ = app.dialogs.open_for_command(&app.engine, "track.group");
        ui.close();
    }
    let Some(group) = id.and_then(|id| app.engine.session().groups.iter().find(|group| group.id == id)).cloned() else { return };
    if ui.button("Select Tracks in Group").clicked() {
        let members: Vec<_> = group.members.iter().map(|id| id.0).collect();
        let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
        ui.close();
    }
    if ui.button(if group.active { "Disable" } else { "Enable" }).clicked() {
        let _ = app.run("track.group_toggle", json!({"group": group.id.0}));
        ui.close();
    }
    if ui.button("Modify Group…").clicked() {
        let _ = app.dialogs.open_modify_group(&app.engine, group.id);
        ui.close();
    }
    if ui.button("Delete Group").clicked() {
        let _ = app.run("track.ungroup", json!({"group": group.id.0}));
        ui.close();
    }
}
