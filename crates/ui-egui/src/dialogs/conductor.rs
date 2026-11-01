//! Conductor event forms shared by ruler buttons and event menus.
use serde_json::{Value, json};
use soundcraft_time::Samples;
/// A modal edit of one musical event. Changes are submitted atomically on OK.
#[derive(Debug, Clone)]
pub struct Conductor {
    pub kind: String,
    pub id: Option<u64>,
    pub original: Samples,
    pub at: Samples,
    pub key: String,
    pub root: String,
    pub quality: String,
    pub bass: String,
    pub numerator: u32,
    pub denominator: u32,
}
impl Conductor {
    pub fn new(s: &soundcraft_model::Session, kind: &str, at: Samples, id: Option<u64>) -> Self {
        let meter = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(at, s.sample_rate));
        let key = s.key_signatures.iter().rev().find(|(t, _)| *t <= at).map_or("C major", |(_, k)| k.as_str()).to_string();
        let chord = id.and_then(|id| s.chords.iter().find(|c| c.id.0 == id));
        Self {
            kind: kind.into(),
            id,
            original: at,
            at,
            key,
            root: chord.map_or("C", |c| c.symbol.root.as_str()).into(),
            quality: chord.map_or("", |c| c.symbol.quality.as_str()).into(),
            bass: chord.and_then(|c| c.symbol.bass.as_deref()).unwrap_or("").into(),
            numerator: meter.numerator,
            denominator: meter.denominator,
        }
    }
    pub fn fields(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Position (samples)");
            ui.add(egui::DragValue::new(&mut self.at).range(0..=soundcraft_time::MAX_POSITION)).accessible_name("Event position");
        });
        match self.kind.as_str() {
            "key" => {
                craft_ui::ComboBox::from_label("Key").selected_text(&self.key).show_ui(ui, |ui| {
                    for minor in [false, true] {
                        for name in soundcraft_model::key_names(minor) {
                            ui.selectable_value(&mut self.key, (*name).into(), *name);
                        }
                    }
                });
            }
            "meter" => {
                ui.horizontal(|ui| {
                    ui.label("Meter");
                    ui.add(egui::DragValue::new(&mut self.numerator).range(1..=64)).accessible_name("Meter numerator");
                    ui.label("/");
                    craft_ui::ComboBox::from_id_salt("meter_denominator").selected_text(self.denominator.to_string()).show_ui(ui, |ui| {
                        for den in [1, 2, 4, 8, 16, 32, 64] {
                            ui.selectable_value(&mut self.denominator, den, den.to_string());
                        }
                    });
                });
                ui.label("Meter changes snap to the start of the bar.");
            }
            _ => {
                for (label, value, empty) in [("Root", &mut self.root, false), ("Bass", &mut self.bass, true)] {
                    craft_ui::ComboBox::from_label(label).selected_text(if value.is_empty() { "None" } else { value.as_str() }).show_ui(ui, |ui| {
                        if empty {
                            ui.selectable_value(value, String::new(), "None");
                        }
                        for note in ["C", "C#", "Db", "D", "D#", "Eb", "E", "F", "F#", "Gb", "G", "G#", "Ab", "A", "A#", "Bb", "B", "Cb"] {
                            ui.selectable_value(value, note.into(), note);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Quality");
                    ui.add(egui::TextEdit::singleline(&mut self.quality).char_limit(64).desired_width(100.0)).accessible_name("Chord quality");
                });
                craft_ui::ComboBox::from_label("Common qualities")
                    .selected_text(if self.quality.is_empty() { "major" } else { &self.quality })
                    .show_ui(ui, |ui| {
                        for q in
                            ["", "m", "7", "maj7", "m7", "dim", "dim7", "aug", "sus2", "sus4", "6", "m6", "9", "m9", "maj9", "11", "13", "7b5", "7#9"]
                        {
                            ui.selectable_value(&mut self.quality, q.into(), if q.is_empty() { "major" } else { q });
                        }
                    });
            }
        }
    }
    pub fn command(&self) -> (String, Value) {
        match self.kind.as_str() {
            "key" => ("event.key_edit".into(), json!({"at":self.original,"to":self.at,"key":self.key})),
            "meter" => {
                ("event.meter".into(), json!({"at":self.at,"original":self.original,"numerator":self.numerator,"denominator":self.denominator}))
            }
            _ => (
                if self.id.is_some() { "chord.edit" } else { "chord.add" }.into(),
                json!({"id":self.id,"at":self.at,"root":self.root,"quality":self.quality,"bass":self.bass}),
            ),
        }
    }
}
