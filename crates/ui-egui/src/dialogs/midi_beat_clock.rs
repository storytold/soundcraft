use crate::localization::text;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct MidiBeatClock {
    pub(super) enabled: bool,
    pub(super) destination: String,
    pub(super) offset: i64,
    // Enumerate only on opening or Refresh. A missing saved port keeps its name.
    pub(super) outputs: Result<Vec<String>, String>,
}

impl MidiBeatClock {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub(super) fn new(session: &soundcraft_model::Session) -> Self {
        let clock = soundcraft_model::MidiBeatClock::from_session(session);
        Self {
            enabled: clock.enabled,
            destination: clock.destination.unwrap_or_default().to_owned(),
            offset: clock.offset,
            outputs: soundcraft_playback::midi_clock_outputs(),
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, enter: bool, error: Option<String>) -> Option<Value> {
        if let Some(error) = error {
            ui.label(crate::localization::error_text(&error));
        }
        crate::widgets::checkbox(ui, &mut self.enabled, text("Enable MIDI Beat Clock"));
        let selected = if self.destination.is_empty() { text("Choose a MIDI output") } else { &self.destination };
        craft_ui::ComboBox::from_label(text("MIDI output")).selected_text(selected).show_ui(ui, |ui| {
            if let Ok(outputs) = &self.outputs {
                for port in outputs {
                    ui.selectable_value(&mut self.destination, port.clone(), port);
                }
            }
        });
        if ui.button(text("Refresh devices")).clicked() {
            self.outputs = soundcraft_playback::midi_clock_outputs();
        }
        let available = self.outputs.as_ref().is_ok_and(|ports| ports.contains(&self.destination));
        match &self.outputs {
            Err(error) => {
                ui.label(crate::localization::error_text(error));
            }
            Ok(ports) if ports.is_empty() => {
                ui.label(text("No MIDI output devices found"));
            }
            Ok(_) if !self.destination.is_empty() && !available => {
                ui.label(text("The saved MIDI output is unavailable."));
            }
            _ => {}
        }
        ui.horizontal(|ui| {
            let label = ui.label(text("Offset (samples)"));
            ui.add(egui::DragValue::new(&mut self.offset).range(-10_000_000..=10_000_000)).labelled_by(label.id);
        });
        super::buttons_enabled(ui, "OK", enter, !self.enabled || available)
            .then(|| json!({"enabled": self.enabled, "destination": self.destination, "offset": self.offset}))
    }
}
