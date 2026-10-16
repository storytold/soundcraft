//! Batch track naming controls; all planning and mutation live in engine commands.

use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::TrackId;

#[derive(Debug, Clone)]
pub struct BatchRename {
    tracks: Vec<u64>,
    revision: u64,
    replace: bool,
    clear: bool,
    find: String,
    replacement: String,
    trim: bool,
    beginning: u64,
    end: u64,
    add: bool,
    prefix: String,
    suffix: String,
    numbering: bool,
    at_end: bool,
    start: u64,
    increment: u64,
    places: usize,
    separator: String,
    preview_params: Value,
    preview: Value,
    error: String,
}

impl BatchRename {
    pub fn new(e: &Engine) -> Option<Self> {
        let s = e.session();
        let tracks: Vec<_> = s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).take(4097).map(|t| t.id.0).collect();
        if tracks.is_empty() || tracks.len() > 4096 {
            return None;
        }
        Some(Self {
            tracks,
            revision: e.revision,
            replace: false,
            clear: false,
            find: String::new(),
            replacement: String::new(),
            trim: false,
            beginning: 0,
            end: 0,
            add: false,
            prefix: String::new(),
            suffix: String::new(),
            numbering: false,
            at_end: false,
            start: 1,
            increment: 1,
            places: 2,
            separator: "_".into(),
            preview_params: Value::Null,
            preview: Value::Null,
            error: String::new(),
        })
    }

    fn params(&self) -> Value {
        let mut p = json!({"tracks":self.tracks,"expected_revision":self.revision});
        if self.replace {
            p["replace"] = json!({"clear":self.clear,"find":self.find,"with":self.replacement});
        }
        if self.trim {
            p["trim"] = json!({"beginning":self.beginning,"end":self.end});
        }
        if self.add {
            p["add"] = json!({"prefix":self.prefix,"suffix":self.suffix});
        }
        if self.numbering {
            p["numbering"] = json!({"position":if self.at_end {"end"} else {"beginning"},"start":self.start,"increment":self.increment,"places":self.places,"separator":self.separator});
        }
        p
    }

    fn refresh(&mut self, e: &mut Engine) {
        if e.revision != self.revision {
            self.error = "Session changed. Reopen Batch Rename to review the current tracks.".into();
            self.preview = Value::Null;
            return;
        }
        let params = self.params();
        if params != self.preview_params {
            self.preview_params = params.clone();
            match e.execute("track.batch_rename_preview", &params) {
                Ok(value) => {
                    self.preview = value;
                    self.error.clear();
                }
                Err(err) => {
                    self.preview = Value::Null;
                    self.error = err.to_string();
                }
            }
        }
    }

    /// Returns Cancel and the captured Apply parameters, respectively.
    pub fn show(&mut self, ui: &mut egui::Ui, e: &mut Engine) -> (bool, Option<Value>) {
        ui.label(format!("{} selected tracks · numbered in session order", self.tracks.len()));
        ui.columns(2, |columns| {
            columns[0].group(|ui| {
                ui.checkbox(&mut self.replace, "1. Replace");
                ui.add_enabled_ui(self.replace, |ui| {
                    ui.checkbox(&mut self.clear, "Clear existing name");
                    ui.add_enabled_ui(!self.clear, |ui| {
                        text(ui, "Find", &mut self.find);
                        text(ui, "Replace with", &mut self.replacement);
                    });
                });
            });
            columns[0].group(|ui| {
                ui.checkbox(&mut self.trim, "2. Trim");
                ui.add_enabled_ui(self.trim, |ui| {
                    count(ui, "From beginning", &mut self.beginning, 1024);
                    count(ui, "From end", &mut self.end, 1024);
                    ui.small("Counts Unicode scalars, including spaces.");
                });
            });
            columns[1].group(|ui| {
                ui.checkbox(&mut self.add, "3. Add");
                ui.add_enabled_ui(self.add, |ui| {
                    text(ui, "Prefix", &mut self.prefix);
                    text(ui, "Suffix", &mut self.suffix);
                });
            });
            columns[1].group(|ui| {
                ui.checkbox(&mut self.numbering, "4. Numbering");
                ui.add_enabled_ui(self.numbering, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.at_end, false, "Beginning");
                        ui.selectable_value(&mut self.at_end, true, "End");
                    });
                    count(ui, "Start", &mut self.start, u64::MAX);
                    count(ui, "Increment", &mut self.increment, u64::MAX);
                    ui.horizontal(|ui| {
                        ui.label("Places");
                        ui.add(egui::DragValue::new(&mut self.places).range(1..=20));
                    });
                    text(ui, "Separator", &mut self.separator);
                });
            });
        });
        self.refresh(e);
        ui.separator();
        ui.strong("Name preview");
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(225, 95, 75), &self.error);
        }
        egui::ScrollArea::vertical().id_salt("batch_track_preview").max_height(180.0).show(ui, |ui| {
            egui::Grid::new("batch_track_names").striped(true).min_col_width(230.0).max_col_width(280.0).show(ui, |ui| {
                ui.strong("Current name");
                ui.strong("Proposed name");
                ui.end_row();
                if let Some(rows) = self.preview.get("rows").and_then(Value::as_array) {
                    for row in rows {
                        ui.label(row.get("old").and_then(Value::as_str).unwrap_or_default());
                        ui.vertical(|ui| {
                            ui.label(row.get("name").and_then(Value::as_str).unwrap_or_default());
                            if let Some(err) = row.get("error").and_then(Value::as_str) {
                                ui.colored_label(egui::Color32::from_rgb(225, 95, 75), err);
                            }
                        });
                        ui.end_row();
                    }
                }
            });
        });
        ui.small("Names must be unique and contain 1–64 Unicode scalars. Apply creates one Undo step.");
        let ready = self.error.is_empty() && self.preview["valid"] == true && self.preview["changed"].as_u64().unwrap_or_default() > 0;
        let mut cancel = false;
        let mut apply = None;
        ui.separator();
        ui.horizontal(|ui| {
            cancel = ui.button("Cancel").clicked();
            if ui.add_enabled(ready, egui::Button::new(egui::RichText::new("Apply").strong())).clicked() {
                apply = Some(self.params());
            }
        });
        (cancel, apply)
    }
}

fn text(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        let label = ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(150.0).char_limit(256)).labelled_by(label.id);
    });
}

fn count(ui: &mut egui::Ui, label: &str, value: &mut u64, max: u64) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).range(0..=max));
    });
}

/// Context actions retain a selected group; clicking outside the group explicitly selects one.
pub fn open_for_track(app: &mut SoundApp, id: TrackId) -> bool {
    if app.engine.session().track(id).is_none() {
        return false;
    }
    if !app.engine.session().edit.selected_tracks.contains(&id) && app.run("edit.select", json!({"tracks":[id.0],"exact":true})).is_err() {
        return false;
    }
    app.dialogs.open_for_command(&app.engine, "track.batch_rename")
}

pub fn context_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId) {
    if ui.button("Batch Rename Tracks…").clicked() {
        open_for_track(app, id);
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use crate::dialogs::Dialog;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    fn app() -> SoundApp {
        let mut s = Session::default();
        for name in ["Kick", "Snare", "Bass"] {
            s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        }
        s.edit.selected_tracks = s.tracks.iter().take(2).map(|t| t.id).collect();
        SoundApp::new(Engine::new(s), None, Services::default())
    }

    #[test]
    fn batch_rename_context_captures_selected_group_or_clicked_track() {
        let mut app = app();
        let ids: Vec<_> = app.engine.session().tracks.iter().map(|t| t.id).collect();
        assert!(open_for_track(&mut app, ids[1]));
        let Some(Dialog::BatchRenameTracks(d)) = &app.dialogs.open else { panic!("missing dialog") };
        assert_eq!(d.tracks, vec![ids[0].0, ids[1].0]);
        assert!(open_for_track(&mut app, ids[2]));
        let Some(Dialog::BatchRenameTracks(d)) = &app.dialogs.open else { panic!("missing dialog") };
        assert_eq!(d.tracks, vec![ids[2].0]);
    }

    #[test]
    fn batch_rename_preview_and_apply_use_captured_ids_and_one_undo() {
        let mut app = app();
        let mut d = BatchRename::new(&app.engine).unwrap();
        d.add = true;
        d.prefix = "Drums_".into();
        d.refresh(&mut app.engine);
        assert_eq!(d.preview["changed"], 2);
        assert!(!app.engine.is_dirty());
        assert!(app.engine.journal.is_empty());
        app.run("track.batch_rename", d.params()).unwrap();
        assert_eq!(app.engine.session().tracks[0].name, "Drums_Kick");
        assert_eq!(app.engine.session().tracks[2].name, "Bass");
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.engine.session().tracks[0].name, "Kick");
        assert!(app.engine.undo_label().is_none());
    }

    #[test]
    fn batch_rename_stale_preview_and_invalid_names_disable_apply() {
        let mut app = app();
        assert!(app.dialogs.open_for_command(&app.engine, "track.batch_rename"));
        let mut d = BatchRename::new(&app.engine).unwrap();
        d.replace = true;
        d.clear = true;
        d.add = true;
        d.prefix = "same".into();
        d.refresh(&mut app.engine);
        assert_eq!(d.preview["valid"], false);
        app.run("track.comments", json!({"track":"Kick","comments":"edited"})).unwrap();
        d.refresh(&mut app.engine);
        assert!(!d.error.is_empty());
        assert!(d.preview.is_null());
        assert!(app.run("track.batch_rename", d.params()).is_err());
        assert!(app.dialogs.batch_rename_open());
        assert_eq!(app.engine.session().tracks[0].name, "Kick");
    }

    fn harness(mut app: SoundApp) -> egui_kittest::Harness<'static, SoundApp> {
        assert!(app.dialogs.open_for_command(&app.engine, "track.batch_rename"));
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(4);
        h
    }

    fn frame(ctx: &egui::Context, input: egui::RawInput, draw: impl FnMut(&mut egui::Ui)) {
        let mut output = ctx.run_ui(input, draw);
        // These CPU-only input checks deliberately do not render textures.
        output.textures_delta.clear();
    }

    #[test]
    fn batch_rename_ui_preview_apply_and_invalid_button() {
        use egui_kittest::kittest::Queryable;
        let mut h = harness(app());
        h.get_by_label("3. Add").click();
        h.run_steps(3);
        h.get_by_label("Prefix").click();
        h.run_steps(2);
        h.get_by_label("Prefix").type_text("Drums_");
        h.run_steps(3);
        let Some(Dialog::BatchRenameTracks(d)) = &h.state().dialogs.open else { panic!("missing dialog") };
        assert_eq!(d.preview["changed"], 2);
        assert_eq!(d.preview["rows"][0]["name"], "Drums_Kick");
        h.get_by_label("Apply").click();
        h.run_steps(3);
        assert!(h.state().dialogs.open.is_none());
        assert_eq!(h.state().engine.session().tracks[0].name, "Drums_Kick");
        h.state_mut().run("edit.undo", json!({})).unwrap();
        assert_eq!(h.state().engine.session().tracks[0].name, "Kick");
        assert!(h.state().engine.undo_label().is_none());

        let mut h = harness(app());
        let Some(Dialog::BatchRenameTracks(d)) = &mut h.state_mut().dialogs.open else { panic!("missing dialog") };
        d.replace = true;
        d.clear = true;
        d.add = true;
        d.prefix = "same".into();
        h.run_steps(3);
        let before = h.state().engine.session_arc();
        h.get_by_label("Apply").click();
        h.run_steps(3);
        assert!(h.state().dialogs.batch_rename_open());
        assert!(std::sync::Arc::ptr_eq(&before, &h.state().engine.session_arc()));
        assert!(h.state().engine.undo_label().is_none());
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(h.state().dialogs.open.is_none());
    }

    #[test]
    fn batch_rename_session_lifecycle_clears_only_on_successful_replacement() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!("soundcraft-batch-rename-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let path = dir.join("session.scraft");
        let mut app = app();
        app.run("session.save_as", json!({"path":path})).unwrap();
        for (cmd, params) in [
            ("session.save", json!({})),
            ("session.save_as", json!({"path":path})),
            ("session.save_copy", json!({"path":dir.join("copy.scraft")})),
            ("session.open", json!({"path":dir.join("missing.scraft")})),
            ("session.new", json!({"sample_rate":0})),
        ] {
            assert!(app.dialogs.open_for_command(&app.engine, "track.batch_rename"));
            let result = app.run(cmd, params);
            assert_eq!(result.is_ok(), cmd.starts_with("session.save"), "{cmd}: {result:?}");
            assert!(app.dialogs.batch_rename_open(), "{cmd}");
        }
        app.engine.path = Some(dir.join("missing.scraft").to_string_lossy().into_owned());
        assert!(app.run("session.revert", json!({})).is_err());
        assert!(app.dialogs.batch_rename_open());
        for (cmd, params) in
            [("session.new", json!({})), ("session.open", json!({"path":path})), ("session.close", json!({})), ("session.revert", json!({}))]
        {
            app.run("session.open", json!({"path":path})).unwrap();
            assert!(app.dialogs.open_for_command(&app.engine, "track.batch_rename"));
            app.run(cmd, params).unwrap();
            assert!(!app.dialogs.batch_rename_open(), "{cmd}");
        }
        app.run("session.open", json!({"path":path})).unwrap();
        let id = app.engine.session().tracks[0].id;
        app.dialogs.open_rename_track(id, "Kick");
        app.run("session.new", json!({})).unwrap();
        assert_eq!(app.dialogs.open_name(), Some("rename_track"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn batch_rename_shortcut_opens_dialog_and_escape_cancels_without_edits() {
        let mut app = app();
        assert!(crate::menus::tree().iter().find(|n| n.label == "Track").unwrap().children.iter().any(|n| n.path == "Track > Batch Rename Tracks"));
        assert_eq!(crate::menus::command_for_path("Track > Batch Rename Tracks", &[]), Some("track.batch_rename".into()));
        assert!(crate::menus::invoke_shortcut_dialog(&mut app, "track.batch_rename"));
        let before = app.engine.session_arc();
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        frame(&ctx, egui::RawInput::default(), |_| {});
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            ..Default::default()
        };
        frame(&ctx, input, |ui| crate::dialogs::show(&mut app, ui.ctx()));
        assert!(app.dialogs.open.is_none());
        assert!(std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()));
        assert!(!app.engine.is_dirty());
        assert!(app.engine.undo_label().is_none());
    }

    #[test]
    fn batch_rename_dialog_blocks_midi_backspace_without_text_focus() {
        let mut app = app();
        app.run("track.new", json!({"kind":"midi","name":"Keys"})).unwrap();
        let clip = app.run("midi.new_clip", json!({"track":"Keys"})).unwrap()["clip"].as_u64().unwrap();
        app.run("midi.note_add", json!({"clip":clip,"pitch":60,"start_ticks":0})).unwrap();
        app.midi.selected = vec![0];
        assert!(app.dialogs.open_for_command(&app.engine, "track.batch_rename"));
        let before = app.engine.session_arc();
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        frame(&ctx, egui::RawInput::default(), |_| {});
        for key in [egui::Key::Backspace, egui::Key::Delete, egui::Key::ArrowUp] {
            let input = egui::RawInput {
                events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
                ..Default::default()
            };
            frame(&ctx, input, |ui| crate::midi_editor::show(&mut app, ui));
        }
        assert!(std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()));
        assert_eq!(app.midi.selected, vec![0]);
    }
}
