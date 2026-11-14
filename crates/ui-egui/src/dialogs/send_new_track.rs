//! The New Track dialog a send's "new track..." opens: one track, fed by the send through a new bus
//! of the same name (`mix.send_new_track`).

use egui::Ui;
use serde_json::{Value, json};
use soundcraft_model::{ChannelFormat, TrackId, TrackKind};

/// The kinds of track a send can feed: the ones with an audio input.
const KINDS: [TrackKind; 3] = [TrackKind::Aux, TrackKind::Audio, TrackKind::Instrument];

#[derive(Debug, Clone)]
/// The send the new track is for, and the new track's width, kind, timebase and name.
pub struct SendToNewTrack {
    pub track: TrackId,
    pub slot: usize,
    /// True when the slot had no send, so the new send's window opens once it is made.
    pub new_send: bool,
    pub format: String,
    pub kind: String,
    pub timebase: String,
    pub name: String,
    /// Create the track next to the one with the send.
    pub next_to_track: bool,
}

impl SendToNewTrack {
    /// A stereo Aux Input in samples, next to the track, as Pro Tools opens this dialog.
    pub fn new(track: TrackId, slot: usize, new_send: bool) -> Self {
        Self {
            track,
            slot,
            new_send,
            format: ChannelFormat::Stereo.label().into(),
            kind: TrackKind::Aux.id().into(),
            timebase: "samples".into(),
            name: TrackKind::Aux.default_name().into(),
            next_to_track: true,
        }
    }

    /// The `mix.send_new_track` parameters for what the dialog shows.
    pub fn params(&self) -> Value {
        json!({
            "track": self.track.0,
            "slot": self.slot,
            "kind": self.kind,
            "format": self.format,
            "timebase": self.timebase,
            "name": self.name,
            "next_to_track": self.next_to_track,
        })
    }

    /// Draws the row and the checkbox; returns the command's parameters when Create is pressed.
    pub(crate) fn show(&mut self, ui: &mut Ui, enter: bool) -> Option<Value> {
        ui.horizontal(|ui| {
            ui.label("Create 1 new");
            craft_ui::ComboBox::from_id_salt("snt_fmt")
                .selected_text(&self.format)
                .show_ui(ui, |ui| {
                    for f in ChannelFormat::ALL {
                        ui.selectable_value(&mut self.format, f.label().to_string(), f.label());
                    }
                })
                .response
                .accessible_name("Channel format");
            craft_ui::ComboBox::from_id_salt("snt_kind")
                .selected_text(TrackKind::from_id(&self.kind).map_or("Aux Input", |k| k.label()))
                .show_ui(ui, |ui| {
                    for k in KINDS {
                        if ui.selectable_label(self.kind == k.id(), k.label()).clicked() {
                            super::new_tracks::set_new_track_kind(&mut self.kind, &mut self.name, k);
                        }
                    }
                })
                .response
                .accessible_name("Track kind");
            ui.label("in");
            craft_ui::ComboBox::from_id_salt("snt_tb")
                .selected_text(if self.timebase == "ticks" { "Ticks" } else { "Samples" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.timebase, "samples".to_string(), "Samples");
                    ui.selectable_value(&mut self.timebase, "ticks".to_string(), "Ticks");
                })
                .response
                .accessible_name("Timebase");
        });
        ui.horizontal(|ui| {
            let label = ui.label("Name:");
            crate::widgets::named_text_field(ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(260.0)).labelled_by(label.id), "Name:");
        });
        crate::widgets::checkbox(ui, &mut self.next_to_track, "Create next to current track");
        super::buttons(ui, "Create", enter).then(|| self.params())
    }
}

#[cfg(test)]
mod tests;
