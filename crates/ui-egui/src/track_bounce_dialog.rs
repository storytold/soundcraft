//! Original Track Bounce dialog; source/range/revision snapshots stay attached to the preview.

use crate::{SoundApp, theme::Tokens};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::TrackId;
use soundcraft_time::Range;

#[derive(Debug, Clone)]
pub struct TrackBounceDialog {
    tracks: Vec<TrackId>,
    revision: u64,
    range: Option<Range>,
    destination: String,
    format: String,
    depth: String,
    volume: bool,
    pan: bool,
    dither: bool,
    tail: f64,
    preview: Option<Preview>,
}

#[derive(Debug, Clone)]
struct Preview {
    params: Value,
    revision: u64,
    result: Result<Value, String>,
}

impl TrackBounceDialog {
    pub(crate) fn new(e: &Engine, tracks: Vec<TrackId>, home: String) -> Self {
        let name = tracks.first().and_then(|id| e.session().track(*id)).map_or("Track", |t| t.name.as_str());
        let destination = if tracks.len() == 1 {
            std::path::Path::new(&home).join(format!("{} Bounce.wav", soundcraft_engine::io::sanitize_name(name))).to_string_lossy().into_owned()
        } else {
            home
        };
        let edit = &e.session().edit;
        let selection = if edit.link_timeline_edit { edit.selection } else { edit.timeline_selection };
        Self {
            tracks,
            revision: e.revision,
            range: (!selection.is_empty()).then_some(selection),
            destination,
            format: "wav".into(),
            depth: "24".into(),
            volume: true,
            pan: true,
            dither: true,
            tail: 0.0,
            preview: None,
        }
    }

    pub fn params(&self) -> Value {
        let mut p = json!({"tracks": self.tracks, "expected_revision": self.revision, "format": self.format, "bit_depth": self.depth,
            "render_volume": self.volume, "render_pan": self.pan, "dither": self.dither, "tail_seconds": self.tail});
        p[if self.tracks.len() == 1 { "path" } else { "dir" }] = json!(self.destination);
        if let Some(r) = self.range {
            p["start"] = json!(r.start);
            p["end"] = json!(r.end);
        }
        p
    }

    pub(crate) fn show(&mut self, app: &mut SoundApp, ui: &mut egui::Ui, enter: bool) -> Option<Value> {
        ui.set_max_width(560.0);
        ui.label(format!("{} source(s) · {} Hz · Offline", self.tracks.len(), app.engine.session().sample_rate.hz()));
        ui.label("Deliver Audio and built-in Instrument files. Source tracks stay in place.");
        ui.horizontal(|ui| {
            ui.label(if self.tracks.len() == 1 { "File" } else { "Folder" });
            ui.add(egui::TextEdit::singleline(&mut self.destination).desired_width(450.0));
        });
        let old_format = self.format.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Format").selected_text(self.format.to_uppercase()).show_ui(ui, |ui| {
                for f in ["wav", "aiff", "flac"] {
                    ui.selectable_value(&mut self.format, f.to_string(), f.to_uppercase());
                }
            });
            egui::ComboBox::from_label("Bit Depth").selected_text(self.depth.as_str()).show_ui(ui, |ui| {
                for b in ["16", "24", "32f"] {
                    ui.selectable_value(&mut self.depth, b.to_string(), b);
                }
            });
        });
        if self.format != old_format && self.tracks.len() == 1 {
            self.destination = std::path::Path::new(&self.destination)
                .with_extension(if self.format == "aiff" { "aif" } else { &self.format })
                .to_string_lossy()
                .into_owned();
        }
        ui.horizontal(|ui| {
            crate::widgets::checkbox(ui, &mut self.volume, "Render Volume");
            crate::widgets::checkbox(ui, &mut self.pan, "Render Pan");
            if self.depth == "32f" {
                ui.add_enabled(false, egui::Checkbox::new(&mut self.dither, "Integer dither"));
            } else {
                crate::widgets::checkbox(ui, &mut self.dither, "Integer dither");
            }
        });
        ui.label(match self.range {
            Some(r) => format!("Selection: {}–{} samples", r.start, r.end),
            None => "Range: session zero through each source's audio/note end".into(),
        });
        ui.horizontal(|ui| {
            ui.label("Extra tail");
            ui.add(egui::DragValue::new(&mut self.tail).range(0.0..=14_400.0).speed(0.1).suffix(" s"));
        });
        ui.label(egui::RichText::new("Tail continues track inserts and instrument release; clip-effect tails are excluded.").small());
        ui.separator();
        let params = self.params();
        if self.preview.as_ref().is_none_or(|p| p.params != params || p.revision != app.engine.revision) {
            let result = app.engine.execute("track.bounce_preview", &params).map_err(|err| err.to_string());
            self.preview = Some(Preview { params: params.clone(), revision: app.engine.revision, result });
        }
        let preview = self.preview.as_ref().map(|p| &p.result);
        let ready = preview.is_some_and(|v| v.as_ref().is_ok_and(|p| p["ready"] == true));
        match preview {
            Some(Ok(p)) => {
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    if let Some(files) = p["files"].as_array() {
                        for f in files {
                            ui.label(format!("{} · {} channel(s) · {} frames", f["name"].as_str().unwrap_or_default(), f["channels"], f["frames"]));
                            if f["preroll_frames"].as_u64().is_some_and(|n| n > 0) {
                                ui.label(
                                    egui::RichText::new(format!("Instrument warm-up: {} samples before this range", f["preroll_frames"])).small(),
                                );
                            }
                            ui.label(egui::RichText::new(f["path"].as_str().unwrap_or_default()).small().color(Tokens::current().text_dim));
                        }
                    }
                    if let Some(issues) = p["issues"].as_array() {
                        for issue in issues {
                            ui.colored_label(Tokens::current().text, issue["reason"].as_str().unwrap_or_default());
                        }
                    }
                });
            }
            Some(Err(err)) => {
                ui.colored_label(Tokens::current().text, err);
            }
            None => {}
        }
        ui.label(
            egui::RichText::new("Existing files are preserved. Aux/Master, hosted plugins and real-time sources are not supported here.").small(),
        );
        ui.separator();
        let mut bounce = false;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("dlg_cancel"), true));
            }
            bounce = ui.add_enabled(ready, egui::Button::new("Bounce").fill(Tokens::current().accent)).clicked() || (ready && enter);
        });
        bounce.then_some(params)
    }
}
