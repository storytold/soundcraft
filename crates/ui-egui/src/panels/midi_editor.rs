//! The MIDI Editor: a piano roll with a velocity lane, docked under the Edit window.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::widgets::{SelectorLabels, selector_box_named};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::cmd::{ScaleChoice, midi_editor_scale, session_key_at};
use soundcraft_midi::{KeyScale, Scale, pitch_class_name};
use soundcraft_model::{ClipContent, ClipId, Tool};
use soundcraft_time::TICKS_PER_QUARTER;

const KEY_W: f32 = 48.0;
const ROW_H: f32 = 9.0;
const VEL_H: f32 = 64.0;
/// Width of the header's whole-clip toolbar.
const TOOLBAR_W: f32 = 330.0;

#[derive(Debug, Clone, Default)]
pub struct MidiEditorState {
    pub selected: Vec<usize>,
    pub drag: Option<NoteDrag>,
    pub scroll: f32,
    pub clip: Option<ClipId>,
    /// Rubber-band selection start (screen position).
    pub band: Option<egui::Pos2>,
}

impl MidiEditorState {
    /// Start editing `id`. Note indices only mean something inside one clip, so the selection and
    /// any gesture in progress are dropped when the clip changes.
    fn switch_clip(&mut self, id: ClipId) {
        if self.clip != Some(id) {
            self.selected.clear();
            self.drag = None;
            self.band = None;
        }
        self.clip = Some(id);
    }
}

#[derive(Debug, Clone)]
pub struct NoteDrag {
    pub index: usize,
    pub resize: bool,
    pub start: egui::Pos2,
    pub dx_ticks: i64,
    pub dy_semi: i64,
}

/// The MIDI clip being edited: the first selected MIDI clip, else a MIDI clip under the insertion
/// point on a selected track.
pub fn target_clip(app: &SoundApp) -> Option<ClipId> {
    let s = app.engine.session();
    for id in &s.edit.selected_clips {
        if let Some((_, c)) = s.find_clip(*id)
            && matches!(c.content, ClipContent::Midi { .. })
        {
            return Some(*id);
        }
    }
    let at = s.edit.selection.start;
    s.tracks
        .iter()
        .filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi())
        .flat_map(|t| t.clips().iter())
        .find(|c| c.range().contains(at) || c.range().overlaps(&s.edit.selection))
        .or_else(|| s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi()).flat_map(|t| t.clips().iter()).next())
        .map(|c| c.id)
}

fn is_black(p: i32) -> bool {
    matches!(p.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// Draw the MIDI editor for the target clip, with an empty-state prompt that adds a clip on the selected MIDI track.
/// The piano roll's geometry: where it is drawn and how ticks and pitches map onto it.
#[derive(Clone, Copy)]
struct Roll<'a> {
    sequence: &'a soundcraft_midi::Sequence,
    roll: Rect,
    keys: Rect,
    vel: Rect,
    px_per_tick: f32,
    len_ticks: i64,
    scroll: f32,
    grid_ticks: i64,
    rows: i32,
}

impl Roll<'_> {
    fn x_of(&self, tick: i64) -> f32 {
        self.roll.min.x + tick as f32 * self.px_per_tick
    }

    fn y_of(&self, pitch: i32) -> f32 {
        self.roll.min.y + (127 - pitch) as f32 * ROW_H - self.scroll
    }

    fn pitch_at(&self, y: f32) -> i32 {
        127 - ((y - self.roll.min.y + self.scroll) / ROW_H).floor() as i32
    }

    /// The note under `pos` and whether `pos` is on its resize edge.
    fn hit(&self, pos: egui::Pos2) -> Option<(usize, bool)> {
        self.sequence.notes.iter().enumerate().rev().find_map(|(i, n)| {
            let r = Rect::from_min_max(
                pos2(self.x_of(n.start), self.y_of(i32::from(n.pitch))),
                pos2(self.x_of(n.start + n.length).max(self.x_of(n.start) + 3.0), self.y_of(i32::from(n.pitch)) + ROW_H),
            );
            r.contains(pos).then_some((i, pos.x > r.max.x - 5.0))
        })
    }

    /// `tk` ticks, rounded to the grid.
    fn snap(&self, tk: f32) -> i64 {
        ((tk / self.grid_ticks as f32).round() as i64) * self.grid_ticks
    }
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    // Painter-only contents do not advance the layout. Claim the resized
    // space, including the empty state, so the dock keeps its chosen height.
    ui.take_available_space();
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let header = Rect::from_min_size(full.min, vec2(full.width(), 22.0));
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().text(
        pos2(header.min.x + 8.0, header.center().y),
        Align2::LEFT_CENTER,
        crate::localization::text("MIDI EDITOR"),
        bold(11.5),
        t.header_text,
    );
    let Some(cid) = target_clip(app) else {
        empty_state(app, ui, full, header);
        return;
    };
    let Some((track, clip)) = app.engine.session().find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
    let ClipContent::Midi { sequence } = &clip.content else { return };
    let track_color = app.engine.session().track(track).map_or([120, 120, 200], |t| t.color);
    ui.painter().text(
        pos2(header.min.x + 110.0, header.center().y),
        Align2::LEFT_CENTER,
        format!(
            "{} · {} notes · {} selected · Pencil: add · drag: move/select · arrows: move · Delete: remove",
            clip.name,
            sequence.notes.len(),
            app.midi.selected.len()
        ),
        regular(11.0),
        t.text_dim,
    );
    let grid_ticks = grid_ticks_of(app);
    let (choice, followed, scale) = {
        let s = app.engine.session();
        let choice = midi_editor_scale(s);
        (choice, session_key_at(s, clip.start), choice.resolve(s, clip.start))
    };
    midi_toolbar(app, ui, header, cid, grid_ticks);
    scale_picker(app, ui, header, ScaleState { choice, followed, shown: scale });
    let s = app.engine.session_arc();
    let sr = s.sample_rate;
    let base_tick = s.tempo.samples_to_ticks(clip.start, sr);
    let len_ticks = (s.tempo.samples_to_ticks(clip.end(), sr) - base_tick).max(1);
    let roll = Rect::from_min_max(pos2(full.min.x + KEY_W, header.max.y), pos2(full.max.x - 4.0, full.max.y - VEL_H - 4.0));
    let keys = Rect::from_min_max(pos2(full.min.x, roll.min.y), pos2(roll.min.x, roll.max.y));
    let vel = Rect::from_min_max(pos2(roll.min.x, roll.max.y + 4.0), pos2(roll.max.x, full.max.y - 2.0));
    // Vertical range: centre on the notes the first time.
    let rows = (roll.height() / ROW_H).floor() as i32;
    if app.midi.clip != Some(cid) {
        let hi = sequence.notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(72);
        let lo = sequence.notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(48);
        app.midi.scroll = (127 - (hi + lo) / 2 - rows / 2) as f32 * ROW_H;
        app.midi.switch_clip(cid);
    }
    if ui.rect_contains_pointer(roll) {
        app.midi.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    app.midi.scroll = app.midi.scroll.clamp(0.0, (128.0 * ROW_H - roll.height()).max(0.0));
    let scroll = app.midi.scroll;
    let px_per_tick = roll.width() / len_ticks as f32;
    let g = Roll { sequence, roll, keys, vel, px_per_tick, len_ticks, scroll, grid_ticks, rows };
    let painter = ui.painter().with_clip_rect(roll.union(keys));
    draw_roll_background(&painter, &g, scale);
    // Notes.
    let base = egui::Color32::from_rgb(track_color[0], track_color[1], track_color[2]);
    draw_notes(app, &painter, &g, base);
    note_pointer(app, ui, &g, cid, s.edit.tool == Tool::Pencil);
    note_keys(app, ui, &g, cid);
    velocity_lane(app, ui, &g, cid, base, full.min.x + 6.0);
}

/// The empty state: a prompt that adds a clip on the selected MIDI track when clicked.
fn empty_state(app: &mut SoundApp, ui: &mut Ui, full: Rect, header: Rect) {
    let t = Tokens::current();
    let track = {
        let s = app.engine.session();
        s.tracks.iter().find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.name.clone()))
    };
    let msg = match &track {
        Some((_, name)) => crate::localized_format!("Click to add a MIDI clip to {name} at the insertion point.", name = name),
        None => crate::localization::text("Select a MIDI clip (or a MIDI track) to edit its notes.").into(),
    };
    ui.painter().text(full.center(), Align2::CENTER_CENTER, msg, regular(13.0), t.text_dim);
    let body = Rect::from_min_max(pos2(full.min.x, header.max.y), full.max);
    if let Some((id, _)) = track
        && ui.interact(body, ui.scope_id().with("new_clip"), Sense::click()).clicked()
    {
        let sel = app.engine.session().edit.selection;
        if let Ok(v) = app.run("midi.new_clip", json!({"track": id.0, "start": sel.start, "end": sel.end})) {
            let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
        }
    }
}

/// The grid step in ticks, from the Edit window's grid value.
fn grid_ticks_of(app: &SoundApp) -> i64 {
    match app.engine.session().edit.grid {
        soundcraft_time::GridValue::Note { value, dotted, triplet } => {
            let mut g = value.ticks();
            if dotted {
                g = g * 3 / 2;
            }
            if triplet {
                g = g * 2 / 3;
            }
            g.max(1)
        }
        _ => TICKS_PER_QUARTER / 4,
    }
}

/// The header toolbar: whole-clip MIDI operations.
fn midi_toolbar(app: &mut SoundApp, ui: &mut Ui, header: Rect, cid: ClipId, grid_ticks: i64) {
    // Header toolbar: whole-clip MIDI operations.
    let bar = Rect::from_min_max(pos2(header.max.x - TOOLBAR_W, header.min.y + 1.0), pos2(header.max.x - 6.0, header.max.y - 1.0));
    let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    tb.spacing_mut().item_spacing.x = 4.0;
    let mut op: Option<(&str, serde_json::Value)> = None;
    if tb.small_button(crate::localization::text("Legato")).clicked() {
        op = Some(("event.change_duration", json!({"legato": 0})));
    }
    if tb.small_button(crate::localization::text("Vel −10")).clicked() {
        op = Some(("event.change_velocity", json!({"add": -10})));
    }
    if tb.small_button(crate::localization::text("Vel +10")).clicked() {
        op = Some(("event.change_velocity", json!({"add": 10})));
    }
    if tb.small_button(crate::localization::text("Quantize")).on_hover_text(crate::localization::text("Quantize to the grid")).clicked() {
        op = Some(("event.quantize", json!({"grid": grid_ticks, "strength": 100})));
    }
    if let Some((id, mut p)) = op {
        p["clips"] = json!([cid.0]);
        let _ = app.run(id, p);
    }
}

/// What the scale picker shows: the user's choice, the session key it would follow, and the
/// scale highlighted.
#[derive(Clone, Copy)]
struct ScaleState {
    choice: ScaleChoice,
    followed: Option<KeyScale>,
    shown: Option<KeyScale>,
}

/// The scale picker in the header, left of the toolbar: a root and a scale, or follow the
/// session key, or none.
fn scale_picker(app: &mut SoundApp, ui: &mut Ui, header: Rect, state: ScaleState) {
    let ScaleState { choice, followed, shown } = state;
    let t = Tokens::current();
    let (root_w, scale_w, h) = (34.0, 200.0, header.height() - 4.0);
    let right = header.max.x - TOOLBAR_W - 10.0;
    let scale_r = Rect::from_min_size(pos2(right - scale_w, header.min.y + 2.0), vec2(scale_w, h));
    let root_r = Rect::from_min_size(pos2(scale_r.min.x - 4.0 - root_w, scale_r.min.y), vec2(root_w, h));
    let scale_value = match (choice, followed) {
        (ScaleChoice::Follow, Some(k)) => {
            let key = format!("{} {}", pitch_class_name(k.root), crate::localization::text(k.scale.label()));
            crate::localized_format!("Session key: {key}", key = key)
        }
        (ScaleChoice::Follow, None) => crate::localization::text("Session key: none").to_owned(),
        (ScaleChoice::Off, _) => crate::localization::text("None").to_owned(),
        (ScaleChoice::Fixed(k), _) => crate::localization::text(k.scale.label()).to_owned(),
    };
    let root_value = shown.map_or("–", |k| pitch_class_name(k.root));
    let mut pick: Option<serde_json::Value> = None;
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(root_r.union(scale_r)));
    c.scope_builder(egui::UiBuilder::new().max_rect(root_r), |ui| {
        let name = crate::localization::text("Scale root");
        let resp = selector_box_named(ui, root_w, h, root_value, t.text, SelectorLabels { name, value: root_value, tooltip: name });
        picker_menu(&resp, |ui| {
            for pc in 0..12u8 {
                if ui.selectable_label(shown.is_some_and(|k| k.root == pc), pitch_class_name(pc)).clicked() {
                    // A root on its own keeps the scale shown, or starts at Major.
                    let scale = shown.map_or(Scale::Major, |k| k.scale);
                    pick = Some(json!({"scale": scale.id(), "root": pc}));
                }
            }
            if pick.is_some() {
                // A choice by Enter is not a pointer click, so close the menu here.
                ui.close();
            }
        });
    });
    c.scope_builder(egui::UiBuilder::new().max_rect(scale_r), |ui| {
        let name = crate::localization::text("Scale");
        let resp = selector_box_named(ui, scale_w, h, &scale_value, t.text, SelectorLabels { name, value: &scale_value, tooltip: &scale_value });
        picker_menu(&resp, |ui| {
            if ui.selectable_label(choice == ScaleChoice::Follow, crate::localization::text("Follow Session Key")).clicked() {
                pick = Some(json!({"scale": "follow"}));
            }
            if ui.selectable_label(choice == ScaleChoice::Off, crate::localization::text("None")).clicked() {
                pick = Some(json!({"scale": "none"}));
            }
            ui.separator();
            for scale in Scale::ALL {
                let on = matches!(choice, ScaleChoice::Fixed(k) if k.scale == scale);
                if ui.selectable_label(on, crate::localization::text(scale.label())).clicked() {
                    // Choosing a scale while following keeps the followed root.
                    let root = shown.map_or(0, |k| k.root);
                    pick = Some(json!({"scale": scale.id(), "root": root}));
                }
            }
            if pick.is_some() {
                // A choice by Enter is not a pointer click, so close the menu here.
                ui.close();
            }
        });
    });
    if let Some(p) = pick {
        let _ = app.run("midi.scale", p);
    }
}

/// Show `menu` under the selector `resp`, handing focus back to the selector when the menu
/// closes (by a choice or Escape), so the keyboard stays on the picker.
fn picker_menu(resp: &egui::Response, menu: impl FnOnce(&mut Ui)) {
    let id = egui::Popup::default_response_id(resp);
    let was_open = egui::Popup::is_id_open(&resp.ctx, id);
    egui::Popup::menu(resp).show(menu);
    if was_open && !egui::Popup::is_id_open(&resp.ctx, id) {
        resp.request_focus();
    }
}

/// Width of the strip that marks an in-scale row at the roll's left edge.
const SCALE_STRIP_W: f32 = 4.0;

/// Background rows, piano keys and the grid. With a scale, rows in it are tinted and marked by a
/// strip at the left edge, and the root's rows are underlined and named on their key.
fn draw_roll_background(painter: &egui::Painter, g: &Roll<'_>, scale: Option<KeyScale>) {
    let m = Tokens::current().midi;
    let Roll { roll, keys, px_per_tick, len_ticks, grid_ticks, rows, .. } = *g;
    // Background rows and keys.
    let top = g.pitch_at(roll.min.y);
    for r in 0..=rows + 1 {
        let pitch = top - r;
        let y = g.y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.min.x, y), vec2(roll.width(), ROW_H));
        let fill = match scale {
            Some(k) if k.contains(pitch) => m.row_in_scale,
            Some(_) => m.row_out_scale,
            None if is_black(pitch) => m.row_black,
            None => m.row_white,
        };
        painter.rect_filled(row, 0.0, fill);
        if scale.is_some_and(|k| k.contains(pitch)) {
            painter.rect_filled(Rect::from_min_size(row.min, vec2(SCALE_STRIP_W, ROW_H)), 0.0, m.scale_mark);
        }
        let key = Rect::from_min_size(pos2(keys.min.x, y), vec2(KEY_W - 2.0, ROW_H - 1.0));
        painter.rect_filled(key, 1.0, if is_black(pitch) { m.key_black } else { m.key_white });
        let root = scale.is_some_and(|k| k.is_root(pitch));
        if (pitch.rem_euclid(12) == 0 || root)
            && let Ok(p) = u8::try_from(pitch)
            && p <= 127
        {
            let ink = if is_black(pitch) { m.key_label_on_black } else { m.key_label };
            painter.text(pos2(key.max.x - 3.0, key.center().y), Align2::RIGHT_CENTER, soundcraft_midi::note_name(p), regular(8.0), ink);
        }
        if pitch.rem_euclid(12) == 0 && (0..=127).contains(&pitch) {
            painter.line_segment([pos2(roll.min.x, y + ROW_H), pos2(roll.max.x, y + ROW_H)], Stroke::new(1.0, m.octave_line));
        }
    }
    // Root rows are underlined inside their own row, after every row is filled.
    if let Some(k) = scale {
        for r in 0..=rows + 1 {
            let pitch = top - r;
            if k.is_root(pitch) && (0..=127).contains(&pitch) {
                let y = g.y_of(pitch) + ROW_H - 1.0;
                painter.line_segment([pos2(roll.min.x, y), pos2(roll.max.x, y)], Stroke::new(2.0, m.scale_mark));
            }
        }
    }
    // Grid.
    let step = grid_stride(grid_ticks, px_per_tick);
    let mut tick = 0;
    while tick <= len_ticks {
        let bar = tick % (TICKS_PER_QUARTER * 4) == 0;
        let x = g.x_of(tick);
        painter.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, if bar { m.bar_line } else { m.grid_line }));
        tick = tick.saturating_add(step);
    }
}

/// Closest lines the grid draws, in points.
const MIN_GRID_PX: f32 = 4.0;

/// Ticks between grid lines: the grid when its lines are more than [`MIN_GRID_PX`] apart, else
/// as few common multiples of the grid and a bar as keep that spacing (so every line is a snap
/// position and falls on a bar). The lines drawn are then bounded by the roll's width, however long
/// the clip, instead of by its length.
fn grid_stride(grid_ticks: i64, px_per_tick: f32) -> i64 {
    let bar_ticks = TICKS_PER_QUARTER * 4;
    if grid_ticks as f32 * px_per_tick > MIN_GRID_PX {
        return grid_ticks.max(1);
    }
    let grid = grid_ticks.max(1);
    let (mut a, mut b) = (grid, bar_ticks);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let common = (grid / a).saturating_mul(bar_ticks);
    // `as` saturates a huge or NaN quotient; a zero `px_per_tick` gives one common multiple.
    let n = (MIN_GRID_PX / (common as f32 * px_per_tick)).ceil().max(1.0) as i64;
    common.saturating_mul(n)
}

/// The notes, with a drag in progress applied.
fn draw_notes(app: &SoundApp, painter: &egui::Painter, g: &Roll<'_>, base: Color32) {
    let Roll { sequence, grid_ticks, .. } = *g;
    let m = Tokens::current().midi;
    // Notes.
    let drag = app.midi.drag.clone();
    for (i, n) in sequence.notes.iter().enumerate() {
        let (mut st, mut ln, mut p) = (n.start, n.length, i32::from(n.pitch));
        if let Some(d) = &drag
            && (d.index == i || (!d.resize && app.midi.selected.contains(&i) && app.midi.selected.contains(&d.index)))
        {
            if d.resize {
                if d.index == i {
                    ln = (ln + d.dx_ticks).max(grid_ticks.min(ln).max(1));
                }
            } else {
                st = (st + d.dx_ticks).max(0);
                p = (p + d.dy_semi as i32).clamp(0, 127);
            }
        }
        let r = Rect::from_min_max(pos2(g.x_of(st), g.y_of(p) + 1.0), pos2(g.x_of(st + ln).max(g.x_of(st) + 3.0), g.y_of(p) + ROW_H - 1.0));
        let sel = app.midi.selected.contains(&i);
        let k = 0.45 + 0.55 * f32::from(n.velocity) / 127.0;
        let fill = if sel {
            m.note_selected
        } else {
            Color32::from_rgb((f32::from(base.r()) * k) as u8, (f32::from(base.g()) * k) as u8, (f32::from(base.b()) * k) as u8)
        };
        painter.rect(r, 2.0, fill, Stroke::new(1.0, m.note_outline), StrokeKind::Inside);
    }
}

/// Pointer interaction on the roll: note and band selection, drags and clicks.
fn note_pointer(app: &mut SoundApp, ui: &mut Ui, g: &Roll<'_>, cid: ClipId, pencil: bool) {
    let Roll { sequence, roll, px_per_tick, grid_ticks, .. } = *g;
    // Interaction.
    let resp = ui.interact(roll, ui.scope_id().with(("roll", cid.0)), Sense::click_and_drag());
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if let Some((i, edge)) = g.hit(p) {
            if !app.midi.selected.contains(&i) {
                app.midi.selected = vec![i];
            }
            app.midi.drag = Some(NoteDrag { index: i, resize: edge, start: p, dx_ticks: 0, dy_semi: 0 });
        } else {
            app.midi.band = Some(p);
        }
    }
    // Rubber-band selection on empty space.
    if let (Some(a), Some(b)) = (app.midi.band, ui.ctx().pointer_latest_pos()) {
        let band = Rect::from_two_pos(a, b);
        let accent = Tokens::current().accent;
        ui.painter().rect(band, 0.0, accent.gamma_multiply(0.16), Stroke::new(1.0, accent), StrokeKind::Inside);
        if resp.drag_stopped() {
            app.midi.selected = sequence
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let r = Rect::from_min_max(
                        pos2(g.x_of(n.start), g.y_of(i32::from(n.pitch))),
                        pos2(g.x_of(n.start + n.length), g.y_of(i32::from(n.pitch)) + ROW_H),
                    );
                    r.intersects(band)
                })
                .map(|(i, _)| i)
                .collect();
            app.midi.band = None;
        }
    }
    if resp.dragged()
        && let (Some(d), Some(p)) = (&mut app.midi.drag, resp.interact_pointer_pos())
    {
        d.dx_ticks = g.snap((p.x - d.start.x) / px_per_tick);
        d.dy_semi = -((p.y - d.start.y) / ROW_H).round() as i64;
    }
    if resp.drag_stopped()
        && let Some(d) = app.midi.drag.take()
    {
        if d.resize {
            if let Some(n) = sequence.notes.get(d.index) {
                let _ = app.run("midi.note_edit", json!({"clip": cid.0, "index": d.index, "length_ticks": (n.length + d.dx_ticks).max(1)}));
            }
        } else if d.dx_ticks != 0 || d.dy_semi != 0 {
            let _ = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": d.dx_ticks, "semitones": d.dy_semi}));
            app.midi.selected.clear();
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match g.hit(p) {
            Some((i, _)) => {
                if ui.input(|x| x.modifiers.shift) {
                    if !app.midi.selected.contains(&i) {
                        app.midi.selected.push(i);
                    }
                } else {
                    app.midi.selected = vec![i];
                }
                // Audition on the instrument would go here (needs a live MIDI input path).
            }
            None if pencil => {
                let pitch = g.pitch_at(p.y).clamp(0, 127);
                let start = (((p.x - roll.min.x) / px_per_tick) as i64 / grid_ticks) * grid_ticks;
                let _ = app.run(
                    "midi.note_add",
                    json!({"clip": cid.0, "pitch": pitch, "start_ticks": start.max(0), "length_ticks": grid_ticks, "velocity": 100}),
                );
                app.midi.selected.clear();
            }
            None => app.midi.selected.clear(),
        }
    }
}

/// Keyboard: select all, move and delete notes.
fn note_keys(app: &mut SoundApp, ui: &mut Ui, g: &Roll<'_>, cid: ClipId) {
    let Roll { sequence, roll, grid_ticks, .. } = *g;
    // Keyboard: Cmd+A select all; arrows transpose (Shift = octave) and move by the grid.
    if ui.rect_contains_pointer(roll) && !crate::framework::widget_has_keyboard(ui.ctx()) {
        let (all, up, down, left, right, shift) = ui.input(|i| {
            let arrows = !i.modifiers.alt && !i.modifiers.command && !i.modifiers.ctrl;
            (
                i.modifiers.command && i.key_pressed(egui::Key::A),
                arrows && i.key_pressed(egui::Key::ArrowUp),
                arrows && i.key_pressed(egui::Key::ArrowDown),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowLeft),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        if all {
            app.midi.selected = (0..sequence.notes.len()).collect();
        }
        let semi = if up {
            1
        } else if down {
            -1
        } else {
            0
        } * if shift { 12 } else { 1 };
        let ticks = if right {
            grid_ticks
        } else if left {
            -grid_ticks
        } else {
            0
        };
        if (semi != 0 || ticks != 0) && !app.midi.selected.is_empty() {
            let moved = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks, "semitones": semi}));
            // The engine re-sorts the notes: follow the moved ones to their new indices, so the
            // selection never points at other notes and the arrows can keep nudging it.
            app.midi.selected = moved.ok().and_then(|v| serde_json::from_value(v["indices"].clone()).ok()).unwrap_or_default();
        }
    }
    if !app.midi.selected.is_empty()
        && !crate::framework::widget_has_keyboard(ui.ctx())
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": app.midi.selected}));
        app.midi.selected.clear();
    }
}

/// The velocity lane under the roll.
fn velocity_lane(app: &mut SoundApp, ui: &mut Ui, g: &Roll<'_>, cid: ClipId, base: Color32, label_x: f32) {
    let Roll { sequence, vel, .. } = *g;
    let t = Tokens::current();
    // Velocity lane.
    ui.painter().rect_filled(vel, 0.0, t.midi.lane_bg);
    ui.painter().text(pos2(label_x, vel.center().y), Align2::LEFT_CENTER, crate::localization::text("Velocity"), regular(10.0), t.text_dim);
    let vresp = ui.interact(vel, ui.scope_id().with(("vel", cid.0)), Sense::click_and_drag());
    for (i, n) in sequence.notes.iter().enumerate() {
        let x = g.x_of(n.start);
        let h = (vel.height() - 4.0) * f32::from(n.velocity) / 127.0;
        let col = if app.midi.selected.contains(&i) { t.midi.note_selected } else { base };
        ui.painter().line_segment([pos2(x + 1.0, vel.max.y - 2.0), pos2(x + 1.0, vel.max.y - 2.0 - h)], Stroke::new(3.0, col));
        ui.painter().circle_filled(pos2(x + 1.0, vel.max.y - 2.0 - h), 2.5, col);
    }
    if (vresp.clicked() || vresp.dragged())
        && let Some(p) = vresp.interact_pointer_pos()
        && let Some((i, n)) = sequence.notes.iter().enumerate().min_by_key(|(_, n)| ((g.x_of(n.start) - p.x).abs() * 10.0) as i64)
        && (g.x_of(n.start) - p.x).abs() < 8.0
    {
        let v = (((vel.max.y - 2.0 - p.y) / (vel.height() - 4.0)) * 127.0).round().clamp(1.0, 127.0) as i64;
        let _ = app.engine.execute_merged("midi.note_edit", &json!({"clip": cid.0, "index": i, "velocity": v}), &format!("vel:{}", cid.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_lines_are_bounded_by_the_roll_width_not_the_clip_length() {
        let sixteenth = TICKS_PER_QUARTER / 4;
        // A 1000 pt roll showing a 4-bar clip: every sixteenth is 16 pt apart and drawn.
        assert_eq!(grid_stride(sixteenth, 1000.0 / (16.0 * TICKS_PER_QUARTER as f32)), sixteenth);
        // The same roll over a clip a billion ticks long: at most a line every 4 pt.
        let (len, px_per_tick) = (1_000_000_000i64, 1000.0 / 1_000_000_000.0f32);
        let step = grid_stride(sixteenth, px_per_tick);
        assert_eq!(step % (TICKS_PER_QUARTER * 4), 0, "lines fall on bars");
        assert!((len / step) <= 251, "{} lines", len / step);
        // a dotted quarter grid (1440 ticks): every line is also a grid position
        let dotted = TICKS_PER_QUARTER * 3 / 2;
        let step = grid_stride(dotted, px_per_tick);
        assert_eq!((step % dotted, step % (TICKS_PER_QUARTER * 4)), (0, 0), "lines are snap positions on bars: {step}");
        assert!((len / step) <= 251, "{} lines", len / step);
        assert!(grid_stride(0, 0.0) >= TICKS_PER_QUARTER * 4, "a zero grid or width still advances");
    }

    #[test]
    fn switching_clip_drops_selection_and_gestures() {
        let mut st = MidiEditorState { selected: vec![0, 2], clip: Some(ClipId(1)), band: Some(pos2(1.0, 2.0)), ..Default::default() };
        st.switch_clip(ClipId(1));
        assert_eq!(st.selected, vec![0, 2]);
        st.switch_clip(ClipId(2));
        assert!(st.selected.is_empty());
        assert!(st.drag.is_none() && st.band.is_none());
        assert_eq!(st.clip, Some(ClipId(2)));
    }
}
