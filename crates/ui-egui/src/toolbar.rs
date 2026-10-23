//! The Edit window toolbar.

use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::{icon_button, key, key_fill};
use crate::{SoundApp, icons};
use egui::{Align2, Color32, CornerRadius, Painter, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit::{SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{EditMode, Session, Tool};
use soundcraft_time::{FeetFramesValue, GridScale, GridValue, NoteValue, TimeFormat, TimecodeValue, format_length};

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
        (s.edit.main_counter, s.edit.sub_counter.unwrap_or(TimeFormat::Samples), s.edit.selection)
    };
    let pos = app.position();
    let (r, _) = ui.allocate_exact_size(vec2(318.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let main_r = Rect::from_min_max(r.min, pos2(r.min.x + 180.0, r.min.y + 44.0));
    let txt = fmt_pos(app, pos, main_fmt);
    ui.painter().text(pos2(main_r.max.x - 18.0, main_r.center().y), Align2::RIGHT_CENTER, txt, mono(26.0), t.counter_text);
    let resp = ui.interact(main_r, ui.id().with("main_counter"), Sense::click());
    icons::draw(ui.painter(), Rect::from_center_size(pos2(main_r.max.x - 9.0, main_r.center().y), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    egui::Popup::menu(&resp).show(|ui| {
        for f in TimeFormat::ALL {
            if ui.selectable_label(main_fmt == f, f.label()).clicked() {
                let _ = app.run("view.main_counter", json!({"format": f.id()}));
            }
        }
    });
    // Start / End / Length.
    let s = app.engine.session();
    let rows = [
        ("Start", fmt_pos(app, sel.start, main_fmt)),
        ("End", fmt_pos(app, sel.end, main_fmt)),
        ("Length", format_length(sel.start, sel.len(), main_fmt, s.sample_rate, &s.tempo, s.frame_rate)),
    ];
    for (i, (label, val)) in rows.iter().enumerate() {
        let y = r.min.y + 10.0 + i as f32 * 13.0;
        ui.painter().text(pos2(r.min.x + 214.0, y), Align2::RIGHT_CENTER, *label, regular(11.0), t.counter_label);
        ui.painter().text(pos2(r.max.x - 8.0, y), Align2::RIGHT_CENTER, val, mono(11.0), t.counter_text);
    }
    // Sub counter row.
    let y = r.max.y - 11.0;
    ui.painter().line_segment([pos2(r.min.x + 6.0, r.max.y - 21.0), pos2(r.max.x - 6.0, r.max.y - 21.0)], Stroke::new(1.0, t.counter_rule));
    ui.painter().text(pos2(r.min.x + 10.0, y), Align2::LEFT_CENTER, "Sub", regular(11.0), t.counter_label);
    ui.painter().text(pos2(r.min.x + 150.0, y), Align2::RIGHT_CENTER, fmt_pos(app, pos, sub_fmt), mono(11.0), t.counter_text);
    let st = if app.engine.is_dirty() { "modified" } else { "saved" };
    let dev = app.player.as_ref().map_or("no audio engine", |p| if p.silent { "silent clock" } else { "audio on" });
    ui.painter().text(pos2(r.max.x - 8.0, y), Align2::RIGHT_CENTER, format!("{st} · {dev}"), regular(10.0), t.text_dim);
}

fn grid_nudge(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (grid, nudge, lines, grid_follow, nudge_follow, clips_markers) = {
        let e = &app.engine.session().edit;
        (e.grid, e.nudge, e.grid_lines, e.grid_follow_main, e.nudge_follow_main, e.grid_clips_markers)
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
    let grid_label = if clips_markers { GridScale::ClipsMarkers.label().to_string() } else { compact_grid_label(app.engine.session(), grid) };
    draw_compact_note_icon(ui.painter(), gr, grid, t.counter_text);
    ui.painter().text(pos2(gr.max.x - 14.0, gr.center().y), Align2::RIGHT_CENTER, grid_label, mono(11.0), t.counter_text);
    let gresp = ui.interact(gr, ui.id().with("grid_value"), Sense::click());
    value_menu(app, &gresp, "view.grid", grid, grid_follow, clips_markers, true);
    ui.painter().text(pos2(r.min.x + 10.0, r.min.y + 44.0), Align2::LEFT_CENTER, "Nudge", regular(11.0), t.counter_label);
    let nr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 34.0), pos2(r.max.x - 4.0, r.min.y + 54.0));
    draw_compact_note_icon(ui.painter(), nr, nudge, t.counter_text);
    ui.painter().text(
        pos2(nr.max.x - 14.0, nr.center().y),
        Align2::RIGHT_CENTER,
        compact_grid_label(app.engine.session(), nudge),
        mono(11.0),
        t.counter_text,
    );
    let nresp = ui.interact(nr, ui.id().with("nudge_value"), Sense::click());
    value_menu(app, &nresp, "view.nudge", nudge, nudge_follow, false, false);
    for rr in [gr, nr] {
        icons::draw(ui.painter(), Rect::from_center_size(pos2(rr.max.x - 6.0, rr.center().y), vec2(10.0, 10.0)), "triangle_down", t.text_dim);
    }
}

fn compact_grid_label(session: &Session, value: GridValue) -> String {
    match value {
        GridValue::Note { value, dotted, triplet } => {
            let at_tick = session.tempo.samples_to_ticks(session.edit.selection.start, session.sample_rate);
            let meter = session.tempo.meter_at_tick(at_tick);
            let mut ticks = if value == NoteValue::Bar { meter.ticks_per_bar() } else { value.ticks() };
            if dotted {
                ticks = ticks * 3 / 2;
            }
            if triplet {
                ticks = ticks * 2 / 3;
            }
            let per_bar = meter.ticks_per_bar().max(1);
            let per_beat = meter.ticks_per_beat().max(1);
            let bars = ticks / per_bar;
            let remaining = ticks % per_bar;
            format!("{bars}|{}|{:03}", remaining / per_beat, remaining % per_beat)
        }
        GridValue::Seconds(seconds) => {
            let samples = session.sample_rate.samples(seconds);
            format_length(session.edit.selection.start, samples, TimeFormat::MinSecs, session.sample_rate, &session.tempo, session.frame_rate)
        }
        GridValue::Frames(frames) => {
            let samples = session.frame_rate.samples_at_frame(frames, session.sample_rate);
            format!(
                "{}.00",
                format_length(session.edit.selection.start, samples, TimeFormat::Timecode, session.sample_rate, &session.tempo, session.frame_rate)
            )
        }
        GridValue::Timecode(value) => match value {
            TimecodeValue::Second => "00:00:01:00.00",
            TimecodeValue::FiveFrames => "00:00:00:05.00",
            TimecodeValue::Frame => "00:00:00:01.00",
            TimecodeValue::HalfFrame => "00:00:00:00.50",
            TimecodeValue::QuarterFrame => "00:00:00:00.25",
            TimecodeValue::SubFrame => "00:00:00:00.01",
        }
        .to_string(),
        GridValue::FeetFrames(value) => match value {
            FeetFramesValue::Foot => "1+00",
            FeetFramesValue::Frame => "0+01",
            FeetFramesValue::QuarterFrame => "0+00.25",
            FeetFramesValue::SubFrame => "0+00.01",
        }
        .to_string(),
        GridValue::Samples(samples) => samples.to_string(),
    }
}

fn draw_compact_note_icon(painter: &Painter, rect: Rect, value: GridValue, color: Color32) {
    let GridValue::Note { value, dotted, triplet } = value else { return };
    draw_note_value_icon(painter, Rect::from_min_size(pos2(rect.min.x + 1.0, rect.min.y + 2.0), vec2(14.0, 16.0)), value, dotted, triplet, color);
}

fn draw_note_value_icon(painter: &Painter, rect: Rect, value: NoteValue, dotted: bool, triplet: bool, color: Color32) {
    let stroke = Stroke::new(1.2, color);
    let head = pos2(rect.min.x + 4.0, rect.max.y - 3.5);
    if value == NoteValue::Bar {
        painter.line_segment([pos2(rect.min.x + 2.0, rect.min.y + 2.0), pos2(rect.min.x + 2.0, rect.max.y - 2.0)], stroke);
        painter.line_segment([pos2(rect.min.x + 5.0, rect.min.y + 2.0), pos2(rect.min.x + 5.0, rect.max.y - 2.0)], stroke);
        painter.line_segment([pos2(rect.min.x + 2.0, rect.min.y + 3.0), pos2(rect.min.x + 10.0, rect.min.y + 3.0)], stroke);
    } else {
        let hollow = matches!(value, NoteValue::Whole | NoteValue::Half);
        if hollow {
            painter.circle_stroke(head, 2.2, stroke);
        } else {
            painter.circle_filled(head, 2.2, color);
        }
        if value != NoteValue::Whole {
            let stem_x = head.x + 2.0;
            painter.line_segment([pos2(stem_x, head.y), pos2(stem_x, rect.min.y + 2.0)], stroke);
            let flags = match value {
                NoteValue::Eighth => 1,
                NoteValue::Sixteenth => 2,
                NoteValue::ThirtySecond => 3,
                NoteValue::SixtyFourth => 4,
                _ => 0,
            };
            for flag in 0..flags {
                let y = rect.min.y + 2.0 + flag as f32 * 2.0;
                painter.line_segment([pos2(stem_x, y), pos2(stem_x + 4.0, y + 2.5)], stroke);
            }
        }
    }
    if dotted {
        painter.circle_filled(pos2(rect.max.x - 1.5, head.y), 1.0, color);
    }
    if triplet {
        painter.text(pos2(rect.min.x, rect.min.y), Align2::LEFT_TOP, "3", regular(7.0), color);
    }
}

fn value_menu(app: &mut SoundApp, resp: &egui::Response, cmd: &str, current: GridValue, follow_main: bool, clips_markers: bool, allow_clips: bool) {
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(210.0);
        match current.scale() {
            GridScale::BarsBeats => {
                let selected = match current {
                    GridValue::Note { value, .. } => value,
                    _ => NoteValue::Sixteenth,
                };
                for value in NoteValue::ALL {
                    let id = match value {
                        NoteValue::Bar => "1 bar",
                        NoteValue::Whole => "1/1",
                        NoteValue::Half => "1/2",
                        NoteValue::Quarter => "1/4",
                        NoteValue::Eighth => "1/8",
                        NoteValue::Sixteenth => "1/16",
                        NoteValue::ThirtySecond => "1/32",
                        NoteValue::SixtyFourth => "1/64",
                    };
                    menu_note_value(ui, app, cmd, selected == value, value, json!(id));
                }
                ui.separator();
                let (value, dotted, triplet) = match current {
                    GridValue::Note { value, dotted, triplet } => (value, dotted, triplet),
                    _ => (NoteValue::Sixteenth, false, false),
                };
                let base = match value {
                    NoteValue::Bar => "1 bar",
                    NoteValue::Whole => "1/1",
                    NoteValue::Half => "1/2",
                    NoteValue::Quarter => "1/4",
                    NoteValue::Eighth => "1/8",
                    NoteValue::Sixteenth => "1/16",
                    NoteValue::ThirtySecond => "1/32",
                    NoteValue::SixtyFourth => "1/64",
                };
                menu_modifier(ui, app, cmd, dotted, "Dotted", "·", json!(if dotted { base.to_string() } else { format!("{base}.") }));
                menu_modifier(ui, app, cmd, triplet, "Triplet", "3", json!(if triplet { base.to_string() } else { format!("{base}t") }));
            }
            GridScale::MinSecs => {
                for seconds in [1.0, 0.5, 0.1, 0.01, 0.001] {
                    let value = GridValue::Seconds(seconds);
                    menu_value(ui, app, cmd, current == value, &value.label(), json!({"seconds": seconds}));
                }
            }
            GridScale::Timecode => {
                for value in TimecodeValue::ALL {
                    let selected = current == GridValue::Timecode(value)
                        || matches!(
                            (current, value),
                            (GridValue::Frames(5), TimecodeValue::FiveFrames) | (GridValue::Frames(1), TimecodeValue::Frame)
                        );
                    menu_value(ui, app, cmd, selected, value.label(), json!({"timecode": value.id()}));
                }
            }
            GridScale::FeetFrames => {
                for value in FeetFramesValue::ALL {
                    menu_value(ui, app, cmd, current == GridValue::FeetFrames(value), value.label(), json!({"feet_frames": value.id()}));
                }
            }
            GridScale::Samples => {
                for samples in [10_000, 1_000, 100, 10, 2, 1] {
                    let value = GridValue::Samples(samples);
                    menu_value(ui, app, cmd, current == value, &value.label(), json!({"samples": samples}));
                }
            }
            GridScale::ClipsMarkers => {}
        }
        ui.separator();
        for scale in GridScale::TIME {
            let selected = !clips_markers && current.scale() == scale;
            if ui.selectable_label(selected, scale.label()).clicked() {
                let _ = app.run(cmd, json!({"format": scale.id(), "follow_main": false}));
                ui.close();
            }
        }
        if allow_clips && ui.selectable_label(clips_markers, GridScale::ClipsMarkers.label()).clicked() {
            let _ = app.run(cmd, json!({"format": GridScale::ClipsMarkers.id(), "follow_main": false}));
            ui.close();
        }
        ui.separator();
        if ui.selectable_label(follow_main, "Follow Main Time Scale").clicked() {
            let _ = app.run(cmd, json!({"follow_main": !follow_main}));
            ui.close();
        }
    });
}

fn menu_value(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, label: &str, value: serde_json::Value) {
    if ui.selectable_label(selected, label).clicked() {
        let _ = app.run(cmd, json!({"value": value}));
        ui.close();
    }
}

fn menu_note_value(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, note: NoteValue, value: serde_json::Value) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        draw_note_value_icon(ui.painter(), rect, note, false, false, ui.visuals().text_color());
        menu_value(ui, app, cmd, selected, note.label(), value);
    });
}

fn menu_modifier(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, label: &str, symbol: &str, value: serde_json::Value) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, symbol, regular(11.0), ui.visuals().text_color());
        menu_value(ui, app, cmd, selected, label, value);
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
mod tests {
    use super::*;
    use crate::Services;
    use egui_kittest::{Harness, kittest::Queryable};

    fn harness() -> Harness<'static, SoundApp> {
        let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let mut harness = Harness::builder().with_size(vec2(700.0, 700.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let edit = &app.engine.session().edit;
                let (grid, grid_follow, clips, nudge, nudge_follow) =
                    (edit.grid, edit.grid_follow_main, edit.grid_clips_markers, edit.nudge, edit.nudge_follow_main);
                let grid_anchor = ui.button("Grid menu");
                value_menu(app, &grid_anchor, "view.grid", grid, grid_follow, clips, true);
                let nudge_anchor = ui.button("Nudge menu");
                value_menu(app, &nudge_anchor, "view.nudge", nudge, nudge_follow, false, false);
            },
            app,
        );
        harness.run_steps(3);
        harness
    }

    fn open(harness: &mut Harness<SoundApp>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    #[test]
    fn compact_grid_values_use_their_counter_formats() {
        let session = Session::default();
        let note = |value, dotted, triplet| GridValue::Note { value, dotted, triplet };
        assert_eq!(compact_grid_label(&session, note(NoteValue::Quarter, false, false)), "0|1|000");
        assert_eq!(compact_grid_label(&session, note(NoteValue::Eighth, false, true)), "0|0|320");
        assert_eq!(compact_grid_label(&session, GridValue::Seconds(0.001)), "0:00.001");
        assert_eq!(compact_grid_label(&session, GridValue::Timecode(TimecodeValue::Second)), "00:00:01:00.00");
        assert_eq!(compact_grid_label(&session, GridValue::Timecode(TimecodeValue::QuarterFrame)), "00:00:00:00.25");
        assert_eq!(compact_grid_label(&session, GridValue::FeetFrames(FeetFramesValue::Foot)), "1+00");
        assert_eq!(compact_grid_label(&session, GridValue::Samples(10_000)), "10000");
    }

    #[test]
    fn grid_menu_exposes_each_scale_and_keeps_the_increment_in_clips_markers_mode() {
        let mut harness = harness();
        open(&mut harness, "Grid menu");
        for label in ["Bars|Beats", "Min:Secs", "Timecode", "Feet+Frames", "Samples", "Clips/Markers", "Follow Main Time Scale"] {
            let _ = harness.get_by_label(label);
        }
        for label in ["1 bar", "Whole note", "1/2 note", "1/4 note", "1/8 note", "1/16 note", "1/32 note", "1/64 note", "Dotted", "Triplet"] {
            let _ = harness.get_by_label(label);
        }

        harness.get_by_label("Timecode").click();
        harness.run_steps(3);
        open(&mut harness, "Grid menu");
        for label in ["1 second", "5 frames", "1 frame", "1/2 frame", "1/4 frame", "1 subframe"] {
            let _ = harness.get_by_label(label);
        }
        harness.get_by_label("1/4 frame").click();
        harness.run_steps(3);
        open(&mut harness, "Grid menu");
        harness.get_by_label("Clips/Markers").click();
        harness.run_steps(3);
        let edit = &harness.state().engine.session().edit;
        assert_eq!(edit.grid, GridValue::Timecode(TimecodeValue::QuarterFrame));
        assert!(edit.grid_clips_markers);
    }

    #[test]
    fn nudge_menu_has_every_numeric_scale_but_no_clips_markers_mode() {
        let mut harness = harness();
        open(&mut harness, "Nudge menu");
        for label in ["Bars|Beats", "Min:Secs", "Timecode", "Feet+Frames", "Samples", "Follow Main Time Scale"] {
            let _ = harness.get_by_label(label);
        }
        assert_eq!(harness.query_all_by_label("Clips/Markers").count(), 0);
        harness.get_by_label("Samples").click();
        harness.run_steps(3);
        open(&mut harness, "Nudge menu");
        for label in ["10000 samples", "1000 samples", "100 samples", "10 samples", "2 samples", "1 sample"] {
            let _ = harness.get_by_label(label);
        }
    }
}
