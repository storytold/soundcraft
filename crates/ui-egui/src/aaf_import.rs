//! Session-data import UI. File parsing goes through engine commands; successful insertion is
//! a single undoable command. Nothing in this window replaces or saves the active document.
use crate::SoundApp;
use egui::{RichText, ScrollArea};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver};

pub struct ImportState {
    pub open: bool,
    pub path: String,
    pub media_dir: String,
    /// CompositionMob to read when the AAF holds several; empty lets the reader require one.
    composition: String,
    /// (name, MobID) choices reported by the reader for a multi-composition AAF.
    compositions: Vec<(String, String)>,
    preview: Option<Value>,
    selected: Vec<bool>,
    filter: String,
    absolute: bool,
    offset: String,
    error: Option<String>,
    busy: Option<Receiver<Result<Value, String>>>,
    pending: bool,
    report: Option<Value>,
}
impl Default for ImportState {
    fn default() -> Self {
        Self {
            open: false,
            path: String::new(),
            media_dir: String::new(),
            composition: String::new(),
            compositions: Vec::new(),
            preview: None,
            selected: Vec::new(),
            filter: String::new(),
            absolute: true,
            offset: "0".into(),
            error: None,
            busy: None,
            pending: false,
            report: None,
        }
    }
}
impl ImportState {
    pub fn set_path(&mut self, path: String) {
        self.media_dir = std::path::Path::new(&path).parent().map_or(String::new(), |p| p.display().to_string());
        self.path = path;
        self.composition.clear();
        self.compositions.clear();
        self.invalidate();
    }
    /// A typed path is a different file: forget its compositions, and follow it with the media
    /// folder unless the user pointed that folder somewhere other than the previous AAF's folder.
    fn path_edited(&mut self, previous: &str) {
        let parent = |p: &str| std::path::Path::new(p).parent().map_or(String::new(), |d| d.display().to_string());
        if self.media_dir == parent(previous) {
            self.media_dir = parent(&self.path);
        }
        self.composition.clear();
        self.compositions.clear();
        self.invalidate();
    }
    /// Drops the current preview and any analysis still running, whose plan would belong to the
    /// previous file or folder. The worker's late result goes to a closed channel.
    fn invalidate(&mut self) {
        self.preview = None;
        self.busy = None;
        self.error = None;
    }
    pub fn inspect(&self) -> Value {
        json!({"open":self.open,"path":self.path,"media_dir":self.media_dir,"busy":self.busy.is_some(),"preview":self.preview,"selected":self.selected,"error":self.error,"report":self.report})
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn analyze(&mut self, ctx: &egui::Context) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.preview = None;
        let path = self.path.clone();
        let media = self.media_dir.clone();
        let mut params = json!({"path":path,"media_dir":media});
        if !self.composition.is_empty() {
            params["composition"] = json!(self.composition);
        }
        let roots = soundcraft_engine::file_access::current();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.busy = Some(rx);
        std::thread::spawn(move || {
            let result = soundcraft_engine::file_access::confine(roots.as_ref(), || {
                soundcraft_engine::Engine::default().execute("file.preview_aaf", &params).map_err(|e| e.to_string())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    fn analyze(&mut self, _: &egui::Context) {
        self.error = Some("AAF import needs the desktop app: its media files are read from a local folder".into());
    }
}

pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    if let Some(path) = p.get("path").and_then(Value::as_str) {
        app.aaf_import.set_path(path.to_string());
    }
    if let Some(dir) = p.get("media_dir").and_then(Value::as_str) {
        app.aaf_import.media_dir = dir.to_string();
        app.aaf_import.invalidate();
    }
    app.aaf_import.open = true;
    Ok(app.aaf_import.inspect())
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut state = std::mem::take(&mut app.aaf_import);
    if let Some(rx) = &state.busy {
        let result = rx.try_recv();
        match result {
            Ok(Ok(preview)) => {
                state.selected = vec![true; preview.get("tracks").and_then(Value::as_array).map_or(0, Vec::len)];
                state.preview = Some(preview);
                state.busy = None;
            }
            Ok(Err(e)) => {
                let choices = composition_choices(&e);
                if !choices.is_empty() {
                    state.compositions = choices;
                }
                state.error = Some(e);
                state.busy = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.error = Some("AAF reader stopped unexpectedly".into());
                state.busy = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }
    if state.pending {
        state.pending = false;
        let indices: Vec<_> = state.selected.iter().enumerate().filter_map(|(i, on)| on.then_some(i)).collect();
        let params = json!({"plan":state.preview.as_ref().and_then(|v|v.get("plan")),"selected":indices,"media":"copy","mapping":if state.absolute{"absolute"}else{"relative"},"offset_samples":state.offset.parse::<i64>().ok()});
        match app.run("file.import_aaf", params) {
            Ok(v) => {
                state.report = Some(v);
                state.open = false;
                app.ui.status = "AAF imported as new editable tracks. Save the session to collect copied media.".into();
            }
            Err(e) => state.error = Some(e),
        }
    }
    if state.open {
        let mut open = true;
        egui::Window::new("Import Session Data — AAF").id(egui::Id::unique("import_aaf")).open(&mut open).default_size([1080.0,690.0]).min_size([850.0,560.0]).show(ctx,|ui| {
            ui.horizontal(|ui| {
                ui.label("AAF file");
                let before=state.path.clone();
                let edited=ui.add_enabled(state.busy.is_none(),egui::TextEdit::singleline(&mut state.path).desired_width((ui.available_width()-200.0).max(200.0))).changed();
                if edited { state.path_edited(&before); }
                if ui.add_enabled(state.busy.is_none(),egui::Button::new("Browse…")).clicked()
                    && let Some(path)=app.services.pick_open.as_ref().and_then(|pick|pick("file.import_aaf",&["aaf"])) {
                    match crate::dialogs::unicode(&path) { Ok(p)=>state.set_path(p.to_string()), Err(e)=>state.error=Some(e.to_string()) }
                }
                if ui.add_enabled(state.busy.is_none()&&!state.path.is_empty()&&!state.media_dir.is_empty(),egui::Button::new("Analyze AAF")).clicked() { state.analyze(ctx); }
            });
            if state.busy.is_some() { ui.horizontal(|ui|{ui.spinner();ui.label("Reading source properties and locating media…");}); }
            if !state.compositions.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Composition");
                    let current=state.compositions.iter().find(|(_,id)|*id==state.composition).map_or("Choose one…".to_string(),|(n,_)|n.clone());
                    // Locked while analyzing, so a finished analysis always matches the selection.
                    ui.add_enabled_ui(state.busy.is_none(),|ui|{
                        craft_ui::ComboBox::from_id_salt("aaf_composition").selected_text(current).show_ui(ui,|ui|{
                            for (name,id) in &state.compositions {
                                if ui.selectable_label(state.composition==*id,format!("{name}  ({id})")).clicked() { state.composition=id.clone(); state.preview=None; }
                            }
                        });
                    });
                });
            }
            if let Some(error)=&state.error { ui.colored_label(ui.visuals().error_fg_color,error); }
            ui.separator();
            let preview=state.preview.clone();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    ui.group(|ui| {
                        ui.label(RichText::new("Source Properties").strong());
                        if let Some(p)=&preview {
                            ui.label(format!("Name: {}",p["name"].as_str().unwrap_or("")));
                            ui.label("Type: AAF • external PCM WAV");
                            ui.label(format!("Start: {}",p["start_timecode"].as_str().unwrap_or("")));
                            ui.label(format!("Frame rate: {} fps",p["frame_rate"].as_str().unwrap_or("")));
                            ui.label(format!("Sample rate: {} Hz",p["sample_rate"]));
                            ui.label(format!("Source files: {}",p["sources"]));
                        } else { ui.label("Choose an AAF and analyze it to preview the montage."); }
                    });
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.label(RichText::new("Media Options").strong());
                        ui.label("Locate source WAV files in:");
                        if ui.add_enabled(state.busy.is_none(),egui::TextEdit::singleline(&mut state.media_dir).desired_width(275.0)).changed(){state.preview=None;}
                        if ui.add_enabled(state.busy.is_none(),egui::Button::new("Choose media folder…")).clicked()
                            && let Some(dir)=app.services.pick_folder.as_ref().and_then(|pick|pick(std::path::Path::new(&state.media_dir))){
                            match crate::dialogs::unicode(&dir) { Ok(d)=>{state.media_dir=d.to_string();state.invalidate();} Err(e)=>state.error=Some(e.to_string()) }
                        }
                        ui.label("Copy full sources when saving");
                        ui.small("Save the session before moving or removing original files.");
                        ui.small("All available handles are retained. Sources are not consolidated or trimmed.");
                    });
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.label(RichText::new("Timecode Mapping").strong());
                        ui.radio_value(&mut state.absolute,true,"Maintain absolute timecode");
                        ui.radio_value(&mut state.absolute,false,"Relative to session start");
                        ui.horizontal(|ui|{ui.label("Offset (samples)");ui.add(egui::TextEdit::singleline(&mut state.offset).desired_width(110.0));});
                        ui.small(format!("Destination: {} Hz • {} fps",app.engine.session().sample_rate.hz(),app.engine.session().frame_rate.label()));
                        ui.small("An empty destination adopts the source rate and timecode. No sample-rate conversion.");
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new("Initial import scope").strong());
                    ui.small("New tracks and editable clips. Clip gain, automation, parametric fades, video, plugins and routing are not transferred. Existing playlists are not replaced.");
                });
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_min_width(450.0);
                    ui.horizontal(|ui|{
                        ui.label("Tracks");ui.text_edit_singleline(&mut state.filter);
                        if ui.button("All").clicked(){state.selected.fill(true);}
                        if ui.button("Clear").clicked(){state.selected.fill(false);}
                    });
                    ui.separator();
                    ScrollArea::vertical().id_salt("aaf_tracks").max_height(400.0).show(ui,|ui| {
                        egui::Grid::new("aaf_track_table").striped(true).min_col_width(120.0).show(ui,|ui|{
                            ui.strong("Import / Source");ui.strong("Clips");ui.strong("Destination");ui.end_row();
                            if let Some(tracks)=preview.as_ref().and_then(|p|p["tracks"].as_array()) {
                                for (i,t) in tracks.iter().enumerate() {
                                    let name=t["name"].as_str().unwrap_or("");
                                    if !name.to_lowercase().contains(&state.filter.to_lowercase()){continue;}
                                    let label=format!("{} ({})",name,if t["channels"].as_u64()==Some(2){"Stereo"}else{"Mono"});
                                    if let Some(on)=state.selected.get_mut(i){ui.checkbox(on,label);}else{ui.label(label);}
                                    ui.label(t["clips"].to_string());ui.label("New Track");ui.end_row();
                                }
                            }
                        });
                    });
                    ui.separator();
                    let warnings=preview.as_ref().and_then(|p|p["warnings"].as_array());
                    ui.collapsing(format!("Compatibility and media report ({} notices)",warnings.map_or(0,Vec::len)),|ui|{
                        ScrollArea::vertical().id_salt("aaf_warnings").max_height(130.0).show(ui,|ui|{
                            if let Some(warnings)=warnings {for w in warnings{ui.small(w.as_str().unwrap_or(""));}}
                        });
                    });
                    ui.label(format!("{} tracks selected",state.selected.iter().filter(|x|**x).count()));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked(){state.open=false;}
                let ready=state.preview.is_some() && state.busy.is_none() && state.selected.iter().any(|x|*x) && state.offset.parse::<i64>().is_ok() && !app.engine.transport.playing && !app.engine.transport.recording;
                if ui.add_enabled(ready,egui::Button::new("Import")).clicked(){state.pending=true;ctx.request_repaint();}
                if app.engine.transport.playing || app.engine.transport.recording {ui.label("Stop transport before importing.");}
                if state.pending {ui.spinner();ui.label("Preparing media and new tracks…");}
            });
        });
        if !open {
            state.open = false;
        }
    }
    if let Some(report) = &state.report {
        let mut open = true;
        egui::Window::new("AAF Import Report").id(egui::Id::unique("aaf_import_report")).open(&mut open).default_size([640.0, 400.0]).show(
            ctx,
            |ui| {
                ui.heading("Session data imported");
                ui.label(format!("{} new tracks • {} complete sources", report["tracks"].as_array().map_or(0, Vec::len), report["sources"]));
                ui.label("Clips remain editable. Use Save Session to collect copied media.");
                if ui.button("Go to first imported clip").clicked()
                    && let Some(at) = report["first_clip_sample"].as_i64()
                {
                    let _ = app.run("transport.locate", json!({"at":at}));
                }
                if ui.button("Copy complete report").clicked() {
                    ctx.copy_text(serde_json::to_string_pretty(report).unwrap_or_default());
                }
                ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    if let Some(warnings) = report["warnings"].as_array() {
                        for w in warnings {
                            ui.label(w.as_str().unwrap_or(""));
                        }
                    }
                });
            },
        );
        if !open {
            state.report = None;
        }
    }
    app.aaf_import = state;
}

/// (name, MobID) pairs from the reader's "Choose exactly one composition" error, so the window
/// can offer a choice instead of asking for an ID it gives no way to enter.
fn composition_choices(error: &str) -> Vec<(String, String)> {
    let Some(list) = error.split_once("Choose exactly one composition by MobID: ").map(|(_, l)| l) else { return Vec::new() };
    list.split("(\"")
        .filter_map(|entry| {
            let (name, rest) = entry.split_once("\", \"")?;
            let id = rest.split('"').next()?;
            id.starts_with("urn:smpte:umid:").then(|| (name.to_string(), id.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn several_compositions_become_choices() {
        let e = r#"io: interchange: Choose exactly one composition by MobID: [("Reel 1", "urn:smpte:umid:060a2b34.01"), ("Alt cut", "urn:smpte:umid:060a2b34.02")]"#;
        assert_eq!(
            composition_choices(e),
            [("Reel 1".to_string(), "urn:smpte:umid:060a2b34.01".to_string()), ("Alt cut".to_string(), "urn:smpte:umid:060a2b34.02".to_string())]
        );
        assert!(composition_choices("Missing media: x.wav").is_empty());
    }
    #[test]
    fn a_new_file_discards_the_analysis_still_running_for_the_previous_one() {
        let mut state = ImportState::default();
        let (tx, rx) = mpsc::channel();
        state.set_path("/a/first.aaf".into());
        state.busy = Some(rx);
        state.set_path("/b/second.aaf".into());
        assert!(state.busy.is_none());
        assert!(tx.send(Ok(json!({"plan": {}}))).is_err());
        assert!(state.preview.is_none());
        assert_eq!(state.media_dir, "/b");
    }
    #[test]
    fn a_typed_path_moves_the_derived_media_folder_but_keeps_a_chosen_one() {
        let mut state = ImportState::default();
        state.set_path("/a/first.aaf".into());
        state.composition = "urn:smpte:umid:01".into();
        state.path = "/b/second.aaf".into();
        state.path_edited("/a/first.aaf");
        assert_eq!(state.media_dir, "/b");
        assert!(state.composition.is_empty());
        state.media_dir = "/media/elsewhere".into();
        state.path = "/c/third.aaf".into();
        state.path_edited("/b/second.aaf");
        assert_eq!(state.media_dir, "/media/elsewhere");
    }
    #[test]
    fn menu_opens_preview_but_programmatic_import_does_not_open_a_dialog() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        assert!(app.run("file.import_aaf", json!({})).is_err());
        assert!(!app.aaf_import.open);
        crate::menus::invoke_menu(&mut app, "file.import_aaf", "File > Import > Session Data...");
        assert!(app.aaf_import.open);
        assert!(app.engine.session().tracks.is_empty());
    }
}
