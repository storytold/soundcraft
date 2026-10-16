//! The MIDI Editor: a piano roll with a velocity lane, docked under the Edit window.

use crate::SoundApp;
use crate::midi_playback::{self, FollowState, MidiScrollingMode};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId, Tool};
use soundcraft_time::TICKS_PER_QUARTER;

const KEY_W: f32 = 48.0;
const ROW_H: f32 = 9.0;
pub const DEFAULT_VELOCITY_HEIGHT: f32 = 64.0;
const MIN_VELOCITY_HEIGHT: f32 = 24.0;
const MIN_ROLL_HEIGHT: f32 = 72.0;
const DIVIDER_H: f32 = 6.0;

#[derive(Debug, Clone, Default)]
pub struct MidiEditorState {
    pub selected: Vec<usize>,
    pub drag: Option<NoteDrag>,
    /// Fractional pitches preserve subpixel trackpad movement and momentum.
    pub top_pitch: f64,
    pub zoom: f64,
    pub scroll_ticks: f64,
    pub len_ticks: f64,
    pub roll: [f32; 4],
    pub velocity: [f32; 4],
    pub velocity_divider: [f32; 4],
    pub(crate) velocity_drag: Option<VelocityDrag>,
    /// Keep the same note while dragging overlapping velocity stems.
    pub(crate) velocity_note: Option<VelocityNoteDrag>,
    pub clip: Option<ClipId>,
    /// Rubber-band selection start (screen position).
    pub band: Option<egui::Pos2>,
    /// Runtime follow/transport ownership is never serialized into a session.
    pub follow: FollowState,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VelocityDrag {
    origin_y: f32,
    height: f32,
    full: Rect,
    clip: ClipId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VelocityNoteDrag {
    clip: ClipId,
    index: usize,
    offset_y: f32,
}

/// A UI preference: resizing never changes notes, selections or transport cues.
pub fn velocity_height_command(app: &mut SoundApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    if p.get("height").is_some() {
        let height = finite_param(p, "height", f64::from(DEFAULT_VELOCITY_HEIGHT))?;
        app.ui.midi_velocity_height = height.clamp(f64::from(MIN_VELOCITY_HEIGHT), 4096.0) as f32;
        app.midi.velocity_drag = None;
    }
    Ok(json!({"height": app.ui.midi_velocity_height}))
}

fn lane_layout(full: Rect, preferred: f32) -> (Rect, Rect, Rect) {
    let top = (full.min.y + 22.0).min(full.max.y);
    let bottom = (full.max.y - 2.0).max(top);
    let divider_h = DIVIDER_H.min(bottom - top);
    let available = (bottom - top - divider_h).max(0.0);
    let min_velocity = MIN_VELOCITY_HEIGHT.min(available * 0.25);
    let max_velocity = (available - MIN_ROLL_HEIGHT.min(available * 0.75)).max(min_velocity);
    let preferred = if preferred.is_finite() { preferred } else { DEFAULT_VELOCITY_HEIGHT };
    let height = preferred.clamp(min_velocity, max_velocity);
    let right = (full.max.x - 4.0).max(full.min.x);
    let left = (full.min.x + KEY_W).min(right);
    let roll = Rect::from_min_max(pos2(left, top), pos2(right, bottom - height - divider_h));
    let divider = Rect::from_min_max(pos2(full.min.x, roll.max.y), pos2(right, roll.max.y + divider_h));
    let velocity = Rect::from_min_max(pos2(left, divider.max.y), pos2(right, bottom));
    (roll, velocity, divider)
}

fn resize_velocity(app: &mut SoundApp, ui: &mut Ui, full: Rect, cid: ClipId) -> (Rect, Rect, Rect) {
    let (_, velocity, divider) = lane_layout(full, app.ui.midi_velocity_height);
    let response = ui.interact(divider, ui.id().with("velocity_divider"), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Velocity lane height"));
    let response = response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize Velocity · Double-click to reset · Up/Down when focused");
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if response.has_focus() {
        ui.memory_mut(|memory| memory.set_focus_lock_filter(response.id, egui::EventFilter { vertical_arrows: true, ..Default::default() }));
    }
    if app.midi.velocity_drag.is_some_and(|drag| drag.full != full || drag.clip != cid) || !ui.input(|i| i.pointer.primary_down()) {
        app.midi.velocity_drag = None;
    }
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        app.midi.velocity_drag = Some(VelocityDrag { origin_y: origin.y, height: velocity.height(), full, clip: cid });
    }
    if response.dragged()
        && let (Some(drag), Some(pointer)) = (app.midi.velocity_drag, response.interact_pointer_pos())
    {
        // Clamp to the displayed bounds so reversing at a limit responds immediately.
        let requested = drag.height + drag.origin_y - pointer.y;
        let (_, resized, _) = lane_layout(full, requested);
        let _ = app.run("ui.midi_velocity_height", json!({"height": resized.height()}));
        app.midi.velocity_drag = Some(VelocityDrag { origin_y: pointer.y, height: resized.height(), ..drag });
    }
    if response.double_clicked() {
        let _ = app.run("ui.midi_velocity_height", json!({"height": DEFAULT_VELOCITY_HEIGHT}));
    } else if response.has_focus() {
        let delta = ui.input_mut(|i| {
            let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
            let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
            (i32::from(up) - i32::from(down)) as f32 * 8.0
        });
        if delta != 0.0 {
            // The first key can arrive immediately after click focus, before egui
            // has retained its filter. Keep that key from navigating to a note.
            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            let (_, resized, _) = lane_layout(full, velocity.height() + delta);
            let _ = app.run("ui.midi_velocity_height", json!({"height": resized.height()}));
        }
    }
    let layout = lane_layout(full, app.ui.midi_velocity_height);
    let color = if response.hovered() || response.dragged() || response.has_focus() { Tokens::current().accent } else { Tokens::current().text_dim };
    ui.painter().rect_filled(layout.2, 0.0, Tokens::current().panel_bg2);
    let center = layout.2.center();
    ui.painter().line_segment([center - vec2(14.0, 0.0), center + vec2(14.0, 0.0)], Stroke::new(1.0, color));
    layout
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

fn finite_param(p: &serde_json::Value, key: &str, default: f64) -> Result<f64, String> {
    p.get(key).map_or(Some(default), serde_json::Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("`{key}` must be finite"))
}

/// UI-only viewport commands, shared by pointer input and the control channel.
pub fn view_command(app: &mut SoundApp, id: &str, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    if !app.ui.show_midi_editor {
        return Err("Open the MIDI editor first".into());
    }
    let cid = target_clip(app).ok_or("Select a MIDI clip first")?;
    let mode = midi_playback::effective(app);
    let base = app
        .engine
        .session()
        .find_clip(cid)
        .map(|(_, c)| app.engine.session().tempo.samples_to_ticks(c.start, app.engine.session().sample_rate) as f64)
        .ok_or("Select a MIDI clip first")?;
    let max_tick = app.engine.session().tempo.samples_to_ticks_f(soundcraft_time::MAX_POSITION, app.engine.session().sample_rate);
    let m = &mut app.midi;
    let width = f64::from(m.roll[2] - m.roll[0]);
    let height = f64::from(m.roll[3] - m.roll[1]);
    if m.clip != Some(cid)
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !m.len_ticks.is_finite()
        || m.len_ticks <= 0.0
        || !m.zoom.is_finite()
        || m.zoom < 1.0
    {
        return Err("Open the MIDI editor first".into());
    }
    let horizontal = match id {
        "ui.midi_zoom_at" => {
            let factor = finite_param(p, "factor", 1.0)?;
            let anchor = finite_param(p, "anchor_px", width * 0.5)?.clamp(0.0, width);
            if factor <= 0.0 {
                return Err("`factor` must be positive".into());
            }
            let tick = m.scroll_ticks + anchor * m.len_ticks / (width * m.zoom);
            m.zoom = (m.zoom * factor).clamp(1.0, 1024.0);
            m.scroll_ticks = tick - anchor * m.len_ticks / (width * m.zoom);
            true
        }
        "ui.midi_scroll" => {
            // Validate both axes before changing either one.
            let dx = finite_param(p, "by_px", 0.0)?;
            let dy = finite_param(p, "by_y_px", 0.0)?;
            m.scroll_ticks += dx * m.len_ticks / (width * m.zoom);
            m.top_pitch = (m.top_pitch - dy / f64::from(ROW_H)).clamp((height / f64::from(ROW_H) - 1.0).clamp(0.0, 127.0), 127.0);
            dx != 0.0
        }
        "ui.midi_fit" => {
            m.zoom = 1.0;
            m.scroll_ticks = 0.0;
            true
        }
        _ => return Err(format!("Unknown MIDI view command `{id}`")),
    };
    let span = m.len_ticks / m.zoom;
    m.scroll_ticks = if mode.centered() {
        let lower = -base - span * 0.5;
        m.scroll_ticks.clamp(lower, (max_tick - base - span * 0.5).max(lower))
    } else {
        m.scroll_ticks.clamp(0.0, (m.len_ticks - span).max(0.0))
    };
    midi_playback::manual_view_changed(app, horizontal);
    Ok(json!({"zoom": app.midi.zoom, "scroll_ticks": app.midi.scroll_ticks, "top_pitch": app.midi.top_pitch}))
}

fn trackpad(app: &mut SoundApp, ui: &mut Ui, area: Rect, roll: Rect) {
    if !ui.rect_contains_pointer(area) {
        return;
    }
    let (delta, factor, pointer, scrolling) = ui.input(|i| (i.translation_delta(), i.zoom_delta(), i.pointer.hover_pos(), i.is_scrolling()));
    if factor.is_finite() && factor > 0.0 && (factor - 1.0).abs() > f32::EPSILON {
        let anchor_px = pointer.map_or(roll.width() * 0.5, |p| (p.x - roll.min.x).clamp(0.0, roll.width()));
        let _ = app.run("ui.midi_zoom_at", json!({"factor": factor, "anchor_px": anchor_px}));
    } else if delta.x.is_finite() && delta.y.is_finite() && delta != egui::Vec2::ZERO {
        // Shift is already mapped onto X by egui, just as in the timeline.
        let _ = app.run("ui.midi_scroll", json!({"by_px": -delta.x, "by_y_px": -delta.y}));
    }
    ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    if scrolling {
        ui.ctx().request_repaint();
    }
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    // Painter-only contents do not advance the layout. Claim the resized
    // space, including the empty state, so the dock keeps its chosen height.
    ui.take_available_space();
    let t = Tokens::current();
    let full = ui.max_rect();
    let (_, _, divider) = lane_layout(full, app.ui.midi_velocity_height);
    if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(full) && !ui.rect_contains_pointer(divider) {
        app.midi.follow.owns_transport = true;
        if midi_playback::effective(app) == MidiScrollingMode::Center && !app.midi.follow.cue_ready {
            midi_playback::initialize_cue(app);
        }
    }
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let header = Rect::from_min_size(full.min, vec2(full.width(), 22.0));
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().text(pos2(header.min.x + 8.0, header.center().y), Align2::LEFT_CENTER, "MIDI EDITOR", bold(11.5), t.header_text);
    ui.interact(Rect::from_min_size(header.min, vec2(108.0, header.height())), ui.id().with("midi_help"), Sense::hover())
        .on_hover_text("Two-finger scroll: time/pitch · Shift+scroll: time · Pinch or Cmd/Ctrl+scroll: zoom at pointer · Fit: show whole clip");
    let Some(cid) = target_clip(app) else {
        app.midi.roll = [0.0; 4];
        app.midi.velocity = [0.0; 4];
        app.midi.velocity_divider = [0.0; 4];
        app.midi.velocity_drag = None;
        app.midi.velocity_note = None;
        midi_playback::release_transport(app);
        let track = {
            let s = app.engine.session();
            s.tracks.iter().find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.name.clone()))
        };
        let msg = match &track {
            Some((_, name)) => format!("Click to add a MIDI clip to {name} at the insertion point."),
            None => "Select a MIDI clip (or a MIDI track) to edit its notes.".into(),
        };
        ui.painter().text(full.center(), Align2::CENTER_CENTER, msg, regular(13.0), t.text_dim);
        let body = Rect::from_min_max(pos2(full.min.x, header.max.y), full.max);
        if let Some((id, _)) = track
            && ui.interact(body, ui.id().with("new_clip"), Sense::click()).clicked()
        {
            let sel = app.engine.session().edit.selection;
            if let Ok(v) = app.run("midi.new_clip", json!({"track": id.0, "start": sel.start, "end": sel.end})) {
                let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
            }
        }
        return;
    };
    let Some((track, clip)) = app.engine.session().find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
    let ClipContent::Midi { sequence } = &clip.content else { return };
    let track_color = app.engine.session().track(track).map_or([120, 120, 200], |t| t.color);
    let controls_w = 545.0_f32.min((header.width() - 110.0).max(0.0));
    let bar = Rect::from_min_max(pos2(header.max.x - controls_w, header.min.y + 1.0), pos2(header.max.x - 6.0, header.max.y - 1.0));
    let caption = Rect::from_min_max(pos2(header.min.x + 110.0, header.min.y), pos2(bar.min.x - 6.0, header.max.y));
    if caption.width() > 8.0 {
        let mut caption_ui = ui.new_child(egui::UiBuilder::new().max_rect(caption));
        caption_ui.set_clip_rect(caption.intersect(ui.clip_rect()));
        caption_ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} · {} notes · {} selected · Pencil: add · drag: move/select · arrows: move · Delete: remove",
                    clip.name,
                    sequence.notes.len(),
                    app.midi.selected.len()
                ))
                .size(11.0)
                .color(t.text_dim),
            )
            .truncate(),
        );
    }
    let grid_ticks = match app.engine.session().edit.grid {
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
    };
    // Header toolbar: whole-clip MIDI operations.
    let mut toolbar = ui.new_child(egui::UiBuilder::new().max_rect(bar));
    toolbar.set_clip_rect(bar.intersect(ui.clip_rect()));
    let mut op: Option<(&str, serde_json::Value)> = None;
    egui::ScrollArea::horizontal().id_salt("midi_toolbar").auto_shrink([false, false]).show(&mut toolbar, |ui| {
        let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()).layout(egui::Layout::right_to_left(egui::Align::Center)));
        tb.spacing_mut().item_spacing.x = 4.0;
        if tb.small_button("Fit").on_hover_text("Show the whole MIDI clip").clicked() {
            let _ = app.run("ui.midi_fit", json!({}));
        }
        if tb.small_button("Legato").clicked() {
            op = Some(("event.change_duration", json!({"legato": 0})));
        }
        if tb.small_button("Vel −10").clicked() {
            op = Some(("event.change_velocity", json!({"add": -10})));
        }
        if tb.small_button("Vel +10").clicked() {
            op = Some(("event.change_velocity", json!({"add": 10})));
        }
        if tb.small_button("Quantize").on_hover_text("Quantize to the grid").clicked() {
            op = Some(("event.quantize", json!({"grid": grid_ticks, "strength": 100})));
        }
        tb.menu_button(format!("Scrolling: {}", app.ui.midi_scrolling.label()), |ui| {
            for mode in MidiScrollingMode::ALL {
                if ui.selectable_label(app.ui.midi_scrolling == mode, mode.label()).clicked() {
                    let _ = app.run("ui.midi_scrolling", json!({"mode": mode.id()}));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("MIDI Editor scrolling is independent of the Edit window. Select the current mode again to resume following playback.");
    });
    if let Some((id, mut p)) = op {
        p["clips"] = json!([cid.0]);
        let _ = app.run(id, p);
    }
    let s = app.engine.session_arc();
    let sr = s.sample_rate;
    let base_tick = s.tempo.samples_to_ticks(clip.start, sr);
    let len_ticks = s.tempo.samples_to_ticks(clip.end(), sr).saturating_sub(base_tick).max(1);
    let (roll, vel, divider) = resize_velocity(app, ui, full, cid);
    let keys = Rect::from_min_max(pos2(full.min.x, roll.min.y), pos2(roll.min.x, roll.max.y));
    // Vertical range: centre on the notes the first time.
    let rows = (roll.height() / ROW_H).floor() as i32;
    if app.midi.clip != Some(cid) {
        if app.midi.clip.is_some() {
            midi_playback::release_transport(app);
            app.midi.follow.pending_stop = None;
        }
        let hi = sequence.notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(72);
        let lo = sequence.notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(48);
        let mid = (hi + lo) / 2;
        app.midi.top_pitch = f64::from((mid + rows / 2).clamp(rows.min(127), 127));
        app.midi.zoom = 1.0;
        app.midi.scroll_ticks = 0.0;
        app.midi.selected.clear();
        app.midi.drag = None;
        app.midi.velocity_note = None;
        app.midi.band = None;
        app.midi.clip = Some(cid);
    }
    app.midi.roll = [roll.min.x, roll.min.y, roll.max.x, roll.max.y];
    app.midi.velocity = [vel.min.x, vel.min.y, vel.max.x, vel.max.y];
    app.midi.velocity_divider = [divider.min.x, divider.min.y, divider.max.x, divider.max.y];
    app.midi.len_ticks = len_ticks as f64;
    app.midi.top_pitch = app.midi.top_pitch.clamp((f64::from(roll.height() / ROW_H) - 1.0).clamp(0.0, 127.0), 127.0);
    if roll.width() <= 0.0 || roll.height() <= 0.0 {
        return;
    }
    trackpad(app, ui, full, roll);
    midi_playback::update_view(app, base_tick as f64);
    let top = app.midi.top_pitch;
    let scroll = app.midi.scroll_ticks;
    let px_per_tick = f64::from(roll.width()) * app.midi.zoom / len_ticks as f64;
    let x_of = |tick: i64| roll.min.x + ((tick as f64 - scroll) * px_per_tick) as f32;
    let y_of = |pitch: i32| roll.min.y + (top - f64::from(pitch)) as f32 * ROW_H;
    let painter = ui.painter().with_clip_rect(roll.union(keys));
    // Background rows and keys.
    for r in 0..=rows + 1 {
        let pitch = top.ceil() as i32 - r;
        let y = y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.min.x, y), vec2(roll.width(), ROW_H));
        painter.rect_filled(row, 0.0, if is_black(pitch) { Color32::from_rgb(30, 30, 32) } else { Color32::from_rgb(40, 40, 42) });
        let key = Rect::from_min_size(pos2(keys.min.x, y), vec2(KEY_W - 2.0, ROW_H - 1.0));
        painter.rect_filled(key, 1.0, if is_black(pitch) { Color32::from_rgb(20, 20, 20) } else { Color32::from_rgb(225, 225, 225) });
        if pitch.rem_euclid(12) == 0 && (0..=127).contains(&pitch) {
            painter.text(
                pos2(key.max.x - 3.0, key.center().y),
                Align2::RIGHT_CENTER,
                soundcraft_midi::note_name(pitch as u8),
                regular(8.0),
                Color32::from_rgb(40, 40, 40),
            );
            painter.line_segment([pos2(roll.min.x, y + ROW_H), pos2(roll.max.x, y + ROW_H)], Stroke::new(1.0, Color32::from_rgb(64, 64, 68)));
        }
    }
    // Grid.
    let roll_painter = painter.with_clip_rect(roll);
    // Draw only visible grid lines; long clips and extreme zoom never cause an unbounded loop.
    let bar_ticks = TICKS_PER_QUARTER * 4;
    let step = if grid_ticks as f64 * px_per_tick >= 4.0 { grid_ticks } else { bar_ticks };
    let step = step.saturating_mul((4.0 / (step as f64 * px_per_tick)).ceil().max(1.0) as i64).max(1);
    let mut tick = (scroll as i64 / step).saturating_mul(step);
    let end_tick = soundcraft_time::to_samples(scroll + f64::from(roll.width()) / px_per_tick).min(len_ticks);
    while tick <= end_tick {
        let bar = tick % bar_ticks == 0;
        let x = x_of(tick);
        if grid_ticks as f64 * px_per_tick >= 4.0 || bar {
            roll_painter.line_segment(
                [pos2(x, roll.min.y), pos2(x, roll.max.y)],
                Stroke::new(1.0, if bar { Color32::from_rgb(80, 80, 86) } else { Color32::from_rgb(50, 50, 54) }),
            );
        }
        let next = tick.saturating_add(step);
        if next <= tick {
            break;
        }
        tick = next;
    }
    // Notes.
    let base = egui::Color32::from_rgb(track_color[0], track_color[1], track_color[2]);
    let drag = app.midi.drag.clone();
    for (i, n) in sequence.notes.iter().enumerate() {
        let (mut st, mut ln, mut p) = (n.start, n.length, i32::from(n.pitch));
        if let Some(d) = &drag
            && (d.index == i || (!d.resize && app.midi.selected.contains(&i) && app.midi.selected.contains(&d.index)))
        {
            if d.resize {
                if d.index == i {
                    ln = ln.saturating_add(d.dx_ticks).max(grid_ticks.min(ln).max(1));
                }
            } else {
                st = st.saturating_add(d.dx_ticks).max(0);
                p = i64::from(p).saturating_add(d.dy_semi).clamp(0, 127) as i32;
            }
        }
        let r = Rect::from_min_max(pos2(x_of(st), y_of(p) + 1.0), pos2(x_of(st.saturating_add(ln)).max(x_of(st) + 3.0), y_of(p) + ROW_H - 1.0));
        let sel = app.midi.selected.contains(&i);
        let k = 0.45 + 0.55 * f32::from(n.velocity) / 127.0;
        let fill = if sel {
            Color32::WHITE
        } else {
            Color32::from_rgb((f32::from(base.r()) * k) as u8, (f32::from(base.g()) * k) as u8, (f32::from(base.b()) * k) as u8)
        };
        roll_painter.rect(r, 2.0, fill, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    }
    // Interaction.
    let resp = ui.interact(roll, ui.id().with(("roll", cid.0)), Sense::click_and_drag());
    let hit = |pos: egui::Pos2| -> Option<(usize, bool)> {
        sequence.notes.iter().enumerate().rev().find_map(|(i, n)| {
            let r = Rect::from_min_max(
                pos2(x_of(n.start), y_of(i32::from(n.pitch))),
                pos2(x_of(n.start.saturating_add(n.length)).max(x_of(n.start) + 3.0), y_of(i32::from(n.pitch)) + ROW_H),
            );
            r.contains(pos).then_some((i, pos.x > r.max.x - 5.0))
        })
    };
    let snap = |tk: f64| -> i64 { soundcraft_time::to_samples((tk / grid_ticks as f64).round()).saturating_mul(grid_ticks) };
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if let Some((i, edge)) = hit(p) {
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
        ui.painter().with_clip_rect(roll).rect(
            band,
            0.0,
            Color32::from_rgba_unmultiplied(120, 170, 255, 40),
            Stroke::new(1.0, Color32::from_rgb(120, 170, 255)),
            StrokeKind::Inside,
        );
        if resp.drag_stopped() {
            app.midi.selected = sequence
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let r = Rect::from_min_max(
                        pos2(x_of(n.start), y_of(i32::from(n.pitch))),
                        pos2(x_of(n.start.saturating_add(n.length)), y_of(i32::from(n.pitch)) + ROW_H),
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
        d.dx_ticks = snap(f64::from(p.x - d.start.x) / px_per_tick);
        d.dy_semi = -((p.y - d.start.y) / ROW_H).round() as i64;
    }
    if resp.drag_stopped()
        && let Some(d) = app.midi.drag.take()
    {
        if d.resize {
            if let Some(n) = sequence.notes.get(d.index) {
                let _ =
                    app.run("midi.note_edit", json!({"clip": cid.0, "index": d.index, "length_ticks": n.length.saturating_add(d.dx_ticks).max(1)}));
            }
        } else if d.dx_ticks != 0 || d.dy_semi != 0 {
            let _ = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": d.dx_ticks, "semitones": d.dy_semi}));
            app.midi.selected.clear();
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(p) {
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
            None if s.edit.tool == Tool::Pencil => {
                let pitch = (top - f64::from((p.y - roll.min.y) / ROW_H)).ceil().clamp(0.0, 127.0) as i32;
                let start = (soundcraft_time::to_samples(scroll + f64::from(p.x - roll.min.x) / px_per_tick) / grid_ticks).saturating_mul(grid_ticks);
                let _ = app.run(
                    "midi.note_add",
                    json!({"clip": cid.0, "pitch": pitch, "start_ticks": start.max(0), "length_ticks": grid_ticks, "velocity": 100}),
                );
                app.midi.selected.clear();
            }
            None => app.midi.selected.clear(),
        }
    }
    // Keyboard: Cmd+A select all; arrows transpose (Shift = octave) and move by the grid.
    if ui.rect_contains_pointer(roll) && !ui.ctx().egui_wants_keyboard_input() {
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
            let _ = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks, "semitones": semi}));
        }
    }
    if !app.midi.selected.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": app.midi.selected}));
        app.midi.selected.clear();
    }
    // Velocity lane.
    ui.painter().rect_filled(vel, 0.0, Color32::from_rgb(28, 28, 30));
    ui.painter().text(pos2(full.min.x + 6.0, vel.center().y), Align2::LEFT_CENTER, "Velocity", regular(10.0), t.text_dim);
    let vresp = ui
        .interact(vel, ui.id().with(("vel", cid.0)), Sense::click_and_drag())
        .on_hover_text("Drag a stem or its note-length tail to change velocity · Double-click to reset to 100");
    let velocity_painter = ui.painter().with_clip_rect(vel.intersect(ui.clip_rect()));
    let velocity_y = |velocity: u8| vel.max.y - 2.0 - (vel.height() - 4.0).max(0.0) * f32::from(velocity) / 127.0;
    let tail = |note: &soundcraft_midi::Note| {
        let start = x_of(note.start) + 1.0;
        let end = x_of(note.start.saturating_add(note.length.max(0))).max(start);
        let y = velocity_y(note.velocity);
        (start <= vel.max.x && end >= vel.min.x).then(|| [pos2(start.max(vel.min.x), y), pos2(end.min(vel.max.x), y)])
    };
    let hit_at = |p: egui::Pos2| {
        // Prefer the visible horizontal handle to a nearby note's vertical stem.
        // Stem clicks retain the existing direct-to-value editing behavior.
        let handle = sequence
            .notes
            .iter()
            .enumerate()
            .filter_map(|(index, note)| {
                let [a, b] = tail(note)?;
                let dx = (a.x - p.x).max(p.x - b.x).max(0.0);
                let dy = (a.y - p.y).abs();
                (dx < 6.0 && dy < 6.0).then_some((index, dx * dx + dy * dy))
            })
            .min_by(|(a, da), (b, db)| da.total_cmp(db).then_with(|| app.midi.selected.contains(b).cmp(&app.midi.selected.contains(a))))
            .map(|(index, _)| (index, true));
        handle.or_else(|| {
            sequence
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| (x_of(n.start) + 1.0 - p.x).abs() < 8.0)
                .min_by(|(_, a), (_, b)| {
                    (x_of(a.start) + 1.0 - p.x)
                        .abs()
                        .total_cmp(&(x_of(b.start) + 1.0 - p.x).abs())
                        .then_with(|| (velocity_y(a.velocity) - p.y).abs().total_cmp(&(velocity_y(b.velocity) - p.y).abs()))
                })
                .map(|(index, _)| (index, false))
        })
    };
    let hit = vresp.interact_pointer_pos().and_then(hit_at);
    let hovered = vresp.hover_pos().and_then(hit_at);
    if hovered.is_some() || vresp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    // Paint selected notes last so their handles remain visible in chords.
    let stems = sequence.notes.iter().enumerate();
    for (i, n) in stems.clone().filter(|(i, _)| !app.midi.selected.contains(i)).chain(stems.filter(|(i, _)| app.midi.selected.contains(i))) {
        let x = x_of(n.start);
        let h = (vel.height() - 4.0).max(0.0) * f32::from(n.velocity) / 127.0;
        let col = if app.midi.selected.contains(&i) { Color32::WHITE } else { base };
        velocity_painter.line_segment([pos2(x + 1.0, vel.max.y - 2.0), pos2(x + 1.0, vel.max.y - 2.0 - h)], Stroke::new(3.0, col));
        velocity_painter.circle_filled(pos2(x + 1.0, vel.max.y - 2.0 - h), 2.5, col);
        if let Some(points) = tail(n) {
            velocity_painter.line_segment(points, Stroke::new(2.0, col));
        }
    }
    if let Some(p) = vresp.interact_pointer_pos() {
        if vresp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
            app.midi.velocity_note = hit.map(|(index, on_handle)| VelocityNoteDrag {
                clip: cid,
                index,
                // The vertical stem keeps click-to-value editing. A tail grab
                // preserves its offset so the wider handle does not cause a jump.
                offset_y: sequence
                    .notes
                    .get(index)
                    .filter(|note| on_handle && (x_of(note.start) + 1.0 - p.x).abs() >= 8.0)
                    .map_or(0.0, |note| p.y - velocity_y(note.velocity)),
            });
        }
        let target = app
            .midi
            .velocity_note
            .filter(|grab| grab.clip == cid)
            .map(|grab| (grab.index, grab.offset_y))
            .or_else(|| hit.map(|(index, _)| (index, 0.0)));
        if (vresp.clicked() || vresp.dragged())
            && let Some((index, offset_y)) = target
            && let Some(note) = sequence.notes.get(index)
        {
            let velocity = if vresp.double_clicked() {
                100
            } else {
                (((vel.max.y - 2.0 - (p.y - offset_y)) / (vel.height() - 4.0).max(1.0)) * 127.0).round().clamp(1.0, 127.0) as u8
            };
            if velocity != note.velocity
                && let Ok(result) = app.engine.execute_merged(
                    "midi.note_edit",
                    &json!({"clip": cid.0, "index": index, "velocity": velocity}),
                    &format!("vel:{}", cid.0),
                )
                && let Some(index) = result.get("index").and_then(serde_json::Value::as_u64).and_then(|index| usize::try_from(index).ok())
            {
                // Editing can reorder duplicate notes; follow the returned index.
                app.midi.velocity_note = Some(VelocityNoteDrag { clip: cid, index, offset_y });
            }
        }
    }
    if !ui.input(|i| i.pointer.primary_down()) {
        app.midi.velocity_note = None;
    }
    // Paint last over the notes, clipped to the piano roll, without claiming input.
    let at = s.tempo.samples_to_ticks_f(midi_playback::display_position(app), sr) - base_tick as f64;
    let x = roll.min.x + ((at - scroll) * px_per_tick) as f32;
    if x.is_finite() && x >= roll.min.x && x <= roll.max.x {
        let color = if midi_playback::effective(app) == MidiScrollingMode::Center && !app.engine.transport.recording { t.accent } else { t.playhead };
        ui.painter().with_clip_rect(roll.intersect(ui.clip_rect())).line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.5, color));
    }
}
