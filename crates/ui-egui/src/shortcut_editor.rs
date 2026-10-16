//! Setup › Keyboard Shortcuts: every command people can give keys to, listed by menu.

use crate::SoundApp;
use crate::shortcuts::{Bindable, CATEGORIES, assign, bindable, chord_text, clashes, shortcut_for, shortcut_label};
use crate::theme::{Tokens, mono};
use egui::{Align, Align2, FontId, Key, Layout, Modifiers, Rect, RichText, Sense, Stroke, UiBuilder, vec2};

const ROW: f32 = 22.0;

#[derive(Debug, Default)]
pub struct ShortcutEditor {
    pub query: String,
    /// The category on show; None shows them all.
    pub category: Option<&'static str>,
    pub changed_only: bool,
    /// The command waiting for its new keys.
    pub capturing: Option<&'static str>,
    /// New keys that other commands have, waiting for Replace: (command, keys).
    pub clash: Option<(&'static str, String)>,
    confirm_reset: bool,
}

/// A key pressed while `capturing`; `shortcuts::handle` passes it on before any widget sees it.
pub fn captured(app: &mut SoundApp, key: Key, mods: Modifiers, mac: bool) {
    if let Some(id) = app.shortcut_editor.capturing.take()
        && key != Key::Escape
    {
        propose(app, id, chord_text(key, mods, mac));
    }
}

/// Gives `id` the keys `chord`, asking first when other commands have them.
fn propose(app: &mut SoundApp, id: &'static str, chord: String) {
    if clashes(&app.ui.shortcuts, &chord, id).is_empty() {
        assign(&mut app.ui.shortcuts, id, &chord);
    } else {
        app.shortcut_editor.clash = Some((id, chord));
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_shortcuts;
    if open {
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .default_size(vec2(800.0, 560.0))
            .min_size(vec2(620.0, 320.0))
            .show(ctx, |ui| contents(app, ui));
    }
    if !open {
        app.shortcut_editor.capturing = None;
        app.shortcut_editor.clash = None;
    }
    app.ui.show_shortcuts = open;
}

fn contents(app: &mut SoundApp, ui: &mut egui::Ui) {
    let mac = ui.ctx().os().is_mac();
    egui::Panel::top("shortcuts_toolbar").frame(egui::Frame::NONE).show(ui, |ui| toolbar(app, ui));
    egui::Panel::bottom("shortcuts_footer").frame(egui::Frame::NONE).show(ui, |ui| footer(app, ui, mac));
    let query = app.shortcut_editor.query.trim().to_lowercase();
    let found: Vec<&'static Bindable> = bindable().iter().filter(|b| listed(app, b, &query, mac)).collect();
    let category = if query.is_empty() { app.shortcut_editor.category } else { None };
    egui::Panel::left("shortcuts_categories")
        .frame(egui::Frame::NONE)
        .resizable(false)
        .exact_size(150.0)
        .show(ui, |ui| categories(app, ui, &found, category));
    let rows: Vec<&'static Bindable> = found.into_iter().filter(|b| category.is_none_or(|c| c == b.category)).collect();
    egui::CentralPanel::no_frame().show(ui, |ui| table(app, ui, &rows, mac));
}

/// Whether `b` matches the search (its name, menu or keys) and the Changed only box.
fn listed(app: &SoundApp, b: &Bindable, query: &str, mac: bool) -> bool {
    let keys = || shortcut_for(&app.ui.shortcuts, b.id).map(|s| shortcut_label(s, mac)).unwrap_or_default();
    (!app.shortcut_editor.changed_only || app.ui.shortcuts.contains_key(b.id))
        && (query.is_empty() || [b.label, &b.menu, &keys()].iter().any(|s| s.to_lowercase().contains(query)))
}

fn toolbar(app: &mut SoundApp, ui: &mut egui::Ui) {
    let ed = &mut app.shortcut_editor;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut ed.query).hint_text("Search commands or keys").desired_width(240.0));
        ui.checkbox(&mut ed.changed_only, "Changed only");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !ed.confirm_reset {
                ed.confirm_reset = ui.add_enabled(!app.ui.shortcuts.is_empty(), egui::Button::new("Reset All…")).clicked();
            } else if ui.button("Cancel").clicked() {
                ed.confirm_reset = false;
            } else if ui.button("Reset All").clicked() {
                app.ui.shortcuts.clear();
                ed.confirm_reset = false;
            } else {
                ui.label("Go back to every built-in shortcut?");
            }
        });
    });
    ui.add_space(4.0);
}

fn footer(app: &mut SoundApp, ui: &mut egui::Ui, mac: bool) {
    let name = |id: &str| bindable().iter().find(|b| b.id == id).map_or(id.to_string(), |b| format!("{} ({})", b.label, b.menu));
    let ed = &app.shortcut_editor;
    let prompt = match (&ed.clash, ed.capturing) {
        (Some((id, chord)), _) => {
            let others = clashes(&app.ui.shortcuts, chord, id).into_iter().map(name).collect::<Vec<_>>().join(", ");
            Some((format!("{} is already the shortcut for {others}.", shortcut_label(chord, mac)), "Replace", *id, chord.clone()))
        }
        (None, Some(id)) => Some((format!("Press the new shortcut for {}. Esc cancels.", name(id)), "No Shortcut", id, String::new())),
        (None, None) => None,
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let Some((text, action, id, chord)) = prompt else {
            let hint = "Click a shortcut, then press the new keys. Your shortcuts take over built-in keys such as F1–F10.";
            ui.label(RichText::new(hint).color(Tokens::current().text_dim));
            return;
        };
        ui.label(text);
        let act = ui.button(action).clicked();
        if act {
            assign(&mut app.ui.shortcuts, id, &chord);
        }
        if act || ui.button("Cancel").clicked() {
            app.shortcut_editor.clash = None;
            app.shortcut_editor.capturing = None;
        }
    });
    ui.add_space(2.0);
}

fn categories(app: &mut SoundApp, ui: &mut egui::Ui, found: &[&'static Bindable], shown: Option<&'static str>) {
    let width = ui.available_width() - 10.0;
    egui::ScrollArea::vertical().id_salt("shortcut_categories").auto_shrink(false).show(ui, |ui| {
        for c in std::iter::once(None).chain(CATEGORIES.map(Some)) {
            let n = found.iter().filter(|b| c.is_none_or(|c| c == b.category)).count();
            let b = egui::Button::selectable(shown == c, c.unwrap_or("All")).shortcut_text(n.to_string()).min_size(vec2(width, 20.0));
            if ui.add(b).clicked() {
                app.shortcut_editor.category = c;
                app.shortcut_editor.query.clear();
            }
        }
    });
}

/// Command, menu and shortcut columns of a row, clear of the scroll bar.
fn columns(r: Rect) -> [Rect; 3] {
    let keys = r.right() - 224.0;
    let menu = keys - 190.0;
    [r.with_min_x(r.left() + 6.0).with_max_x(menu - 8.0), r.with_min_x(menu).with_max_x(keys - 8.0), r.with_min_x(keys).with_max_x(r.right() - 14.0)]
}

fn table(app: &mut SoundApp, ui: &mut egui::Ui, rows: &[&'static Bindable], mac: bool) {
    let t = Tokens::current();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    for (r, title) in columns(rect).into_iter().zip(["Command", "Menu", "Shortcut"]) {
        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, title, FontId::proportional(11.0), t.text_dim);
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, t.border));
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical().id_salt("shortcut_rows").auto_shrink(false).show_rows(ui, ROW, rows.len(), |ui, range| {
        for i in range {
            if let Some(b) = rows.get(i) {
                ui.push_id(b.id, |ui| row(app, ui, b, i % 2 == 0, mac));
            }
        }
    });
}

fn row(app: &mut SoundApp, ui: &mut egui::Ui, b: &'static Bindable, stripe: bool, mac: bool) {
    let t = Tokens::current();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    if stripe {
        ui.painter().rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    }
    let [name, menu, keys] = columns(rect);
    let mut cell = |r: Rect| ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::left_to_right(Align::Center)));
    cell(name).add(egui::Label::new(b.label).truncate());
    cell(menu).add(egui::Label::new(RichText::new(&b.menu).small().color(t.text_dim)).truncate());
    let mut ui = cell(keys);
    let capturing = app.shortcut_editor.capturing == Some(b.id);
    let custom = app.ui.shortcuts.contains_key(b.id);
    let pending = app.shortcut_editor.clash.as_ref().filter(|c| c.0 == b.id).map(|c| c.1.as_str());
    let (text, color) = match pending.or(shortcut_for(&app.ui.shortcuts, b.id)).map(|s| shortcut_label(s, mac)).filter(|s| !s.is_empty()) {
        _ if capturing => ("Press keys…".into(), ui.visuals().selection.stroke.color),
        Some(keys) if pending.is_some() => (keys, ui.visuals().warn_fg_color),
        Some(keys) => (keys, if custom { t.accent } else { t.text }),
        None => ("—".into(), if custom { t.accent } else { t.text_dim }),
    };
    let chip = egui::Button::new(RichText::new(text).font(mono(11.5)).color(color)).min_size(vec2(150.0, ROW - 4.0)).selected(capturing);
    if ui.add_enabled(b.editable, chip).on_disabled_hover_text("A built-in key; it can't be changed here.").clicked() {
        app.shortcut_editor.capturing = if capturing { None } else { Some(b.id) };
        app.shortcut_editor.clash = None;
    }
    let back = || b.default.map(|d| shortcut_label(d, mac)).filter(|d| !d.is_empty()).unwrap_or_else(|| "no shortcut".into());
    if custom && ui.small_button("Reset").on_hover_text(format!("Back to {}", back())).clicked() {
        propose(app, b.id, b.default.unwrap_or("").to_string());
    }
}
