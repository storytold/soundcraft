//! Modal dialogs opened by menu-style invocations.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, vec2};
use serde_json::{Value, json};
use soundcraft_audio_io::FileFormat;
use soundcraft_engine::Engine;
use soundcraft_engine::io::StripNames;
use soundcraft_model::{ClipId, GroupId, Session, SourceId, TrackId};
use soundcraft_time::Samples;
use std::sync::Arc;

mod audiosuite;
mod files;
mod strip_silence;

#[cfg(test)]
use audiosuite::SPEED_RAMP_CHOICES;
use audiosuite::audiosuite_window;
pub use files::{NotUnicode, ask_to_save, open_path, open_session_bytes, run_or_ask, takes_path, unicode};
use files::{OPEN_PENDING, default_dir, open_pending_session, path_dialog};
use strip_silence::{AUTO_NAME_LABELS, STRIP_NAME_LABELS, StripRectangles, auto_rename_action, strip_name_fields, strip_silence_action};
pub use strip_silence::{StripRects, strip_silence_rectangles};

pub mod ops_windows;
pub(super) mod recent;
pub mod recovery;
pub mod spot;

#[derive(Debug, Clone)]
pub enum Dialog {
    NewTracks { count: u32, format: String, kind: String, timebase: String, name: String },
    NewSession { name: String, sample_rate: u32, demo: bool },
    Bounce { path: String, format: FileFormat, bit_depth: String, normalize: bool, dither: bool },
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
    // Strip Silence › Rename: `strip` holds the Strip Silence values to return to.
    StripSilenceRename { strip: [f64; 4], names: StripNames },
    // Clip List › Auto Rename: the names that editing gives the clips it separates from `clips`.
    AutoRename { clips: Vec<ClipId>, names: StripNames },
    // `id: None` creates (Track › Group…); `id: Some` modifies that group (Groups list › Modify Group…).
    Group { id: Option<GroupId>, name: String, edit: bool, mix: bool, members: Vec<(u64, String, bool)> },
    Session { frame_rate: String, bit_depth: String },
    // Clip › Loop…: `length` is typed in `format`, measured from `start` (the first selected clip's).
    // `filled` is the text the field opened with and the exact length it stands for.
    Loop { by: LoopBy, count: u32, length: String, format: soundcraft_time::TimeFormat, start: Samples, filled: (String, Samples) },
    ScoreSetup { title: String, composer: String, bars_per_system: u32, show_track_names: bool },
    SaveChanges { then: (String, Value) },
}

/// The Clip Looping dialog's choice of how far a clip is looped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopBy {
    /// Number of Loops.
    Count,
    /// Loop Length.
    Length,
    /// Loop Until End of Session or Next Clip.
    Fill,
}

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<Dialog>,
    /// Runs once the session has been saved from `SaveChanges`.
    after_save: Option<(String, Value)>,
    /// A session the browser read while the open one had changes, waiting for `SaveChanges`.
    pending_session: Option<(String, Vec<u8>)>,
    /// The naming scheme Strip Silence gives the clips it leaves, kept from one use to the next.
    pub strip_names: StripNames,
    /// The Strip Silence rectangles the Edit window draws while Strip Silence is open.
    strip_rectangles: StripRectangles,
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
            Dialog::StripSilenceRename { .. } => "strip_silence_rename",
            Dialog::AutoRename { .. } => "auto_rename",
            Dialog::Group { id: None, .. } => "group",
            Dialog::Group { id: Some(_), .. } => "modify_group",
            Dialog::Session { .. } => "session",
            Dialog::Loop { .. } => "clip_looping",
            Dialog::ScoreSetup { .. } => "score_setup",
            Dialog::SaveChanges { .. } => "save_changes",
        })
    }

    /// Whether a worker thread is finding Strip Silence rectangles.
    pub fn strip_scan_running(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.strip_rectangles.running.is_some() {
            return true;
        }
        false
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

    /// Groups list › Modify Group…: the creation dialog, pre-filled from an existing group. False
    /// when the group is gone (deleted between the click and the menu).
    pub fn open_modify_group(&mut self, e: &Engine, id: GroupId) -> bool {
        let s = e.session();
        let Some(g) = s.group(id) else { return false };
        let members = s.tracks.iter().map(|t| (t.id.0, t.name.clone(), g.members.contains(&t.id))).collect();
        self.open = Some(Dialog::Group { id: Some(id), name: g.name.clone(), edit: g.edit, mix: g.mix, members });
        true
    }

    /// Clip List › Clear: confirm before clips and files leave the session. False when there is nothing to clear.
    pub fn open_clear(&mut self, e: &Engine, clips: Vec<ClipId>, sources: Vec<SourceId>) -> bool {
        let Some(message) = clear_message(e.session(), &clips, &sources) else { return false };
        self.open = Some(Dialog::ClearClips { clips, sources, message });
        true
    }

    /// Clip List › Auto Rename: the clips' naming scheme, filled in from the first clip's. False
    /// when none of the clips is in the session.
    pub fn open_auto_rename(&mut self, e: &Engine, clips: Vec<ClipId>) -> bool {
        let s = e.session();
        let Some((_, first)) = clips.iter().find_map(|id| s.find_clip(*id)) else { return false };
        let names = first.auto_name.and_then(|k| s.auto_names.get(&k)).map_or_else(StripNames::default, StripNames::from);
        self.open = Some(Dialog::AutoRename { clips, names });
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
                Dialog::Bounce {
                    path: format!("{dir}/{name} Bounce.wav"),
                    format: FileFormat::Wav,
                    bit_depth: "24".into(),
                    normalize: false,
                    dither: false,
                }
            }
            "file.import_audio" => Dialog::PathPrompt { cmd: id.into(), title: "Import Audio".into(), path: home, key: "path".into() },
            "file.import_midi" => Dialog::PathPrompt { cmd: id.into(), title: "Import MIDI".into(), path: home, key: "path".into() },
            "session.open" => Dialog::PathPrompt { cmd: id.into(), title: "Open Session".into(), path: home, key: "path".into() },
            "session.save" if e.path.is_some() => return false,
            "session.save" | "session.save_as" | "session.save_copy" => Dialog::PathPrompt {
                cmd: if id == "session.save_copy" { id } else { "session.save_as" }.into(),
                title: "Save Session As".into(),
                // The browser has no folders: the name becomes the bundle file's name.
                path: if home.is_empty() {
                    format!("{name}.{}", soundcraft_engine::io::BUNDLE_EXTENSION)
                } else {
                    format!("{home}/{name}/{name}.scraft")
                },
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
                Dialog::Group { id: None, name: format!("Group {}", s.groups.len() + 1), edit: true, mix: true, members }
            }
            "setup.session" => Dialog::Session { frame_rate: e.session().frame_rate.label().into(), bit_depth: "24".into() },
            "file.score_setup" => {
                let s = soundcraft_engine::score::ScoreSetup::from_session(e.session());
                Dialog::ScoreSetup { title: s.title, composer: s.composer, bars_per_system: s.bars_per_system, show_track_names: s.show_track_names }
            }
            "clip.auto_rename" => return self.open_auto_rename(e, e.session().edit.selected_clips.clone()),
            "clip.rename" => {
                let target = soundcraft_engine::cmd::clip_ids_param(e, &json!({})).first().and_then(|id| e.session().find_clip(*id));
                let Some((_, clip)) = target else { return false };
                Dialog::RenameClip { id: clip.id, name: clip.name.clone() }
            }
            "clip.loop" => {
                let target = soundcraft_engine::cmd::clip_ids_param(e, &json!({})).first().and_then(|id| e.session().find_clip(*id));
                let Some((_, clip)) = target else { return false };
                let s = e.session();
                let format = s.edit.main_counter;
                let exact = clip.length.saturating_mul(4);
                let length = soundcraft_time::format_length(clip.start, exact, format, s.sample_rate, &s.tempo, s.frame_rate);
                Dialog::Loop { by: LoopBy::Count, count: 4, length: length.clone(), format, start: clip.start, filled: (length, exact) }
            }
            _ => match path_dialog(id, &home, &name) {
                Some(d) => d,
                None => return false,
            },
        });
        true
    }
}

fn clear_message(s: &Session, clips: &[ClipId], sources: &[SourceId]) -> Option<String> {
    let listed: std::collections::BTreeSet<ClipId> = clips.iter().copied().collect();
    let names: Vec<&str> = s.tracks.iter().flat_map(|t| t.clips()).filter(|c| listed.contains(&c.id)).map(|c| c.name.as_str()).collect();
    let files: Vec<_> = sources.iter().filter_map(|id| s.source(*id)).collect();
    let count = |n: usize, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    let what = match (names.as_slice(), files.as_slice()) {
        ([], []) => return None,
        ([name], []) => format!("the clip \"{name}\""),
        ([], [f]) => format!("the file \"{}\"", f.name),
        (c, []) => count(c.len(), "clip"),
        ([], f) => count(f.len(), "file"),
        (c, f) => format!("{} and {}", count(c.len(), "clip"), count(f.len(), "file")),
    };
    let mut text = format!("Remove {what} from the session?");
    let used: Vec<(&str, usize)> =
        files.iter().map(|f| (f.name.as_str(), s.clips_using(f.id).filter(|c| !listed.contains(&c.id)).count())).filter(|(_, n)| *n > 0).collect();
    let n: usize = used.iter().map(|u| u.1).sum();
    let users = if n == 1 {
        "a clip on the timeline, which is removed too.".to_string()
    } else {
        format!("{n} clips on the timeline, which are removed too.")
    };
    match used.as_slice() {
        [] => {}
        [_] if files.len() == 1 => text += &format!(" It is used by {users}"),
        u if u.len() == files.len() => text += &format!(" They are used by {users}"),
        [(name, _)] => text += &format!(" \"{name}\" is used by {users}"),
        many => text += &format!(" {} of the files are used by {users}", many.len()),
    }
    if !files.is_empty() && files.iter().all(|f| !f.unsaved) {
        text += if files.len() == 1 { " The file stays on disk." } else { " The files stay on disk." };
    }
    Some(text)
}

/// The New Tracks form; the command to run once Create is pressed.
fn new_tracks_form(
    ui: &mut egui::Ui,
    count: &mut u32,
    format: &mut String,
    kind: &mut String,
    timebase: &mut String,
    name: &mut String,
    enter: bool,
) -> Option<(String, Value)> {
    ui.horizontal(|ui| {
        let create = ui.label("Create");
        ui.add(egui::DragValue::new(count).range(1..=128)).labelled_by(create.id);
        ui.label("new");
        let fmt = craft_ui::ComboBox::from_id_salt("nt_fmt").selected_text(format.as_str()).show_ui(ui, |ui| {
            for f in soundcraft_model::ChannelFormat::ALL {
                ui.selectable_value(format, f.label().to_string(), f.label());
            }
        });
        fmt.response.accessible_name("Channel format");
        let kind_combo = craft_ui::ComboBox::from_id_salt("nt_kind")
            .selected_text(soundcraft_model::TrackKind::from_id(kind).map_or("Audio Track", |k| k.label()))
            .show_ui(ui, |ui| {
                for k in soundcraft_model::TrackKind::ALL {
                    if ui.selectable_label(kind == k.id(), k.label()).clicked() {
                        *kind = k.id().to_string();
                        *name = k.default_name().to_string();
                    }
                }
            });
        kind_combo.response.accessible_name("Track kind");
        ui.label("in");
        let tb = craft_ui::ComboBox::from_id_salt("nt_tb").selected_text(if timebase == "ticks" { "Ticks" } else { "Samples" }).show_ui(ui, |ui| {
            ui.selectable_value(timebase, "samples".to_string(), "Samples");
            ui.selectable_value(timebase, "ticks".to_string(), "Ticks");
        });
        tb.response.accessible_name("Timebase");
    });
    ui.horizontal(|ui| {
        ui.label("Name:");
        crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(name).desired_width(240.0)), "Name");
    });
    if buttons(ui, "Create", enter) {
        return Some(("track.new".into(), json!({"count": count, "format": format, "kind": kind, "timebase": timebase, "name": name})));
    }
    None
}

/// The Bounce form's file row: a path field and a Browse button that also picks the format.
fn bounce_file_row(app: &mut SoundApp, ui: &mut egui::Ui, path: &mut String, format: &mut FileFormat) {
    ui.horizontal(|ui| {
        ui.label("File");
        crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(path).desired_width(420.0)), "File");
        if let Some(pick) = &app.services.pick_save
            && ui.button("Browse…").clicked()
            && let Some(p) = pick("file.bounce_mix", std::path::Path::new(path.as_str()))
        {
            match unicode(&p) {
                Ok(text) => {
                    *path = text.to_owned();
                    if let Some(picked) = p.extension().and_then(std::ffi::OsStr::to_str).and_then(FileFormat::from_write_extension) {
                        *format = picked;
                    }
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    });
}

/// The path prompt's form; the command to run once it is confirmed.
fn path_prompt_form(app: &mut SoundApp, ui: &mut egui::Ui, cmd: &str, path: &mut String, key: &str, enter: bool) -> Option<(String, Value)> {
    crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(path).desired_width(420.0)), "Path");
    let mut picked = None;
    if let Some(pick) = &app.services.pick_open
        && cmd != "session.save_as"
        && cmd != "session.save_copy"
        && key != "dir"
        && ui.button("Browse…").clicked()
        && let Some(p) = pick(cmd, &[])
    {
        picked = Some(p);
    }
    if let Some(pick) = &app.services.pick_folder
        && key == "dir"
        && ui.button("Browse…").clicked()
        && let Some(p) = pick(std::path::Path::new(path.as_str()))
    {
        picked = Some(p);
    }
    if let Some(pick) = &app.services.pick_save
        && (cmd == "session.save_as" || cmd == "session.save_copy")
        && ui.button("Browse…").clicked()
        && let Some(p) = pick(cmd, std::path::Path::new(path.as_str()))
    {
        picked = Some(p);
    }
    match picked.as_deref().map(unicode) {
        Some(Ok(text)) => *path = text.to_owned(),
        Some(Err(e)) => app.ui.status = e.to_string(),
        None => {}
    }
    let file = soundcraft_engine::io::session_file(path);
    let replaces = soundcraft_engine::io::replaces(&app.engine, cmd, &file);
    if replaces {
        ui.label(format!("“{}” already exists. Do you want to replace it?", file.file_name().unwrap_or_default().to_string_lossy()));
    }
    if buttons(ui, if replaces { "Replace" } else { "OK" }, enter && !replaces) {
        let mut v = json!({});
        v[key] = json!(path);
        if replaces {
            v["replace"] = json!(true);
        }
        return Some((cmd.to_string(), v));
    }
    None
}

/// The Strip Silence form: the Strip Silence command to run, or the values to carry into Rename.
fn strip_silence_form(
    ui: &mut egui::Ui,
    values: &mut [f64; 4],
    strip_names: &StripNames,
    enter: bool,
) -> (Option<(String, Value)>, Option<[f64; 4]>) {
    let mut action = None;
    let mut rename_from = None;
    let [threshold, min_ms, pre_ms, post_ms] = values;
    let before = [*threshold, *min_ms, *pre_ms, *post_ms];
    ui.add(egui::Slider::new(threshold, -96.0..=0.0).text("Threshold dB"));
    ui.add(egui::Slider::new(min_ms, 0.0..=4000.0).text("Min strip duration ms"));
    ui.add(egui::Slider::new(pre_ms, 0.0..=500.0).text("Clip start pad ms"));
    ui.add(egui::Slider::new(post_ms, 0.0..=2000.0).text("Clip end pad ms"));
    let values = [*threshold, *min_ms, *pre_ms, *post_ms];
    // The Edit window is drawn before this window, so it draws the rectangles for new values in the next frame.
    if values != before {
        ui.ctx().request_repaint();
    }
    if !strip_names.name.trim().is_empty() {
        let names = format!("New clips are named {}, {} and so on.", strip_names.clip_name(0), strip_names.clip_name(1));
        ui.label(egui::RichText::new(names).small().color(Tokens::current().text_dim));
    }
    ui.separator();
    ui.horizontal(|ui| {
        let mut mode = None;
        let mut rename = false;
        if ui.button("Extract").clicked() {
            mode = Some("extract");
        }
        if ui.button("Rename…").clicked() {
            rename = true;
        }
        if ui.button("Separate").clicked() {
            mode = Some("separate");
        }
        // Enter on a focused Extract or Separate clicks that button, so Enter presses Strip only when neither was clicked.
        if buttons(ui, "Strip", enter) && mode.is_none() {
            mode = Some("strip");
        }
        // Enter on a focused Rename… also presses Strip, so Rename… goes first.
        if rename {
            rename_from = Some(values);
        } else if let Some(mode) = mode {
            action = Some(strip_silence_action(mode, values, strip_names));
        }
    });
    (action, rename_from)
}

/// Show the open dialog, if any.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audiosuite_window(app, ctx);
    crate::bounce::show(app, ctx);
    crate::import::show(app, ctx);
    let Some(mut d) = app.dialogs.open.take() else { return };
    let mut keep = true;
    let mut action: Option<(String, Value)> = None;
    let mut save_first: Option<(String, Value)> = None;
    let confirmed = matches!(d, Dialog::SaveChanges { .. });
    // Strip Silence's Rename button, and the naming scheme its dialog's OK keeps.
    let mut rename_from: Option<[f64; 4]> = None;
    let mut renamed: Option<StripNames> = None;
    let strip_names = app.dialogs.strip_names.clone();
    let title = match &d {
        Dialog::NewTracks { .. } => "New Tracks",
        Dialog::NewSession { .. } => "New Session",
        Dialog::Bounce { .. } => "Bounce Mix",
        Dialog::RenameTrack { .. } => "Rename Track",
        Dialog::RenameClip { .. } => "Rename Clip",
        Dialog::RenameMarker { .. } => "Rename Marker",
        Dialog::ClearClips { .. } => "Clear",
        Dialog::PathPrompt { title, .. } | Dialog::Number { title, .. } => title.as_str(),
        Dialog::TempoChange { .. } => "Tempo Change (BPM)",
        Dialog::Spot(_) => "Spot",
        Dialog::Fades { .. } => "Fades",
        Dialog::StripSilence { .. } => "Strip Silence",
        Dialog::StripSilenceRename { .. } | Dialog::AutoRename { .. } => "Rename Selected Clips",
        Dialog::Group { id: None, .. } => "Create Group",
        Dialog::Group { id: Some(_), .. } => "Modify Group",
        Dialog::Session { .. } => "Session Setup",
        Dialog::Loop { .. } => "Clip Looping",
        Dialog::ScoreSetup { .. } => "Score Setup",
        Dialog::SaveChanges { then } if then.0 == "session.revert" => "Revert to Saved",
        Dialog::SaveChanges { .. } => "Unsaved Changes",
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
                    if let Some(cmd) = new_tracks_form(ui, count, format, kind, timebase, name, enter) {
                        action = Some(cmd);
                    }
                }
                Dialog::NewSession { name, sample_rate, demo } => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        crate::widgets::named_text_field(ui.text_edit_singleline(name), "Name");
                    });
                    craft_ui::ComboBox::from_label("Sample Rate").selected_text(format!("{sample_rate} Hz")).show_ui(ui, |ui| {
                        for r in soundcraft_time::SampleRate::COMMON {
                            ui.selectable_value(sample_rate, r, format!("{r} Hz"));
                        }
                    });
                    crate::widgets::checkbox(ui, demo, "Start from the demo session");
                    if buttons(ui, "Create", enter) {
                        action = Some((
                            "session.new".into(),
                            json!({"name": name, "sample_rate": sample_rate, "template": if *demo { "demo" } else { "blank" }}),
                        ));
                    }
                }
                Dialog::Bounce { path, format, bit_depth, normalize, dither } => {
                    bounce_file_row(app, ui, path, format);
                    ui.horizontal(|ui| {
                        craft_ui::ComboBox::from_label("Format").selected_text(format.name()).show_ui(ui, |ui| {
                            for &f in FileFormat::WRITE {
                                ui.selectable_value(format, f, f.name());
                            }
                        });
                        craft_ui::ComboBox::from_label("Bit Depth").selected_text(format!("{bit_depth}-bit")).show_ui(ui, |ui| {
                            for b in ["16", "24", "32f"] {
                                ui.selectable_value(bit_depth, b.to_string(), format!("{b}-bit"));
                            }
                        });
                    });
                    crate::widgets::checkbox(ui, normalize, "Normalize to -0.1 dBFS");
                    // 32-bit float keeps every sample as it is: there is nothing to dither.
                    ui.add_enabled_ui(bit_depth.as_str() != "32f", |ui| {
                        crate::widgets::checkbox(ui, dither, "Dither (TPDF)")
                            .on_hover_text("Adds ±1 LSB triangular dither when writing 16- or 24-bit files. Leave it off when the master chain already dithers.");
                    });
                    ui.label(
                        egui::RichText::new("Bounces the edit selection, or the whole session when nothing is selected.")
                            .small()
                            .color(Tokens::current().text_dim),
                    );
                    if buttons(ui, "Bounce", enter) {
                        let p = std::path::Path::new(path.as_str()).with_extension(format.default_extension());
                        crate::bounce::start(
                            app,
                            &json!({"path": p.to_string_lossy(), "format": format.default_extension(), "bit_depth": bit_depth, "normalize": normalize, "dither": *dither && bit_depth.as_str() != "32f"}),
                        );
                        keep = false;
                    }
                }
                Dialog::RenameTrack { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
                        action = Some(("track.rename".into(), json!({"track": id.0, "name": name})));
                    }
                }
                Dialog::RenameClip { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
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
                    if buttons(ui, "Remove", enter) {
                        action = Some(("clip.clear".into(), json!({"clips": clips, "sources": sources, "with_clips": true})));
                    }
                }
                Dialog::PathPrompt { cmd, path, key, .. } => {
                    if let Some(c) = path_prompt_form(app, ui, cmd, path, key, enter) {
                        action = Some(c);
                    }
                }
                Dialog::Number { cmd, key, value, suffix, .. } => {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(value).speed(0.1)).accessible_name(format!("Value in {suffix}"));
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
                        ui.add(egui::DragValue::new(bpm).speed(0.1).range(5.0..=1000.0)).accessible_name("Tempo (BPM)");
                        ui.label("bpm");
                    });
                    if buttons(ui, "OK", enter) {
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
                        ui.radio_value(shape, s.label().to_lowercase(), s.label());
                    }
                    if buttons(ui, "OK", enter) {
                        action = Some(("edit.fades_create".into(), json!({"shape": shape})));
                    }
                }
                Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms } => {
                    let mut values = [*threshold, *min_ms, *pre_ms, *post_ms];
                    let (a, r) = strip_silence_form(ui, &mut values, &strip_names, enter);
                    [*threshold, *min_ms, *pre_ms, *post_ms] = values;
                    action = a.or(action.take());
                    rename_from = r.or(rename_from);
                }
                Dialog::StripSilenceRename { names, .. } => {
                    strip_name_fields(ui, names, STRIP_NAME_LABELS);
                    let cleared = ui.button("Clear").clicked();
                    if cleared {
                        *names = StripNames::default();
                    }
                    // Enter on a focused Clear clicks it, and leaves the dialog open.
                    if buttons(ui, "OK", enter) && !cleared {
                        renamed = Some(names.clone());
                    }
                }
                Dialog::AutoRename { clips, names } => {
                    ui.label("Text for naming the clips that editing makes from the selected clips:");
                    strip_name_fields(ui, names, AUTO_NAME_LABELS);
                    if buttons(ui, "OK", enter) {
                        action = Some(auto_rename_action(clips, names));
                    }
                }
                Dialog::Group { id, name, edit, mix, members } => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        crate::widgets::named_text_field(ui.text_edit_singleline(name), "Name");
                    });
                    ui.horizontal(|ui| {
                        crate::widgets::checkbox(ui, edit, "Edit group");
                        crate::widgets::checkbox(ui, mix, "Mix group");
                    });
                    ui.label("Tracks");
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (_, n, on) in members.iter_mut() {
                            crate::widgets::checkbox(ui, on, n.as_str());
                        }
                    });
                    let ids: Vec<u64> = members.iter().filter(|m| m.2).map(|m| m.0).collect();
                    // Both commands reject an empty group, so don't offer an OK that can only fail.
                    if buttons_enabled(ui, "OK", enter, !ids.is_empty()) {
                        let mut params = json!({"name": name, "edit": edit, "mix": mix, "tracks": ids});
                        let cmd = match id {
                            Some(g) => {
                                params["group"] = json!(g.0);
                                "track.group_modify"
                            }
                            None => "track.group",
                        };
                        action = Some((cmd.into(), params));
                    }
                }
                Dialog::Session { frame_rate, bit_depth } => {
                    craft_ui::ComboBox::from_label("Timecode Rate").selected_text(frame_rate.as_str()).show_ui(ui, |ui| {
                        for r in soundcraft_time::FrameRate::ALL {
                            ui.selectable_value(frame_rate, r.label().to_string(), r.label());
                        }
                    });
                    craft_ui::ComboBox::from_label("Bit Depth").selected_text(bit_depth.as_str()).show_ui(ui, |ui| {
                        for b in ["16", "24", "32f"] {
                            ui.selectable_value(bit_depth, b.to_string(), b);
                        }
                    });
                    if buttons(ui, "OK", enter) {
                        action = Some(("setup.session".into(), json!({"frame_rate": frame_rate, "bit_depth": bit_depth})));
                    }
                }
                Dialog::Loop { by, count, length, format, start, filled } => {
                    let s = app.engine.session();
                    ui.horizontal(|ui| {
                        let count_label = ui.radio_value(by, LoopBy::Count, "Number of Loops");
                        ui.add_enabled(*by == LoopBy::Count, egui::DragValue::new(count).range(1..=512)).labelled_by(count_label.id);
                    });
                    ui.horizontal(|ui| {
                        let length_label = ui.radio_value(by, LoopBy::Length, "Loop Length");
                        ui.add_enabled(*by == LoopBy::Length, egui::TextEdit::singleline(length).desired_width(140.0)).labelled_by(length_label.id);
                        ui.label(format.label());
                    });
                    ui.radio_value(by, LoopBy::Fill, "Loop Until End of Session or Next Clip");
                    let params = loop_params(s, *by, *count, length, *format, *start, filled);
                    if buttons_enabled(ui, "OK", enter, params.is_some()) {
                        action = params.map(|p| ("clip.loop".into(), p));
                    }
                }
                Dialog::ScoreSetup { title, composer, bars_per_system, show_track_names } => {
                    egui::Grid::new("score_setup").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                        ui.label("Title");
                        // The painted label is not an accesskit name; name the field itself.
                        crate::widgets::named_text_field(ui.text_edit_singleline(title), "Score title");
                        ui.end_row();
                        ui.label("Composer");
                        crate::widgets::named_text_field(ui.text_edit_singleline(composer), "Score composer");
                        ui.end_row();
                        ui.label("Bars per system");
                        ui.add(egui::DragValue::new(bars_per_system).range(1..=16)).accessible_name("Bars per system");
                        ui.end_row();
                    });
                    crate::widgets::checkbox(ui, show_track_names, "Show track names");
                    if buttons(ui, "OK", enter) {
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
                            (format!("Revert “{name}” to its last saved version?"), "Revert")
                        } else {
                            (format!("Close “{name}” without saving?"), "Don't Save")
                        };
                        ui.label(format!("{question} Your changes will be lost."));
                        if buttons(ui, ok, false) {
                            action = Some(then.clone());
                        }
                    } else {
                        ui.label(format!("Save changes to “{name}” before closing?"));
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Don't Save").clicked() {
                                action = Some(then.clone());
                            }
                            if buttons(ui, "Save", enter) {
                                save_first = Some(then.clone());
                            }
                        });
                    }
                }
            }
        });
    if esc || ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::unique("dlg_cancel")).unwrap_or(false)) {
        keep = false;
        app.dialogs.after_save = None;
        app.dialogs.pending_session = None;
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::unique("dlg_cancel")));
    }
    // Enter on a focused Cancel also presses OK. In Auto Rename, the dialog then closes without running the command.
    if !keep && matches!(d, Dialog::AutoRename { .. }) {
        action = None;
    }
    if let Some(strip) = rename_from
        && keep
    {
        d = Dialog::StripSilenceRename { strip, names: strip_names };
    }
    // OK, Cancel and Escape in Rename Selected Clips go back to Strip Silence; OK keeps the names.
    // Enter on a focused Cancel also presses OK, and `keep` is false then, so Cancel wins.
    let back = match &d {
        Dialog::StripSilenceRename { strip, .. } if !keep || renamed.is_some() => Some(*strip),
        _ => None,
    };
    if let Some([threshold, min_ms, pre_ms, post_ms]) = back {
        if let Some(names) = renamed
            && keep
        {
            app.dialogs.strip_names = names;
        }
        d = Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms };
        keep = true;
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
        if cmd == "file.import_audio" {
            // Decoded in the background so a long file doesn't freeze the app (#597).
            crate::import::start(app, params);
        } else {
            match run_then(app, &cmd, params) {
                Ok(v) if cmd == "file.export_clips" => {
                    app.ui.status = format!("Exported {} clip(s) to {}", v["written"], v["dir"].as_str().unwrap_or_default());
                }
                Ok(_) => {}
                Err(e) => app.ui.status = e,
            }
        }
    }
    if saving
        && let Some((then, params)) = app.dialogs.after_save.take()
        && !app.engine.is_dirty()
        && let Err(e) = run_then(app, &then, params)
    {
        app.ui.status = e;
    }
    if keep {
        app.dialogs.open = Some(d);
    }
}

/// Runs a dialog's command, or what `SaveChanges` carries on with.
fn run_then(app: &mut SoundApp, cmd: &str, params: Value) -> Result<Value, String> {
    if cmd == OPEN_PENDING {
        open_pending_session(app);
        return Ok(Value::Null);
    }
    app.run(cmd, params)
}

/// Cancel/OK row; returns true when OK was pressed (or Enter).
fn buttons(ui: &mut egui::Ui, ok: &str, enter: bool) -> bool {
    buttons_enabled(ui, ok, enter, true)
}

/// `buttons`, but a greyed-out OK when the dialog's values can't be submitted yet. Cancel always works.
fn buttons_enabled(ui: &mut egui::Ui, ok: &str, enter: bool, ready: bool) -> bool {
    let mut pressed = enter && ready;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::unique("dlg_cancel"), true));
        }
        let b = egui::Button::new(egui::RichText::new(ok).strong()).fill(Tokens::current().accent);
        if ui.add_enabled(ready, b).clicked() {
            pressed = true;
        }
    });
    pressed
}

/// A name field that takes the focus with its text selected, so typing replaces the old name.
fn name_field(ui: &mut egui::Ui, name: &mut String) {
    let mut out = egui::TextEdit::singleline(name).show(ui);
    let _ = crate::widgets::named_text_field(out.response.response.clone(), "Name");
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

/// The `clip.loop` params the Clip Looping dialog's OK sends; None while the typed length does not
/// read as a duration longer than zero. An untouched length field sends the exact length it was
/// filled with, which its text can round.
fn loop_params(
    s: &Session,
    by: LoopBy,
    count: u32,
    length: &str,
    format: soundcraft_time::TimeFormat,
    start: Samples,
    filled: &(String, Samples),
) -> Option<Value> {
    match by {
        LoopBy::Count => Some(json!({"count": count})),
        LoopBy::Length if length == filled.0 => Some(json!({"length": filled.1})),
        LoopBy::Length => loop_length(s, length, format, start).filter(|l| *l > 0).map(|l| json!({"length": l})),
        LoopBy::Fill => Some(json!({"fill": true})),
    }
}

/// A duration typed in `format`, the way `format_length` writes it, for a span that starts at `start`.
fn loop_length(s: &Session, text: &str, format: soundcraft_time::TimeFormat, start: Samples) -> Option<Samples> {
    if format != soundcraft_time::TimeFormat::BarsBeats {
        return soundcraft_time::parse_position(text, format, s.sample_rate, &s.tempo, s.frame_rate, 0, 0).ok();
    }
    let parts: Vec<i64> = text.split(['|', '.', ' ']).filter(|p| !p.is_empty()).map(str::parse::<i64>).collect::<Result<_, _>>().ok()?;
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| *p < 0) {
        return None;
    }
    let from = s.tempo.samples_to_ticks(start, s.sample_rate);
    let meter = s.tempo.meter_at_tick(from);
    let part = |i: usize, unit: i64| parts.get(i).copied().unwrap_or(0).checked_mul(unit);
    let ticks = part(0, meter.ticks_per_bar())?.checked_add(part(1, meter.ticks_per_beat())?)?.checked_add(part(2, 1)?)?;
    Some(s.tempo.tick_to_samples(from.checked_add(ticks)?, s.sample_rate).saturating_sub(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that is not valid Unicode is not opened under a lossy rendering (another file): the
    /// status line names its bytes.
    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_are_reported_not_mangled() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(unicode(std::path::Path::new("/a/café.wav")), Ok("/a/café.wav"));
        let latin1 = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/a/caf\xe9.scraft"));
        assert_eq!(unicode(latin1), Err(NotUnicode(latin1.to_path_buf())));
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        open_path(&mut app, latin1);
        assert_eq!(app.ui.status, r#""/a/caf\xE9.scraft" is not valid Unicode"#);
        assert!(app.engine.path.is_none() && app.dialogs.open.is_none());
    }

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
    fn speed_ramp_menu_opens_the_window_and_its_choices_reach_the_engine() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        crate::menus::invoke_menu(&mut app, "audiosuite.process", "AudioSuite > Pitch Shift > Vari-Fi");
        assert_eq!(app.ui.audiosuite.as_deref(), Some("speed_ramp"));
        // Renders the Kick clip with `params` and returns its new length and samples.
        let render = |params: Value| {
            let mut e = soundcraft_engine::demo::demo_engine();
            let clip = e.session().track_by_name("Kick").unwrap().clips()[0].id;
            e.execute("audiosuite.process", &json!({"process": "speed_ramp", "params": params, "clips": [clip.0]})).unwrap();
            let c = e.session().track_by_name("Kick").unwrap().clips()[0].clone();
            (c.length, e.session().pool.get(c.source().unwrap()).unwrap().buffer.channels.clone())
        };
        let defaults: serde_json::Map<String, Value> = SPEED_RAMP_CHOICES.iter().map(|(id, _, d, _)| (id.to_string(), json!(d))).collect();
        let (length, audio) = render(Value::Object(defaults));
        assert!(render(json!({})) == (length, audio.clone()), "the window's defaults are the engine's");
        let (extended, _) = render(json!({"selection": 1.0}));
        assert_eq!(extended, (length as f64 * 1.5).round() as i64, "Extend makes the clip 150 % long");
        let (_, fast) = render(json!({"change": 1.0}));
        assert_ne!(fast, audio, "Speed Up renders a different ramp");
        let (_, unfaded) = render(json!({"fades": 0.0}));
        assert_ne!(unfaded, audio, "Fades Off renders without the fade");
    }

    #[test]
    fn tempo_change_dialog_runs_a_real_command_at_its_position() {
        let (cmd, params) = tempo_change_action(96_000, 133.5);
        assert!(soundcraft_engine::command_specs().iter().any(|c| c.id == cmd), "{cmd} is not a command");
        assert_eq!(params["at"], 96_000);
        assert_eq!(params["bpm"], 133.5);
    }

    #[test]
    fn strip_silence_buttons_send_strip_extract_and_separate() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let end = e.session().content_end();
        e.execute("edit.select", &json!({"tracks": [kick.0], "start": 0, "end": end})).unwrap();
        let mut d = Dialogs::default();
        assert!(d.open_for_command(&e, "edit.strip_silence"));
        let Some(Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms }) = d.open.take() else {
            panic!("Strip Silence did not open its dialog")
        };
        let values = [threshold, min_ms, pre_ms, post_ms];
        let clips = |e: &Engine| e.session().track(kick).unwrap().clips().iter().map(|c| (c.start, c.end())).collect::<Vec<_>>();
        let covered = |c: &[(i64, i64)]| c.iter().map(|(a, b)| b - a).sum::<i64>();
        let before = clips(&e);
        let mut run = |mode: &str| {
            let (cmd, params) = strip_silence_action(mode, values, &StripNames::default());
            assert_eq!(params["mode"], mode);
            e.execute(&cmd, &params).unwrap();
            let after = clips(&e);
            e.execute("edit.undo", &json!({})).unwrap();
            after
        };
        let (strip, extract, separate) = (run("strip"), run("extract"), run("separate"));
        assert!(separate.len() > before.len(), "Separate cuts the clips");
        assert_eq!(covered(&separate), covered(&before), "Separate keeps all the audio");
        assert!(!strip.is_empty() && !extract.is_empty());
        assert_eq!(covered(&strip) + covered(&extract), covered(&before), "Strip and Extract keep opposite parts");
    }

    #[test]
    fn strip_silence_sends_the_naming_scheme_only_with_a_name() {
        let values = [-48.0, 50.0, 5.0, 20.0];
        let (_, plain) = strip_silence_action("strip", values, &StripNames::default());
        assert!(plain.get("name").is_none() && plain.get("suffix").is_none());
        let names = StripNames { name: "SFX".into(), number: 23, zeros: 1, suffix: ".Reel1".into() };
        let (cmd, params) = strip_silence_action("separate", values, &names);
        assert_eq!((params["name"].clone(), params["number"].clone(), params["zeros"].clone()), (json!("SFX"), json!(23), json!(1)));
        assert_eq!(params["suffix"], ".Reel1");
        let mut e = soundcraft_engine::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let end = e.session().content_end();
        e.execute("edit.select", &json!({"tracks": [kick.0], "start": 0, "end": end})).unwrap();
        e.execute(&cmd, &params).unwrap();
        let named: Vec<String> = e.session().track(kick).unwrap().clips().iter().map(|c| c.name.clone()).collect();
        assert!(named.len() > 1);
        assert_eq!(named[..2], ["SFX023.Reel1", "SFX024.Reel1"]);
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

    #[test]
    fn clip_loop_opens_clip_looping_for_the_selected_clip() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let mut d = Dialogs::default();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(!d.open_for_command(&e, "clip.loop"));
        let clip = e.session().track_by_name("Kick").unwrap().clips()[0].clone();
        e.execute("edit.select", &json!({"clips": [clip.id.0]})).unwrap();
        assert!(d.open_for_command(&e, "clip.loop"));
        assert_eq!(d.open_name(), Some("clip_looping"));
        let Some(Dialog::Loop { by, count, length, format, start, filled }) = &d.open else { panic!("{:?}", d.open) };
        assert_eq!((*by, *count, *start), (LoopBy::Count, 4, clip.start));
        assert_eq!(*format, e.session().edit.main_counter);
        // OK on the untouched field loops exactly four times, whatever its text rounds to.
        let sent = loop_params(e.session(), LoopBy::Length, *count, length, *format, *start, filled);
        assert_eq!(sent, Some(json!({"length": clip.length * 4})));
    }

    #[test]
    fn loop_length_reads_a_duration_in_every_time_format() {
        let e = soundcraft_engine::demo::demo_engine();
        let s = e.session();
        let start = s.sample_rate.samples(3.0);
        for format in soundcraft_time::TimeFormat::ALL {
            for len in [s.sample_rate.samples(2.0), s.sample_rate.samples(7.5)] {
                let text = soundcraft_time::format_length(start, len, format, s.sample_rate, &s.tempo, s.frame_rate);
                assert_eq!(loop_length(s, &text, format, start), Some(len), "{format:?} {text}");
            }
        }
        let bars = soundcraft_time::TimeFormat::BarsBeats;
        assert_eq!(loop_length(s, "1| 0| 000", bars, start), loop_length(s, "1", bars, start));
        assert_eq!(loop_length(s, "-1| 0| 000", bars, start), None);
        assert_eq!(loop_length(s, "two bars", bars, start), None);
    }

    #[test]
    fn clip_looping_sends_the_chosen_option() {
        let e = soundcraft_engine::demo::demo_engine();
        let s = e.session();
        let samples = soundcraft_time::TimeFormat::Samples;
        let filled = ("0:01.000".to_string(), 48_001);
        let send = |by, length: &str| loop_params(s, by, 3, length, samples, 0, &filled);
        assert_eq!(send(LoopBy::Count, ""), Some(json!({"count": 3})));
        assert_eq!(send(LoopBy::Length, "4800"), Some(json!({"length": 4800})));
        assert_eq!(send(LoopBy::Length, "0:01.000"), Some(json!({"length": 48_001})));
        assert_eq!(send(LoopBy::Length, "0"), None);
        assert_eq!(send(LoopBy::Length, "soon"), None);
        assert_eq!(send(LoopBy::Fill, "soon"), Some(json!({"fill": true})));
        assert!(soundcraft_engine::command_specs().iter().any(|c| c.id == "clip.loop"));
    }
}
