//! The Edit window toolbar.

use crate::theme::{Tokens, bold, mono, regular};
use crate::widgets::shade;
use crate::{SoundApp, icons};
use egui::{Align2, Color32, CornerRadius, Painter, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::edit::{SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{EditMode, EditState, Session, Tool};
use soundcraft_time::{FeetFramesValue, GridScale, GridValue, NoteValue, TimeFormat, TimecodeValue, format_length};

mod quantize;

pub const HEIGHT: f32 = 82.0;

/// Fixed-height slots keep the two rows aligned even when the toolbar scrolls.
const CONTENT_HEIGHT: f32 = 76.0;
const DIVIDER_W: f32 = 1.5;

fn slot(ui: &mut Ui, width: f32) -> Rect {
    ui.allocate_exact_size(vec2(width, CONTENT_HEIGHT), Sense::hover()).0
}

fn at(r: Rect, x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::from_min_size(r.min + vec2(x, y), vec2(width, height))
}

struct Dividers {
    painter: egui::Painter,
    bounds: Rect,
}

impl Dividers {
    fn paint(&self, slot: Rect) {
        let gap = Rect::from_min_max(pos2(slot.max.x - DIVIDER_W, self.bounds.min.y), pos2(slot.max.x, self.bounds.max.y));
        self.painter.rect_filled(gap, 0.0, Color32::BLACK);
    }

    fn centered(&self, slot: Rect, size: egui::Vec2) -> Rect {
        let column = Rect::from_min_max(pos2(slot.min.x, self.bounds.min.y), pos2(slot.max.x - DIVIDER_W, self.bounds.max.y));
        Rect::from_center_size(column.center(), size)
    }
}

fn lcd(ui: &Ui, r: Rect) {
    let t = Tokens::current();
    ui.painter().rect(r, 1.0, t.counter_bg, Stroke::new(1.0, t.border), StrokeKind::Inside);
}

fn key_fill(resp: &egui::Response) -> Color32 {
    let face = Tokens::current().toolbar_button;
    if resp.hovered() { shade(face, 0.1) } else { face }
}

/// Toolbar keys have flat faces and square rims; interaction changes only their tint.
fn key(p: &egui::Painter, r: Rect, resp: &egui::Response, fill: Color32) {
    let fill = if resp.is_pointer_button_down_on() { shade(fill, -0.12) } else { fill };
    p.rect(r, 0.0, fill, Stroke::new(1.0, Tokens::current().button_border), StrokeKind::Inside);
}

/// A positioned toolbar key shares keyboard focus, accessibility and pressed/selected feedback.
fn icon_at(ui: &mut Ui, r: Rect, icon: &str, on: bool, tip: &str, tint: Color32) -> egui::Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.scope_id().with(tip), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, tip));
    let fill = if on { t.toolbar_selected } else { key_fill(&resp) };
    let color = if on { t.toolbar_selected_icon } else { tint };
    key(ui.painter(), r, &resp, fill);
    let size = r.height().min(r.width()) * 0.86;
    let wide = matches!(
        icon,
        "zoom_toggle"
            | "tab_transient"
            | "link_timeline"
            | "link_track"
            | "insertion_playback"
            | "automation_edit"
            | "markers_edit"
            | "mirrored_midi"
            | "layered_edit"
    );
    let icon_size = if r.height() <= 20.0 && wide { vec2(r.width() * 0.8, r.height() * 0.74) } else { vec2(size, size) };
    let color = if ui.is_enabled() { color } else { shade(color, -0.5) };
    icons::draw_toolbar(ui.painter(), Rect::from_center_size(r.center(), icon_size), icon, color);
    if resp.has_focus() {
        ui.painter().rect_stroke(r.shrink(1.0), 1.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    resp.on_hover_text(tip)
}

fn fit_session(app: &mut SoundApp) {
    let [x0, _, x1, _] = app.edit_layout.timeline;
    let width = if x1 - x0 > 0.0 { x1 - x0 } else { 1200.0 };
    let _ = app.run("view.zoom_fit", json!({"width_px": width}));
}

fn waveform_zoom(app: &mut SoundApp, ui: &mut Ui, r: Rect) {
    let t = Tokens::current();
    let top = Rect::from_min_max(r.min, pos2(r.max.x, r.center().y));
    let bottom = Rect::from_min_max(pos2(r.min.x, r.center().y), r.max);
    let up = ui.interact(top, ui.scope_id().with("waveform_zoom_in"), Sense::click());
    let down = ui.interact(bottom, ui.scope_id().with("waveform_zoom_out"), Sense::click());
    let response = up.clone().union(down.clone());
    key(ui.painter(), r, &response, key_fill(&response));
    icons::draw_toolbar(ui.painter(), r.shrink(1.0), "wave_zoom", t.toolbar_icon);
    for (resp, label, factor) in [(up, "Waveform zoom in (Cmd+Alt+])", 2.0), (down, "Waveform zoom out (Cmd+Alt+[)", 0.5)] {
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
        if resp.clicked() {
            let _ = app.run("view.waveform_zoom", json!({"factor": factor}));
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(resp.rect.shrink(1.0), 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        resp.on_hover_text(label);
    }
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let dividers = Dividers { painter: ui.painter().with_clip_rect(ui.clip_rect().intersect(full)), bounds: full };
    ui.painter().rect_filled(full, 0.0, t.toolbar_bg);
    ui.add_space(2.0);
    egui::ScrollArea::horizontal()
        .id_salt("edit_toolbar_scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                edit_modes(app, ui, &dividers);
                zoom_group(app, ui, &dividers);
                tools(app, ui, &dividers);
                counters(app, ui, &dividers);
                grid_nudge(app, ui, &dividers);
                transport(app, ui, &dividers);
                output_meter(app, ui, &dividers);
                quantize::show(app, ui);
                if app.engine.session().edit.flag("view.transport.midi_controls") {
                    ui.add_space(12.0);
                    tempo_meter(app, ui);
                }
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
    let fill = if on && m == EditMode::Grid {
        t.mode_grid
    } else if on {
        t.toolbar_readout
    } else {
        t.mode_off
    };
    let fill = if resp.is_pointer_button_down_on() {
        shade(fill, -0.12)
    } else if resp.hovered() {
        shade(fill, 0.06)
    } else {
        fill
    };
    p.rect_filled(cell, 0.0, fill);
    let fg = if on { t.mode_on_text } else { t.toolbar_readout };
    if m == EditMode::Grid && state.snap {
        p.rect_stroke(cell.shrink(1.0), 0.0, Stroke::new(1.0, t.toolbar_readout), StrokeKind::Inside);
    }
    let mut label_rect = cell.shrink2(vec2(2.0, 0.0));
    if m == EditMode::Shuffle && state.locked {
        let lock = Rect::from_center_size(pos2(cell.min.x + 2.5, cell.center().y - 0.25), vec2(7.0, 8.5));
        icons::draw(p, lock, "lock", fg);
        label_rect.min.x = cell.min.x + 8.0;
        label_rect.max.x = cell.max.x - 3.0;
    }
    let txt = if m == EditMode::Grid && state.cur == EditMode::GridRelative { "REL GRID" } else { label };
    if m == EditMode::Grid {
        if state.cur == EditMode::GridRelative {
            label_rect.max.x = cell.max.x - 10.0;
        }
        let arrow = Rect::from_center_size(pos2(cell.max.x - 5.0, cell.center().y - 0.5), vec2(12.0, 12.0));
        icons::draw(p, arrow, "triangle_down", fg);
    }
    let width = p.layout_no_wrap(txt.into(), bold(8.5), fg).size().x;
    let size = 8.5 * (label_rect.width() / width.max(1.0)).min(1.0);
    p.text(label_rect.center(), Align2::CENTER_CENTER, txt, bold(size), fg);
    if resp.has_focus() {
        p.rect_stroke(cell.shrink(1.0), 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
}

/// The Shuffle, Spot, Slip and Grid mode selector with its keyboard hints and relative-grid state.
fn edit_modes(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
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
    let r = slot(ui, 120.0);
    dividers.paint(r);
    let well = at(r, 8.0, 18.0, 104.0, 42.0);
    ui.painter().rect_filled(well, 0.0, Color32::BLACK);
    {
        let inner = well.shrink2(vec2(7.0, 6.0));
        let cw = inner.width() / 2.0;
        let ch = inner.height() / 2.0;
        for (m, cx, cy, label, fkey) in modes {
            let cell = Rect::from_min_size(pos2(inner.min.x + cw * cx as f32, inner.min.y + ch * cy as f32), vec2(cw, ch));
            let resp = ui.interact(cell, ui.scope_id().with(("mode", label)), Sense::click());
            draw_mode_cell(ui.painter(), cell, &resp, &t, m, &state, label);
            let on = state.cur == m || (m == EditMode::Grid && state.cur == EditMode::GridRelative);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, label));
            if resp.clicked() {
                mode_click(app, m, ui.input(|i| i.modifiers));
            }
            resp.on_hover_text(edit_mode_tooltip(m, state.locked, &cmd, fkey));
        }
        // One continuous four-segment face on a black surround and shared dividers.
        let line = Stroke::new(0.75, Color32::BLACK);
        ui.painter().vline(inner.center().x, inner.y_range(), line);
        ui.painter().hline(inner.x_range(), inner.center().y, line);
        ui.painter().rect_stroke(inner, 0.0, line, StrokeKind::Inside);
    }
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

fn zoom_group(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let t = Tokens::current();
    let r = slot(ui, 102.0);
    dividers.paint(r);
    for (x, icon, id, tip) in [(8.0, "arrow_left", "view.zoom_out", "Zoom out (Cmd+[)"), (80.0, "arrow_right", "view.zoom_in", "Zoom in (Cmd+])")] {
        if icon_at(ui, at(r, x, 14.0, 12.0, 28.0), icon, false, tip, t.toolbar_icon).clicked() {
            let _ = app.run(id, json!({}));
        }
    }
    waveform_zoom(app, ui, at(r, 20.0, 14.0, 30.0, 28.0));
    if icon_at(ui, at(r, 50.0, 14.0, 30.0, 28.0), "zoom_fit", false, "Fill window with session (Alt+A)", t.toolbar_icon).clicked() {
        fit_session(app);
    }
    for i in 1..=5 {
        let cell = at(r, 8.0 + (i - 1) as f32 * 17.0, 53.0, 17.0, 18.0);
        let resp = ui.interact(cell, ui.scope_id().with(("zoom_preset", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), format!("Zoom preset {i}")));
        key(ui.painter(), cell, &resp, key_fill(&resp));
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, i.to_string(), bold(11.0), t.text);
        if resp.clicked() {
            let store = ui.input(|x| x.modifiers.command);
            let _ = app.run("view.zoom_preset", json!({"preset": i, "store": store}));
        }
        resp.on_hover_text("Zoom preset (click to recall, Cmd-click to store)");
    }
}

/// The tool selector, including Smart-tool membership and Zoom Toggle.
fn tools(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let t = Tokens::current();
    let cur = app.engine.session().edit.tool;
    let r = slot(ui, 310.0);
    dividers.paint(r);
    let smart = at(r, 92.0, 6.0, 120.0, 36.0);
    let smart_resp = ui.interact(at(r, 92.0, 6.0, 120.0, 8.0), ui.scope_id().with("smart"), Sense::click());
    smart_resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), cur == Tool::Smart, "Smart Tool (F6+F7 / F7+F8)"));
    ui.painter().rect(
        smart,
        1.0,
        if cur == Tool::Smart { t.toolbar_selected } else { t.tool_bracket_off },
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    ui.painter().rect_stroke(
        smart.shrink(1.0),
        1.0,
        Stroke::new(1.0, if cur == Tool::Smart { t.accent } else { t.border_light }),
        StrokeKind::Inside,
    );
    if smart_resp.has_focus() {
        ui.painter().rect_stroke(smart, 1.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    if smart_resp.on_hover_text("Smart Tool (F6+F7 / F7+F8)").clicked() {
        let _ = app.run("edit.tool", json!({"tool": "smart"}));
    }
    if icon_at(ui, at(r, 24.0, 19.0, 28.0, 18.0), "zoom_toggle", app.engine.session().edit.zoom_toggle.is_some(), "Zoom Toggle (E)", t.toolbar_icon)
        .clicked()
    {
        crate::edit_window::zoom_toggle(app);
    }
    // The tool faces meet their neighbours and the Smart frame without gaps.
    for (tool, icon, x, width) in [
        (Tool::Zoom, "zoom", 58.0, 34.0),
        (Tool::Trim, "trim", 100.0, 36.0),
        (Tool::Selector, "selector", 136.0, 36.0),
        (Tool::Grabber, "grabber", 172.0, 34.0),
        (Tool::Scrubber, "scrubber", 212.0, 36.0),
        (Tool::Pencil, "pencil", 248.0, 36.0),
    ] {
        let smart_member = cur == Tool::Smart && matches!(tool, Tool::Trim | Tool::Selector | Tool::Grabber);
        let tip = format!("{}{}", tool.label(), tool.fkey().map(|k| format!(" ({k})")).unwrap_or_default());
        let button = at(r, x, 14.0, width, 28.0);
        let resp = icon_at(ui, button, icon, cur == tool || smart_member, &tip, t.toolbar_icon);
        if tool == Tool::Zoom && (resp.double_clicked() || resp.triple_clicked()) {
            fit_session(app);
        } else if resp.clicked() {
            let _ = app.run("edit.tool", json!({"tool": tool.id()}));
        }
    }
    let e = &app.engine.session().edit;
    let toggles = option_toggles(e);
    for (i, (id, icon, on, tip)) in toggles.into_iter().enumerate() {
        let x = 6.0 + i as f32 * 34.0 + if i >= 4 { 28.0 } else { 0.0 };
        if icon_at(ui, at(r, x, 53.0, 30.0, 19.0), icon, on, tip, t.toolbar_icon).clicked() {
            let _ = app.run(id, json!({}));
        }
    }
}

/// The Edit-window toolbar's option toggles: (command id, icon, on, tooltip).
fn option_toggles(e: &EditState) -> [(&'static str, &'static str, bool, &'static str); 8] {
    [
        ("options.tab_to_transient", "tab_transient", e.tab_to_transient, "Tab to Transient"),
        ("options.link_timeline_edit", "link_timeline", e.link_timeline_edit, "Link Timeline and Edit Selection"),
        ("options.link_track_edit", "link_track", e.link_track_edit, "Link Track and Edit Selection"),
        ("options.insertion_follows_playback", "insertion_playback", e.insertion_follows_playback, "Insertion Follows Playback"),
        ("options.automation_follows_edit", "automation_edit", e.automation_follows_edit, "Automation Follows Edit"),
        ("options.markers_follow_edit", "markers_edit", e.markers_follow_edit, "Markers Follow Edit"),
        ("options.mirrored_midi", "mirrored_midi", e.mirrored_midi, "Mirrored MIDI Editing"),
        ("options.layered_editing", "layered_edit", e.layered_editing, "Layered Editing"),
    ]
}

fn fmt_pos(app: &SoundApp, at: i64, f: TimeFormat) -> String {
    let s = app.engine.session();
    s.format_position(at, f)
}

fn readout(ui: &Ui, r: Rect, text: &str, size: f32) {
    let color = Tokens::current().toolbar_readout;
    let width = ui.painter().layout_no_wrap(text.to_owned(), mono(size), color).size().x;
    let size = size * (r.width() / width.max(1.0)).min(1.0);
    ui.painter().with_clip_rect(ui.clip_rect().intersect(r)).text(r.right_center(), Align2::RIGHT_CENTER, text, mono(size), color);
}

/// Main/selection counters, the last cursor position over the timeline, and live session indicators.
fn counters(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let t = Tokens::current();
    let (main_fmt, sub_fmt, sel) = {
        let s = app.engine.session();
        (s.edit.main_counter, s.edit.sub_counter.unwrap_or(TimeFormat::Samples), s.edit.selection)
    };
    let pos = app.position();
    let slot = slot(ui, 316.0);
    dividers.paint(slot);
    let r = at(slot, 6.0, 6.0, 306.0, 64.0);
    lcd(ui, r);
    let main_r = at(r, 2.0, 0.0, 171.0, 44.0);
    let txt = fmt_pos(app, pos, main_fmt).replace("| ", "|");
    // Long sample and timecode counters fit the same well without crossing the selection readouts.
    readout(ui, at(main_r, 7.0, 0.0, main_r.width() - 24.0, main_r.height()), &txt, 24.0);
    let resp = ui.interact(main_r, ui.scope_id().with("main_counter"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Main counter format"));
    if resp.has_focus() {
        ui.painter().rect_stroke(main_r.shrink(1.0), 1.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    icons::draw(ui.painter(), at(main_r, main_r.width() - 11.0, 17.0, 9.0, 9.0), "triangle_down", t.toolbar_icon);
    {
        egui::Popup::menu(&resp).show(|ui| {
            for f in TimeFormat::ALL {
                if ui.selectable_label(main_fmt == f, f.label()).clicked() {
                    let _ = app.run("view.main_counter", json!({"format": f.id()}));
                }
            }
        });
    }
    let s = app.engine.session();
    let rows = [
        ("Start", fmt_pos(app, sel.start, main_fmt)),
        ("End", fmt_pos(app, sel.end, main_fmt)),
        ("Length", format_length(sel.start, sel.len(), main_fmt, s.sample_rate, &s.tempo, s.frame_rate)),
    ];
    ui.painter().vline(r.min.x + 174.0, r.min.y + 3.0..=r.min.y + 43.0, Stroke::new(1.0, t.counter_rule));
    for (i, (label, val)) in rows.iter().enumerate() {
        let y = r.min.y + 9.0 + i as f32 * 12.0;
        ui.painter().text(pos2(r.min.x + 209.0, y), Align2::RIGHT_CENTER, *label, regular(9.5), t.counter_label);
        let field = at(r, 212.0, y - r.min.y - 6.0, 88.0, 12.0);
        readout(ui, field, &val.replace("| ", "|"), 10.0);
    }
    let cursor_id = ui.make_persistent_id(("toolbar_cursor", ui.ctx().viewport_id()));
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let timeline = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    let cursor = ui.ctx().pointer_hover_pos().filter(|p| timeline.contains(*p)).map(|p| crate::edit_window::sample_at(s, timeline, p.x));
    let cursor = ui.ctx().data_mut(|data| {
        if let Some(cursor) = cursor {
            data.insert_temp(cursor_id, cursor);
        }
        data.get_temp::<i64>(cursor_id).unwrap_or(pos)
    });
    let y = r.max.y - 8.0;
    ui.painter().hline(r.min.x + 3.0..=r.max.x - 3.0, r.max.y - 17.0, Stroke::new(1.0, t.counter_rule));
    ui.painter().text(pos2(r.min.x + 5.0, y), Align2::LEFT_CENTER, "Cursor", regular(9.0), t.counter_label);
    let cursor_r = at(r, 35.0, 47.0, 73.0, 16.0);
    readout(ui, cursor_r, &fmt_pos(app, cursor, main_fmt).replace("| ", "|"), 9.0);
    icons::draw(ui.painter(), at(r, 112.0, 51.0, 14.0, 10.0), "wave", t.toolbar_icon);
    let sub_r = at(r, 128.0, 47.0, 57.0, 16.0);
    readout(ui, sub_r, &fmt_pos(app, pos, sub_fmt), 9.0);
    counter_status(app, ui, at(r, 190.0, 49.0, 112.0, 14.0));
}

fn counter_status(app: &SoundApp, ui: &mut Ui, r: Rect) {
    let t = Tokens::current();
    let s = app.engine.session();
    let statuses = [
        ("audio_status", app.player.as_ref().is_some_and(|p| !p.silent), "Audio engine active"),
        ("session_status", app.engine.is_dirty(), "Session modified"),
        ("Dly", s.edit.delay_compensation, "Delay compensation"),
        ("clock_status", s.edit.pre_post_roll, "Pre/Post-Roll"),
        ("S", s.tracks.iter().any(|t| t.mixer.solo), "Solo active"),
        ("M", s.tracks.iter().any(|t| t.mixer.mute), "Mute active"),
    ];
    let mut x = r.min.x;
    for (label, on, tip) in statuses {
        let width = if label == "Dly" { 24.0 } else { 15.0 };
        let cell = Rect::from_min_size(pos2(x, r.min.y), vec2(width, r.height()));
        let color = if on {
            match label {
                "S" => t.solo,
                "M" => t.mute,
                _ => t.toolbar_readout,
            }
        } else {
            shade(t.toolbar_readout, -0.66)
        };
        if label.len() > 3 {
            icons::draw(ui.painter(), cell.shrink(1.0), label, color);
        } else {
            if matches!(label, "S" | "M") {
                ui.painter().rect_filled(cell.shrink(1.0), 0.0, color);
            }
            ui.painter().text(cell.center(), Align2::CENTER_CENTER, label, bold(9.0), if matches!(label, "S" | "M") { t.counter_bg } else { color });
        }
        ui.interact(cell, ui.scope_id().with(tip), Sense::hover()).on_hover_text(format!("{tip}: {}", if on { "on" } else { "off" }));
        x += width + 1.0;
    }
}

/// Grid-line toggle and compact, independently selectable Grid/Nudge timebases.
fn grid_nudge(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let t = Tokens::current();
    let (grid, nudge, lines, grid_follow, nudge_follow, clips_markers) = {
        let e = &app.engine.session().edit;
        (e.grid, e.nudge, e.grid_lines, e.grid_follow_main, e.nudge_follow_main, e.grid_clips_markers)
    };
    let g = slot(ui, 160.0);
    dividers.paint(g);
    let r = dividers.centered(g, vec2(142.0, 40.0));
    lcd(ui, r);
    ui.painter().hline(r.min.x + 2.0..=r.max.x - 2.0, r.center().y, Stroke::new(1.0, t.counter_rule));
    let gl = at(r, 3.0, 4.0, 34.0, 13.0);
    let resp = ui.interact(gl, ui.scope_id().with("grid_lines"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), lines, "Show grid lines"));
    key(ui.painter(), gl, &resp, if lines { t.toolbar_readout } else { t.counter_off });
    ui.painter().text(gl.center(), Align2::CENTER_CENTER, "Grid", bold(9.0), if lines { t.mode_on_text } else { t.toolbar_readout });
    if resp.on_hover_text("Show grid lines").clicked() {
        let _ = app.run("view.grid_lines", json!({}));
    }
    ui.painter().text(pos2(r.min.x + 3.0, r.min.y + 29.0), Align2::LEFT_CENTER, "Nudge", regular(9.5), t.counter_label);
    for (y, value, command, label) in [(0.0, grid, "view.grid", "Grid value"), (20.0, nudge, "view.nudge", "Nudge value")] {
        let rr = at(r, 40.0, y, 101.0, 20.0);
        if matches!(value, GridValue::Note { .. }) {
            draw_compact_note_icon(ui.painter(), at(rr, 0.0, 0.0, 12.0, 20.0), value, t.toolbar_readout);
        } else {
            icons::draw(ui.painter(), at(rr, 0.0, 4.0, 12.0, 12.0), "clock_status", t.toolbar_readout);
        }
        let text = if command == "view.grid" && clips_markers {
            GridScale::ClipsMarkers.label().to_string()
        } else {
            compact_grid_label(app.engine.session(), value)
        };
        let field = at(rr, 13.0, 0.0, 77.0, 20.0);
        ui.painter().with_clip_rect(ui.clip_rect().intersect(field)).text(
            pos2(field.max.x, field.center().y),
            Align2::RIGHT_CENTER,
            text,
            mono(10.0),
            t.toolbar_readout,
        );
        icons::draw(ui.painter(), at(rr, 91.0, 5.0, 9.0, 10.0), "triangle_down", t.toolbar_icon);
        let resp = ui.interact(rr, ui.scope_id().with(command), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), label));
        let resp = resp.on_hover_text(format!("{label}: {}", value.label()));
        value_menu(
            app,
            &resp,
            command,
            value,
            if command == "view.grid" { grid_follow } else { nudge_follow },
            command == "view.grid" && clips_markers,
            command == "view.grid",
        );
    }
}

/// Formats a Grid/Nudge increment in the compact counter style for its time scale.
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

/// Draws the musical symbol beside a compact Bars|Beats value.
fn draw_compact_note_icon(painter: &Painter, rect: Rect, value: GridValue, color: Color32) {
    let GridValue::Note { value, dotted, triplet } = value else { return };
    draw_note_value_icon(painter, Rect::from_min_size(pos2(rect.min.x + 1.0, rect.min.y + 2.0), vec2(14.0, 16.0)), value, dotted, triplet, color);
}

/// Draws a musical duration and its optional dotted or triplet modifier.
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

/// Shows the complete Grid or Nudge increment, scale, and follow-mode menu.
fn value_menu(app: &mut SoundApp, resp: &egui::Response, cmd: &str, current: GridValue, follow_main: bool, clips_markers: bool, allow_clips: bool) {
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(210.0);
        match current.scale() {
            GridScale::BarsBeats => {
                let selected = match current {
                    GridValue::Note { value, .. } => value,
                    _ => NoteValue::Sixteenth,
                };
                let suffix = match current {
                    GridValue::Note { dotted: true, .. } => ".",
                    GridValue::Note { triplet: true, .. } => "t",
                    _ => "",
                };
                for value in NoteValue::ALL {
                    let id = match value {
                        NoteValue::Bar => "1 bar",
                        NoteValue::Whole => "whole",
                        NoteValue::Half => "1/2",
                        NoteValue::Quarter => "1/4",
                        NoteValue::Eighth => "1/8",
                        NoteValue::Sixteenth => "1/16",
                        NoteValue::ThirtySecond => "1/32",
                        NoteValue::SixtyFourth => "1/64",
                    };
                    menu_note_value(ui, app, cmd, selected == value, value, json!(format!("{id}{suffix}")));
                }
                ui.separator();
                let (value, dotted, triplet) = match current {
                    GridValue::Note { value, dotted, triplet } => (value, dotted, triplet),
                    _ => (NoteValue::Sixteenth, false, false),
                };
                let base = match value {
                    NoteValue::Bar => "1 bar",
                    NoteValue::Whole => "whole",
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

/// Adds one text menu value and dispatches its command when selected.
fn menu_value(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, label: &str, value: serde_json::Value) {
    if ui.selectable_label(selected, label).clicked() {
        let _ = app.run(cmd, json!({"value": value}));
        ui.close();
    }
}

/// Adds one musical value with its duration symbol.
fn menu_note_value(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, note: NoteValue, value: serde_json::Value) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        draw_note_value_icon(ui.painter(), rect, note, false, false, ui.visuals().text_color());
        menu_value(ui, app, cmd, selected, note.label(), value);
    });
}

/// Adds a dotted or triplet modifier with its compact symbol.
fn menu_modifier(ui: &mut Ui, app: &mut SoundApp, cmd: &str, selected: bool, label: &str, symbol: &str, value: serde_json::Value) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, symbol, regular(11.0), ui.visuals().text_color());
        menu_value(ui, app, cmd, selected, label, value);
    });
}

/// Neutral transport key with a colored original glyph and live activity/focus feedback.
fn transport_key(ui: &mut Ui, r: Rect, icon: &str, on: bool, tip: &str, tint: Color32) -> egui::Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.scope_id().with(tip), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, tip));
    key(ui.painter(), r, &resp, key_fill(&resp));
    ui.painter().rect_stroke(r.shrink(1.0), 0.0, Stroke::new(0.5, shade(t.toolbar_button, 0.10)), StrokeKind::Inside);
    let active_action = on && icon != "transport_cycle";
    let color = if active_action { shade(tint, 0.12) } else { tint };
    icons::draw_toolbar(ui.painter(), r.shrink(2.0), icon, color);
    if active_action || resp.has_focus() {
        ui.painter().rect_stroke(
            r.shrink(1.5),
            0.0,
            Stroke::new(0.75, if resp.has_focus() { t.accent } else { shade(tint, -0.25) }),
            StrokeKind::Inside,
        );
    }
    resp.on_hover_text(tip)
}

fn transport(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let t = Tokens::current();
    let playing = app.is_playing();
    let recording = app.engine.transport.recording;
    let looped = app.engine.session().edit.loop_playback;
    let g = slot(ui, 164.0);
    dividers.paint(g);
    for (i, (icon, command, tip, on, color)) in [
        ("transport_cycle", "options.loop_playback", "Loop Playback (Cmd+Shift+L)", looped, t.transport_blue),
        ("transport_stop", "transport.stop", "Stop (Space)", false, t.transport_blue),
        ("transport_play", "transport.play", "Play (Space)", playing, t.transport_green),
        ("transport_record", "transport.record", "Record (F12 or Cmd+Space)", recording, t.transport_red),
    ]
    .into_iter()
    .enumerate()
    {
        let r = at(g, 12.0 + i as f32 * 33.0, 14.0, 28.0, 28.0);
        let resp = transport_key(ui, r, icon, on, tip, color);
        if command == "transport.record"
            && let Some(why) = app.record_hint()
        {
            let below = Rect::from_min_max(r.min, pos2(r.max.x, g.max.y));
            egui::Tooltip::always_open(ui.ctx().clone(), resp.layer_id, resp.id, below).show(|ui| ui.label(why));
        }
        if resp.clicked() {
            let _ = app.run(command, json!({}));
        }
    }
    for (i, (icon, cmd, tip)) in [
        ("rtz", "transport.rtz", "Return to Zero (Home)"),
        ("rewind", "transport.rewind", "Rewind"),
        ("ffwd", "transport.fast_forward", "Fast Forward"),
        ("end", "transport.go_to_end", "Go to End (End)"),
    ]
    .into_iter()
    .enumerate()
    {
        if icon_at(ui, at(g, 12.0 + i as f32 * 33.0, 53.0, 28.0, 18.0), icon, false, tip, t.toolbar_icon).clicked() {
            let _ = app.run(cmd, json!({}));
        }
    }
    for (y, on, color, tip) in [(17.0, recording, t.transport_record_led, "Recording"), (31.0, playing, t.transport_play_led, "Playing")] {
        let r = at(g, 143.0, y, 10.0, 10.0);
        ui.painter().rect(r, 1.0, if on { color } else { shade(color, -0.55) }, Stroke::new(1.0, t.border), StrokeKind::Inside);
        ui.interact(r, ui.scope_id().with(tip), Sense::hover()).on_hover_text(format!("{tip}: {}", if on { "yes" } else { "no" }));
    }
}

/// Count-off, meter and tempo readouts; the tempo value edits the event at or before the playhead.
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

fn output_meter(app: &mut SoundApp, ui: &mut Ui, dividers: &Dividers) {
    let g = slot(ui, 44.0);
    dividers.paint(g);
    let r = dividers.centered(g, vec2(26.0, 64.0));
    lcd(ui, r);
    let m = app.main_meter;
    let w = (r.width() - 6.0) / 2.0;
    for c in 0..2 {
        let rr = Rect::from_min_size(pos2(r.min.x + 2.0 + c as f32 * (w + 2.0), r.min.y + 3.0), vec2(w, r.height() - 6.0));
        crate::widgets::meter(ui, rr, m.level[c], m.hold[c], m.clip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use egui_kittest::{Harness, kittest::Queryable};

    /// Builds a compact Grid/Nudge toolbar test harness.
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

    /// Opens a toolbar value menu and lets the popup settle.
    fn open(harness: &mut Harness<SoundApp>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    /// Compact values use their corresponding counter formats and symbols.
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

    /// The Grid menu exposes every scale and retains its numeric value in Clips/Markers mode.
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
        open(&mut harness, "Grid menu");
        harness.get_by_label("1/2 frame").click();
        harness.run_steps(3);
        let edit = &harness.state().engine.session().edit;
        assert_eq!(edit.grid, GridValue::Timecode(TimecodeValue::HalfFrame));
        assert!(!edit.grid_clips_markers);
    }

    /// Selecting another musical increment preserves the active dotted or triplet modifier.
    #[test]
    fn note_menu_keeps_the_current_modifier_when_changing_value() {
        let mut harness = harness();
        harness.state_mut().run("view.grid", json!({"value": "1/8t"})).unwrap();
        harness.run_steps(3);
        open(&mut harness, "Grid menu");
        harness.get_by_label("1/16 note").click();
        harness.run_steps(3);
        assert_eq!(harness.state().engine.session().edit.grid, GridValue::Note { value: NoteValue::Sixteenth, dotted: false, triplet: true });
        open(&mut harness, "Grid menu");
        harness.get_by_label("Whole note").click();
        harness.run_steps(3);
        assert_eq!(harness.state().engine.session().edit.grid, GridValue::Note { value: NoteValue::Whole, dotted: false, triplet: true });
    }

    /// The Nudge menu exposes every numeric scale without the Grid-only Clips/Markers mode.
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

    /// #1262: the two link toggles differ only by their icon, so each option toggle needs its own.
    #[test]
    fn option_toggles_have_distinct_icons() {
        let toggles = option_toggles(&EditState::default());
        let timeline = toggles.iter().find(|t| t.0 == "options.link_timeline_edit").unwrap();
        let track = toggles.iter().find(|t| t.0 == "options.link_track_edit").unwrap();
        assert_ne!(timeline.1, track.1, "the timeline and track links need different icons");
        let mut icons: Vec<&str> = toggles.iter().map(|t| t.1).collect();
        icons.sort_unstable();
        icons.dedup();
        assert_eq!(icons.len(), toggles.len(), "every option toggle needs its own icon");
    }
}
