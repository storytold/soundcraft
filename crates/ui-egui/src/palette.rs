//! Search: a command palette over every engine and UI command (Window › Search, Cmd+Ctrl+S).
use crate::SoundApp;
use crate::i18n::tr;
use crate::theme::{Tokens, mono};
use egui::{Align2, Key, vec2};

#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}

struct Entry {
    id: String,
    label: String,
    path: String,
    shortcut: String,
    /// Label and menu path in the interface language (search matches both languages).
    shown_label: String,
    shown_path: String,
}

impl Entry {
    fn new(id: &str, label: &str, path: String, shortcut: Option<&str>) -> Self {
        let shown_path = path.split(" › ").map(tr).collect::<Vec<_>>().join(" › ");
        Entry { id: id.into(), label: label.into(), shown_label: tr(label).into(), path, shown_path, shortcut: shortcut.unwrap_or("").into() }
    }
}

fn entries() -> Vec<Entry> {
    let mut v: Vec<Entry> = soundcraft_engine::command_specs().iter().map(|c| Entry::new(c.id, c.label, c.menu.join(" › "), c.shortcut)).collect();
    for (id, label, path, sc) in crate::menus::UI_COMMANDS {
        v.push(Entry::new(id, label, path.replace(" > ", " › "), *sc));
    }
    v
}

/// Subsequence match score (higher is better), or None.
fn score(hay: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    let n = needle.to_lowercase();
    if let Some(i) = h.find(&n) {
        return Some(1000 - i32::try_from(i).unwrap_or(999));
    }
    let mut it = h.chars();
    let mut gaps = 0;
    for c in n.chars() {
        let mut found = false;
        for hc in it.by_ref() {
            if hc == c {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(500 - gaps)
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_search {
        return;
    }
    let t = Tokens::current();
    let mut run: Option<String> = None;
    let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
    egui::Window::new(tr("Search"))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 90.0))
        .fixed_size(vec2(560.0, 380.0))
        .show(ctx, |ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut app.palette.query).hint_text(tr("Search commands…")).desired_width(f32::INFINITY).font(mono(15.0)),
            );
            r.request_focus();
            let q = app.palette.query.trim().to_string();
            let mut hits: Vec<(i32, Entry)> = entries()
                .into_iter()
                .filter_map(|e| score(&format!("{} {} {} {} {}", e.shown_label, e.label, e.shown_path, e.path, e.id), &q).map(|s| (s, e)))
                .collect();
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.shown_label.cmp(&b.1.shown_label)));
            hits.truncate(14);
            let (down, up, enter) = ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Enter)));
            if down {
                app.palette.selected = (app.palette.selected + 1).min(hits.len().saturating_sub(1));
            }
            if up {
                app.palette.selected = app.palette.selected.saturating_sub(1);
            }
            app.palette.selected = app.palette.selected.min(hits.len().saturating_sub(1));
            ui.separator();
            for (i, (_, e)) in hits.iter().enumerate() {
                let sel = i == app.palette.selected;
                let resp = ui.horizontal(|ui| {
                    let l = ui.selectable_label(sel, &e.shown_label);
                    ui.label(egui::RichText::new(&e.shown_path).small().color(t.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let sc = crate::shortcuts::shortcut_label(&e.shortcut, ctx.os().is_mac());
                        ui.label(egui::RichText::new(sc).font(mono(10.0)).color(t.text_dim));
                    });
                    l
                });
                if resp.inner.clicked() {
                    run = Some(e.id.clone());
                }
            }
            if enter && let Some((_, e)) = hits.get(app.palette.selected) {
                run = Some(e.id.clone());
            }
        });
    if let Some(id) = run {
        close = true;
        let path = entries().into_iter().find(|e| e.id == id).map(|e| e.path.replace(" › ", " > ")).unwrap_or_default();
        crate::menus::invoke_menu(app, &id, &path);
    }
    if close {
        app.ui.show_search = false;
        app.palette = PaletteState::default();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scoring_prefers_substrings() {
        assert!(super::score("Duplicate", "dup") > super::score("Duplicate", "dpl"));
        assert!(super::score("Bounce Mix", "xyz").is_none());
        assert_eq!(super::score("anything", ""), Some(0));
    }
}
