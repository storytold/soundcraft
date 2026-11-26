use crate::midi_sync::MidiSync;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct MidiBeatClock {
    pub(super) settings: MidiSync,
    pub(super) outputs: Result<Vec<String>, String>,
}

impl MidiBeatClock {
    pub(super) fn new(settings: &MidiSync) -> Self {
        let mut dialog = Self { settings: settings.clone(), outputs: Ok(Vec::new()) };
        dialog.refresh();
        dialog
    }

    fn refresh(&mut self) {
        self.outputs = soundcraft_playback::midi_sync_output_ids(&self.settings.port_ids).map(|found| {
            let labels = found.keys().cloned().collect();
            self.settings.port_ids.extend(found);
            labels
        });
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, enter: bool, app: &mut crate::SoundApp) -> Option<Value> {
        if let Some(error) = app.player.as_ref().and_then(soundcraft_playback::Player::midi_output_error) {
            ui.label(crate::localization::error_text(&error));
        }
        crate::widgets::checkbox(ui, &mut self.settings.enabled, crate::localization::text("Enable MIDI Beat Clock"));
        if ui.button(crate::localization::text("Refresh devices")).clicked() {
            self.refresh();
        }
        let available = self.outputs.as_ref().map(Vec::as_slice).unwrap_or_default();
        if let Err(error) = &self.outputs {
            ui.label(crate::localization::error_text(error));
        } else if available.is_empty() {
            ui.label(crate::localization::text("No MIDI output devices found"));
        }
        let names: BTreeSet<_> = available.iter().chain(self.settings.ports.keys()).cloned().collect();
        let rate = app.engine.session().sample_rate;
        let sr = rate.as_f64();
        let limit = soundcraft_playback::max_sync_offset_samples(rate);
        let timing = app.player.as_ref().map(soundcraft_playback::Player::midi_sync_status);
        egui::ScrollArea::vertical().max_height(240.).show(ui, |ui| {
            for name in names {
                let mut port = self.settings.ports.get(&name).copied().unwrap_or_default();
                let changed = ui
                    .horizontal(|ui| {
                        let enabled = crate::widgets::checkbox(ui, &mut port.enabled, name.as_str()).changed();
                        let label = ui.label(crate::localization::text("Offset (samples)"));
                        let offset = ui
                            .add(egui::DragValue::new(&mut port.offset_samples).range(-limit..=limit).clamp_existing_to_range(false))
                            .labelled_by(label.id)
                            .changed();
                        ui.label(format!("{:.2} ms", port.offset_samples as f64 * 1000. / sr));
                        if port.offset_samples.unsigned_abs() > limit as u64 {
                            ui.label(crate::localized_format!(
                                "Saved value exceeds ±250 ms at this rate; playback uses {effective} samples.",
                                effective = port.offset_samples.clamp(-limit, limit)
                            ));
                        }
                        if let Some(port_timing) = timing.as_ref().and_then(|status| status.ports.get(&name))
                            && let Some(lead_us) = port_timing.device_lead_us
                            && port_timing.advance_limited
                            && port_timing.requested_offset_samples == port.offset_samples
                        {
                            ui.label(crate::localized_format!("Limited by the audio output lead ({lead} ms)", lead = lead_us as f64 / 1000.));
                        }
                        if !available.contains(&name) {
                            ui.label(crate::localization::text("The saved MIDI output is unavailable."));
                        }
                        enabled || offset
                    })
                    .inner;
                if changed {
                    self.settings.ports.insert(name, port);
                }
            }
        });
        if ui.button(crate::localization::text("Copy diagnostics")).clicked()
            && let Ok(status) = app.sync_command("setup.midi_sync_status", &json!({}))
        {
            ui.ctx().copy_text(status.to_string());
        }
        super::buttons_enabled(ui, "OK", enter, !self.settings.enabled || self.settings.ports.values().any(|p| p.enabled)).then(|| {
            self.settings.port_ids.retain(|label, _| self.settings.ports.contains_key(label));
            json!({"enabled": self.settings.enabled, "ports": self.settings.ports, "port_ids": self.settings.port_ids})
        })
    }
}
