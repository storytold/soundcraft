use super::{SoundApp, Value, json};

/// The speed ramp's choices in the AudioSuite window: param id, label, default, and (value, name) pairs.
pub(super) const SPEED_RAMP_CHOICES: [(&str, &str, f32, &[(f32, &str)]); 3] = [
    ("change", "Change", 0.0, &[(0.0, "Slow Down"), (1.0, "Speed Up")]),
    ("selection", "Selection", 0.0, &[(0.0, "Fit To"), (1.0, "Extend")]),
    ("fades", "Fades", 1.0, &[(1.0, "On"), (0.0, "Off")]),
];

/// Process-parameter window for the chosen AudioSuite process, with its controls and Render action.
pub(super) fn audiosuite_window(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(process) = app.ui.audiosuite.clone() else { return };
    let mut open = true;
    let title = soundcraft_dsp::plugin_info(&process).map_or_else(|| process.replace('_', " "), |p| p.name.to_string());
    egui::Window::new(format!("AudioSuite · {title}")).open(&mut open).default_width(360.0).show(ctx, |ui| {
        let key = egui::Id::unique(("as_params", process.clone()));
        let mut params: std::collections::BTreeMap<String, f32> = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_default();
        if let Some(info) = soundcraft_dsp::plugin_info(&process) {
            for p in info.params {
                let v = params.entry(p.id.to_string()).or_insert(p.default);
                ui.add(egui::Slider::new(v, p.min..=p.max).text(p.name));
            }
        } else {
            match process.as_str() {
                "normalize" => {
                    let v = params.entry("target_db".into()).or_insert(-0.1);
                    ui.add(egui::Slider::new(v, -24.0..=0.0).text("Peak dB"));
                }
                "time_stretch" => {
                    let v = params.entry("ratio".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text("Length ratio"));
                }
                "pitch_shift" => {
                    let v = params.entry("semitones".into()).or_insert(0.0);
                    ui.add(egui::Slider::new(v, -24.0..=24.0).text("Semitones"));
                }
                "speed_ramp" => {
                    for (id, label, default, choices) in SPEED_RAMP_CHOICES {
                        let v = params.entry(id.to_string()).or_insert(default);
                        ui.horizontal(|ui| {
                            ui.label(label);
                            for &(value, name) in choices {
                                ui.radio_value(v, value, name);
                            }
                        });
                    }
                }
                _ => {
                    ui.label("No parameters.");
                }
            }
        }
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, params.clone()));
        let hint = if soundcraft_dsp::is_generator(&process) {
            "Fills the selection on the selected tracks with a new clip."
        } else {
            "Processes the selected clips and replaces them with rendered audio."
        };
        ui.label(egui::RichText::new(hint).small());
        if ui.button(egui::RichText::new("Render").strong()).clicked() {
            let p: serde_json::Map<String, Value> = params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            let _ = app.run("audiosuite.process", json!({"process": process, "params": p}));
        }
    });
    if !open {
        app.ui.audiosuite = None;
    }
}
