use super::plugin_menu::{MenuSearch, menu_row};
use super::routing::route_name;
use super::slots::{COMPACT_SLOT, Mark, empty_slot, level_icon, used_slot};
use super::{
    Align2, Rect, Route, Sense, SoundApp, Stroke, StrokeKind, Tokens, Track, TrackId, TrackKind, Ui, db_text, fader, json, pan_knob, pan_text, pos2,
    regular, text_toggle, vec2,
};
use craft_ui::combo::Menu;

/// Draw one send slot: the route area picks a destination, while the level area opens or edits the send level.
pub(crate) fn send_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let snd = track.mixer.sends.get(slot).cloned().flatten();
    let compact = r.width() < COMPACT_SLOT;
    // As in Pro Tools, only the fader icon opens a send's level; the rest of the slot picks its
    // destination. A short track's square has no icon, so all of it is the level.
    let level_r = if compact { r } else { level_icon(r).expand2(vec2(3.0, 0.0)).with_min_y(r.min.y).with_max_y(r.max.y) };
    let route_r = Rect::from_min_max(r.min, pos2((level_r.min.x - 1.0).max(r.min.x), r.max.y));
    let route_resp = ui.interact(if snd.is_some() { route_r } else { r }, ui.scope_id().with(("snd-route", id.0, slot)), Sense::click());
    route_resp
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Send {} for {}", (b'A' + slot as u8) as char, track.name)));
    let mark = if compact { Mark::Arrow } else { Mark::Letter((b'a' + slot as u8) as char) };
    match &snd {
        Some(s) => {
            let name = route_name(app, &s.target);
            let level = soundcraft_model::fader_db_to_pos(s.level_db);
            let color = if s.mute { t.mute } else { t.text };
            used_slot(ui.painter(), r, t.slot_face, mark, &name, color, Some(level));
            let which = (b'A' + slot as u8) as char;
            let shown = app.ui.send_window == Some((id, slot));
            let level_resp = ui.interact(level_r, ui.scope_id().with(("send-value", id.0, slot)), Sense::click_and_drag());
            level_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Send {which} level for {}", track.name)));
            if shown {
                ui.painter().rect_stroke(level_r.shrink(0.5), 1.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
            }
            if level_resp.clicked() {
                if ui.input(|i| i.modifiers.alt) {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
                } else {
                    app.ui.send_window = (!shown).then_some((id, slot));
                }
            }
            if level_resp.dragged() {
                // Use this frame's pointer movement. `Response::drag_delta()` is cumulative, so
                // applying it to the already-updated value made trackpad adjustment accelerate.
                let (dy, fine) = ui.input(|i| (i.pointer.delta().y, i.modifiers.shift));
                let db = dragged_send_db(s.level_db, dy, fine);
                let _ =
                    app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
            }
            let _ = level_resp.clone().on_hover_text(format!(
                "Send {which} to {name}: {} dB\nClick for the send fader · drag vertically to adjust · Shift-drag for fine control · Alt-click for 0 dB · right-click for options",
                db_text(s.level_db)
            ));
            level_resp.context_menu(|ui| send_options_menu(app, ui, id, slot, s));
        }
        None => empty_slot(ui.painter(), r, ui.rect_contains_pointer(r), mark),
    }
    let current = snd.as_ref().map(|send| (send.target.clone(), route_name(app, &send.target)));
    send_route_menu(app, ui, &route_resp, id, slot, current);
}

/// The Send window, as in Pro Tools: one send's fader, level, pan, Mute and Pre-fader. Clicking a
/// send's level opens it, moves it to that send, or closes it, and so does Escape.
pub(crate) fn send_window(app: &mut SoundApp, ctx: &egui::Context) {
    // egui closes menus and drops text focus on Escape before this runs, so whether this Escape
    // was theirs comes from the last frame. A dialog or the search on top takes it too.
    let busy_id = egui::Id::unique("send_window_escape");
    let busy = ctx.data(|d| d.get_temp(busy_id)).unwrap_or(false) || app.dialogs.open.is_some() || app.ui.show_search;
    if !busy && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.send_window = None;
    }
    send_window_contents(app, ctx);
    let busy = egui::Popup::is_any_open(ctx) || ctx.text_edit_focused();
    ctx.data_mut(|d| d.insert_temp(busy_id, busy));
}

/// Draw the floating send window for the remembered track and slot, or close it when that send no longer exists.
fn send_window_contents(app: &mut SoundApp, ctx: &egui::Context) {
    let Some((id, slot)) = app.ui.send_window else { return };
    let Some((name, send)) = app.engine.session().track(id).and_then(|t| Some((t.name.clone(), t.mixer.sends.get(slot).cloned().flatten()?))) else {
        app.ui.send_window = None;
        return;
    };
    let t = Tokens::current();
    let mut open = true;
    egui::Window::new(format!("{name} · Send {}", (b'A' + slot as u8) as char))
        .id(egui::Id::unique("send_window"))
        .open(&mut open)
        .auto_sized()
        .collapsible(false)
        .default_pos(ctx.pointer_latest_pos().map_or(pos2(200.0, 200.0), |p| p + vec2(24.0, -160.0)))
        .show(ctx, |ui| {
            ui.set_width(150.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(route_name(app, &send.target)).color(t.counter_text));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(((ui.available_width() - 62.0) / 2.0).max(0.0));
                    if text_toggle(ui, vec2(36.0, 18.0), "PRE", send.pre_fader, t.accent, "Pre-fader").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": !send.pre_fader}));
                    }
                    if text_toggle(ui, vec2(22.0, 18.0), "M", send.mute, t.mute, "Mute send").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": !send.mute}));
                    }
                });
                let mut pan = send.pan;
                pan_knob(ui, 32.0, &mut pan, "Send pan (drag; double-click centres)");
                if (pan - send.pan).abs() > f32::EPSILON {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "pan": pan}),
                        &format!("send-pan:{}:{slot}", id.0),
                    );
                }
                ui.label(egui::RichText::new(pan_text(send.pan)).color(t.counter_text).small());
                let (fr, fresp) = ui.allocate_exact_size(vec2(56.0, 220.0), Sense::hover());
                fresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Send level"));
                // Distinct from the strip's "Send A level for <track>" icon and from the slot's "Send level", so a
                // label query while the window is open still picks one node.
                if let Some(db) = fader(ui, fr, send.level_db, ui.scope_id().with("send_fader"), "Send level fader") {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "db": db}),
                        &format!("send:{}:{slot}", id.0),
                    );
                }
                let (rr, _) = ui.allocate_exact_size(vec2(64.0, 15.0), Sense::hover());
                ui.painter().rect_filled(rr, 1.0, t.counter_bg);
                let shown = db_text(send.level_db);
                let (_, typed) = editable_readout(
                    ui,
                    rr,
                    ui.scope_id().with("send_level_value"),
                    Readout {
                        name: &format!("{name} send level"),
                        display: &shown,
                        edit_initial: &format!("{:.1}", send.level_db),
                        parse: parse_db,
                        reset: 0.0,
                    },
                );
                if let Some(db) = typed {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
                }
            });
        });
    if !open {
        app.ui.send_window = None;
    }
}

/// Secondary controls live behind the amount readout, separate from the bus selector like Logic.
fn send_options_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, send: &soundcraft_model::SendSlot) {
    ui.set_min_width(230.0);
    ui.strong(format!("Send {} · {}", (b'A' + slot as u8) as char, route_name(app, &send.target)));
    let mut db = send.level_db.max(-60.0);
    if ui.add(egui::Slider::new(&mut db, -60.0..=12.0).text("Level").suffix(" dB")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
    }
    send_level_scale(ui, send.level_db);
    ui.horizontal(|ui| {
        for (label, delta) in [("−3", -3.0), ("−1", -1.0), ("+1", 1.0), ("+3", 3.0)] {
            if ui.small_button(label).clicked() {
                let db = (send.level_db.max(-60.0) + delta).clamp(-60.0, 12.0);
                let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
            }
        }
        if ui.small_button("0 dB").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
        }
        if ui.small_button("−∞").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": -144.0}));
        }
    });
    let mut pan = send.pan;
    if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).text("Pan")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "pan": pan}), &format!("send-pan:{}:{slot}", id.0));
    }
    ui.horizontal(|ui| {
        let mut mute = send.mute;
        if crate::widgets::checkbox(ui, &mut mute, "Mute").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": mute}));
        }
        let mut pre = send.pre_fader;
        if crate::widgets::checkbox(ui, &mut pre, "Pre-fader").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": pre}));
        }
    });
    if let Route::Bus(bus) = send.target
        && let Some(aux) =
            app.engine.session().tracks.iter().find(|t| t.kind == TrackKind::Aux && t.mixer.input == Route::Bus(bus)).map(|t| (t.id, t.name.clone()))
        && ui.button("Rename bus…").clicked()
    {
        app.dialogs.open_rename_track(aux.0, &aux.1);
        ui.close();
    }
}

/// A send level after a vertical drag of `dy` points: along the fader's taper, like the level bar,
/// so it runs smoothly down to −∞ and back. About 0.15 dB a point near 0 dB (0.02 when fine).
pub(super) fn dragged_send_db(db: f32, dy: f32, fine: bool) -> f32 {
    let per_point = if fine { 0.0004 } else { 0.003 };
    soundcraft_model::fader_pos_to_db(soundcraft_model::fader_db_to_pos(db) - dy * per_point)
}

/// What an [`editable_readout`] shows and how a typed value is parsed.
pub(super) struct Readout<'a> {
    /// Accessible name of the text field shown while editing.
    pub(super) name: &'a str,
    /// The value painted while not editing.
    pub(super) display: &'a str,
    /// The editor's initial text.
    pub(super) edit_initial: &'a str,
    pub(super) parse: fn(&str) -> Option<f32>,
    /// The value an Alt-click sets.
    pub(super) reset: f32,
}

/// Paint a value until clicked, then replace it with a focused text field. Enter or clicking away
/// commits a valid value; Escape cancels, and so does the field losing focus while hidden.
/// Alt-click returns `reset`. The interaction response remains available for dragging.
pub(super) fn editable_readout(ui: &mut Ui, r: Rect, id: egui::Id, readout: Readout<'_>) -> (Option<egui::Response>, Option<f32>) {
    let Readout { name, display, edit_initial, parse, reset } = readout;
    let t = Tokens::current();
    let active_id = id.with("active");
    let text_id = id.with("text");
    let mut active: bool = ui.data(|d| d.get_temp(active_id)).unwrap_or(false);
    let mut started = false;
    let mut response = None;
    let mut value = None;
    if !active {
        let resp = ui.interact(r, id, Sense::click_and_drag());
        let alt = ui.input(|i| i.modifiers.alt);
        if resp.clicked() && alt {
            value = Some(reset);
        }
        if resp.clicked() && !alt && !resp.double_clicked() {
            active = true;
            started = true;
            ui.data_mut(|d| d.insert_temp(text_id, edit_initial.to_string()));
        } else {
            ui.painter().with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, display, regular(9.0), t.counter_text);
        }
        response = Some(resp);
    }
    if active {
        let mut text: String = ui.data(|d| d.get_temp(text_id)).unwrap_or_else(|| edit_initial.to_string());
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(r));
        child.visuals_mut().override_text_color = Some(t.counter_text);
        let edit = egui::TextEdit::singleline(&mut text)
            .id(id.with("editor"))
            .frame(egui::Frame::NONE)
            .align(egui::Align2::CENTER_TOP)
            .font(regular(9.0))
            .desired_width(r.width());
        let edit_resp = crate::widgets::named_text_field(child.add_sized(r.size(), edit), name);
        if started {
            edit_resp.request_focus();
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if escape {
            active = false;
        } else if (enter && edit_resp.has_focus()) || edit_resp.lost_focus() {
            if let Some(parsed) = parse(&text) {
                value = Some(parsed);
                active = false;
            } else if edit_resp.lost_focus() {
                active = false;
            }
        } else if !started && !edit_resp.has_focus() {
            active = false;
        }
        ui.data_mut(|d| d.insert_temp(text_id, text));
    }
    ui.data_mut(|d| d.insert_temp(active_id, active));
    (response, value)
}

pub(super) fn parse_db(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace("db", "").replace(',', ".");
    if matches!(cleaned.trim(), "-inf" | "-infinity" | "off") {
        return Some(-144.0);
    }
    cleaned.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(-144.0, 12.0))
}

pub(super) fn parse_pan(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace(',', ".");
    let (sign, number) = if let Some(v) = cleaned.strip_prefix('<').or_else(|| cleaned.strip_prefix('l')) {
        (-1.0, v)
    } else if let Some(v) = cleaned.strip_suffix('>').or_else(|| cleaned.strip_prefix('r')) {
        (1.0, v)
    } else {
        (1.0, cleaned.as_str())
    };
    let raw = number.trim().parse::<f32>().ok()? * sign;
    let value = if number.contains('.') && raw.abs() <= 1.0 { raw } else { raw / 100.0 };
    value.is_finite().then_some(value.clamp(-1.0, 1.0))
}

/// A compact calibrated scale for a send. The marks use the same taper as the strip's level bar.
fn send_level_scale(ui: &mut Ui, db: f32) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 25.0), Sense::hover());
    let line_y = r.min.y + 4.0;
    ui.painter().line_segment([pos2(r.min.x, line_y), pos2(r.max.x, line_y)], Stroke::new(1.0, t.border));
    for (i, mark) in [-60.0f32, -24.0, -10.0, 0.0, 6.0, 12.0].into_iter().enumerate() {
        let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(mark);
        ui.painter().line_segment([pos2(x, line_y - 3.0), pos2(x, line_y + 3.0)], Stroke::new(1.0, t.text_dim));
        let align = if i == 0 {
            Align2::LEFT_TOP
        } else if i == 5 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        let label = if mark > 0.0 { format!("+{mark:.0}") } else { format!("{mark:.0}") };
        ui.painter().text(pos2(x, line_y + 5.0), align, label, regular(8.0), t.text_dim);
    }
    let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(db);
    ui.painter().line_segment([pos2(x, r.min.y), pos2(x, line_y + 3.0)], Stroke::new(2.0, t.counter_text));
}

/// What the send menu's footer was used for.
enum SendFooter {
    NewBus,
    RemoveSend,
}

/// The send's destination menu: a title and the bus search on top, the matching buses in a list
/// that scrolls, "new bus..." (and "Remove send" for an existing send) fixed under it.
/// `current` is the send's destination and its name, if the slot has a send.
fn send_route_menu(app: &mut SoundApp, ui: &Ui, field: &egui::Response, id: TrackId, slot: usize, current: Option<(Route, String)>) {
    let busses = app.engine.session().busses.clone();
    let which = (b'A' + slot as u8) as char;
    let has_send = current.is_some();
    let header = |ui: &mut Ui| {
        ui.set_min_width(250.0);
        match &current {
            Some((_, name)) => {
                ui.horizontal(|ui| {
                    ui.strong(format!("Send {which}"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(name).color(Tokens::current().counter_text));
                    });
                });
                ui.label(egui::RichText::new("DESTINATION").size(8.0).color(Tokens::current().text_dim));
            }
            None => {
                ui.strong(format!("Send {which} · Choose destination"));
            }
        }
        let search = MenuSearch::show(ui, egui::Id::unique(("send-bus-search", id.0, slot)), "Search buses…");
        let visible: Vec<_> = busses.iter().filter(|bus| search.matches(&bus.name)).collect();
        (search, visible)
    };
    let target = current.as_ref().map(|(route, _)| route);
    let row_h = ui.spacing().interact_size.y;
    let shown = Menu::new(ui, field).height(260.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show_rows_with_header_footer(
        header,
        row_h,
        |(_, visible)| visible.len(),
        |ui, range, (search, visible)| {
            let n = visible.len();
            let picked = search.list_rows(ui, range, n, |ui, i, highlighted| {
                let is_current = matches!(target, Some(Route::Bus(bus_id)) if *bus_id == visible[i].id);
                menu_row(ui, highlighted || (is_current && !search.typing()), &visible[i].name)
            });
            picked.map(|i| {
                ui.close();
                visible[i].name.clone()
            })
        },
        |ui| {
            if ui.button("new bus...").clicked() {
                ui.close();
                return Some(SendFooter::NewBus);
            }
            if has_send && ui.button("Remove send").clicked() {
                ui.close();
                return Some(SendFooter::RemoveSend);
            }
            None
        },
    );
    let Some((picked, footer)) = shown else { return };
    if let Some(bus) = picked {
        let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": bus}));
    }
    match footer {
        Some(SendFooter::NewBus) => {
            let mut n = busses.len() + 1;
            while busses.iter().any(|bus| bus.name.eq_ignore_ascii_case(&format!("Bus {n}")))
                || app.engine.session().track_by_name(&format!("Bus {n}")).is_some()
            {
                n += 1;
            }
            if app.run("mix.new_bus", json!({"name": format!("Bus {n}")})).is_ok() {
                let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": format!("Bus {n}")}));
            }
        }
        Some(SendFooter::RemoveSend) => {
            let _ = app.run("mix.send_remove", json!({"track": id.0, "slot": slot}));
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use egui_kittest::{Harness, kittest::Queryable};

    /// Filtering the destination list to nothing shrinks the menu; clearing the filter grows it
    /// back, with the search field in the same place and the footer under the list.
    #[test]
    fn the_send_destination_menu_grows_back_when_the_search_is_cleared() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        for n in 0..30 {
            app.run("mix.new_bus", json!({"name": format!("Aux {n}")})).unwrap();
        }
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.send", json!({"track": kick.0, "slot": 0, "bus": "Aux 0"})).unwrap();
        let _palette = crate::theme::lock_test_palette();
        let mut ready = false;
        let mut h = Harness::builder().with_size(vec2(600.0, 800.0)).build_ui_state(
            move |ui, (app, kick): &mut (SoundApp, TrackId)| {
                if !ready {
                    crate::fonts::install(ui.ctx(), true);
                    crate::theme::apply(ui.ctx(), false);
                    ready = true;
                    return;
                }
                let track = app.engine.session().track(*kick).cloned().unwrap();
                let r = Rect::from_min_size(ui.max_rect().min + vec2(20.0, 20.0), vec2(120.0, 24.0));
                send_slot(app, ui, &track, 0, r);
            },
            (app, kick),
        );
        h.run();
        h.get_by_label("Send A for Kick").click();
        h.run();
        let field = |h: &Harness<'_, (SoundApp, TrackId)>| h.get_by_role(egui::accesskit::Role::TextInput).rect();
        let footer = |h: &Harness<'_, (SoundApp, TrackId)>| h.get_by_label("Remove send").rect();
        let (search, full) = (field(&h), footer(&h));
        assert!(h.query_by_label("new bus...").is_some(), "the footer lists new bus...");
        assert!(full.top() > search.bottom() + 150.0, "the list sits between the search and the footer: {search:?} {full:?}");

        h.get_by_role(egui::accesskit::Role::TextInput).type_text("zzzz");
        h.run();
        let none = footer(&h);
        assert!(none.top() < full.top() - 100.0, "no matches shrink the menu: {none:?} vs {full:?}");
        assert_eq!(field(&h), search, "the search field stays put");

        for _ in 0..4 {
            h.key_press(egui::Key::Backspace);
            h.run();
        }
        assert_eq!(field(&h), search, "the search field stays put");
        assert!((footer(&h).top() - full.top()).abs() < 1.0, "cleared, the footer is back under the full list: {:?} vs {full:?}", footer(&h));
    }
}
