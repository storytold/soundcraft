//! Event-menu operation windows: Beat Detective, Tempo Operations, Time Operations, MIDI
//! Operations and MIDI Real-Time Properties. Each is a small form over engine commands.

use crate::SoundApp;
use crate::i18n::tr;
use crate::theme::Tokens;
use egui::vec2;
use serde_json::{Value, json};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct OpsState {
    pub beat_sensitivity: f32,
    pub beat_separate: bool,
    pub beat_set_tempo: bool,
    pub beat_result: String,
    pub tempo_curve: String,
    pub tempo_start: f64,
    pub tempo_end: f64,
    pub time_op: String,
    pub time_seconds: f64,
    pub meter_num: u32,
    pub meter_den: u32,
    pub midi_op: String,
    pub quant_grid: String,
    pub quant_strength: f64,
    pub quant_swing: f64,
    pub transpose: i64,
    pub velocity_add: i64,
    pub rtp_velocity: i64,
    pub rtp_transpose: i64,
    pub rtp_duration: f64,
    pub rtp_delay: i64,
    pub identify_bars: u32,
}

impl Default for OpsState {
    fn default() -> Self {
        OpsState {
            beat_sensitivity: 0.5,
            beat_separate: true,
            beat_set_tempo: false,
            beat_result: String::new(),
            tempo_curve: "linear".into(),
            tempo_start: 100.0,
            tempo_end: 120.0,
            time_op: "insert".into(),
            time_seconds: 4.0,
            meter_num: 4,
            meter_den: 4,
            midi_op: "quantize".into(),
            quant_grid: "1/16".into(),
            quant_strength: 100.0,
            quant_swing: 0.0,
            transpose: 12,
            velocity_add: 10,
            rtp_velocity: 0,
            rtp_transpose: 0,
            rtp_duration: 100.0,
            rtp_delay: 0,
            identify_bars: 1,
        }
    }
}

fn result_line(ui: &mut egui::Ui, r: &Result<Value, String>) {
    match r {
        Ok(v) => {
            let s = v.to_string();
            ui.label(egui::RichText::new(if s.len() > 160 { format!("{}…", &s[..160]) } else { s }).small().color(Tokens::current().counter_text));
        }
        Err(e) => {
            ui.label(egui::RichText::new(e).small().color(Tokens::current().rec));
        }
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    beat_detective(app, ctx);
    tempo_ops(app, ctx);
    time_ops(app, ctx);
    midi_ops(app, ctx);
    rtp(app, ctx);
}

fn beat_detective(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_beat_detective;
    if !open {
        return;
    }
    egui::Window::new(tr("Beat Detective")).id(egui::Id::new("Beat Detective")).open(&mut open).default_size(vec2(360.0, 220.0)).show(ctx, |ui| {
        ui.label(tr("Analyses the edit selection on the selected tracks."));
        ui.add(egui::Slider::new(&mut app.ops.beat_sensitivity, 0.0..=1.0).text(tr("Sensitivity")));
        ui.checkbox(&mut app.ops.beat_separate, tr("Separate clips at detected beats"));
        ui.checkbox(&mut app.ops.beat_set_tempo, tr("Set the session tempo from the beats"));
        ui.horizontal(|ui| {
            if ui.button(tr("Analyze")).clicked() {
                let r = app.run("event.beat_detective", json!({"sensitivity": app.ops.beat_sensitivity, "separate": false, "set_tempo": false}));
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
            if ui.button(egui::RichText::new(tr("Apply")).strong()).clicked() {
                let r = app.run(
                    "event.beat_detective",
                    json!({"sensitivity": app.ops.beat_sensitivity, "separate": app.ops.beat_separate, "set_tempo": app.ops.beat_set_tempo}),
                );
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(tr("Identify Beat: selection is"));
            ui.add(egui::DragValue::new(&mut app.ops.identify_bars).range(1..=64));
            ui.label(tr("bar(s)"));
            if ui.button(tr("Set Tempo")).clicked() {
                let r = app.run("event.identify_beat", json!({"bars": app.ops.identify_bars}));
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
        });
        if !app.ops.beat_result.is_empty() {
            let s = app.ops.beat_result.clone();
            ui.label(egui::RichText::new(if s.len() > 300 { format!("{}…", &s[..300]) } else { s }).small());
        }
    });
    app.ui.show_beat_detective = open;
}

/// Command the Tempo Operations Apply button sends. Stretch uses `factor = start / end`
/// so a lower end BPM lengthens the selection (`event.tempo_stretch` has no `end_bpm`).
pub(crate) fn tempo_ops_apply(curve: &str, start_bpm: f64, end_bpm: f64) -> (&'static str, Value) {
    let start = if start_bpm.is_finite() { start_bpm.max(1.0) } else { 1.0 };
    let end = if end_bpm.is_finite() { end_bpm.max(1.0) } else { 1.0 };
    match curve {
        "constant" => ("event.tempo_constant", json!({"bpm": start_bpm})),
        "parabolic" => ("event.tempo_parabolic", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
        "s-curve" => ("event.tempo_s_curve", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
        "scale" => ("event.tempo_scale", json!({"factor": end / start})),
        "stretch" => ("event.tempo_stretch", json!({"factor": start / end})),
        _ => ("event.tempo_linear", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
    }
}

fn tempo_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_tempo_ops;
    if !open {
        return;
    }
    egui::Window::new(tr("Tempo Operations")).id(egui::Id::new("Tempo Operations")).open(&mut open).default_size(vec2(360.0, 200.0)).show(
        ctx,
        |ui| {
            egui::ComboBox::from_label(tr("Curve")).selected_text(tr(&app.ops.tempo_curve)).show_ui(ui, |ui| {
                for c in ["constant", "linear", "parabolic", "s-curve", "scale", "stretch"] {
                    ui.selectable_value(&mut app.ops.tempo_curve, c.to_string(), tr(c));
                }
            });
            ui.add(egui::DragValue::new(&mut app.ops.tempo_start).range(5.0..=999.0).prefix(tr("start bpm ")));
            ui.add(egui::DragValue::new(&mut app.ops.tempo_end).range(5.0..=999.0).prefix(tr("end bpm ")));
            ui.label(
                egui::RichText::new(tr(
                    "Applies across the edit selection (constant: at the insertion point; scale: factor = end / start; stretch: factor = start / end).",
                ))
                .small(),
            );
            if ui.button(egui::RichText::new(tr("Apply")).strong()).clicked() {
                let (id, p) = tempo_ops_apply(&app.ops.tempo_curve, app.ops.tempo_start, app.ops.tempo_end);
                let r = app.run(id, p);
                result_line(ui, &r);
            }
        },
    );
    app.ui.show_tempo_ops = open;
}

fn time_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_time_ops;
    if !open {
        return;
    }
    egui::Window::new(tr("Time Operations")).id(egui::Id::new("Time Operations")).open(&mut open).default_size(vec2(360.0, 200.0)).show(ctx, |ui| {
        egui::ComboBox::from_label(tr("Operation")).selected_text(tr(&app.ops.time_op)).show_ui(ui, |ui| {
            for c in ["insert", "cut", "change meter", "move song start"] {
                ui.selectable_value(&mut app.ops.time_op, c.to_string(), tr(c));
            }
        });
        match app.ops.time_op.as_str() {
            "change meter" => {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut app.ops.meter_num).range(1..=64));
                    ui.label("/");
                    egui::ComboBox::from_id_salt("den").selected_text(app.ops.meter_den.to_string()).show_ui(ui, |ui| {
                        for d in [2u32, 4, 8, 16] {
                            ui.selectable_value(&mut app.ops.meter_den, d, d.to_string());
                        }
                    });
                });
            }
            "insert" => {
                ui.add(egui::DragValue::new(&mut app.ops.time_seconds).range(0.0..=3600.0).suffix(tr(" s")));
            }
            _ => {
                ui.label(egui::RichText::new(tr("Uses the edit selection.")).small());
            }
        }
        if ui.button(egui::RichText::new(tr("Apply")).strong()).clicked() {
            let at = app.engine.session().edit.selection.start;
            let r = match app.ops.time_op.as_str() {
                "insert" => app.run("event.insert_time", json!({"start": at, "length": {"seconds": app.ops.time_seconds}})),
                "cut" => app.run("event.cut_time", json!({})),
                "change meter" => app.run("event.meter", json!({"numerator": app.ops.meter_num, "denominator": app.ops.meter_den, "at": at})),
                _ => app.run("event.move_song_start", json!({"to": at})),
            };
            result_line(ui, &r);
        }
    });
    app.ui.show_time_ops = open;
}

fn midi_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_ops;
    if !open {
        return;
    }
    egui::Window::new(tr("MIDI Operations")).id(egui::Id::new("MIDI Operations")).open(&mut open).default_size(vec2(360.0, 220.0)).show(ctx, |ui| {
        egui::ComboBox::from_label(tr("Operation")).selected_text(tr(&app.ops.midi_op)).show_ui(ui, |ui| {
            for c in ["quantize", "transpose", "change velocity", "flatten performance", "restore performance"] {
                ui.selectable_value(&mut app.ops.midi_op, c.to_string(), tr(c));
            }
        });
        match app.ops.midi_op.as_str() {
            "quantize" => {
                egui::ComboBox::from_label(tr("Grid")).selected_text(app.ops.quant_grid.clone()).show_ui(ui, |ui| {
                    for g in ["1/4", "1/8", "1/8t", "1/16", "1/16t", "1/32"] {
                        ui.selectable_value(&mut app.ops.quant_grid, g.to_string(), g);
                    }
                });
                ui.add(egui::Slider::new(&mut app.ops.quant_strength, 0.0..=100.0).text(tr("Strength %")));
                ui.add(egui::Slider::new(&mut app.ops.quant_swing, 0.0..=100.0).text(tr("Swing %")));
            }
            "transpose" => {
                ui.add(egui::Slider::new(&mut app.ops.transpose, -48..=48).text(tr("Semitones")));
            }
            "change velocity" => {
                ui.add(egui::Slider::new(&mut app.ops.velocity_add, -127..=127).text(tr("Add")));
            }
            _ => {}
        }
        ui.label(egui::RichText::new(tr("Applies to the selected MIDI clips (or those under the selection).")).small());
        if ui.button(egui::RichText::new(tr("Apply")).strong()).clicked() {
            let r = match app.ops.midi_op.as_str() {
                "quantize" => {
                    app.run("event.quantize", json!({"grid": app.ops.quant_grid, "strength": app.ops.quant_strength, "swing": app.ops.quant_swing}))
                }
                "transpose" => app.run("event.transpose", json!({"semitones": app.ops.transpose})),
                "change velocity" => app.run("event.change_velocity", json!({"add": app.ops.velocity_add})),
                "flatten performance" => app.run("event.flatten_performance", json!({})),
                _ => app.run("event.restore_performance", json!({})),
            };
            result_line(ui, &r);
        }
    });
    app.ui.show_midi_ops = open;
}

fn rtp(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_rtp;
    if !open {
        return;
    }
    egui::Window::new(tr("MIDI Real-Time Properties")).id(egui::Id::new("MIDI Real-Time Properties")).open(&mut open).default_size(vec2(340.0, 200.0)).show(ctx, |ui| {
        ui.label(tr("Non-destructive, applied at playback to the selected MIDI tracks."));
        let mut changed = false;
        changed |= ui.add(egui::Slider::new(&mut app.ops.rtp_velocity, -127..=127).text(tr("Velocity +"))).changed();
        changed |= ui.add(egui::Slider::new(&mut app.ops.rtp_transpose, -48..=48).text(tr("Transpose"))).changed();
        changed |= ui.add(egui::Slider::new(&mut app.ops.rtp_duration, 1.0..=400.0).text(tr("Duration %"))).changed();
        changed |= ui.add(egui::Slider::new(&mut app.ops.rtp_delay, -960..=960).text(tr("Delay (ticks)"))).changed();
        if changed {
            let p = json!({"velocity": app.ops.rtp_velocity, "transpose": app.ops.rtp_transpose, "duration": app.ops.rtp_duration, "delay": app.ops.rtp_delay});
            let _ = app.engine.execute_merged("event.midi_rtp", &p, "rtp");
        }
        ui.horizontal(|ui| {
            if ui.button(tr("Write to Notes")).clicked() {
                let _ = app.run("track.write_midi_rtp", json!({}));
            }
            if ui.button(tr("Clear")).clicked() {
                let _ = app.run("event.midi_rtp", json!({"clear": true}));
                app.ops.rtp_velocity = 0;
                app.ops.rtp_transpose = 0;
                app.ops.rtp_duration = 100.0;
                app.ops.rtp_delay = 0;
            }
        });
    });
    app.ui.show_rtp = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_uses_start_over_end_and_scale_uses_end_over_start() {
        let (id, p) = tempo_ops_apply("stretch", 120.0, 60.0);
        assert_eq!(id, "event.tempo_stretch");
        assert!((p["factor"].as_f64().unwrap() - 2.0).abs() < 1e-9, "{p}");
        let (_, p) = tempo_ops_apply("stretch", 120.0, 120.0);
        assert!((p["factor"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{p}");
        let (id, p) = tempo_ops_apply("scale", 120.0, 60.0);
        assert_eq!(id, "event.tempo_scale");
        assert!((p["factor"].as_f64().unwrap() - 0.5).abs() < 1e-9, "{p}");
        let (_, p) = tempo_ops_apply("stretch", f64::NAN, f64::INFINITY);
        assert!((p["factor"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{p}");
        let (id, p) = tempo_ops_apply("linear", 100.0, 140.0);
        assert_eq!(id, "event.tempo_linear");
        assert_eq!(p["end_bpm"], 140.0);
    }
}
