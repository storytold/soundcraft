//! The Edit window toolbar.

use crate::counter::{CounterEdit, CounterTarget, component_ranges, display as counter_display};
use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::{icon_button, key, key_fill};
use crate::{MainWindow, SoundApp, icons};
use egui::{Align2, Color32, CornerRadius, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit::{SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{AutoParam, EditMode, Tool, TrackKind};
use soundcraft_time::{Samples, TimeFormat};

pub const HEIGHT: f32 = 78.0;

fn group(ui: &mut Ui, w: f32, h: f32, add: impl FnOnce(&mut Ui, Rect)) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.toolbar_group, Stroke::new(1.0, t.group_border), StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(5.0)).layout(egui::Layout::left_to_right(egui::Align::Min)));
    add(&mut child, r);
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.toolbar_bg);
    ui.add_space(4.0);
    egui::ScrollArea::horizontal()
        .id_salt("edit_toolbar_scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                edit_modes(app, ui);
                zoom_group(app, ui);
                tools(app, ui);
                counters(app, ui);
                grid_nudge(app, ui);
                transport(app, ui);
                tempo_meter(app, ui);
                output_meter(app, ui);
            });
        });
}

fn edit_modes(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let ed = &app.engine.session().edit;
    let (cur, snap, locked) = (ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(SHUFFLE_LOCK));
    let cmd = crate::shortcuts::shortcut_label("Cmd", ui.ctx().os() == egui::os::OperatingSystem::Mac);
    let modes = [
        (EditMode::Shuffle, 0, 0, "SHUFFLE", "F1"),
        (EditMode::Spot, 1, 0, "SPOT", "F3"),
        (EditMode::Slip, 0, 1, "SLIP", "F2"),
        (EditMode::Grid, 1, 1, "GRID", "F4"),
    ];
    let widest =
        modes.iter().map(|m| m.3).chain(["REL GRID"]).map(|l| ui.painter().layout_no_wrap(l.into(), bold(10.5), t.text).size().x).fold(0.0, f32::max);
    group(ui, 2.0 * widest + 44.0, 52.0, |ui, r| {
        let inner = r.shrink(6.0);
        let cw = inner.width() / 2.0;
        let ch = inner.height() / 2.0;
        for (m, cx, cy, label, fkey) in modes {
            let cell = Rect::from_min_size(pos2(inner.min.x + cw * cx as f32, inner.min.y + ch * cy as f32), vec2(cw, ch)).shrink(1.0);
            let resp = ui.interact(cell, ui.id().with(("mode", label)), Sense::click());
            let on = cur == m || (m == EditMode::Grid && cur == EditMode::GridRelative);
            key(ui.painter(), cell, &resp, if on { t.mode_on } else { t.mode_off });
            let fg = if on { t.mode_on_text } else { t.mode_off_text };
            if m == EditMode::Grid && snap {
                ui.painter().rect_stroke(cell, 1.0, Stroke::new(1.5, t.mode_on), StrokeKind::Inside);
            }
            if m == EditMode::Shuffle && locked {
                icons::draw(ui.painter(), Rect::from_center_size(cell.center(), vec2(15.0, 15.0)), "lock", fg);
            } else {
                let txt = if m == EditMode::Grid && cur == EditMode::GridRelative { "REL GRID" } else { label };
                ui.painter().text(cell.center(), Align2::CENTER_CENTER, txt, bold(10.5), fg);
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, label));
            if resp.clicked() {
                mode_click(app, m, ui.input(|i| i.modifiers));
            }
            resp.on_hover_text(match m {
                EditMode::Shuffle if locked => format!("Shuffle is locked ({cmd}-click to unlock)"),
                EditMode::Shuffle => format!("Shuffle mode ({fkey}); {cmd}-click locks it out"),
                EditMode::Grid => format!("Grid mode ({fkey}; again for Relative); Shift-click: Snap to Grid in the other modes"),
                _ => format!("{} mode ({fkey}); Shift-click adds Snap to Grid", if m == EditMode::Slip { "Slip" } else { "Spot" }),
            });
        }
    });
}

/// Shift adds Snap to Grid to Shuffle, Slip and Spot (on Grid it toggles it for the current mode);
/// Cmd/Ctrl on Shuffle toggles Shuffle Lock.
fn mode_click(app: &mut SoundApp, m: EditMode, mods: egui::Modifiers) {
    let cur = app.engine.session().edit.edit_mode;
    let _ = if m == EditMode::Shuffle && mods.command {
        app.run("edit.shuffle_lock", json!({}))
    } else if m == EditMode::Grid && mods.shift && !cur.is_grid() {
        app.run("edit.snap_to_grid", json!({}))
    } else {
        let id = match m {
            EditMode::Shuffle => "shuffle",
            EditMode::Spot => "spot",
            EditMode::Slip => "slip",
            _ => crate::shortcuts::grid_mode(app),
        };
        app.run("edit.mode", json!({"mode": id, "snap_to_grid": mods.shift}))
    };
}

fn zoom_group(app: &mut SoundApp, ui: &mut Ui) {
    group(ui, 128.0, 52.0, |ui, _| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, vec2(28.0, 22.0), "minus", false, "Zoom out (Cmd+[)").clicked() {
                    let _ = app.run("view.zoom_out", json!({}));
                }
                if icon_button(ui, vec2(30.0, 22.0), "wave", false, "Waveform zoom in (Cmd+Alt+])").clicked() {
                    let _ = app.run("view.waveform_zoom", json!({"factor": 2.0}));
                }
                if icon_button(ui, vec2(28.0, 22.0), "plus", false, "Zoom in (Cmd+])").clicked() {
                    let _ = app.run("view.zoom_in", json!({}));
                }
                if icon_button(ui, vec2(28.0, 22.0), "zoom", false, "Fill window with session (Alt+A)").clicked() {
                    let w = ui.ctx().content_rect().width() - 420.0;
                    let _ = app.run("view.zoom_fit", json!({"width_px": w}));
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for i in 1..=5 {
                    let r = crate::widgets::text_toggle(
                        ui,
                        vec2(22.0, 18.0),
                        &i.to_string(),
                        false,
                        Color32::GRAY,
                        "Zoom preset (click to recall, Cmd-click to store)",
                    );
                    if r.clicked() {
                        let store = ui.input(|x| x.modifiers.command);
                        let _ = app.run("view.zoom_preset", json!({"preset": i, "store": store}));
                    }
                }
            });
        });
    });
}

fn tools(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let cur = app.engine.session().edit.tool;
    group(ui, 268.0 + 38.0, 52.0, |ui, r| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 3.0);
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, vec2(36.0, 26.0), "zoom_toggle", app.engine.session().edit.zoom_toggle.is_some(), "Zoom Toggle (E)").clicked() {
                    crate::edit_window::zoom_toggle(app);
                }
                for (tool, icon) in [
                    (Tool::Zoom, "zoom"),
                    (Tool::Trim, "trim"),
                    (Tool::Selector, "selector"),
                    (Tool::Grabber, "grabber"),
                    (Tool::Scrubber, "scrubber"),
                    (Tool::Pencil, "pencil"),
                ] {
                    let smart_member = cur == Tool::Smart && matches!(tool, Tool::Trim | Tool::Selector | Tool::Grabber);
                    let tip = format!("{}{}", tool.label(), tool.fkey().map(|k| format!(" ({k})")).unwrap_or_default());
                    if icon_button(ui, vec2(36.0, 26.0), icon, cur == tool || smart_member, &tip).clicked() {
                        let _ = app.run("edit.tool", json!({"tool": tool.id()}));
                    }
                }
            });
            // Smart tool bracket above Trim/Selector/Grabber.
            let rr = row.response.rect;
            let bx0 = rr.min.x + 76.0;
            let bx1 = bx0 + 38.0 * 3.0 - 2.0;
            let bracket = Rect::from_min_max(pos2(bx0 - 2.0, r.min.y + 1.0), pos2(bx1 + 2.0, r.min.y + 5.0));
            let resp = ui.interact(bracket.expand2(vec2(0.0, 2.0)), ui.id().with("smart"), Sense::click());
            ui.painter().rect_filled(bracket, 1.0, if cur == Tool::Smart { t.accent } else { t.tool_bracket_off });
            if resp.on_hover_text("Smart Tool (F6+F7 / F7+F8)").clicked() {
                let _ = app.run("edit.tool", json!({"tool": "smart"}));
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let e = app.engine.session().edit.clone();
                let toggles: [(&str, &str, bool, &str); 5] = [
                    ("options.tab_to_transient", "tab_transient", e.tab_to_transient, "Tab to Transient"),
                    ("options.link_timeline_edit", "link", e.link_timeline_edit, "Link Timeline and Edit Selection"),
                    ("options.link_track_edit", "link", e.link_track_edit, "Link Track and Edit Selection"),
                    ("options.mirrored_midi", "note", e.mirrored_midi, "Mirrored MIDI Editing"),
                    ("options.automation_follows_edit", "wave", e.automation_follows_edit, "Automation Follows Edit"),
                ];
                for (id, icon, on, tip) in toggles {
                    if icon_button(ui, vec2(36.0, 18.0), icon, on, tip).clicked() {
                        let _ = app.run(id, json!({}));
                    }
                }
                let focus = e.keyboard_focus == "commands";
                if crate::widgets::text_toggle(ui, vec2(36.0, 18.0), "a-z", focus, t.accent, "Commands Keyboard Focus (single-key editing)").clicked()
                {
                    let _ = app.run("options.keyboard_focus", json!({}));
                }
            });
        });
    });
}

fn fmt_pos(app: &SoundApp, at: i64, f: TimeFormat) -> String {
    let s = app.engine.session();
    s.format_position(at, f)
}

fn counters(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (main_fmt, sub_fmt, sel) = {
        let s = app.engine.session();
        (s.edit.main_counter, s.edit.sub_counter, s.edit.selection)
    };
    let pos = app.position();
    let (r, _) = ui.allocate_exact_size(vec2(350.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let main_y = r.min.y + 14.0;
    ui.painter().text(pos2(r.min.x + 10.0, main_y), Align2::LEFT_CENTER, "Main", regular(10.0), t.counter_label);
    let main_value = Rect::from_min_max(pos2(r.min.x + 40.0, r.min.y + 4.0), pos2(r.min.x + 158.0, r.min.y + 24.0));
    counter_field(app, ui, main_value, CounterTarget::Main, pos, main_fmt, 21.0, "Main Counter");
    let main_menu = Rect::from_min_max(pos2(r.min.x + 160.0, r.min.y + 4.0), pos2(r.min.x + 176.0, r.min.y + 24.0));
    let main_resp = ui.interact(main_menu, ui.id().with("main_counter_format"), Sense::click());
    icons::draw(ui.painter(), Rect::from_center_size(main_menu.center(), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    egui::Popup::menu(&main_resp).show(|ui| {
        for f in TimeFormat::ALL {
            if ui.selectable_label(main_fmt == f, f.label()).clicked() {
                clear_counter_edit(app, ui);
                let _ = app.run("view.main_counter", json!({"format": f.id()}));
            }
        }
    });
    let sub_y = r.min.y + 34.0;
    ui.painter().text(pos2(r.min.x + 10.0, sub_y), Align2::LEFT_CENTER, "Sub", regular(10.0), t.counter_label);
    let sub_text = sub_fmt.map_or_else(|| "—".into(), |format| fmt_pos(app, pos, format));
    fitted_text(ui, Rect::from_min_max(pos2(r.min.x + 40.0, r.min.y + 25.0), pos2(r.min.x + 158.0, r.min.y + 44.0)), &sub_text, 20.0, t.counter_text);
    let sub_menu = Rect::from_min_max(pos2(r.min.x + 160.0, r.min.y + 25.0), pos2(r.min.x + 176.0, r.min.y + 44.0));
    let sub_resp = ui.interact(sub_menu, ui.id().with("sub_counter_format"), Sense::click());
    icons::draw(ui.painter(), Rect::from_center_size(sub_menu.center(), vec2(9.0, 9.0)), "triangle_down", t.text_dim);
    egui::Popup::menu(&sub_resp).show(|ui| {
        if ui.selectable_label(sub_fmt.is_none(), "Hidden").clicked() {
            clear_counter_edit(app, ui);
            let _ = app.run("view.sub_counter", json!({"format": null}));
        }
        ui.separator();
        for format in TimeFormat::ALL {
            if ui.selectable_label(sub_fmt == Some(format), format.label()).clicked() {
                clear_counter_edit(app, ui);
                let _ = app.run("view.sub_counter", json!({"format": format.id()}));
            }
        }
    });

    let rows = [("Start", CounterTarget::Start, sel.start), ("End", CounterTarget::End, sel.end), ("Length", CounterTarget::Length, sel.len())];
    for (i, (label, target, value)) in rows.into_iter().enumerate() {
        let y = r.min.y + 9.0 + i as f32 * 13.0;
        ui.painter().text(pos2(r.min.x + 226.0, y), Align2::RIGHT_CENTER, label, regular(10.0), t.counter_label);
        let field = Rect::from_min_max(pos2(r.min.x + 230.0, y - 6.0), pos2(r.min.x + 344.0, y + 6.0));
        counter_field(app, ui, field, target, value, main_fmt, 10.0, label);
    }

    let y = r.max.y - 10.0;
    ui.painter().line_segment([pos2(r.min.x + 6.0, r.max.y - 21.0), pos2(r.max.x - 6.0, r.max.y - 21.0)], Stroke::new(1.0, t.counter_rule));
    let cursor_at = app.cursor_readout.as_ref().map_or_else(|| "—".into(), |(at, _)| fmt_pos(app, *at, main_fmt));
    let cursor_value = app.cursor_readout.as_ref().map_or("—", |(_, value)| value.as_str());
    for x in [124.0, 202.0] {
        ui.painter().vline(r.min.x + x, r.max.y - 19.0..=r.max.y - 3.0, Stroke::new(1.0, t.counter_rule));
    }
    ui.painter().text(pos2(r.min.x + 10.0, y), Align2::LEFT_CENTER, "Cursor", regular(9.5), t.counter_label);
    ui.painter().text(pos2(r.min.x + 120.0, y), Align2::RIGHT_CENTER, cursor_at, mono(9.5), t.counter_text);
    icons::draw(ui.painter(), Rect::from_center_size(pos2(r.min.x + 135.0, y), vec2(12.0, 11.0)), "wave", t.counter_text);
    let value_rect = Rect::from_min_max(pos2(r.min.x + 144.0, r.max.y - 19.0), pos2(r.min.x + 198.0, r.max.y - 3.0));
    let value_font = fitted_mono(ui, cursor_value, 9.5, value_rect.width());
    ui.painter().text(pos2(value_rect.max.x, y), Align2::RIGHT_CENTER, cursor_value, value_font, t.counter_text);
    status_indicators(app, ui, Rect::from_min_max(pos2(r.min.x + 204.0, r.max.y - 19.0), pos2(r.max.x - 5.0, r.max.y - 3.0)));
}

fn fitted_text(ui: &Ui, rect: Rect, text: &str, maximum_size: f32, color: Color32) {
    ui.painter().text(pos2(rect.max.x, rect.center().y), Align2::RIGHT_CENTER, text, fitted_mono(ui, text, maximum_size, rect.width()), color);
}

fn fitted_mono(ui: &Ui, text: &str, maximum_size: f32, width: f32) -> egui::FontId {
    let mut size = maximum_size;
    while size > 8.0 && ui.painter().layout_no_wrap(text.into(), mono(size), Color32::WHITE).size().x > width {
        size -= 0.5;
    }
    mono(size)
}

fn counter_id(target: CounterTarget) -> egui::Id {
    egui::Id::new(("toolbar_counter", target))
}

fn clear_counter_edit(app: &mut SoundApp, ui: &mut Ui) {
    if let Some(edit) = app.counter_edit.take() {
        ui.memory_mut(|memory| memory.surrender_focus(counter_id(edit.target)));
    }
}

fn counter_field(app: &mut SoundApp, ui: &mut Ui, rect: Rect, target: CounterTarget, value: Samples, format: TimeFormat, size: f32, label: &str) {
    let t = Tokens::current();
    let shown = app
        .counter_edit
        .as_ref()
        .filter(|edit| edit.target == target && edit.format == format)
        .map_or_else(|| counter_display(app.engine.session(), target, value, format), |edit| edit.display(app.engine.session()));
    let mut fitted_size = size;
    while fitted_size > 9.0 && ui.painter().layout_no_wrap(shown.clone(), mono(fitted_size), t.counter_text).size().x > rect.width() {
        fitted_size -= 1.0;
    }
    let font = mono(fitted_size);
    let galley = ui.painter().layout_no_wrap(shown.clone(), font.clone(), t.counter_text);
    let text_left = rect.max.x - galley.size().x;
    let highlight = app.counter_edit.as_ref().filter(|edit| edit.target == target && edit.format == format).map(|edit| {
        let char_width = ui.painter().layout_no_wrap("0".into(), font.clone(), t.counter_text).size().x;
        edit.unit.and_then(|unit| component_ranges(&shown, format).get(unit).cloned()).map_or_else(
            || Rect::from_min_max(pos2(text_left, rect.min.y), pos2(rect.max.x, rect.max.y)),
            |range| {
                Rect::from_min_max(
                    pos2(text_left + range.start as f32 * char_width, rect.min.y),
                    pos2(text_left + range.end as f32 * char_width, rect.max.y),
                )
            },
        )
    });
    if let Some(highlight) = highlight {
        ui.painter().rect_filled(highlight, 1.0, t.counter_text.gamma_multiply(0.86));
    }
    ui.painter().text(pos2(rect.max.x, rect.center().y), Align2::RIGHT_CENTER, &shown, font.clone(), t.counter_text);
    if let Some(highlight) = highlight {
        ui.painter().with_clip_rect(highlight).text(pos2(rect.max.x, rect.center().y), Align2::RIGHT_CENTER, &shown, font.clone(), t.counter_bg);
    }
    let response = ui.interact(rect, counter_id(target), Sense::click());
    response.clone().widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, label));
    let active_here = app.counter_edit.as_ref().is_some_and(|edit| edit.target == target && edit.format == format);
    if active_here && ui.input(|input| input.pointer.primary_clicked()) && !response.hovered() {
        clear_counter_edit(app, ui);
    }
    if response.clicked() {
        let was_active = active_here;
        let unit = was_active
            .then(|| {
                response.interact_pointer_pos().and_then(|point| {
                    if point.x < text_left || point.x >= rect.max.x {
                        return None;
                    }
                    let char_width = ui.painter().layout_no_wrap("0".into(), font, t.counter_text).size().x.max(1.0);
                    let index = ((point.x - text_left) / char_width).floor() as usize;
                    component_ranges(&shown, format).iter().position(|range| range.contains(&index))
                })
            })
            .flatten();
        let edit_value = app.counter_edit.as_ref().filter(|edit| was_active && edit.target == target).map_or(value, |edit| edit.value);
        app.counter_edit = Some(CounterEdit::new(target, format, edit_value, unit));
        response.request_focus();
    }
}

pub(crate) fn handle_counter_keyboard(app: &mut SoundApp, ui: &mut Ui) {
    let edit_visible = app.ui.mix_separate || app.ui.window == MainWindow::Edit;
    if !edit_visible || app.dialogs.open.is_some() {
        clear_counter_edit(app, ui);
        return;
    }
    if app.counter_edit.as_ref().is_some_and(|edit| edit.format != app.engine.session().edit.main_counter) {
        clear_counter_edit(app, ui);
    }
    if app.counter_edit.is_none() {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let star = ui.input_mut(|input| {
            let found = input.events.iter().any(|event| matches!(event, egui::Event::Text(text) if text == "*"));
            if found {
                input.events.retain(|event| !matches!(event, egui::Event::Text(text) if text == "*"));
            }
            found
        });
        if star {
            let format = app.engine.session().edit.main_counter;
            app.counter_edit = Some(CounterEdit::new(CounterTarget::Main, format, app.position(), None));
            ui.memory_mut(|memory| memory.request_focus(counter_id(CounterTarget::Main)));
        } else {
            return;
        }
    }
    let (digits, period, slash, calculation, left, right, up, down, enter, escape) = ui.input_mut(|input| {
        let mut digits = String::new();
        let mut period = false;
        let mut slash = false;
        let mut calculation = None;
        input.events.retain(|event| {
            if let egui::Event::Text(text) = event
                && text.chars().all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '/' | '+' | '-' | '−'))
            {
                for ch in text.chars() {
                    match ch {
                        '0'..='9' => digits.push(ch),
                        '.' => period = true,
                        '/' => slash = true,
                        '+' => calculation = Some(1),
                        '-' | '−' => calculation = Some(-1),
                        _ => {}
                    }
                }
                return false;
            }
            if let egui::Event::Key { key: Key::Plus | Key::Minus, pressed: true, modifiers, .. } = event
                && !modifiers.alt
                && !modifiers.shift
            {
                calculation = Some(if matches!(event, egui::Event::Key { key: Key::Plus, .. }) { 1 } else { -1 });
                return false;
            }
            true
        });
        (
            digits,
            period,
            slash,
            calculation,
            input.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
            input.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
            input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, Key::Enter),
            input.consume_key(egui::Modifiers::NONE, Key::Escape),
        )
    });
    let session = app.engine.session_arc();
    let Some(edit) = app.counter_edit.as_mut() else { return };
    if escape {
        clear_counter_edit(app, ui);
        return;
    }
    if let Some(direction) = calculation {
        edit.begin_calculation(direction);
    }
    for digit in digits.chars() {
        edit.digit(&session, digit);
    }
    if left {
        edit.move_unit(-1, &session);
    }
    if right || period {
        edit.move_unit(1, &session);
    }
    if up {
        edit.step(&session, 1);
    }
    if down {
        edit.step(&session, -1);
    }
    let (target, value) = (edit.target, edit.value);
    if slash && target != CounterTarget::Main {
        apply_counter_value(app, target, value);
        let next = match target {
            CounterTarget::Start => CounterTarget::End,
            CounterTarget::End => CounterTarget::Length,
            CounterTarget::Length => CounterTarget::Start,
            CounterTarget::Main => CounterTarget::Main,
        };
        let selection = app.engine.session().edit.selection;
        let value = match next {
            CounterTarget::Main => app.position(),
            CounterTarget::Start => selection.start,
            CounterTarget::End => selection.end,
            CounterTarget::Length => selection.len(),
        };
        app.counter_edit = Some(CounterEdit::new(next, session.edit.main_counter, value, None));
        ui.memory_mut(|memory| memory.request_focus(counter_id(next)));
        return;
    }
    if enter {
        apply_counter_value(app, target, value);
        clear_counter_edit(app, ui);
    }
}

fn apply_counter_value(app: &mut SoundApp, target: CounterTarget, value: Samples) {
    let selection = app.engine.session().edit.selection;
    match target {
        CounterTarget::Main => {
            let _ = app.run("edit.select", json!({"start": value}));
            let _ = app.run("transport.locate", json!({"at": value}));
        }
        CounterTarget::Start => {
            let _ = app.run("edit.select", json!({"start": value, "end": selection.end.max(value)}));
        }
        CounterTarget::End => {
            let _ = app.run("edit.select", json!({"start": selection.start.min(value), "end": value}));
        }
        CounterTarget::Length => {
            let _ = app.run("edit.select", json!({"start": selection.start, "end": selection.start.saturating_add(value)}));
        }
    }
}

fn status_indicators(app: &mut SoundApp, ui: &mut Ui, rect: Rect) {
    let t = Tokens::current();
    let now = ui.input(|input| input.time);
    let revision = app.engine.revision;
    let (timeline_online, session_online) = match app.media_status {
        Some((cached_revision, checked_at, status)) if cached_revision == revision && now - checked_at < 1.0 => status,
        _ => {
            let status = soundcraft_engine::inspect::media_online(app.engine.session());
            app.media_status = Some((revision, now, status));
            status
        }
    };
    let overload = app.meter_snapshot.delay_compensation_overloaded;
    let delay = app.engine.session().edit.delay_compensation;
    let busy = app.bounce.is_some();
    let freeze_eligible = |track: &&soundcraft_model::Track| {
        !track.hidden && !track.inactive && matches!(track.kind, TrackKind::Audio | TrackKind::Aux | TrackKind::Instrument)
    };
    let frozen = app.engine.session().tracks.iter().filter(freeze_eligible).any(|track| track.frozen);
    let solo = app.engine.session().tracks.iter().any(|track| track.mixer.solo);
    let position = app.position();
    let mute = app.engine.session().tracks.iter().any(|track| {
        track.mixer.mute
            || (track.mixer.automation_mode.reads() && track.lane(&AutoParam::Mute).is_some_and(|lane| lane.value_at(position, 0.0) >= 0.5))
    });
    let green = t.counter_text;
    let red = t.rec;
    let dim = t.counter_off;
    let specs: Vec<_> = [
        (
            "TL",
            16.0,
            if timeline_online { green } else { red },
            if timeline_online { "Timeline media is online" } else { "Timeline media needs attention" },
            0u8,
        ),
        (
            "S",
            14.0,
            if session_online { green } else { red },
            if session_online { "Session media is online" } else { "Session media needs attention (click for details)" },
            1,
        ),
        (
            "Dly",
            22.0,
            if overload { red } else { green },
            if overload { "Delay compensation is overloaded" } else { "Delay compensation is active" },
            2,
        ),
        (
            "Bg",
            16.0,
            if app.task_error.is_some() {
                red
            } else if busy {
                green
            } else {
                dim
            },
            if app.task_error.is_some() {
                "A background task needs attention (click to open)"
            } else if busy {
                "A background task is running (click to open)"
            } else {
                "No background tasks (click to open)"
            },
            3,
        ),
        ("Frz", 18.0, if frozen { green } else { dim }, "Frozen tracks (Option/Alt-click toggles all eligible tracks)", 4),
        ("S", 14.0, if solo { t.solo } else { dim }, "Soloed tracks (click to clear)", 5),
        ("M", 14.0, if mute { t.mute } else { dim }, "Muted tracks", 6),
    ]
    .into_iter()
    .filter(|(_, _, _, _, action)| delay || *action != 2)
    .collect();
    let total: f32 = specs.iter().map(|(_, width, _, _, _)| *width + 2.0).sum();
    let mut x = rect.max.x - total;
    for (label, width, color, tip, action) in specs {
        let badge = Rect::from_min_size(pos2(x, rect.min.y), vec2(width, rect.height()));
        let response = ui.interact(badge, ui.id().with(("counter_status", action)), Sense::click());
        let paint = color.gamma_multiply(if response.hovered() { 1.15 } else { 0.9 });
        match action {
            0 => icons::draw(ui.painter(), badge.shrink(2.0), "timeline_status", paint),
            1 => icons::draw(ui.painter(), badge.shrink(2.0), "session_status", paint),
            2 => {
                ui.painter().text(badge.center(), Align2::CENTER_CENTER, label, bold(8.5), paint);
            }
            3 => icons::draw(ui.painter(), badge.shrink(2.0), "task_status", paint),
            4 => icons::draw(ui.painter(), badge.shrink(2.0), "freeze", paint),
            _ => {
                ui.painter().rect_filled(badge, 1.0, color.gamma_multiply(if color == dim { 0.45 } else { 0.9 }));
                let foreground = if color == dim { t.counter_text.gamma_multiply(0.55) } else { t.mode_on_text };
                ui.painter().text(badge.center(), Align2::CENTER_CENTER, label, bold(9.0), foreground);
            }
        };
        let response = response.on_hover_text(tip);
        if response.clicked() {
            match action {
                1 if !session_online => {
                    let _ = app.run("window.audio_health", json!({"value": true}));
                }
                3 => {
                    let _ = app.run("window.task_manager", json!({}));
                }
                4 if ui.input(|input| input.modifiers.alt) => {
                    let _ = app.run("track.freeze_all", json!({}));
                }
                5 if solo => {
                    let _ = app.run("mix.clear_solos", json!({}));
                }
                _ => {}
            }
        }
        x += width + 2.0;
    }
}

fn grid_nudge(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (grid, nudge, lines) = {
        let e = &app.engine.session().edit;
        (e.grid, e.nudge, e.grid_lines)
    };
    let (r, _) = ui.allocate_exact_size(vec2(170.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let gl = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 8.0), vec2(48.0, 16.0));
    let resp = ui.interact(gl, ui.id().with("grid_lines"), Sense::click());
    key(ui.painter(), gl, &resp, if lines { t.mode_on } else { t.counter_off });
    ui.painter().text(gl.center(), Align2::CENTER_CENTER, "Grid", bold(11.0), if lines { t.mode_on_text } else { t.mode_off_text });
    if resp.on_hover_text("Show grid lines").clicked() {
        let _ = app.run("view.grid_lines", json!({}));
    }
    let gr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 6.0), pos2(r.max.x - 4.0, r.min.y + 26.0));
    ui.painter().text(pos2(gr.max.x - 14.0, gr.center().y), Align2::RIGHT_CENTER, grid.label(), mono(11.0), t.counter_text);
    let gresp = ui.interact(gr, ui.id().with("grid_value"), Sense::click());
    value_menu(app, &gresp, "view.grid");
    ui.painter().text(pos2(r.min.x + 10.0, r.min.y + 44.0), Align2::LEFT_CENTER, "Nudge", regular(11.0), t.counter_label);
    let nr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 34.0), pos2(r.max.x - 4.0, r.min.y + 54.0));
    ui.painter().text(pos2(nr.max.x - 14.0, nr.center().y), Align2::RIGHT_CENTER, nudge.label(), mono(11.0), t.counter_text);
    let nresp = ui.interact(nr, ui.id().with("nudge_value"), Sense::click());
    value_menu(app, &nresp, "view.nudge");
    for rr in [gr, nr] {
        icons::draw(ui.painter(), Rect::from_center_size(pos2(rr.max.x - 6.0, rr.center().y), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    }
}

fn value_menu(app: &mut SoundApp, resp: &egui::Response, cmd: &str) {
    egui::Popup::menu(resp).show(|ui| {
        for v in ["1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/64", "1/8t", "1/16t", "1/4."] {
            if ui.button(v).clicked() {
                let _ = app.run(cmd, json!({"value": v}));
            }
        }
        ui.separator();
        for (label, v) in [
            ("1 msec", json!({"seconds": 0.001})),
            ("10 msec", json!({"seconds": 0.01})),
            ("100 msec", json!({"seconds": 0.1})),
            ("1 second", json!({"seconds": 1.0})),
            ("1 frame", json!({"frames": 1})),
            ("100 samples", json!({"samples": 100})),
        ] {
            if ui.button(label).clicked() {
                let _ = app.run(cmd, json!({"value": v}));
            }
        }
    });
}

fn transport(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let playing = app.is_playing();
    let recording = app.engine.transport.recording;
    let looped = app.engine.session().edit.loop_playback;
    group(ui, 176.0, 64.0, |ui, g| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(3.0, 4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if icon_button(ui, vec2(38.0, 28.0), "loop", looped, "Loop Playback (Cmd+Shift+L)").clicked() {
                    let _ = app.run("options.loop_playback", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, key_fill(&resp));
                icons::draw(ui.painter(), r.shrink(5.0), "stop", if playing { t.text } else { t.accent });
                if resp.on_hover_text("Stop (Space)").clicked() {
                    let _ = app.run("transport.stop", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, if playing { Color32::from_rgb(40, 90, 50) } else { key_fill(&resp) });
                icons::draw(ui.painter(), r.shrink(5.0), "play", if playing { Color32::from_rgb(120, 255, 140) } else { t.counter_text });
                if resp.on_hover_text("Play (Space)").clicked() {
                    let _ = app.run("transport.play", json!({}));
                }
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 28.0), Sense::click());
                key(ui.painter(), r, &resp, if recording { Color32::from_rgb(110, 30, 30) } else { key_fill(&resp) });
                icons::draw(ui.painter(), r.shrink(5.0), "record", t.rec);
                let resp = resp.on_hover_text("Record (Cmd+Space)");
                if let Some(why) = app.record_hint() {
                    // Below the whole group, so the hint covers none of its buttons.
                    let below = Rect::from_min_max(r.min, pos2(r.max.x, g.max.y));
                    egui::Tooltip::always_open(ui.ctx().clone(), resp.layer_id, resp.id, below).show(|ui| ui.label(why));
                }
                if resp.clicked() {
                    let _ = app.run("transport.record", json!({}));
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (icon, cmd, tip) in [
                    ("rtz", "transport.rtz", "Return to Zero (Home)"),
                    ("rewind", "transport.rewind", "Rewind"),
                    ("ffwd", "transport.fast_forward", "Fast Forward"),
                    ("end", "transport.go_to_end", "Go to End (End)"),
                ] {
                    if icon_button(ui, vec2(38.0, 22.0), icon, false, tip).clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                }
            });
        });
    });
}

fn tempo_meter(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (bpm, meter, countoff, click, bars) = {
        let s = app.engine.session();
        let tick = s.tempo.samples_to_ticks(app.position(), s.sample_rate);
        let m = s.tempo.meter_at_tick(tick);
        (s.tempo.tempo_at_tick(tick), format!("{}/{}", m.numerator, m.denominator), s.edit.countoff, s.edit.click, s.edit.countoff_bars)
    };
    let (r, _) = ui.allocate_exact_size(vec2(150.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let co = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 6.0), vec2(64.0, 15.0));
    let resp = ui.interact(co, ui.id().with("countoff"), Sense::click());
    key(ui.painter(), co, &resp, if countoff { t.mode_on } else { t.counter_off });
    ui.painter().text(co.center(), Align2::CENTER_CENTER, "Count Off", bold(10.0), if countoff { t.mode_on_text } else { t.mode_off_text });
    if resp.clicked() {
        let _ = app.run("options.countoff", json!({}));
    }
    ui.painter().text(pos2(r.max.x - 8.0, co.center().y), Align2::RIGHT_CENTER, format!("{bars} bars"), mono(11.0), t.counter_text);
    ui.painter().text(pos2(r.min.x + 70.0, r.min.y + 32.0), Align2::RIGHT_CENTER, "Meter", regular(11.0), t.counter_label);
    ui.painter().text(pos2(r.max.x - 8.0, r.min.y + 32.0), Align2::RIGHT_CENTER, meter, mono(11.0), t.counter_text);
    ui.painter().text(pos2(r.min.x + 70.0, r.min.y + 48.0), Align2::RIGHT_CENTER, "Tempo", regular(11.0), t.counter_label);
    icons::draw(ui.painter(), Rect::from_center_size(pos2(r.min.x + 82.0, r.min.y + 48.0), vec2(13.0, 13.0)), "note", t.counter_text);
    tempo_value(app, ui, r.max.x - 8.0, r.min.y + 48.0, bpm);
    let clk = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 40.0), vec2(18.0, 18.0));
    icons::draw(ui.painter(), clk, "click", if click { t.counter_text } else { t.text_dim });
    if ui.interact(clk, ui.id().with("click"), Sense::click()).on_hover_text("Click").clicked() {
        let _ = app.run("options.click", json!({}));
    }
}

/// The tempo in effect at the playhead, right-aligned at `right`: drag it (Shift for fine steps),
/// or click and type.
fn tempo_value(app: &mut SoundApp, ui: &mut Ui, right: f32, y: f32, bpm: f64) {
    let t = Tokens::current();
    let w = ui.painter().layout_no_wrap(format!("{bpm:.4}"), mono(11.0), t.counter_text).size().x;
    let rect = Rect::from_min_max(pos2(right - w - 4.0, y - 8.0), pos2(right + 4.0, y + 8.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    c.style_mut().override_font_id = Some(mono(11.0));
    let v = c.visuals_mut();
    v.override_text_color = Some(t.counter_text);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.weak_bg_fill = Color32::TRANSPARENT;
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.counter_border);
    let mut value = bpm;
    let resp = c.add_sized(rect.size(), egui::DragValue::new(&mut value).speed(0.1).range(5.0..=1000.0).fixed_decimals(4));
    if resp.changed() {
        let s = app.engine.session();
        let tick = s.tempo.samples_to_ticks(app.position(), s.sample_rate);
        let event = s.tempo.tempos().iter().rev().find(|e| e.tick <= tick).map_or(0, |e| e.tick);
        let at = s.tempo.tick_to_samples(event, s.sample_rate);
        let _ = app.engine.execute_merged("event.tempo", &json!({"bpm": value, "at": at}), "toolbar_tempo");
    }
    if resp.drag_stopped() || resp.lost_focus() {
        app.engine.end_merge();
    }
    resp.on_hover_text("Drag to change the tempo (Shift: fine steps), or click to type it");
}

fn output_meter(app: &mut SoundApp, ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(26.0, 64.0), Sense::hover());
    let m = app.main_meter;
    let w = (r.width() - 6.0) / 2.0;
    for c in 0..2 {
        let rr = Rect::from_min_size(pos2(r.min.x + 2.0 + c as f32 * (w + 2.0), r.min.y), vec2(w, r.height()));
        crate::widgets::meter(ui, rr, m.level[c], m.hold[c], m.clip);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod counter_input_tests {
    use super::*;
    use crate::Services;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};
    use egui_kittest::{Harness, kittest::Queryable};
    use soundcraft_engine::Engine;
    use soundcraft_time::Range;

    fn app() -> SoundApp {
        SoundApp::new(Engine::default(), None, Services::default())
    }

    fn click(harness: &mut Harness<'_, SoundApp>, pos: Pos2) {
        for pressed in [true, false] {
            harness.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            harness.run();
        }
    }

    #[test]
    fn first_counter_click_selects_all_and_second_click_selects_a_unit() {
        let mut fonts_ready = false;
        let mut harness = Harness::builder().with_size(vec2(500.0, 100.0)).build_ui_state(
            move |ui, app| {
                if !fonts_ready {
                    crate::fonts::install(ui.ctx());
                    fonts_ready = true;
                    return;
                }
                counters(app, ui);
            },
            app(),
        );
        harness.run();
        let rect = harness.get_by_label("Main Counter").rect();
        click(&mut harness, pos2(rect.max.x - 4.0, rect.center().y));
        assert_eq!(harness.state().counter_edit.as_ref().and_then(|edit| edit.unit), None);
        click(&mut harness, pos2(rect.max.x - 4.0, rect.center().y));
        assert!(harness.state().counter_edit.as_ref().and_then(|edit| edit.unit).is_some());
    }

    #[test]
    fn another_focused_field_keeps_the_counter_shortcut() {
        let ctx = egui::Context::default();
        let mut app = app();
        let raw = RawInput { events: vec![Event::Text("*".into())], ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("another_text_field")));
            handle_counter_keyboard(&mut app, ui);
        });
        output.textures_delta.clear();
        assert!(app.counter_edit.is_none());
    }

    #[test]
    fn typing_is_local_until_enter_commits_the_selection() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.run("view.main_counter", json!({"format": "samples"})).unwrap();
        app.engine.session_mut().edit.selection = Range::new(100, 200);
        app.counter_edit = Some(CounterEdit::new(CounterTarget::Start, TimeFormat::Samples, 100, None));
        let raw = RawInput { events: vec![Event::Text("150".into())], ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            ui.memory_mut(|memory| memory.request_focus(counter_id(CounterTarget::Start)));
            handle_counter_keyboard(&mut app, ui);
        });
        output.textures_delta.clear();
        assert_eq!(app.engine.session().edit.selection, Range::new(100, 200));
        assert_eq!(app.counter_edit.as_ref().map(|edit| edit.value), Some(150));

        let enter = Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let mut output = ctx.run_ui(RawInput { events: vec![enter], ..Default::default() }, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert_eq!(app.engine.session().edit.selection, Range::new(150, 200));
        assert!(app.counter_edit.is_none());
    }

    #[test]
    fn changing_the_main_timebase_cancels_an_old_edit() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.counter_edit = Some(CounterEdit::new(CounterTarget::Main, TimeFormat::MinSecs, 0, None));
        app.run("view.main_counter", json!({"format": "timecode"})).unwrap();
        let mut output = ctx.run_ui(RawInput::default(), |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert!(app.counter_edit.is_none());
    }

    #[test]
    fn arrows_keep_editing_active_without_transient_widget_focus() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.counter_edit = Some(CounterEdit::new(CounterTarget::Main, TimeFormat::MinSecs, 48_000, Some(1)));
        let right = Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let mut output = ctx.run_ui(RawInput { events: vec![right], ..Default::default() }, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert_eq!(app.counter_edit.as_ref().and_then(|edit| edit.unit), Some(2));

        let down = Event::Key { key: Key::ArrowDown, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let mut output = ctx.run_ui(RawInput { events: vec![down], ..Default::default() }, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert!(app.counter_edit.is_some());
    }

    #[test]
    fn main_entry_inside_a_selection_moves_to_a_point() {
        let mut app = app();
        app.engine.session_mut().edit.selection = Range::new(100, 300);
        apply_counter_value(&mut app, CounterTarget::Main, 200);
        assert_eq!(app.engine.session().edit.selection, Range::point(200));
        assert_eq!(app.engine.session().edit.playhead, 200);
    }

    #[test]
    fn crossing_a_selection_boundary_collapses_it_at_the_edited_value() {
        let mut app = app();
        app.engine.session_mut().edit.selection = Range::new(100, 200);
        apply_counter_value(&mut app, CounterTarget::Start, 250);
        assert_eq!(app.engine.session().edit.selection, Range::point(250));

        app.engine.session_mut().edit.selection = Range::new(100, 200);
        apply_counter_value(&mut app, CounterTarget::End, 50);
        assert_eq!(app.engine.session().edit.selection, Range::point(50));
    }

    #[test]
    fn slash_commits_one_selection_field_and_moves_to_the_next() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.run("view.main_counter", json!({"format": "samples"})).unwrap();
        app.engine.session_mut().edit.selection = Range::new(100, 300);
        app.counter_edit = Some(CounterEdit::new(CounterTarget::Start, TimeFormat::Samples, 100, None));

        let raw = RawInput { events: vec![Event::Text("200/".into())], ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert_eq!(app.engine.session().edit.selection, Range::new(200, 300));
        assert_eq!(app.counter_edit.as_ref().map(|edit| (edit.target, edit.value)), Some((CounterTarget::End, 300)));
    }

    #[test]
    fn calculator_entry_applies_and_closes_on_the_first_enter() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.run("view.main_counter", json!({"format": "samples"})).unwrap();
        app.engine.session_mut().edit.selection = Range::new(1_000, 3_000);
        app.counter_edit = Some(CounterEdit::new(CounterTarget::Start, TimeFormat::Samples, 1_000, None));

        let raw = RawInput { events: vec![Event::Text("+250".into())], ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert_eq!(app.counter_edit.as_ref().map(|edit| edit.value), Some(1_250));
        assert_eq!(app.engine.session().edit.selection, Range::new(1_000, 3_000));

        let enter = Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let mut output = ctx.run_ui(RawInput { events: vec![enter], ..Default::default() }, |ui| handle_counter_keyboard(&mut app, ui));
        output.textures_delta.clear();
        assert_eq!(app.engine.session().edit.selection.start, 1_250);
        assert!(app.counter_edit.is_none());
    }
}
