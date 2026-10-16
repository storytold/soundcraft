//! Modal dialogs opened by menu-style invocations.
use crate::SoundApp;
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ClipId, Session, SourceId, TrackId};
use soundcraft_time::Samples;

#[derive(Debug, Clone)]
pub enum Dialog {
    NewTracks { count: u32, format: String, kind: String, timebase: String, name: String },
    NewSession { name: String, sample_rate: u32, demo: bool },
    Bounce { path: String, format: String, bit_depth: String, normalize: bool },
    RenameTrack { id: TrackId, name: String },
    RenameClip { id: ClipId, name: String },
    RenameMarker { id: soundcraft_model::MarkerId, name: String },
    ClearClips { clips: Vec<ClipId>, sources: Vec<SourceId>, message: String },
    PathPrompt { cmd: String, title: String, path: String, key: String },
    Number { cmd: String, title: String, key: String, value: f64, suffix: String },
    TempoChange { at: Samples, bpm: f64 }, // from double-clicking the tempo ruler
    Spot(crate::spot::Spot),
    Fades { shape: String },
    StripSilence { threshold: f64, min_ms: f64, pre_ms: f64, post_ms: f64 },
    Group { name: String, edit: bool, mix: bool, members: Vec<(u64, String, bool)> },
    Session { frame_rate: String, bit_depth: String },
    ScoreSetup { title: String, composer: String, bars_per_system: u32, show_track_names: bool },
    SaveChanges { then: (String, Value) },
}

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<Dialog>,
    /// Runs once the session has been saved from `SaveChanges`.
    after_save: Option<(String, Value)>,
}

impl Dialogs {
    pub fn open_name(&self) -> Option<&'static str> {
        self.open.as_ref().map(|d| match d {
            Dialog::NewTracks { .. } => "new_tracks",
            Dialog::NewSession { .. } => "new_session",
            Dialog::Bounce { .. } => "bounce",
            Dialog::RenameTrack { .. } => "rename_track",
            Dialog::RenameClip { .. } => "rename_clip",
            Dialog::RenameMarker { .. } => "rename_marker",
            Dialog::ClearClips { .. } => "clear_clips",
            Dialog::PathPrompt { .. } => "path",
            Dialog::Number { .. } => "number",
            Dialog::TempoChange { .. } => "tempo_change",
            Dialog::Spot(_) => "spot",
            Dialog::Fades { .. } => "fades",
            Dialog::StripSilence { .. } => "strip_silence",
            Dialog::Group { .. } => "group",
            Dialog::Session { .. } => "session",
            Dialog::ScoreSetup { .. } => "score_setup",
            Dialog::SaveChanges { .. } => "save_changes",
        })
    }

    pub fn open_rename_track(&mut self, id: TrackId, name: &str) {
        self.open = Some(Dialog::RenameTrack { id, name: name.to_string() });
    }

    pub fn open_rename_clip(&mut self, id: ClipId, name: &str) {
        self.open = Some(Dialog::RenameClip { id, name: name.to_string() });
    }

    pub fn open_rename_marker(&mut self, id: soundcraft_model::MarkerId, name: &str) {
        self.open = Some(Dialog::RenameMarker { id, name: name.to_string() });
    }

    /// Clip List › Clear: confirm before clips and files leave the session. False when there is nothing to clear.
    pub fn open_clear(&mut self, e: &Engine, clips: Vec<ClipId>, sources: Vec<SourceId>) -> bool {
        let Some(message) = clear_message(e.session(), &clips, &sources) else { return false };
        self.open = Some(Dialog::ClearClips { clips, sources, message });
        true
    }

    /// Open the dialog for a command; false when the command has no dialog.
    pub fn open_for_command(&mut self, e: &Engine, id: &str) -> bool {
        self.after_save = None;
        let home = default_dir();
        let name = e.session().name.clone();
        self.open = Some(match id {
            "clip.clear" => {
                let ed = &e.session().edit;
                return self.open_clear(e, ed.selected_clips.clone(), ed.selected_sources.clone());
            }
            "track.new" => {
                Dialog::NewTracks { count: 1, format: "Mono".into(), kind: "audio".into(), timebase: "samples".into(), name: "Audio".into() }
            }
            "session.new" => Dialog::NewSession { name: "Untitled".into(), sample_rate: 48_000, demo: false },
            "file.bounce_mix" => {
                let dir = e
                    .path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).parent())
                    .filter(|d| !d.as_os_str().is_empty())
                    .map_or(home.clone(), |d| d.display().to_string());
                Dialog::Bounce { path: format!("{dir}/{name} Bounce.wav"), format: "wav".into(), bit_depth: "24".into(), normalize: false }
            }
            "file.import_audio" => Dialog::PathPrompt { cmd: id.into(), title: tr("Import Audio").into(), path: home, key: "path".into() },
            "file.import_midi" => Dialog::PathPrompt { cmd: id.into(), title: tr("Import MIDI").into(), path: home, key: "path".into() },
            "session.open" => Dialog::PathPrompt { cmd: id.into(), title: tr("Open Session").into(), path: home, key: "path".into() },
            "session.save" if e.path.is_some() => return false,
            "session.save" | "session.save_as" | "session.save_copy" => Dialog::PathPrompt {
                cmd: if id == "session.save_copy" { id } else { "session.save_as" }.into(),
                title: tr("Save Session As").into(),
                // The browser has no folders: the name becomes the bundle file's name.
                path: if home.is_empty() {
                    format!("{name}.{}", soundcraft_engine::io::BUNDLE_EXTENSION)
                } else {
                    format!("{home}/{name}/{name}.scraft")
                },
                key: "path".into(),
            },
            "edit.repeat" => {
                Dialog::Number { cmd: id.into(), title: tr("Repeat").into(), key: "count".into(), value: 2.0, suffix: tr("times").into() }
            }
            "edit.shift" => {
                Dialog::Number { cmd: id.into(), title: tr("Shift (seconds)").into(), key: "by_seconds".into(), value: 1.0, suffix: "s".into() }
            }
            "event.transpose" => {
                Dialog::Number { cmd: id.into(), title: tr("Transpose").into(), key: "semitones".into(), value: 12.0, suffix: tr("semitones").into() }
            }
            "event.quantize" => {
                Dialog::Number { cmd: id.into(), title: tr("Quantize strength").into(), key: "strength".into(), value: 100.0, suffix: "%".into() }
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
            "clip.rename" => {
                let target = soundcraft_engine::cmd::clip_ids_param(e, &json!({})).first().and_then(|id| e.session().find_clip(*id));
                let Some((_, clip)) = target else { return false };
                Dialog::RenameClip { id: clip.id, name: clip.name.clone() }
            }
            _ => match path_dialog(id, &home, &name) {
                Some(d) => d,
                None => return false,
            },
        });
        true
    }
}

/// When `id` would close a session with unsaved changes, ask first; true when the question is
/// open instead. A take in progress is stopped first, so it counts as a change.
pub fn ask_to_save(app: &mut SoundApp, id: &str, params: &Value) -> bool {
    if !matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit")
        || soundcraft_engine::find_command(id).is_some_and(|c| (c.enabled)(&app.engine).is_err())
    {
        return false;
    }
    if app.engine.transport.recording {
        app.stop_play();
    }
    if !app.engine.is_dirty() {
        return false;
    }
    app.dialogs.open = Some(Dialog::SaveChanges { then: (id.into(), params.clone()) });
    true
}

/// Runs `id`, unless it asks to save first (`ask_to_save`).
pub fn run_or_ask(app: &mut SoundApp, id: &str, params: Value) -> Result<Value, String> {
    if ask_to_save(app, id, &params) {
        return Ok(Value::Null);
    }
    app.run(id, params)
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
    let title = format!("{} — {}", tr(spec.menu.last().copied().unwrap_or("File")), tr(spec.label.trim_end_matches("...")));
    Some(Dialog::PathPrompt { cmd: id.into(), title, path, key: key.into() })
}

fn clear_message(s: &Session, clips: &[ClipId], sources: &[SourceId]) -> Option<String> {
    let listed: std::collections::BTreeSet<ClipId> = clips.iter().copied().collect();
    let names: Vec<&str> = s.tracks.iter().flat_map(|t| t.clips()).filter(|c| listed.contains(&c.id)).map(|c| c.name.as_str()).collect();
    let files: Vec<_> = sources.iter().filter_map(|id| s.source(*id)).collect();
    let clips_n = |n: usize| if n == 1 { tr("1 clip").to_string() } else { trf("{} clips", &[&n]) };
    let files_n = |n: usize| if n == 1 { tr("1 file").to_string() } else { trf("{} files", &[&n]) };
    let what = match (names.as_slice(), files.as_slice()) {
        ([], []) => return None,
        ([name], []) => trf("the clip \"{}\"", &[name]),
        ([], [f]) => trf("the file \"{}\"", &[&f.name]),
        (c, []) => clips_n(c.len()),
        ([], f) => files_n(f.len()),
        (c, f) => trf("{} and {}", &[&clips_n(c.len()), &files_n(f.len())]),
    };
    let mut text = trf("Remove {} from the session?", &[&what]);
    let used: Vec<(&str, usize)> =
        files.iter().map(|f| (f.name.as_str(), s.clips_using(f.id).filter(|c| !listed.contains(&c.id)).count())).filter(|(_, n)| *n > 0).collect();
    let n: usize = used.iter().map(|u| u.1).sum();
    let users = if n == 1 {
        tr("a clip on the timeline, which is removed too.").to_string()
    } else {
        trf("{} clips on the timeline, which are removed too.", &[&n])
    };
    match used.as_slice() {
        [] => {}
        [_] if files.len() == 1 => text += &trf(" It is used by {}", &[&users]),
        u if u.len() == files.len() => text += &trf(" They are used by {}", &[&users]),
        [(name, _)] => text += &trf(" \"{}\" is used by {}", &[name, &users]),
        many => text += &trf(" {} of the files are used by {}", &[&many.len(), &users]),
    }
    if !files.is_empty() && files.iter().all(|f| !f.unsaved) {
        text += tr(if files.len() == 1 { " The file stays on disk." } else { " The files stay on disk." });
    }
    Some(text)
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
    crate::bounce::show(app, ctx);
    let Some(mut d) = app.dialogs.open.take() else { return };
    let mut keep = true;
    let mut action: Option<(String, Value)> = None;
    let mut save_first: Option<(String, Value)> = None;
    let confirmed = matches!(d, Dialog::SaveChanges { .. });
    let title = match &d {
        Dialog::NewTracks { .. } => tr("New Tracks"),
        Dialog::NewSession { .. } => tr("New Session"),
        Dialog::Bounce { .. } => tr("Bounce Mix"),
        Dialog::RenameTrack { .. } => tr("Rename Track"),
        Dialog::RenameClip { .. } => tr("Rename Clip"),
        Dialog::RenameMarker { .. } => tr("Rename Marker"),
        Dialog::ClearClips { .. } => tr("Clear"),
        Dialog::PathPrompt { title, .. } | Dialog::Number { title, .. } => title.as_str(),
        Dialog::TempoChange { .. } => tr("Tempo Change (BPM)"),
        Dialog::Spot(_) => tr("Spot"),
        Dialog::Fades { .. } => tr("Fades"),
        Dialog::StripSilence { .. } => tr("Strip Silence"),
        Dialog::Group { .. } => tr("Create Group"),
        Dialog::Session { .. } => tr("Session Setup"),
        Dialog::ScoreSetup { .. } => tr("Score Setup"),
        Dialog::SaveChanges { then } if then.0 == "session.revert" => tr("Revert to Saved"),
        Dialog::SaveChanges { .. } => tr("Unsaved Changes"),
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
                        ui.label(tr("Create"));
                        ui.add(egui::DragValue::new(count).range(1..=128));
                        ui.label(tr("new"));
                        egui::ComboBox::from_id_salt("nt_fmt").selected_text(tr(format.as_str())).show_ui(ui, |ui| {
                            for f in soundcraft_model::ChannelFormat::ALL {
                                ui.selectable_value(format, f.label().to_string(), tr(f.label()));
                            }
                        });
                        egui::ComboBox::from_id_salt("nt_kind")
                            .selected_text(tr(soundcraft_model::TrackKind::from_id(kind).map_or("Audio Track", |k| k.label())))
                            .show_ui(ui, |ui| {
                                for k in soundcraft_model::TrackKind::ALL {
                                    if ui.selectable_label(kind == k.id(), tr(k.label())).clicked() {
                                        *kind = k.id().to_string();
                                        *name = k.default_name().to_string();
                                    }
                                }
                            });
                        ui.label(tr("in"));
                        egui::ComboBox::from_id_salt("nt_tb").selected_text(tr(if timebase == "ticks" { "Ticks" } else { "Samples" })).show_ui(
                            ui,
                            |ui| {
                                ui.selectable_value(timebase, "samples".to_string(), tr("Samples"));
                                ui.selectable_value(timebase, "ticks".to_string(), tr("Ticks"));
                            },
                        );
                        ui.label(tr("Name:"));
                        ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                    });
                    if buttons(ui, tr("Create"), enter) {
                        action =
                            Some(("track.new".into(), json!({"count": count, "format": format, "kind": kind, "timebase": timebase, "name": name})));
                    }
                }
                Dialog::NewSession { name, sample_rate, demo } => {
                    ui.horizontal(|ui| {
                        ui.label(tr("Name"));
                        ui.text_edit_singleline(name);
                    });
                    egui::ComboBox::from_label(tr("Sample Rate")).selected_text(format!("{sample_rate} Hz")).show_ui(ui, |ui| {
                        for r in soundcraft_time::SampleRate::COMMON {
                            ui.selectable_value(sample_rate, r, format!("{r} Hz"));
                        }
                    });
                    crate::widgets::checkbox(ui, demo, tr("Start from the demo session"));
                    if buttons(ui, tr("Create"), enter) {
                        action = Some((
                            "session.new".into(),
                            json!({"name": name, "sample_rate": sample_rate, "template": if *demo { "demo" } else { "blank" }}),
                        ));
                    }
                }
                Dialog::Bounce { path, format, bit_depth, normalize } => {
                    ui.horizontal(|ui| {
                        ui.label(tr("File"));
                        ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                        if let Some(pick) = &app.services.pick_save
                            && ui.button(tr("Browse…")).clicked()
                            && let Some(p) = pick("file.bounce_mix", path)
                        {
                            *path = p;
                            match std::path::Path::new(path.as_str()).extension().map(|x| x.to_string_lossy().to_lowercase()).as_deref() {
                                Some("wav") => *format = "wav".into(),
                                Some("aif" | "aiff") => *format = "aiff".into(),
                                Some("flac") => *format = "flac".into(),
                                _ => {}
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label(tr("Format")).selected_text(format.to_uppercase()).show_ui(ui, |ui| {
                            for f in ["wav", "aiff", "flac"] {
                                ui.selectable_value(format, f.to_string(), f.to_uppercase());
                            }
                        });
                        egui::ComboBox::from_label(tr("Bit Depth")).selected_text(trf("{}-bit", &[&*bit_depth])).show_ui(ui, |ui| {
                            for b in ["16", "24", "32f"] {
                                ui.selectable_value(bit_depth, b.to_string(), trf("{}-bit", &[&b]));
                            }
                        });
                    });
                    crate::widgets::checkbox(ui, normalize, tr("Normalize to -0.1 dBFS"));
                    ui.label(
                        egui::RichText::new(tr("Bounces the edit selection, or the whole session when nothing is selected."))
                            .small()
                            .color(Tokens::current().text_dim),
                    );
                    if buttons(ui, tr("Bounce"), enter) {
                        let p = std::path::Path::new(path.as_str()).with_extension(if format == "aiff" { "aif" } else { format.as_str() });
                        crate::bounce::start(
                            app,
                            &json!({"path": p.to_string_lossy(), "format": format, "bit_depth": bit_depth, "normalize": normalize}),
                        );
                        keep = false;
                    }
                }
                Dialog::RenameTrack { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, tr("OK"), enter) {
                        action = Some(("track.rename".into(), json!({"track": id.0, "name": name})));
                    }
                }
                Dialog::RenameClip { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, tr("OK"), enter) {
                        action = Some(("clip.rename".into(), json!({"clip": id.0, "name": name})));
                    }
                }
                Dialog::RenameMarker { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
                        action = Some(("markers.edit".into(), json!({"id": id.0, "rename": name})));
                    }
                }
                Dialog::ClearClips { clips, sources, message } => {
                    ui.set_max_width(340.0);
                    ui.label(message.as_str());
                    if buttons(ui, tr("Remove"), enter) {
                        action = Some(("clip.clear".into(), json!({"clips": clips, "sources": sources, "with_clips": true})));
                    }
                }
                Dialog::PathPrompt { cmd, path, key, .. } => {
                    ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                    if let Some(pick) = &app.services.pick_open
                        && cmd != "session.save_as"
                        && cmd != "session.save_copy"
                        && key != "dir"
                        && ui.button(tr("Browse…")).clicked()
                        && let Some(p) = pick(cmd, &[])
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_folder
                        && key == "dir"
                        && ui.button(tr("Browse…")).clicked()
                        && let Some(p) = pick(path)
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_save
                        && (cmd == "session.save_as" || cmd == "session.save_copy")
                        && ui.button(tr("Browse…")).clicked()
                        && let Some(p) = pick(cmd, path)
                    {
                        *path = p;
                    }
                    let file = soundcraft_engine::io::session_file(path);
                    let replaces = soundcraft_engine::io::replaces(&app.engine, cmd, &file);
                    if replaces {
                        ui.label(trf("“{}” already exists. Do you want to replace it?", &[&file.file_name().unwrap_or_default().to_string_lossy()]));
                    }
                    if buttons(ui, tr(if replaces { "Replace" } else { "OK" }), enter && !replaces) {
                        let mut v = json!({});
                        v[key.as_str()] = json!(path);
                        if replaces {
                            v["replace"] = json!(true);
                        }
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::Number { cmd, key, value, suffix, .. } => {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(value).speed(0.1));
                        ui.label(suffix.as_str());
                    });
                    if buttons(ui, tr("OK"), enter) {
                        let v = if key == "by_seconds" { json!({"by": {"seconds": value}}) } else { json!({key.as_str(): value}) };
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::TempoChange { at, bpm } => {
                    ui.horizontal(|ui| {
                        // Same limits the engine enforces (time crate `valid_bpm`).
                        ui.add(egui::DragValue::new(bpm).speed(0.1).range(5.0..=1000.0));
                        ui.label(tr("bpm"));
                    });
                    if buttons(ui, tr("OK"), enter) {
                        action = Some(tempo_change_action(*at, *bpm));
                    }
                }
                Dialog::Spot(d) => {
                    crate::spot::fields(ui, app.engine.session(), d);
                    if buttons(ui, "OK", enter) {
                        keep = false;
                        action = d.command(app.engine.session());
                    }
                }
                Dialog::Fades { shape } => {
                    for s in soundcraft_model::FadeShape::ALL {
                        ui.radio_value(shape, s.label().to_lowercase(), tr(s.label()));
                    }
                    if buttons(ui, tr("OK"), enter) {
                        action = Some(("edit.fades_create".into(), json!({"shape": shape})));
                    }
                }
                Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms } => {
                    ui.add(egui::Slider::new(threshold, -96.0..=0.0).text(tr("Threshold dB")));
                    ui.add(egui::Slider::new(min_ms, 0.0..=2000.0).text(tr("Min strip duration ms")));
                    ui.add(egui::Slider::new(pre_ms, 0.0..=500.0).text(tr("Clip start pad ms")));
                    ui.add(egui::Slider::new(post_ms, 0.0..=2000.0).text(tr("Clip end pad ms")));
                    if buttons(ui, tr("Strip"), enter) {
                        action = Some((
                            "edit.strip_silence".into(),
                            json!({"threshold_db": threshold, "min_length_ms": min_ms, "pad_before_ms": pre_ms, "pad_after_ms": post_ms}),
                        ));
                    }
                }
                Dialog::Group { name, edit, mix, members } => {
                    ui.horizontal(|ui| {
                        ui.label(tr("Name"));
                        ui.text_edit_singleline(name);
                    });
                    ui.horizontal(|ui| {
                        crate::widgets::checkbox(ui, edit, tr("Edit group"));
                        crate::widgets::checkbox(ui, mix, tr("Mix group"));
                    });
                    ui.label(tr("Tracks"));
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (_, n, on) in members.iter_mut() {
                            crate::widgets::checkbox(ui, on, n.as_str());
                        }
                    });
                    if buttons(ui, tr("OK"), enter) {
                        let ids: Vec<u64> = members.iter().filter(|m| m.2).map(|m| m.0).collect();
                        action = Some(("track.group".into(), json!({"name": name, "edit": edit, "mix": mix, "tracks": ids})));
                    }
                }
                Dialog::Session { frame_rate, bit_depth } => {
                    egui::ComboBox::from_label(tr("Timecode Rate")).selected_text(frame_rate.as_str()).show_ui(ui, |ui| {
                        for r in soundcraft_time::FrameRate::ALL {
                            ui.selectable_value(frame_rate, r.label().to_string(), r.label());
                        }
                    });
                    egui::ComboBox::from_label(tr("Bit Depth")).selected_text(bit_depth.as_str()).show_ui(ui, |ui| {
                        for b in ["16", "24", "32f"] {
                            ui.selectable_value(bit_depth, b.to_string(), b);
                        }
                    });
                    if buttons(ui, tr("OK"), enter) {
                        action = Some(("setup.session".into(), json!({"frame_rate": frame_rate, "bit_depth": bit_depth})));
                    }
                }
                Dialog::ScoreSetup { title, composer, bars_per_system, show_track_names } => {
                    egui::Grid::new("score_setup").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                        ui.label(tr("Title"));
                        ui.text_edit_singleline(title);
                        ui.end_row();
                        ui.label(tr("Composer"));
                        ui.text_edit_singleline(composer);
                        ui.end_row();
                        ui.label(tr("Bars per system"));
                        ui.add(egui::DragValue::new(bars_per_system).range(1..=16));
                        ui.end_row();
                    });
                    crate::widgets::checkbox(ui, show_track_names, tr("Show track names"));
                    if buttons(ui, tr("OK"), enter) {
                        action = Some((
                            "file.score_setup".into(),
                            json!({"title": title, "composer": composer, "bars_per_system": bars_per_system, "show_track_names": show_track_names}),
                        ));
                    }
                }
                Dialog::SaveChanges { then } => {
                    let name = &app.engine.session().name;
                    let revert = then.0 == "session.revert";
                    if revert || cfg!(target_arch = "wasm32") {
                        let (question, ok) = if revert {
                            (trf("Revert “{}” to its last saved version?", &[name]), tr("Revert"))
                        } else {
                            (trf("Close “{}” without saving?", &[name]), tr("Don't Save"))
                        };
                        ui.label(trf("{} Your changes will be lost.", &[&question]));
                        if buttons(ui, ok, false) {
                            action = Some(then.clone());
                        }
                    } else {
                        ui.label(trf("Save changes to “{}” before closing?", &[name]));
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button(tr("Don't Save")).clicked() {
                                action = Some(then.clone());
                            }
                            if buttons(ui, tr("Save"), enter) {
                                save_first = Some(then.clone());
                            }
                        });
                    }
                }
            }
        });
    if esc || ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("dlg_cancel")).unwrap_or(false)) {
        keep = false;
        app.dialogs.after_save = None;
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("dlg_cancel")));
    }
    if let Some(then) = save_first {
        keep = false;
        if !app.dialogs.open_for_command(&app.engine, "session.save") {
            action = Some(("session.save".into(), json!({})));
        }
        app.dialogs.after_save = Some(then);
    }
    if let Some((cmd, params)) = &action
        && !confirmed
        && ask_to_save(app, cmd, params)
    {
        keep = false;
        action = None;
    }
    let saving = action.as_ref().is_some_and(|(cmd, _)| cmd.starts_with("session.save"));
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
    if saving
        && let Some((then, params)) = app.dialogs.after_save.take()
        && !app.engine.is_dirty()
        && let Err(e) = app.run(&then, params)
    {
        app.ui.status = e;
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
        if ui.button(tr("Cancel")).clicked() {
            ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("dlg_cancel"), true));
        }
        if ui.add(egui::Button::new(egui::RichText::new(ok).strong()).fill(Tokens::current().accent)).clicked() {
            pressed = true;
        }
    });
    pressed
}

/// A name field that takes the focus with its text selected, so typing replaces the old name.
fn name_field(ui: &mut egui::Ui, name: &mut String) {
    let mut out = egui::TextEdit::singleline(name).show(ui);
    if !out.response.has_focus() {
        out.response.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(name.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), out.response.id);
    }
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
                    ui.add(egui::Slider::new(v, -24.0..=0.0).text(tr("Peak dB")));
                }
                "time_stretch" => {
                    let v = params.entry("ratio".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text(tr("Length ratio")));
                }
                "pitch_shift" => {
                    let v = params.entry("semitones".into()).or_insert(0.0);
                    ui.add(egui::Slider::new(v, -24.0..=24.0).text(tr("Semitones")));
                }
                "varispeed" => {
                    let v = params.entry("speed".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text(tr("Speed")));
                }
                _ => {
                    ui.label(tr("No parameters."));
                }
            }
        }
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, params.clone()));
        let hint = if soundcraft_dsp::is_generator(&process) {
            "Fills the selection on the selected tracks with a new clip."
        } else {
            "Processes the selected clips and replaces them with rendered audio."
        };
        ui.label(egui::RichText::new(tr(hint)).small());
        if ui.button(egui::RichText::new(tr("Render")).strong()).clicked() {
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
    fn clip_rename_opens_the_rename_dialog_for_the_selected_clip() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let mut d = Dialogs::default();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(!d.open_for_command(&e, "clip.rename"));
        let clip = e.session().track_by_name("Kick").unwrap().clips()[0].clone();
        e.execute("edit.select", &json!({"clips": [clip.id.0]})).unwrap();
        assert!(d.open_for_command(&e, "clip.rename"));
        assert!(matches!(d.open, Some(Dialog::RenameClip { id, ref name }) if id == clip.id && *name == clip.name));
    }

    #[test]
    fn tempo_change_dialog_runs_a_real_command_at_its_position() {
        let (cmd, params) = tempo_change_action(96_000, 133.5);
        assert!(soundcraft_engine::command_specs().iter().any(|c| c.id == cmd), "{cmd} is not a command");
        assert_eq!(params["at"], 96_000);
        assert_eq!(params["bpm"], 133.5);
    }

    #[test]
    fn clear_always_asks_and_names_the_timeline_clips_that_go_too() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let file = |e: &Engine, name: &str| e.session().sources.iter().find(|f| f.name == name).map(|f| f.id).unwrap();
        let (kick, hats) = (file(&e, "Kick"), file(&e, "Hats"));
        let kick_clips: Vec<ClipId> = e.session().clips_using(kick).map(|c| c.id).collect();
        let snare = e.session().track_by_name("Snare").unwrap().clips()[0].id;
        let message = |d: &Dialogs| match &d.open {
            Some(Dialog::ClearClips { message, .. }) => message.clone(),
            other => panic!("{other:?}"),
        };
        let mut d = Dialogs::default();
        assert!(!d.open_for_command(&e, "clip.clear"));
        e.execute("edit.select", &json!({"clips": [snare]})).unwrap();
        assert!(d.open_for_command(&e, "clip.clear"));
        assert_eq!(message(&d), "Remove the clip \"Snare\" from the session?");
        e.execute("edit.select", &json!({"sources": [kick]})).unwrap();
        assert!(d.open_for_command(&e, "clip.clear"));
        assert!(matches!(&d.open, Some(Dialog::ClearClips { clips, sources, .. }) if clips.is_empty() && *sources == vec![kick]));
        assert_eq!(message(&d), "Remove the file \"Kick\" from the session? It is used by 3 clips on the timeline, which are removed too.");
        assert!(d.open_clear(&e, kick_clips.clone(), vec![kick]));
        assert_eq!(message(&d), "Remove 3 clips and 1 file from the session?");
        assert!(d.open_clear(&e, kick_clips, vec![kick, hats]));
        assert_eq!(message(&d), "Remove 3 clips and 2 files from the session? \"Hats\" is used by a clip on the timeline, which is removed too.");
        e.session_mut().sources.iter_mut().for_each(|f| f.unsaved = false);
        assert!(d.open_clear(&e, vec![], vec![kick, hats]));
        assert_eq!(
            message(&d),
            "Remove 2 files from the session? They are used by 4 clips on the timeline, which are removed too. The files stay on disk."
        );
    }
}
