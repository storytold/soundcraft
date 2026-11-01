//! The Edit window toolbar.

use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::{icon_button, key, key_fill};
use crate::{SoundApp, icons};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit::{SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{EditMode, Tool};
use soundcraft_time::{TimeFormat, format_length};

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

struct ModeCellState {
    cur: EditMode,
    snap: bool,
    locked: bool,
}

fn edit_mode_tooltip(m: EditMode, locked: bool, cmd: &str, fkey: &str) -> String {
    match m {
        EditMode::Shuffle if locked => format!("Shuffle is locked ({cmd}-click to unlock)"),
        EditMode::Shuffle => format!("Shuffle mode ({fkey}); {cmd}-click locks it out"),
        EditMode::Grid => format!("Grid mode ({fkey}; again for Relative); Shift-click: Snap to Grid in the other modes"),
        _ => format!("{} mode ({fkey}); Shift-click adds Snap to Grid", if m == EditMode::Slip { "Slip" } else { "Spot" }),
    }
}

fn draw_mode_cell(p: &egui::Painter, cell: Rect, resp: &egui::Response, t: &Tokens, m: EditMode, state: &ModeCellState, label: &str) {
    let on = state.cur == m || (m == EditMode::Grid && state.cur == EditMode::GridRelative);
    key(p, cell, resp, if on { t.mode_on } else { t.mode_off });
    let fg = if on { t.mode_on_text } else { t.mode_off_text };
    if m == EditMode::Grid && state.snap {
        p.rect_stroke(cell, 1.0, Stroke::new(1.5, t.mode_on), StrokeKind::Inside);
    }
    if m == EditMode::Shuffle && state.locked {
        icons::draw(p, Rect::from_center_size(cell.center(), vec2(15.0, 15.0)), "lock", fg);
    } else {
        let txt = if m == EditMode::Grid && state.cur == EditMode::GridRelative { "REL GRID" } else { label };
        p.text(cell.center(), Align2::CENTER_CENTER, txt, bold(10.5), fg);
    }
}

/// The Shuffle, Spot, Slip and Grid mode selector with its keyboard hints and relative-grid state.
fn edit_modes(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let ed = &app.engine.session().edit;
    let state = ModeCellState { cur: ed.edit_mode, snap: ed.flag(SNAP_TO_GRID), locked: ed.flag(SHUFFLE_LOCK) };
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
            let resp = ui.interact(cell, ui.scope_id().with(("mode", label)), Sense::click());
            draw_mode_cell(ui.painter(), cell, &resp, &t, m, &state, label);
            let on = state.cur == m || (m == EditMode::Grid && state.cur == EditMode::GridRelative);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, label));
            if resp.clicked() {
                mode_click(app, m, ui.input(|i| i.modifiers));
            }
            resp.on_hover_text(edit_mode_tooltip(m, state.locked, &cmd, fkey));
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

/// The tool selector, including Smart-tool membership and Zoom Toggle.
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
            let resp = ui.interact(bracket.expand2(vec2(0.0, 2.0)), ui.scope_id().with("smart"), Sense::click());
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

/// Main and sub position counters, a main-counter format menu, selection readouts and dirty/device status.
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
    let resp = ui.interact(main_r, ui.scope_id().with("main_counter"), Sense::click());
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

/// Grid-line toggle, grid value menu and nudge value menu.
fn grid_nudge(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let (grid, nudge, lines) = {
        let e = &app.engine.session().edit;
        (e.grid, e.nudge, e.grid_lines)
    };
    let (r, _) = ui.allocate_exact_size(vec2(170.0, 64.0), Sense::hover());
    ui.painter().rect(r, CornerRadius::same(4), t.counter_bg, Stroke::new(1.0, t.counter_border), StrokeKind::Inside);
    let gl = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 8.0), vec2(48.0, 16.0));
    let resp = ui.interact(gl, ui.scope_id().with("grid_lines"), Sense::click());
    key(ui.painter(), gl, &resp, if lines { t.mode_on } else { t.counter_off });
    ui.painter().text(gl.center(), Align2::CENTER_CENTER, "Grid", bold(11.0), if lines { t.mode_on_text } else { t.mode_off_text });
    if resp.on_hover_text("Show grid lines").clicked() {
        let _ = app.run("view.grid_lines", json!({}));
    }
    let gr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 6.0), pos2(r.max.x - 4.0, r.min.y + 26.0));
    ui.painter().text(pos2(gr.max.x - 14.0, gr.center().y), Align2::RIGHT_CENTER, grid.label(), mono(11.0), t.counter_text);
    let gresp = ui.interact(gr, ui.scope_id().with("grid_value"), Sense::click());
    value_menu(app, &gresp, "view.grid");
    ui.painter().text(pos2(r.min.x + 10.0, r.min.y + 44.0), Align2::LEFT_CENTER, "Nudge", regular(11.0), t.counter_label);
    let nr = Rect::from_min_max(pos2(r.min.x + 60.0, r.min.y + 34.0), pos2(r.max.x - 4.0, r.min.y + 54.0));
    ui.painter().text(pos2(nr.max.x - 14.0, nr.center().y), Align2::RIGHT_CENTER, nudge.label(), mono(11.0), t.counter_text);
    let nresp = ui.interact(nr, ui.scope_id().with("nudge_value"), Sense::click());
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
                // macOS gives Cmd+Space to Spotlight unless the user frees it, so name F12 first.
                let resp = resp.on_hover_text("Record (F12 or Cmd+Space)");
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

/// Count-off, meter and tempo readouts; the tempo value is editable in place for the tempo event at or before the
/// playhead.
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
    let resp = ui.interact(co, ui.scope_id().with("countoff"), Sense::click());
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
    if ui.interact(clk, ui.scope_id().with("click"), Sense::click()).on_hover_text("Click").clicked() {
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
    let resp =
        c.add_sized(rect.size(), egui::DragValue::new(&mut value).speed(0.1).range(5.0..=1000.0).fixed_decimals(4)).accessible_name("Tempo (BPM)");
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
