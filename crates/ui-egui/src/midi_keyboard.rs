//! Window › MIDI Keyboard: an on-screen keyboard of one to four octaves, played with the mouse or
//! with the computer keys, on the record-enabled MIDI and Instrument tracks.
//!
//! The computer keys play two octaves: Z to M is the lower one (black keys on S D, G H J) and Q to
//! I the upper one (black keys on 2 3, 5 6 7). The two keys right of P and L move the velocity and the
//! octave. Keys are read by position, so the layout works whatever the keyboard's language. They
//! belong to the keyboard only while it is active (the last click landed in its window); then the
//! shortcuts on those keys wait until a click elsewhere.
//!
//! In Step mode each key also writes its note into the target MIDI clip at the insertion point and
//! moves the insertion point on by the step length; keys held together make a chord.

use std::collections::BTreeMap;

use egui::{Align2, Color32, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_time::{Samples, TICKS_PER_QUARTER};

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::widgets::{rounded_vgradient, shade, vgradient};

/// The keyboard's settings, kept with the window layout.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KeyboardPrefs {
    /// Octaves on screen, 1..=4.
    pub octaves: u8,
    /// The octave of the lower computer row, in note names (C3 is middle C, MIDI 60).
    pub octave: i8,
    pub velocity: u8,
    /// Semitones added to the notes the computer keys play, -24..=24.
    pub transpose: i8,
    /// Step input instead of playing.
    pub step: bool,
    /// The length of a step-entered note, in ticks.
    pub step_ticks: i64,
    /// What the four control keys print on this computer (learnt from typing), in the order octave
    /// down, octave up, velocity down, velocity up.
    pub control_labels: [String; 4],
}

impl Default for KeyboardPrefs {
    fn default() -> Self {
        Self {
            octaves: 2,
            octave: 3,
            velocity: 100,
            transpose: 0,
            step: false,
            step_ticks: TICKS_PER_QUARTER / 2,
            control_labels: [";", "'", "[", "]"].map(String::from),
        }
    }
}

/// A note held by a computer key or the mouse, and where it went.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    tracks: Vec<TrackId>,
    pitch: u8,
}

/// The keyboard while the app runs.
#[derive(Debug, Default)]
pub struct KeyboardState {
    /// The computer keys play the keyboard (the last click landed in its window).
    pub active: bool,
    was_open: bool,
    keys: BTreeMap<KeyName, Held>,
    mouse: Option<Held>,
    /// Notes playing, per track and pitch, with how many keys hold each.
    pub sounding: BTreeMap<(TrackId, u8), u32>,
    /// Step input: where the chord being held goes.
    chord_at: Option<Samples>,
}

/// `egui::Key` has no `Ord`; its name does.
type KeyName = &'static str;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    /// Semitones above the lower row's C.
    Note(i32),
    OctaveDown,
    OctaveUp,
    VelocityDown,
    VelocityUp,
}

/// The lower row, then the upper one, as (key, semitones above the lower C).
const NOTE_KEYS: [(Key, i32); 25] = [
    (Key::Z, 0),
    (Key::S, 1),
    (Key::X, 2),
    (Key::D, 3),
    (Key::C, 4),
    (Key::V, 5),
    (Key::G, 6),
    (Key::B, 7),
    (Key::H, 8),
    (Key::N, 9),
    (Key::J, 10),
    (Key::M, 11),
    (Key::Q, 12),
    (Key::Num2, 13),
    (Key::W, 14),
    (Key::Num3, 15),
    (Key::E, 16),
    (Key::R, 17),
    (Key::Num5, 18),
    (Key::T, 19),
    (Key::Num6, 20),
    (Key::Y, 21),
    (Key::Num7, 22),
    (Key::U, 23),
    (Key::I, 24),
];

/// Octave down, octave up, velocity down, velocity up: the two keys right of L and the two right
/// of P (ò à è + on an Italian keyboard, ; ' [ ] on a US one).
const CONTROL_KEYS: [Key; 4] = [Key::Semicolon, Key::Quote, Key::OpenBracket, Key::CloseBracket];

fn action(key: Key) -> Option<Action> {
    if let Some((_, n)) = NOTE_KEYS.iter().find(|(k, _)| *k == key) {
        return Some(Action::Note(*n));
    }
    let i = CONTROL_KEYS.iter().position(|k| *k == key)?;
    [Action::OctaveDown, Action::OctaveUp, Action::VelocityDown, Action::VelocityUp].get(i).copied()
}

/// The label a note key shows on screen.
fn note_label(key: Key) -> &'static str {
    match key {
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        k => k.name(),
    }
}

const fn is_black(pitch: i32) -> bool {
    matches!(pitch.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// MIDI pitch of the lower row's C.
fn low_c(p: &KeyboardPrefs) -> i32 {
    (i32::from(p.octave) + 2) * 12
}

/// The pitch a computer key plays, if in range.
fn key_pitch(p: &KeyboardPrefs, semis: i32) -> Option<u8> {
    u8::try_from(low_c(p) + semis + i32::from(p.transpose)).ok().filter(|v| *v <= 127)
}

/// The first C on screen: the octave holding the lowest computer key, kept inside MIDI's range.
fn view_start(p: &KeyboardPrefs) -> i32 {
    let lowest = low_c(p) + i32::from(p.transpose);
    let max = (127 - 12 * i32::from(p.octaves.clamp(1, 4))) / 12 * 12;
    (lowest - lowest.rem_euclid(12)).clamp(0, max.max(0))
}

/// The tracks the keyboard plays: the record-enabled MIDI and Instrument tracks, else the selected
/// ones, else the track of the clip in the MIDI Editor.
fn targets(app: &SoundApp) -> Vec<TrackId> {
    let s = app.engine.session();
    let midi = |t: &&soundcraft_model::Track| t.kind.is_midi() && !t.inactive;
    let armed: Vec<TrackId> = s.tracks.iter().filter(midi).filter(|t| t.mixer.record_arm).map(|t| t.id).collect();
    if !armed.is_empty() {
        return armed;
    }
    let selected: Vec<TrackId> = s.tracks.iter().filter(midi).filter(|t| s.edit.selected_tracks.contains(&t.id)).map(|t| t.id).collect();
    if !selected.is_empty() {
        return selected;
    }
    crate::midi_editor::target_clip(app).and_then(|c| s.find_clip(c).map(|(t, _)| t)).into_iter().collect()
}

fn start(app: &mut SoundApp, pitch: u8) -> Held {
    let held = Held { tracks: targets(app), pitch };
    let velocity = app.ui.midi_keyboard.velocity.clamp(1, 127);
    for t in &held.tracks {
        let n = app.keyboard.sounding.entry((*t, pitch)).or_insert(0);
        *n += 1;
        if *n == 1
            && let Some(player) = &app.player
        {
            player.note(*t, pitch, velocity);
        }
    }
    if app.ui.midi_keyboard.step {
        step_enter(app, pitch, velocity);
    }
    held
}

fn stop(app: &mut SoundApp, held: Held) {
    for t in held.tracks {
        let Some(n) = app.keyboard.sounding.get_mut(&(t, held.pitch)) else { continue };
        *n = n.saturating_sub(1);
        if *n == 0 {
            app.keyboard.sounding.remove(&(t, held.pitch));
            if let Some(player) = &app.player {
                player.note(t, held.pitch, 0);
            }
        }
    }
    if app.keyboard.keys.is_empty() && app.keyboard.mouse.is_none() {
        app.keyboard.chord_at = None;
    }
}

/// Lets go of every note (the window closed, the keyboard lost the keys, the app lost focus).
fn release_all(app: &mut SoundApp) {
    let keys = std::mem::take(&mut app.keyboard.keys);
    for held in keys.into_values() {
        stop(app, held);
    }
    if let Some(held) = app.keyboard.mouse.take() {
        stop(app, held);
    }
    app.keyboard.chord_at = None;
}

/// Step input: the note goes into the target clip at the insertion point (or with the chord being
/// held), and the insertion point moves on by one step.
fn step_enter(app: &mut SoundApp, pitch: u8, velocity: u8) {
    // Step input is for a stopped transport, as in Pro Tools: while playing or recording the keys
    // only play (moving the insertion point would also locate the playhead and cut the note).
    if app.is_playing() {
        return;
    }
    let Some(cid) = crate::midi_editor::target_clip(app) else {
        app.ui.status = "Step input: select a MIDI clip or a MIDI track".into();
        return;
    };
    let len = app.ui.midi_keyboard.step_ticks.clamp(1, TICKS_PER_QUARTER * 4);
    let s = app.engine.session();
    let at = app.keyboard.chord_at.unwrap_or(s.edit.selection.start);
    let clip_start = s.find_clip(cid).map_or(0, |(_, c)| c.start);
    let at_ticks = s.tempo.samples_to_ticks(at, s.sample_rate);
    let rel = at_ticks - s.tempo.samples_to_ticks(clip_start, s.sample_rate);
    let next = s.tempo.tick_to_samples(at_ticks + len, s.sample_rate);
    let _ = app.run("midi.note_add", json!({"clip": cid.0, "pitch": pitch, "start_ticks": rel.max(0), "length_ticks": len, "velocity": velocity}));
    if app.keyboard.chord_at.is_none() {
        app.keyboard.chord_at = Some(at);
        let _ = app.run("transport.locate", json!({"at": next}));
    }
}

fn octave_by(app: &mut SoundApp, d: i8) {
    let p = &mut app.ui.midi_keyboard;
    p.octave = p.octave.saturating_add(d).clamp(-2, 8);
}

fn velocity_by(app: &mut SoundApp, d: i32) {
    let p = &mut app.ui.midi_keyboard;
    p.velocity = (i32::from(p.velocity) + d).clamp(1, 127) as u8;
}

/// Takes the keyboard's keys out of this frame's input before the shortcuts see them. Call before
/// `shortcuts::handle`.
pub fn capture(app: &mut SoundApp, ctx: &egui::Context) {
    let open = app.ui.show_midi_keyboard;
    if open && !app.keyboard.was_open {
        // Opening the keyboard means playing it.
        app.keyboard.active = true;
    }
    if !open && app.keyboard.was_open {
        app.keyboard.active = false;
        release_all(app);
    }
    app.keyboard.was_open = open;
    if !open {
        return;
    }
    // A click elsewhere hands the keys back to the shortcuts; one in the window takes them. A
    // click in a popup (the step length menu) changes nothing.
    let click = ctx.input(|i| if i.pointer.primary_pressed() { i.pointer.interact_pos() } else { None });
    if let Some(at) = click {
        let layer = ctx.layer_id_at(at);
        let inside = match layer {
            Some(l) if l.id == window_id() => true,
            Some(l) if l.order != egui::Order::Middle && l.order != egui::Order::Background => app.keyboard.active,
            _ => false,
        };
        if app.keyboard.active && !inside {
            release_all(app);
        }
        app.keyboard.active = inside;
    }
    if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false)))) {
        release_all(app);
    }
    if !app.keyboard.active || ctx.egui_wants_keyboard_input() || app.dialogs.open.is_some() {
        return;
    }
    let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
    let mut kept = Vec::with_capacity(events.len());
    let mut learn: Option<usize> = None;
    for e in events {
        match &e {
            egui::Event::Key { key, physical_key, pressed, repeat, modifiers } if !(modifiers.command || modifiers.ctrl || modifiers.alt) => {
                let key = physical_key.unwrap_or(*key);
                let Some(a) = action(key) else {
                    kept.push(e);
                    continue;
                };
                learn = None;
                if *repeat {
                    continue;
                }
                let name = key.name();
                match (a, *pressed) {
                    (Action::Note(semis), true) => {
                        if !app.keyboard.keys.contains_key(name)
                            && let Some(pitch) = key_pitch(&app.ui.midi_keyboard, semis)
                        {
                            let held = start(app, pitch);
                            app.keyboard.keys.insert(name, held);
                        }
                    }
                    (Action::Note(_), false) => {
                        if let Some(held) = app.keyboard.keys.remove(name) {
                            stop(app, held);
                        }
                    }
                    (_, false) => {}
                    (control, true) => {
                        let big = modifiers.shift;
                        match control {
                            Action::OctaveDown => octave_by(app, -1),
                            Action::OctaveUp => octave_by(app, 1),
                            Action::VelocityDown => velocity_by(app, if big { -15 } else { -5 }),
                            Action::VelocityUp => velocity_by(app, if big { 15 } else { 5 }),
                            Action::Note(_) => {}
                        }
                        learn = CONTROL_KEYS.iter().position(|k| *k == key);
                    }
                }
            }
            // The character a control key typed names it on this computer's layout.
            egui::Event::Text(t) => {
                if let Some(i) = learn.take() {
                    if let Some(label) = app.ui.midi_keyboard.control_labels.get_mut(i)
                        && t.chars().count() == 1
                        && *label != t.to_uppercase()
                    {
                        *label = t.to_uppercase();
                    }
                    continue;
                }
                kept.push(e);
            }
            _ => kept.push(e),
        }
    }
    ctx.input_mut(|i| i.events = kept);
}

fn window_id() -> egui::Id {
    egui::Id::new("MIDI Keyboard")
}

/// Step lengths offered in Step mode.
const STEPS: [(&str, i64); 6] = [
    ("1/1", TICKS_PER_QUARTER * 4),
    ("1/2", TICKS_PER_QUARTER * 2),
    ("1/4", TICKS_PER_QUARTER),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/32", TICKS_PER_QUARTER / 8),
];

const WHITE_W: f32 = 40.0;
const WHITE_H: f32 = 150.0;

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_keyboard;
    if !open {
        return;
    }
    egui::Window::new("MIDI Keyboard").id(window_id()).open(&mut open).auto_sized().collapsible(false).show(ctx, |ui| body(app, ui));
    app.ui.show_midi_keyboard = open;
}

fn body(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let prefs = app.ui.midi_keyboard.clone();
    let octaves = i32::from(prefs.octaves.clamp(1, 4));
    let first = view_start(&prefs);
    let last = (first + 12 * octaves).min(127);
    let whites = (first..=last).filter(|p| !is_black(*p)).count();
    let size = vec2(whites as f32 * WHITE_W, WHITE_H + 6.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(r);
    // The felt strip above the keys.
    vgradient(&painter, Rect::from_min_size(r.min, vec2(r.width(), 6.0)), Color32::from_gray(24), Color32::from_gray(48));
    let keys = Rect::from_min_max(pos2(r.min.x, r.min.y + 6.0), r.max);
    // Key rectangles: whites first, blacks over them.
    let mut rects: Vec<(i32, Rect)> = Vec::new();
    let mut wi = 0.0;
    for p in first..=last {
        if !is_black(p) {
            rects.push((p, Rect::from_min_size(pos2(keys.min.x + wi * WHITE_W, keys.min.y), vec2(WHITE_W, keys.height()))));
            wi += 1.0;
        }
    }
    let black_w = WHITE_W * 0.6;
    let mut wi = 0.0;
    for p in first..=last {
        if is_black(p) {
            // Sits on the line between its two white neighbours, nudged the way real keys lean.
            let lean = match p.rem_euclid(12) {
                1 | 6 => -0.12,
                3 | 10 => 0.12,
                _ => 0.0,
            } * black_w;
            let x = keys.min.x + wi * WHITE_W - black_w / 2.0 + lean;
            rects.push((p, Rect::from_min_size(pos2(x, keys.min.y), vec2(black_w, keys.height() * 0.62))));
        } else {
            wi += 1.0;
        }
    }
    // The mouse plays the key under it while the button is down.
    let under = resp
        .is_pointer_button_down_on()
        .then(|| ui.ctx().pointer_latest_pos())
        .flatten()
        .and_then(|at| rects.iter().rev().find(|(_, kr)| kr.contains(at)).and_then(|(p, _)| u8::try_from(*p).ok()));
    if app.keyboard.mouse.as_ref().map(|h| h.pitch) != under {
        if let Some(held) = app.keyboard.mouse.take() {
            stop(app, held);
        }
        if let Some(p) = under {
            app.keyboard.mouse = Some(start(app, p));
        }
    }
    // What is down: the mouse's key and the computer keys'.
    let down =
        |p: i32| app.keyboard.mouse.as_ref().is_some_and(|h| i32::from(h.pitch) == p) || app.keyboard.keys.values().any(|h| i32::from(h.pitch) == p);
    // Which computer key plays each pitch on screen.
    let mut labels: BTreeMap<i32, &'static str> = BTreeMap::new();
    for (k, semis) in NOTE_KEYS {
        labels.insert(low_c(&prefs) + semis + i32::from(prefs.transpose), note_label(k));
    }
    let lit = app.keyboard.active;
    for (p, kr) in &rects {
        let pressed = down(*p);
        if is_black(*p) {
            black_key(&painter, *kr, pressed);
        } else {
            white_key(&painter, *kr, pressed, t.accent);
        }
        if let Some(l) = labels.get(p) {
            let (color, y) = if is_black(*p) {
                (if lit { Color32::from_gray(235) } else { Color32::from_gray(120) }, kr.max.y - 14.0)
            } else {
                (if lit { Color32::from_gray(40) } else { Color32::from_gray(165) }, kr.max.y - 34.0)
            };
            painter.text(pos2(kr.center().x, y), Align2::CENTER_CENTER, *l, regular(if is_black(*p) { 15.0 } else { 19.0 }), color);
        }
        if p.rem_euclid(12) == 0 {
            painter.text(
                pos2(kr.center().x, kr.max.y - 9.0),
                Align2::CENTER_BOTTOM,
                soundcraft_midi::note_name(u8::try_from(*p).unwrap_or(0)),
                regular(9.0),
                Color32::from_gray(120),
            );
        }
    }
    if !lit {
        let _ = resp.on_hover_text("Click the keyboard to play it with the computer keys");
    }
    ui.add_space(6.0);
    controls(app, ui);
    ui.add_space(4.0);
    // As wide as the keys or the buttons, whichever is wider.
    let width = ui.min_rect().width().max(size.x);
    readout(app, ui, width);
}

/// A white key, lit from above with a lip at the bottom; it sinks and tints while down.
fn white_key(p: &egui::Painter, r: Rect, pressed: bool, accent: Color32) {
    let face = r.shrink2(vec2(0.5, 0.0));
    p.rect_filled(face, 3.0, Color32::from_gray(60));
    let inner = Rect::from_min_max(pos2(face.min.x + 1.0, face.min.y), pos2(face.max.x - 1.0, face.max.y - 1.0));
    if pressed {
        let (top, bottom) = (Color32::from_gray(196).lerp_to_gamma(accent, 0.18), Color32::from_gray(228).lerp_to_gamma(accent, 0.12));
        rounded_vgradient(p, inner, 3.0, top, bottom);
    } else {
        rounded_vgradient(p, inner, 3.0, Color32::from_gray(214), Color32::from_gray(250));
        // The lip: the front edge of the key, in shadow.
        let lip = Rect::from_min_max(pos2(inner.min.x, inner.max.y - 7.0), inner.max);
        rounded_vgradient(p, lip, 3.0, Color32::from_gray(232), Color32::from_gray(186));
        p.hline(inner.min.x + 1.0..=inner.max.x - 1.0, lip.min.y, Stroke::new(1.0, Color32::from_gray(200)));
    }
}

/// A black key: a dark body with a sloped, lighter top face; the face lengthens while down.
fn black_key(p: &egui::Painter, r: Rect, pressed: bool) {
    p.rect_filled(r.translate(vec2(1.5, 2.0)), 3.0, Color32::from_black_alpha(70));
    rounded_vgradient(p, r, 3.0, Color32::from_gray(12), Color32::from_gray(40));
    let bottom = if pressed { 4.0 } else { 11.0 };
    let face = Rect::from_min_max(pos2(r.min.x + 3.0, r.min.y), pos2(r.max.x - 3.0, r.max.y - bottom));
    let (top, low) = if pressed { (Color32::from_gray(30), Color32::from_gray(52)) } else { (Color32::from_gray(44), Color32::from_gray(78)) };
    rounded_vgradient(p, face, 2.0, top, low);
    p.vline(face.min.x + 1.0, face.y_range(), Stroke::new(1.0, Color32::from_white_alpha(14)));
}

/// A square button with an arrow over its key's label, in the octave (orange) or velocity (blue)
/// colour.
fn control_button(ui: &mut Ui, up: bool, label: &str, color: Color32, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    let p = ui.painter();
    let fill = if resp.is_pointer_button_down_on() {
        shade(color, -0.18)
    } else if resp.hovered() {
        shade(color, 0.08)
    } else {
        color
    };
    rounded_vgradient(p, r, 3.0, shade(fill, 0.12), shade(fill, -0.08));
    p.rect_stroke(r, 3.0, Stroke::new(1.0, shade(color, -0.4)), StrokeKind::Inside);
    let c = pos2(r.center().x, r.min.y + 10.0);
    let tri = if up {
        vec![pos2(c.x - 6.0, c.y + 3.0), pos2(c.x + 6.0, c.y + 3.0), pos2(c.x, c.y - 3.0)]
    } else {
        vec![pos2(c.x - 6.0, c.y - 3.0), pos2(c.x + 6.0, c.y - 3.0), pos2(c.x, c.y + 3.0)]
    };
    p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
    p.text(pos2(r.center().x, r.max.y - 15.0), Align2::CENTER_CENTER, label, regular(18.0), Color32::WHITE);
    resp
}

fn controls(app: &mut SoundApp, ui: &mut Ui) {
    let orange = Color32::from_rgb(222, 128, 46);
    let blue = Color32::from_rgb(40, 150, 214);
    let labels = app.ui.midi_keyboard.control_labels.clone();
    let shift = ui.input(|i| i.modifiers.shift);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if control_button(ui, false, &labels[0], orange, "Octave down").clicked() {
            octave_by(app, -1);
        }
        if control_button(ui, true, &labels[1], orange, "Octave up").clicked() {
            octave_by(app, 1);
        }
        ui.add_space(4.0);
        if control_button(ui, false, &labels[2], blue, "Velocity down (Shift: by 15)").clicked() {
            velocity_by(app, if shift { -15 } else { -5 });
        }
        if control_button(ui, true, &labels[3], blue, "Velocity up (Shift: by 15)").clicked() {
            velocity_by(app, if shift { 15 } else { 5 });
        }
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let p = &mut app.ui.midi_keyboard;
                if ui.selectable_label(!p.step, "Play").on_hover_text("The keys play the instrument").clicked() {
                    p.step = false;
                }
                if ui
                    .selectable_label(p.step, "Step")
                    .on_hover_text("Each key also writes its note into the selected MIDI clip at the insertion point and moves on")
                    .clicked()
                {
                    p.step = true;
                }
                if p.step {
                    let current = STEPS.iter().find(|(_, t)| *t == p.step_ticks).map_or("1/8", |(n, _)| *n);
                    egui::ComboBox::from_id_salt("midi-keyboard-step").width(56.0).selected_text(current).show_ui(ui, |ui| {
                        for (name, ticks) in STEPS {
                            ui.selectable_value(&mut p.step_ticks, ticks, name);
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label("Octaves");
                let p = &mut app.ui.midi_keyboard;
                for n in 1..=4u8 {
                    if ui.selectable_label(p.octaves == n, n.to_string()).clicked() {
                        p.octaves = n;
                    }
                }
            });
        });
    });
}

/// The dark readout bar: octave, velocity and transpose.
fn readout(app: &mut SoundApp, ui: &mut Ui, width: f32) {
    egui::Frame::new().fill(Color32::from_gray(22)).corner_radius(3.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        // Not the available width: that would keep the window as wide as it ever was.
        ui.set_width(width - 20.0);
        ui.horizontal(|ui| {
            let p = &mut app.ui.midi_keyboard;
            let low = u8::try_from(low_c(p).clamp(0, 127)).unwrap_or(60);
            let label = |ui: &mut Ui, name: &str| ui.label(egui::RichText::new(name).font(regular(13.0)).color(Color32::from_gray(200)));
            label(ui, "Octave:");
            ui.label(egui::RichText::new(soundcraft_midi::note_name(low)).font(bold(13.0)).color(Color32::WHITE));
            ui.add_space(14.0);
            label(ui, "Velocity:");
            let mut v = i32::from(p.velocity);
            if ui.add(egui::DragValue::new(&mut v).range(1..=127)).changed() {
                p.velocity = v.clamp(1, 127) as u8;
            }
            ui.add_space(14.0);
            label(ui, "Transpose:");
            let mut tr = i32::from(p.transpose);
            let s =
                ui.add(egui::Slider::new(&mut tr, -24..=24).show_value(true)).on_hover_text("Semitones added to the computer keys (double-click: 0)");
            if s.double_clicked() {
                tr = 0;
            }
            p.transpose = tr.clamp(-24, 24) as i8;
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_two_octaves_like_reaper() {
        assert_eq!(action(Key::Z), Some(Action::Note(0)));
        assert_eq!(action(Key::M), Some(Action::Note(11)));
        assert_eq!(action(Key::Q), Some(Action::Note(12)));
        assert_eq!(action(Key::I), Some(Action::Note(24)));
        assert_eq!(action(Key::Num2), Some(Action::Note(13)));
        assert_eq!(action(Key::A), None);
        assert_eq!(action(Key::Num4), None);
        assert_eq!(action(Key::Semicolon), Some(Action::OctaveDown));
        assert_eq!(action(Key::CloseBracket), Some(Action::VelocityUp));
        // Every sharp is on a black key, every other note on a white one.
        for (k, n) in NOTE_KEYS {
            let black = matches!(k, Key::S | Key::D | Key::G | Key::H | Key::J | Key::Num2 | Key::Num3 | Key::Num5 | Key::Num6 | Key::Num7);
            assert_eq!(is_black(n), black, "{k:?}");
        }
    }

    #[test]
    fn transpose_and_octave_move_the_pitch_within_midi() {
        let mut p = KeyboardPrefs::default();
        assert_eq!(key_pitch(&p, 0), Some(60));
        p.transpose = 24;
        assert_eq!(key_pitch(&p, 24), Some(108));
        p.octave = 8;
        assert_eq!(key_pitch(&p, 24), None);
        p.octave = -2;
        p.transpose = -24;
        assert_eq!(key_pitch(&p, 0), None);
        assert_eq!(view_start(&p), 0);
        p.octave = 8;
        p.transpose = 24;
        p.octaves = 4;
        assert!(view_start(&p) + 48 <= 127);
    }
}
