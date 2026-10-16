//! Importing audio the Pro Tools way: File › Import › Audio picks any number of files and asks
//! where they go (new tracks, the selected track, or the Clip List only) and from where (session
//! start or the insertion point); files dropped on the Edit window land where they are dropped.

use crate::SoundApp;
use crate::edit_window::{rect, sample_at};
use crate::theme::Tokens;
use egui::{Rect, Stroke, StrokeKind, pos2};
use serde_json::{Value, json};
use soundcraft_model::{Session, TrackId, TrackKind};
use soundcraft_time::Samples;

/// Extensions the audio import reads (also the desktop file dialog's filter).
pub const AUDIO_EXTENSIONS: &[&str] =
    &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "m4b", "aac", "mp4", "caf", "mkv", "webm"];

/// Whether a file name has an extension the audio import reads.
pub fn is_importable_audio(name: &str) -> bool {
    std::path::Path::new(name).extension().and_then(|x| x.to_str()).is_some_and(|x| AUDIO_EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Where imported files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// One new track per file.
    NewTracks,
    /// The selected audio track, files one after another.
    SelectedTrack,
    /// The Clip List only: no tracks, no clips.
    ClipList,
}

/// From where imported clips start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    SessionStart,
    /// The edit selection's start (the insertion point).
    Selection,
}

/// The File › Import › Audio dialog's state.
#[derive(Debug, Clone)]
pub struct ImportAudio {
    pub files: Vec<String>,
    /// A path being typed (hosts without a file dialog, such as the browser and tests).
    pub typed: String,
    pub destination: Destination,
    pub location: Location,
}

impl Default for ImportAudio {
    fn default() -> Self {
        ImportAudio { files: Vec::new(), typed: String::new(), destination: Destination::NewTracks, location: Location::Selection }
    }
}

impl ImportAudio {
    /// Adds files, skipping blanks and ones already listed.
    pub fn add(&mut self, paths: impl IntoIterator<Item = String>) {
        for p in paths.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
            if !self.files.contains(&p) {
                self.files.push(p);
            }
        }
    }

    /// `file.import_audio` parameters for this choice.
    pub fn params(&self, s: &Session) -> Value {
        let at = match self.location {
            Location::SessionStart => 0,
            Location::Selection => s.edit.selection.start.max(0),
        };
        json!({
            "paths": self.files,
            "at": at,
            "new_tracks": self.destination == Destination::NewTracks,
            "clip_list": self.destination == Destination::ClipList,
        })
    }
}

/// Whether the session has a selected audio track to import onto.
fn has_selected_audio_track(s: &Session) -> bool {
    s.edit.selected_tracks.iter().any(|id| s.track(*id).is_some_and(|t| t.kind == TrackKind::Audio))
}

/// The dialog's body; `Some(params)` when Import is pressed.
pub fn body(app: &SoundApp, ui: &mut egui::Ui, d: &mut ImportAudio, enter: bool) -> Option<Value> {
    let t = Tokens::current();
    ui.set_min_width(460.0);
    // Options on one line each: a grid remembers its column widths, and wrapping text would
    // keep it narrow.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    ui.label(egui::RichText::new(format!("Files to import ({})", d.files.len())).strong());
    let mut remove = None;
    egui::ScrollArea::vertical().max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
        if d.files.is_empty() {
            ui.label(egui::RichText::new("None yet: add files below, or drop them on the Edit window.").color(t.text_dim));
        }
        for (i, f) in d.files.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text("Remove from the list").clicked() {
                    remove = Some(i);
                }
                let name = std::path::Path::new(f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().into_owned());
                ui.label(name).on_hover_text(f.as_str());
            });
        }
    });
    if let Some(i) = remove {
        d.files.remove(i);
    }
    let mut typed_enter = false;
    ui.horizontal(|ui| {
        if let Some(pick) = &app.services.pick_open_many
            && ui.button("Add Files…").clicked()
        {
            d.add(pick("file.import_audio", &[]));
        }
        let field = ui.add(egui::TextEdit::singleline(&mut d.typed).hint_text("or type a file path").desired_width(260.0));
        typed_enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !d.typed.trim().is_empty();
        if (ui.add_enabled(!d.typed.trim().is_empty(), egui::Button::new("Add")).clicked() || typed_enter) && !d.typed.trim().is_empty() {
            let p = std::mem::take(&mut d.typed);
            d.add([p]);
        }
    });
    ui.separator();
    let s = app.engine.session();
    egui::Grid::new("import_audio_opts").num_columns(2).spacing(egui::vec2(16.0, 6.0)).show(ui, |ui| {
        ui.label("Destination");
        ui.vertical(|ui| {
            ui.radio_value(&mut d.destination, Destination::NewTracks, "New tracks (one per file)");
            let selected = has_selected_audio_track(s);
            ui.add_enabled_ui(selected, |ui| ui.radio_value(&mut d.destination, Destination::SelectedTrack, "Selected track (one after another)"))
                .response
                .on_disabled_hover_text("Select an audio track first");
            ui.radio_value(&mut d.destination, Destination::ClipList, "Clip List only");
            if !selected && d.destination == Destination::SelectedTrack {
                d.destination = Destination::NewTracks;
            }
        });
        ui.end_row();
        ui.label("Location");
        ui.add_enabled_ui(d.destination != Destination::ClipList, |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut d.location, Location::SessionStart, "Session start");
                ui.radio_value(&mut d.location, Location::Selection, "Insertion point");
            });
        });
        ui.end_row();
    });
    // Enter that added a typed path does not also import.
    let go = crate::dialogs::buttons_enabled(ui, "Import", enter && !typed_enter, !d.files.is_empty());
    go.then(|| d.params(s))
}

/// Where files dropped at a point of the Edit window go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// Onto this audio track from `at`, one after another.
    Track(TrackId, Samples),
    /// New tracks, each starting at `at`.
    NewTracks(Samples),
    /// The Clip List only.
    ClipList,
}

impl DropTarget {
    pub fn params(self, paths: &[String]) -> Value {
        match self {
            DropTarget::Track(t, at) => json!({"paths": paths, "track": t.0, "at": at, "sequential": true}),
            DropTarget::NewTracks(at) => json!({"paths": paths, "at": at, "new_tracks": true}),
            DropTarget::ClipList => json!({"paths": paths, "clip_list": true}),
        }
    }

    /// What dropping here does, for the hint shown while files are dragged over the window.
    pub fn describe(self, s: &Session, n: usize) -> String {
        let files = if n == 1 { "1 file".to_string() } else { format!("{n} files") };
        let when = |at: Samples| s.format_position(at, s.edit.main_counter);
        match self {
            DropTarget::Track(t, at) => format!("Import {files} onto {} at {}", s.track(t).map_or("the track", |x| x.name.as_str()), when(at)),
            DropTarget::NewTracks(at) => format!("Import {files} to new tracks at {}", when(at)),
            DropTarget::ClipList => format!("Add {files} to the Clip List"),
        }
    }
}

/// The drop target under `pos`, like Pro Tools: an audio track's row takes the files from the
/// time under the pointer, the Clip List keeps them there only, and anywhere else in the Edit
/// window makes new tracks (from the time under the pointer over the timeline, else from the
/// insertion point). `None` outside the Edit window.
pub fn drop_target(app: &SoundApp, pos: Option<egui::Pos2>) -> Option<DropTarget> {
    let s = app.engine.session();
    let insertion = s.edit.selection.start.max(0);
    let Some(p) = pos else { return Some(DropTarget::NewTracks(insertion)) };
    if app.sample_drop_targets.iter().any(|(r, _)| r.contains(p)) {
        // A Sampler drop target: the sampler takes the first file and outlines itself.
        return None;
    }
    if app.clip_list_rect.is_some_and(|r| r.contains(p)) {
        return Some(DropTarget::ClipList);
    }
    if app.ui.window != crate::MainWindow::Edit {
        return None;
    }
    let tl = rect(app.edit_layout.timeline);
    let time = |x: f32| crate::edit_window::snap_drop(s, sample_at(s, tl, x.max(tl.min.x)).max(0));
    if p.y >= tl.min.y && p.y <= tl.max.y {
        let row = app.edit_layout.rows.iter().find(|(_, r)| rect(*r).contains(p));
        if let Some((id, _)) = row {
            let t = TrackId(*id);
            let audio = s.track(t).is_some_and(|x| x.kind == TrackKind::Audio);
            let at = if p.x >= tl.min.x { time(p.x) } else { insertion };
            return Some(if audio { DropTarget::Track(t, at) } else { DropTarget::NewTracks(at) });
        }
        if p.x >= tl.min.x && p.x <= tl.max.x {
            return Some(DropTarget::NewTracks(time(p.x)));
        }
    }
    Some(DropTarget::NewTracks(insertion))
}

/// Outlines the drop target and says what a drop would do, while files are dragged over the
/// window.
pub fn paint_drop_hint(app: &SoundApp, ctx: &egui::Context) {
    let (hovered, pos) = ctx.input(|i| (i.raw.hovered_files.len(), i.pointer.latest_pos()));
    if hovered == 0 {
        return;
    }
    let Some(target) = drop_target(app, pos) else { return };
    let s = app.engine.session();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("import_drop_hint")));
    let accent = Tokens::current().accent;
    let area = match target {
        DropTarget::Track(t, _) => app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).map(|(_, r)| rect(*r)),
        DropTarget::NewTracks(_) => Some(rect(app.edit_layout.timeline)).filter(|r| r.width() > 0.0),
        DropTarget::ClipList => app.clip_list_rect,
    };
    if let Some(r) = area {
        painter.rect_stroke(r, 2.0, Stroke::new(2.0, accent), StrokeKind::Inside);
    }
    if let (DropTarget::Track(_, at) | DropTarget::NewTracks(at), Some(_)) = (target, area) {
        let tl = rect(app.edit_layout.timeline);
        let x = crate::edit_window::x_of(s, tl, at);
        if x >= tl.min.x && x <= tl.max.x {
            painter.vline(x, tl.y_range(), Stroke::new(1.5, accent));
        }
    }
    let at = pos.unwrap_or_else(|| ctx.content_rect().center()) + egui::vec2(16.0, 16.0);
    let galley = painter.layout_no_wrap(target.describe(s, hovered), egui::FontId::proportional(13.0), egui::Color32::WHITE);
    let bg = Rect::from_min_size(pos2(at.x, at.y), galley.size()).expand(5.0);
    painter.rect_filled(bg, 4.0, egui::Color32::from_black_alpha(210));
    painter.galley(pos2(at.x, at.y), galley, egui::Color32::WHITE);
}

impl SoundApp {
    /// Imports audio files dropped on the window where they were dropped (see [`drop_target`]),
    /// in one step. Returns the files it did not take: other kinds, and files without a full
    /// path (the browser reads those itself).
    pub(crate) fn import_dropped_audio(&mut self, files: Vec<egui::DroppedFileHandle>, pos: Option<egui::Pos2>) -> Vec<egui::DroppedFileHandle> {
        let (audio, rest): (Vec<_>, Vec<_>) =
            files.into_iter().partition(|f| f.path().is_absolute() && is_importable_audio(&f.path().to_string_lossy()));
        if audio.is_empty() {
            return rest;
        }
        // Outside the Edit window (the Mix, a Sampler window's extra files): new tracks at the
        // insertion point, as dropped files always did.
        let target = drop_target(self, pos).unwrap_or(DropTarget::NewTracks(self.engine.session().edit.selection.start.max(0)));
        let paths: Vec<String> = audio.iter().map(|f| f.path().to_string_lossy().into_owned()).collect();
        let done = target.describe(self.engine.session(), paths.len());
        self.ui.status = match self.run("file.import_audio", target.params(&paths)) {
            Ok(_) => done.replacen("Import ", "Imported ", 1).replacen("Add ", "Added ", 1),
            Err(e) => e,
        };
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_choices_become_import_parameters() {
        let mut s = Session::default();
        s.edit.selection.start = 4_800;
        let mut d = ImportAudio::default();
        d.add(["a.wav".to_string(), " ".to_string(), "b.wav".to_string(), "a.wav".to_string()]);
        assert_eq!(d.files, ["a.wav", "b.wav"], "blanks and duplicates are skipped");
        let p = d.params(&s);
        assert_eq!((p["at"].as_i64(), p["new_tracks"].as_bool(), p["clip_list"].as_bool()), (Some(4_800), Some(true), Some(false)));
        d.location = Location::SessionStart;
        d.destination = Destination::SelectedTrack;
        let p = d.params(&s);
        assert_eq!((p["at"].as_i64(), p["new_tracks"].as_bool()), (Some(0), Some(false)));
        d.destination = Destination::ClipList;
        assert_eq!(d.params(&s)["clip_list"].as_bool(), Some(true));
    }

    #[test]
    fn drop_targets_become_import_parameters() {
        let paths = vec!["a.wav".to_string()];
        let p = DropTarget::Track(TrackId(7), 100).params(&paths);
        assert_eq!((p["track"].as_u64(), p["at"].as_i64(), p["sequential"].as_bool()), (Some(7), Some(100), Some(true)));
        assert_eq!(DropTarget::NewTracks(5).params(&paths)["new_tracks"].as_bool(), Some(true));
        assert_eq!(DropTarget::ClipList.params(&paths)["clip_list"].as_bool(), Some(true));
    }
}
