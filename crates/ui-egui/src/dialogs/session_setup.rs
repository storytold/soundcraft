use super::{buttons, external_offset_label};
use crate::{SoundApp, localization::text};
use egui::Ui;
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{BitDepthSetting, Session};
use soundcraft_time::{FrameRate, TimeFormat};

/// A pending edit. Incoming MIDI refreshes the readout without overwriting typed values.
#[derive(Debug, Clone)]
pub struct SessionSetup {
    pub(super) start: String,
    frame_rate: String,
    bit_depth: String,
    external: String,
    freewheel_frames: u8,
    freewheel: String,
    confirm_start: bool,
}

impl SessionSetup {
    pub(super) fn new(engine: &Engine) -> Self {
        let session = engine.session();
        Self {
            start: session.format_position(0, TimeFormat::Timecode),
            frame_rate: session.frame_rate.label().to_owned(),
            bit_depth: match session.bit_depth {
                BitDepthSetting::Int16 => "16",
                BitDepthSetting::Float32 => "32f",
                _ => "24",
            }
            .into(),
            external: external_offset_label(session),
            freewheel_frames: engine.mtc_freewheel_frames(),
            freewheel: engine.mtc_freewheel_mode().into(),
            confirm_start: false,
        }
    }

    fn params(&self, session: &Session, keep: &str) -> Value {
        let mut params = json!({
            "frame_rate": self.frame_rate, "bit_depth": self.bit_depth,
            "freewheel_frames": self.freewheel_frames, "freewheel": self.freewheel,
            "keep": keep,
        });
        if self.start != session.format_position(0, TimeFormat::Timecode) {
            params["start_timecode"] = json!(self.start);
        }
        // A sub-frame offset cannot be round-tripped through its display label.
        if self.external != external_offset_label(session) {
            params["external_offset_timecode"] = json!(self.external);
        }
        params
    }

    fn received_position(&mut self, app: &SoundApp, ui: &mut Ui) {
        let incoming = app.midi_inputs.as_ref().and_then(|m| m.feed().mtc_input_status().0).map(|p| p.label());
        if app.mtc_online {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        ui.horizontal(|ui| {
            ui.label(crate::localization::text("Received timecode"));
            ui.monospace(incoming.as_deref().unwrap_or(crate::localization::text("No timecode received")));
            if let Some(warning) = app.mtc_rate_warning() {
                ui.label(warning);
            }
            if ui.add_enabled(incoming.is_some(), egui::Button::new(crate::localization::text("Use received position"))).clicked()
                && let Some(label) = &incoming
            {
                self.start.clone_from(label);
            }
        });
        ui.horizontal(|ui| {
            let caption = ui.label(crate::localization::text("Session Start"));
            let id = ui.make_persistent_id("session_start_label");
            let capture = ui.memory(|m| m.has_focus(id))
                && ui.input_mut(|input| {
                    let pressed = input.consume_key(egui::Modifiers::NONE, egui::Key::Equals);
                    if pressed {
                        input.events.retain(|event| !matches!(event, egui::Event::Text(t) if t == "="));
                    }
                    pressed
                });
            ui.add(egui::TextEdit::singleline(&mut self.start).id(id)).labelled_by(caption.id);
            if capture && let Some(label) = incoming {
                self.start = label;
            }
        });
    }

    pub(super) fn show(&mut self, app: &SoundApp, ui: &mut Ui, enter: bool) -> Option<Value> {
        let session = app.engine.session();
        if self.confirm_start {
            ui.label(crate::localization::text("When changing the session start"));
            for (key, caption) in [("relative", "Keep clips' distance from the session start"), ("timecode", "Keep clips at their timecode")] {
                if ui.button(text(caption)).clicked() {
                    return Some(self.params(session, key));
                }
            }
            if ui.button(crate::localization::text("Cancel")).clicked() {
                ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::unique("dlg_cancel"), true));
            }
            return None;
        }
        self.received_position(app, ui);
        ui.horizontal(|ui| {
            let caption = ui.label(crate::localization::text("External Timecode Offset"));
            ui.text_edit_singleline(&mut self.external).labelled_by(caption.id);
        });
        ui.horizontal(|ui| {
            ui.label(crate::localization::text("Freewheel"));
            for (key, label) in [("none", "None"), ("frames", "Frames"), ("jam_sync", "Jam Sync")] {
                ui.radio_value(&mut self.freewheel, key.to_string(), text(label));
            }
            if self.freewheel == "frames" {
                let caption = ui.label(crate::localization::text("Freewheel (frames)"));
                ui.add(egui::DragValue::new(&mut self.freewheel_frames).range(4..=40)).labelled_by(caption.id);
            }
        });
        egui::ComboBox::from_label(crate::localization::text("Timecode Rate")).selected_text(text(&self.frame_rate)).show_ui(ui, |ui| {
            for rate in FrameRate::ALL {
                ui.selectable_value(&mut self.frame_rate, rate.label().to_owned(), text(rate.label()));
            }
        });
        egui::ComboBox::from_label(crate::localization::text("Bit Depth")).selected_text(text(&self.bit_depth)).show_ui(ui, |ui| {
            for value in ["16", "24", "32f"] {
                ui.selectable_value(&mut self.bit_depth, value.to_string(), text(value));
            }
        });
        if !buttons(ui, "OK", enter) {
            return None;
        }
        if !session.tracks.is_empty() && self.start != session.format_position(0, TimeFormat::Timecode) {
            self.confirm_start = true;
            return None;
        }
        Some(self.params(session, "relative"))
    }
}
