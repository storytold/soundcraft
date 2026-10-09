//! Modal dialogs opened by menu-style invocations.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ClipId, TrackId};
use soundcraft_time::Samples;

#[derive(Debug, Clone)]
pub enum Dialog {
    NewTracks { count: u32, format: String, kind: String, timebase: String, name: String },
    NewSession { name: String, sample_rate: u32, demo: bool },
    Bounce { path: String, format: String, bit_depth: String, normalize: bool },
    RenameTrack { id: TrackId, name: String },
    RenameClip { id: ClipId, name: String },
    PathPrompt { cmd: String, title: String, path: String, key: String },
    Number { cmd: String, title: String, key: String, value: f64, suffix: String },
    TempoChange { at: Samples, bpm: f64 }, // from double-clicking the tempo ruler
    Fades { shape: String },
    StripSilence { threshold: f64, min_ms: f64, pre_ms: f64, post_ms: f64 },
    Group { name: String, edit: bool, mix: bool, members: Vec<(u64, String, bool)> },
    Session { frame_rate: String, bit_depth: String },
    ScoreSetup { title: String, composer: String, bars_per_system: u32, show_track_names: bool },
}

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<Dialog>,
}

impl Dialogs {
    pub fn open_name(&self) -> Option<&'static str> {
        self.open.as_ref().map(|d| match d {
            Dialog::NewTracks { .. } => "new_tracks",
            Dialog::NewSession { .. } => "new_session",
            Dialog::Bounce { .. } => "bounce",
            Dialog::RenameTrack { .. } => "rename_track",
            Dialog::RenameClip { .. } => "rename_clip",
            Dialog::PathPrompt { .. } => "path",
            Dialog::Number { .. } => "number",
            Dialog::TempoChange { .. } => "tempo_change",
            Dialog::Fades { .. } => "fades",
            Dialog::StripSilence { .. } => "strip_silence",
            Dialog::Group { .. } => "group",
            Dialog::Session { .. } => "session",
            Dialog::ScoreSetup { .. } => "score_setup",
        })
    }

    pub fn open_rename_track(&mut self, id: TrackId, name: &str) {
        self.open = Some(Dialog::RenameTrack { id, name: name.to_string() });
    }

    pub fn open_rename_clip(&mut self, id: ClipId, name: &str) {
        self.open = Some(Dialog::RenameClip { id, name: name.to_string() });
    }

    /// Open the dialog for a command; false when the command has no dialog.
    pub fn open_for_command(&mut self, e: &Engine, id: &str) -> bool {
        let home = default_dir();
        let name = e.session().name.clone();
        self.open = Some(match id {
            "track.new" => {
                Dialog::NewTracks { count: 1, format: "Mono".into(), kind: "audio".into(), timebase: "samples".into(), name: "Audio".into() }
            }
            "session.new" => Dialog::NewSession { name: "Untitled".into(), sample_rate: 48_000, demo: false },
            "file.bounce_mix" => {
                Dialog::Bounce { path: format!("{home}/{name} Bounce.wav"), format: "wav".into(), bit_depth: "24".into(), normalize: false }
            }
            "file.import_audio" => Dialog::PathPrompt { cmd: id.into(), title: "Import Audio".into(), path: home, key: "path".into() },
            "file.import_midi" => Dialog::PathPrompt { cmd: id.into(), title: "Import MIDI".into(), path: home, key: "path".into() },
            "session.open" => Dialog::PathPrompt { cmd: id.into(), title: "Open Session".into(), path: home, key: "path".into() },
            "session.save_as" | "session.save_copy" => Dialog::PathPrompt {
                cmd: id.into(),
                title: "Save Session As".into(),
                path: format!("{home}/{name}/{name}.scraft"),
                key: "path".into(),
            },
            "edit.repeat" => Dialog::Number { cmd: id.into(), title: "Repeat".into(), key: "count".into(), value: 2.0, suffix: "times".into() },
            "edit.shift" => {
                Dialog::Number { cmd: id.into(), title: "Shift (seconds)".into(), key: "by_seconds".into(), value: 1.0, suffix: "s".into() }
            }
            "event.transpose" => {
                Dialog::Number { cmd: id.into(), title: "Transpose".into(), key: "semitones".into(), value: 12.0, suffix: "semitones".into() }
            }
            "event.quantize" => {
                Dialog::Number { cmd: id.into(), title: "Quantize strength".into(), key: "strength".into(), value: 100.0, suffix: "%".into() }
            }
            "edit.fades_create" => Dialog::Fades { shape: "equal power".into() },
            "edit.strip_silence" => Dialog::StripSilence { threshold: -48.0, min_ms: 50.0, pre_ms: 5.0, post_ms: 20.0 },
            "track.group" => {
                let s = e.session();
                let members = s.tracks.iter().map(|t| (t.id.0, t.name.clone(), s.edit.selected_tracks.contains(&t.id))).collect();
                Dialog::Group { name: format!("Group {}", s.groups.len() + 1), edit: true, mix: true, members }
            }
            "setup.session" => Dialog::Session { frame_rate: e.session().frame_rate.label().into(), bit_depth: "24".into() },
            "file.score_setup" => {
                let s = soundcraft_engine::score::ScoreSetup::from_session(e.session());
                Dialog::ScoreSetup { title: s.title, composer: s.composer, bars_per_system: s.bars_per_system, show_track_names: s.show_track_names }
            }
            _ => match path_dialog(id, &home, &name) {
                Some(d) => d,
                None => return false,
            },
        });
        true
    }
}

/// A path prompt for any command whose parameters start with a required `path` or `dir` (plus
/// Print Score, whose path is optional), with a default file name from the command.
pub fn takes_path(id: &str) -> bool {
    path_key(id).is_some()
}

fn path_key(id: &str) -> Option<&'static str> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    if spec.params.starts_with("{path?") && id != "file.print_score" {
        None
    } else if spec.params.starts_with("{path") {
        Some("path")
    } else if spec.params.starts_with("{dir") {
        Some("dir")
    } else {
        None
    }
}

fn path_dialog(id: &str, home: &str, name: &str) -> Option<Dialog> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    let key = path_key(id)?;
    let importing = id.contains("import") || id.contains("open");
    let ext = match id {
        "file.export_midi" => ".mid",
        "file.export_sibelius" => ".musicxml",
        "file.print_score" => ".svg",
        "file.export_clip_groups" => ".scgrp",
        "file.export_session_text" => ".txt",
        _ if id.contains("session") => ".scraft",
        _ => "",
    };
    let path = if importing || key == "dir" { format!("{home}/") } else { format!("{home}/{name}{ext}") };
    let title = format!("{} — {}", spec.menu.last().copied().unwrap_or("File"), spec.label.trim_end_matches("..."));
    Some(Dialog::PathPrompt { cmd: id.into(), title, path, key: key.into() })
}

fn default_dir() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into())
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}

/// Show the open dialog, if any.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audiosuite_window(app, ctx);
    let Some(mut d) = app.dialogs.open.take() else { return };
    let mut keep = true;
    let mut action: Option<(String, Value)> = None;
    let title = match &d {
        Dialog::NewTracks { .. } => "New Tracks",
        Dialog::NewSession { .. } => "New Session",
        Dialog::Bounce { .. } => "Bounce Mix",
        Dialog::RenameTrack { .. } => "Rename Track",
        Dialog::RenameClip { .. } => "Rename Clip",
        Dialog::PathPrompt { title, .. } | Dialog::Number { title, .. } => title.as_str(),
        Dialog::TempoChange { .. } => "Tempo Change (BPM)",
        Dialog::Fades { .. } => "Fades",
        Dialog::StripSilence { .. } => "Strip Silence",
        Dialog::Group { .. } => "Create Group",
        Dialog::Session { .. } => "Session Setup",
        Dialog::ScoreSetup { .. } => "Score Setup",
    }
    .to_string();
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Window::new(egui::RichText::new(&title).font(bold(13.0)))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            match &mut d {
                Dialog::NewTracks { count, format, kind, timebase, name } => {
                    ui.horizontal(|ui| {
                        ui.label("Create");
                        ui.add(egui::DragValue::new(count).range(1..=128));
                        ui.label("new");
                        egui::ComboBox::from_id_salt("nt_fmt").selected_text(format.as_str()).show_ui(ui, |ui| {
                            for f in soundcraft_model::ChannelFormat::ALL {
                                ui.selectable_value(format, f.label().to_string(), f.label());
                            }
                        });
                        egui::ComboBox::from_id_salt("nt_kind")
                            .selected_text(soundcraft_model::TrackKind::from_id(kind).map_or("Audio Track", |k| k.label()))
                            .show_ui(ui, |ui| {
                                for k in soundcraft_model::TrackKind::ALL {
                                    if ui.selectable_label(kind == k.id(), k.label()).clicked() {
                                        *kind = k.id().to_string();
                                        *name = k.default_name().to_string();
                                    }
                                }
                            });
                        ui.label("in");
                        egui::ComboBox::from_id_salt("nt_tb").selected_text(if timebase == "ticks" { "Ticks" } else { "Samples" }).show_ui(
                            ui,
                            |ui| {
                                ui.selectable_value(timebase, "samples".to_string(), "Samples");
                                ui.selectable_value(timebase, "ticks".to_string(), "Ticks");
                            },
                        );
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                    });
                    if buttons(ui, "Create", enter) {
                        action =
                            Some(("track.new".into(), json!({"count": count, "format": format, "kind": kind, "timebase": timebase, "name": name})));
                    }
                }
                Dialog::NewSession { name, sample_rate, demo } => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                    });
                    egui::ComboBox::from_label("Sample Rate").selected_text(format!("{sample_rate} Hz")).show_ui(ui, |ui| {
                        for r in soundcraft_time::SampleRate::COMMON {
                            ui.selectable_value(sample_rate, r, format!("{r} Hz"));
                        }
                    });
                    ui.checkbox(demo, "Start from the demo session");
                    if buttons(ui, "Create", enter) {
                        action = Some((
                            "session.new".into(),
                            json!({"name": name, "sample_rate": sample_rate, "template": if *demo { "demo" } else { "blank" }}),
                        ));
                    }
                }
                Dialog::Bounce { path, format, bit_depth, normalize } => {
                    ui.horizontal(|ui| {
                        ui.label("File");
                        ui.add(egui::TextEdit::singleline(path).desired_width(360.0));
                    });
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("Format").selected_text(format.to_uppercase()).show_ui(ui, |ui| {
                            for f in ["wav", "aiff", "flac"] {
                                ui.selectable_value(format, f.to_string(), f.to_uppercase());
                            }
                        });
                        egui::ComboBox::from_label("Bit Depth").selected_text(format!("{bit_depth}-bit")).show_ui(ui, |ui| {
                            for b in ["16", "24", "32f"] {
                                ui.selectable_value(bit_depth, b.to_string(), format!("{b}-bit"));
                            }
                        });
                    });
                    ui.checkbox(normalize, "Normalize to -0.1 dBFS");
                    ui.label(
                        egui::RichText::new("Bounces the edit selection, or the whole session when nothing is selected.")
                            .small()
                            .color(Tokens::current().text_dim),
                    );
                    if buttons(ui, "Bounce", enter) {
                        let p = std::path::Path::new(path.as_str()).with_extension(if format == "aiff" { "aif" } else { format.as_str() });
                        action = Some((
                            "file.bounce_mix".into(),
                            json!({"path": p.to_string_lossy(), "format": format, "bit_depth": bit_depth, "normalize": normalize}),
                        ));
                    }
                }
                Dialog::RenameTrack { id, name } => {
                    let r = ui.text_edit_singleline(name);
                    r.request_focus();
                    if buttons(ui, "OK", enter) {
                        action = Some(("track.rename".into(), json!({"track": id.0, "name": name})));
                    }
                }
                Dialog::RenameClip { id, name } => {
                    let r = ui.text_edit_singleline(name);
                    r.request_focus();
                    if buttons(ui, "OK", enter) {
                        action = Some(("clip.rename".into(), json!({"clip": id.0, "name": name})));
                    }
                }
                Dialog::PathPrompt { cmd, path, key, .. } => {
                    ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                    if let Some(pick) = &app.services.pick_open
                        && cmd != "session.save_as"
                        && cmd != "session.save_copy"
                        && key != "dir"
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(cmd, &[])
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_folder
                        && key == "dir"
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(path)
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_save
                        && (cmd == "session.save_as" || cmd == "session.save_copy")
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(cmd, path)
                    {
                        *path = p;
                    }
                    if buttons(ui, "OK", enter) {
                        let mut v = json!({});
                        v[key.as_str()] = json!(path);
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::Number { cmd, key, value, suffix, .. } => {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(value).speed(0.1));
                        ui.label(suffix.as_str());
                    });
                    if buttons(ui, "OK", enter) {
                        let v = if key == "by_seconds" { json!({"by": {"seconds": value}}) } else { json!({key.as_str(): value}) };
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::TempoChange { at, bpm } => {
                    ui.horizontal(|ui| {
                        // Same limits the engine enforces (time crate `valid_bpm`).
                        ui.add(egui::DragValue::new(bpm).speed(0.1).range(5.0..=1000.0));
                        ui.label("bpm");
                    });
                    if buttons(ui, "OK", enter) {
                        action = Some(tempo_change_action(*at, *bpm));
                    }
                }
                Dialog::Fades { shape } => {
                    for s in soundcraft_model::FadeShape::ALL {
                        ui.radio_value(shape, s.label().to_lowercase(), s.label());
                    }
                    if buttons(ui, "OK", enter) {
                        action = Some(("edit.fades_create".into(), json!({"shape": shape})));
                    }
                }
                Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms } => {
                    ui.add(egui::Slider::new(threshold, -96.0..=0.0).text("Threshold dB"));
                    ui.add(egui::Slider::new(min_ms, 0.0..=2000.0).text("Min strip duration ms"));
                    ui.add(egui::Slider::new(pre_ms, 0.0..=500.0).text("Clip start pad ms"));
                    ui.add(egui::Slider::new(post_ms, 0.0..=2000.0).text("Clip end pad ms"));
                    if buttons(ui, "Strip", enter) {
                        action = Some((
                            "edit.strip_silence".into(),
                            json!({"threshold_db": threshold, "min_length_ms": min_ms, "pad_before_ms": pre_ms, "pad_after_ms": post_ms}),
                        ));
                    }
                }
                Dialog::Group { name, edit, mix, members } => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(edit, "Edit group");
                        ui.checkbox(mix, "Mix group");
                    });
                    ui.label("Tracks");
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (_, n, on) in members.iter_mut() {
                            ui.checkbox(on, n.as_str());
                        }
                    });
                    if buttons(ui, "OK", enter) {
                        let ids: Vec<u64> = members.iter().filter(|m| m.2).map(|m| m.0).collect();
                        action = Some(("track.group".into(), json!({"name": name, "edit": edit, "mix": mix, "tracks": ids})));
                    }
                }
                Dialog::Session { frame_rate, bit_depth } => {
                    egui::ComboBox::from_label("Timecode Rate").selected_text(frame_rate.as_str()).show_ui(ui, |ui| {
                        for r in soundcraft_time::FrameRate::ALL {
                            ui.selectable_value(frame_rate, r.label().to_string(), r.label());
                        }
                    });
                    egui::ComboBox::from_label("Bit Depth").selected_text(bit_depth.as_str()).show_ui(ui, |ui| {
                        for b in ["16", "24", "32f"] {
                            ui.selectable_value(bit_depth, b.to_string(), b);
                        }
                    });
                    if buttons(ui, "OK", enter) {
                        action = Some(("setup.session".into(), json!({"frame_rate": frame_rate, "bit_depth": bit_depth})));
                    }
                }
                Dialog::ScoreSetup { title, composer, bars_per_system, show_track_names } => {
                    egui::Grid::new("score_setup").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                        ui.label("Title");
                        ui.text_edit_singleline(title);
                        ui.end_row();
                        ui.label("Composer");
                        ui.text_edit_singleline(composer);
                        ui.end_row();
                        ui.label("Bars per system");
                        ui.add(egui::DragValue::new(bars_per_system).range(1..=16));
                        ui.end_row();
                    });
                    ui.checkbox(show_track_names, "Show track names");
                    if buttons(ui, "OK", enter) {
                        action = Some((
                            "file.score_setup".into(),
                            json!({"title": title, "composer": composer, "bars_per_system": bars_per_system, "show_track_names": show_track_names}),
                        ));
                    }
                }
            }
        });
    if esc || ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("dlg_cancel")).unwrap_or(false)) {
        keep = false;
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("dlg_cancel")));
    }
    if let Some((cmd, params)) = action {
        keep = false;
        match app.run(&cmd, params) {
            Ok(v) if cmd == "file.export_clips" => {
                app.ui.status = format!("Exported {} clip(s) to {}", v["written"], v["dir"].as_str().unwrap_or_default());
            }
            Ok(_) => {}
            Err(e) => app.ui.status = e,
        }
    }
    if keep {
        app.dialogs.open = Some(d);
    }
}

/// Cancel/OK row; returns true when OK was pressed (or Enter).
fn buttons(ui: &mut egui::Ui, ok: &str, enter: bool) -> bool {
    let mut pressed = enter;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("dlg_cancel"), true));
        }
        if ui.add(egui::Button::new(egui::RichText::new(ok).strong()).fill(Tokens::current().accent)).clicked() {
            pressed = true;
        }
    });
    pressed
}

/// The command the tempo-change dialog runs on OK.
fn tempo_change_action(at: Samples, bpm: f64) -> (String, Value) {
    ("event.tempo".into(), json!({"bpm": bpm, "at": at}))
}

fn audiosuite_window(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(process) = app.ui.audiosuite.clone() else { return };
    let mut open = true;
    let title = soundcraft_dsp::plugin_info(&process).map_or_else(|| process.replace('_', " "), |p| p.name.to_string());
    egui::Window::new(format!("AudioSuite · {title}")).open(&mut open).default_width(360.0).show(ctx, |ui| {
        let key = egui::Id::new(("as_params", process.clone()));
        let mut params: std::collections::BTreeMap<String, f32> = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_default();
        if let Some(info) = soundcraft_dsp::plugin_info(&process) {
            for p in info.params {
                let v = params.entry(p.id.to_string()).or_insert(p.default);
                ui.add(egui::Slider::new(v, p.min..=p.max).text(p.name));
            }
        } else {
            match process.as_str() {
                "normalize" => {
                    let v = params.entry("target_db".into()).or_insert(-0.1);
                    ui.add(egui::Slider::new(v, -24.0..=0.0).text("Peak dB"));
                }
                "time_stretch" => {
                    let v = params.entry("ratio".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text("Length ratio"));
                }
                "pitch_shift" => {
                    let v = params.entry("semitones".into()).or_insert(0.0);
                    ui.add(egui::Slider::new(v, -24.0..=24.0).text("Semitones"));
                }
                "varispeed" => {
                    let v = params.entry("speed".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text("Speed"));
                }
                _ => {
                    ui.label("No parameters.");
                }
            }
        }
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, params.clone()));
        ui.label(egui::RichText::new("Processes the selected clips and replaces them with rendered audio.").small());
        if ui.button(egui::RichText::new("Render").strong()).clicked() {
            let p: serde_json::Map<String, Value> = params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            let _ = app.run("audiosuite.process", json!({"process": process, "params": p}));
        }
    });
    if !open {
        app.ui.audiosuite = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_commands_get_path_prompts_with_sensible_names() {
        let e = soundcraft_engine::demo::demo_engine();
        let mut d = Dialogs::default();
        for (id, ext) in [("file.export_sibelius", ".musicxml"), ("file.print_score", ".svg"), ("file.export_clip_groups", ".scgrp")] {
            assert!(takes_path(id), "{id}");
            assert!(d.open_for_command(&e, id), "{id}");
            match d.open.take() {
                Some(Dialog::PathPrompt { cmd, path, key, .. }) => {
                    assert_eq!(cmd, id);
                    assert_eq!(key, "path");
                    assert!(path.ends_with(ext), "{path}");
                }
                other => panic!("{id}: {other:?}"),
            }
        }
        // Optional-path commands run directly.
        assert!(!takes_path("file.send_to_sibelius"));
        assert!(!takes_path("edit.copy"));
        assert!(d.open_for_command(&e, "file.score_setup"));
        assert!(matches!(d.open, Some(Dialog::ScoreSetup { bars_per_system: 4, .. })));
    }

    #[test]
    fn tempo_change_dialog_runs_a_real_command_at_its_position() {
        let (cmd, params) = tempo_change_action(96_000, 133.5);
        assert!(soundcraft_engine::command_specs().iter().any(|c| c.id == cmd), "{cmd} is not a command");
        assert_eq!(params["at"], 96_000);
        assert_eq!(params["bpm"], 133.5);
    }
}
