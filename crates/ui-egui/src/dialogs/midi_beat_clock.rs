use crate::{localization::text, midi_sync::MidiSync};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct MidiBeatClock {
    pub(super) settings: MidiSync,
    pub(super) outputs: Result<Vec<String>, String>,
}

impl MidiBeatClock {
    pub(super) fn new(settings: &MidiSync) -> Self {
        Self { settings: settings.clone(), outputs: soundcraft_playback::midi_sync_outputs() }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, enter: bool, app: &mut crate::SoundApp) -> Option<Value> {
        if let Some(error) = app.player.as_ref().and_then(soundcraft_playback::Player::midi_clock_error) {
            ui.label(crate::localization::error_text(&error));
        }
        crate::widgets::checkbox(ui, &mut self.settings.enabled, text("Enable MIDI Beat Clock"));
        if ui.button(text("Refresh devices")).clicked() {
            self.outputs = soundcraft_playback::midi_sync_outputs();
        }
        let available = self.outputs.as_ref().map(Vec::as_slice).unwrap_or_default();
        if let Err(error) = &self.outputs {
            ui.label(crate::localization::error_text(error));
        } else if available.is_empty() {
            ui.label(text("No MIDI output devices found"));
        }
        let saved_outputs = self.settings.ports.iter().filter_map(|(name, port)| {
            (port.enabled || port.out_offset_samples != 0 || app.ui.mtc_output_port.as_ref() == Some(name)).then_some(name)
        });
        let names: BTreeSet<_> = available.iter().chain(saved_outputs).cloned().collect();
        let rate = app.engine.session().sample_rate;
        let timing = app.player.as_ref().map(soundcraft_playback::Player::midi_sync_status);
        egui::ScrollArea::vertical().max_height(240.).show(ui, |ui| {
            for name in names {
                let mut port = self.settings.ports.get(&name).copied().unwrap_or_default();
                let changed = ui
                    .horizontal(|ui| {
                        let enabled = crate::widgets::checkbox(ui, &mut port.enabled, name.as_str()).changed();
                        let limited_lead = timing
                            .as_ref()
                            .and_then(|status| status.ports.get(&name))
                            .filter(|p| p.advance_limited && p.requested_offset_samples == port.out_offset_samples)
                            .and_then(|p| p.device_lead_us);
                        let offset = super::port_offset_field(ui, &mut port.out_offset_samples, rate, limited_lead);
                        if !available.contains(&name) {
                            ui.label(text("The saved MIDI output is unavailable."));
                        }
                        enabled || offset
                    })
                    .inner;
                if changed {
                    self.settings.ports.insert(name, port);
                }
            }
        });
        if ui.button(text("Copy diagnostics")).clicked()
            && let Ok(status) = app.sync_command("setup.midi_sync_status", &json!({}))
        {
            ui.ctx().copy_text(status.to_string());
        }
        super::buttons_enabled(ui, "OK", enter, !self.settings.enabled || self.settings.ports.values().any(|p| p.enabled))
            .then(|| json!({"enabled":self.settings.enabled,"ports":crate::midi_sync::beat_destinations(&self.settings.ports)}))
    }
}
