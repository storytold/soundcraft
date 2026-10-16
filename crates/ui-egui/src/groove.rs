//! Groove Builder: a step worksheet using SoundCraft's native widgets and palette.

use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_midi::groove::{Config, Division, VOICES};
use soundcraft_model::ClipId;

#[derive(Default)]
pub struct State {
    pub config: Config,
    pub target: Option<ClipId>,
    saved: Option<Config>,
    pub dirty: bool,
    error: String,
}

pub fn open(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let cid = match p.get("clip") {
        Some(value) => Some(ClipId(value.as_u64().ok_or("clip must be an integer id")?)),
        None => app.engine.session().edit.selected_clips.first().copied(),
    };
    let cid = cid.ok_or("Select a groove clip first")?;
    let (_, clip) = app.engine.session().find_clip(cid).ok_or("Clip no longer exists")?;
    let recipe = clip.groove.as_ref().ok_or("This clip has no groove recipe")?;
    if recipe.version != 1 {
        return Err("Unsupported groove recipe version".into());
    }
    recipe.config.validate()?;
    app.groove = State { config: recipe.config.clone(), target: Some(cid), saved: Some(recipe.config.clone()), ..State::default() };
    app.ui.show_groove = true;
    Ok(json!({"clip":cid}))
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_groove {
        return;
    }
    // Undo/reload can replace a recipe. Never silently apply a stale draft over that change.
    if let Some(cid) = app.groove.target {
        let current = app.engine.session().find_clip(cid).and_then(|(_, c)| c.groove.as_ref());
        if current.map(|r| &r.config) != app.groove.saved.as_ref() {
            if let Some(recipe) = current {
                if recipe.version == 1 && recipe.config.validate().is_ok() {
                    app.groove = State { config: recipe.config.clone(), saved: Some(recipe.config.clone()), target: Some(cid), ..State::default() };
                } else {
                    app.groove = State::default();
                }
            } else {
                app.groove = State::default();
            }
        }
    }
    let mut visible = true;
    let mut action = None;
    egui::Window::new("Groove Builder").open(&mut visible).default_width(920.0).resizable(true).show(ctx, |ui| {
        let state = &mut app.groove;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
            crate::icons::draw(ui.painter(), r, "groove", ui.visuals().selection.stroke.color);
            ui.label("Distribute pulses, rotate each row, or click individual steps.");
        });
        let choices: Vec<_> = app
            .engine
            .session()
            .tracks
            .iter()
            .flat_map(|t| t.clips().iter().filter(|c| c.groove.is_some()).map(|c| (c.id, format!("{} / {}", t.name, c.name))))
            .collect();
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Pattern")
                .selected_text(choices.iter().find(|(id, _)| Some(*id) == state.target).map_or("New pattern", |(_, name)| name.as_str()))
                .show_ui(ui, |ui| {
                    for (cid, label) in &choices {
                        if ui.selectable_label(state.target == Some(*cid), label).clicked() {
                            action = Some(("groove.open", json!({"clip":cid})));
                        }
                    }
                });
            if ui.button("New pattern").clicked() {
                *state = State::default();
            }
            if state.dirty {
                ui.label("Unapplied changes");
            }
        });
        let before = state.config.clone();
        let c = &mut state.config;
        ui.horizontal_wrapped(|ui| {
            ui.label("Steps");
            ui.add(egui::DragValue::new(&mut c.steps).range(1..=32));
            ui.label("Cycles");
            ui.add(egui::DragValue::new(&mut c.repeats).range(1..=32));
            egui::ComboBox::from_label("Step size").selected_text(if c.division == Division::Eighth { "1/8" } else { "1/16" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.division, Division::Eighth, "1/8");
                ui.selectable_value(&mut c.division, Division::Sixteenth, "1/16");
            });
        });
        // Shrinking a cycle removes only cells outside its new bounds.
        c.ending = c.ending.min(c.steps);
        for lane in &mut c.lanes {
            lane.pulses = lane.pulses.min(c.steps);
            lane.rotate %= c.steps;
            if let Some(mask) = &mut lane.cells {
                *mask &= u32::MAX >> (32 - c.steps);
            }
        }
        ui.separator();
        egui::ScrollArea::horizontal().id_salt("groove_steps").show(ui, |ui| {
            egui::Grid::new("groove_rows").min_col_width(0.0).spacing(egui::vec2(7.0, 9.0)).show(ui, |ui| {
                ui.label("Voice");
                ui.label("Pulses");
                ui.label("Rotate");
                ui.label("Source");
                for step in 0..c.steps {
                    ui.label((step + 1).to_string());
                }
                ui.end_row();
                for (lane, (name, _)) in c.lanes.iter_mut().zip(VOICES) {
                    crate::widgets::checkbox(ui, &mut lane.enabled, name);
                    if ui.add(egui::DragValue::new(&mut lane.pulses).range(0..=c.steps)).changed() {
                        lane.cells = None;
                    }
                    if ui.add(egui::DragValue::new(&mut lane.rotate).range(0..=c.steps - 1)).changed() {
                        lane.cells = None;
                    }
                    if ui
                        .small_button(if lane.cells.is_some() { "Manual" } else { "Pulses" })
                        .on_hover_text("Return this row to its pulse distribution")
                        .clicked()
                    {
                        lane.cells = None;
                    }
                    for step in 0..c.steps {
                        let on = lane.hit(step, c.steps);
                        let response = ui
                            .add_enabled(lane.enabled, egui::Button::new(if on { "●" } else { "·" }).selected(on).min_size(egui::vec2(21.0, 23.0)))
                            .on_hover_text(format!("{name} step {}", step + 1));
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, lane.enabled, on, format!("{name} step {}", step + 1))
                        });
                        if response.clicked() {
                            lane.cells = Some(lane.mask(c.steps) ^ (1u32 << step));
                        }
                    }
                    ui.end_row();
                }
            });
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut c.velocity, 1..=127).text("Velocity"));
            ui.add(egui::Slider::new(&mut c.swing, 0..=75).suffix("%").text("Swing"));
            ui.add(egui::Slider::new(&mut c.ending, 0..=c.steps).text("Ending steps"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut c.velocity_spread, 0..=40).text("Velocity spread"));
            ui.add(egui::Slider::new(&mut c.timing_spread, 0..=20).suffix("%").text("Timing spread"));
            if ui.button("New variation").on_hover_text("Changes timing and velocity variation; enable a spread to hear it").clicked() {
                c.seed = c.seed.wrapping_add(1);
            }
        });
        if *c != before {
            state.dirty = true;
            state.error.clear();
        }
        ui.small("Ending steps add alternating snare/tom hits in the final cycle. Muted rows remain silent.");
        ui.separator();
        ui.horizontal(|ui| {
            if let Some(cid) = state.target {
                if ui.add_enabled(state.dirty, egui::Button::new("Apply changes")).clicked() {
                    action = Some(("groove.update", json!({"clip":cid,"config":state.config})));
                }
                if ui.button("Reload pattern").clicked() {
                    action = Some(("groove.open", json!({"clip":cid})));
                }
                if ui.button("Make ordinary MIDI").on_hover_text("Keep all notes and edit them freely in the MIDI Editor").clicked() {
                    action = Some(("groove.detach", json!({"clip":cid})));
                }
            } else if ui.button("Create groove track").clicked() {
                action = Some(("groove.create", json!({"config":state.config})));
            }
            ui.small("Each Apply is one undo step.");
        });
        if !state.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &state.error);
        }
    });
    app.ui.show_groove = visible;
    if let Some((command, params)) = action {
        match app.run(command, params) {
            Ok(result) => {
                if matches!(command, "groove.create" | "groove.update") {
                    if let Err(err) = open(app, &json!({"clip":result["clip"]})) {
                        app.groove.error = err;
                    }
                } else if command == "groove.detach" {
                    app.groove = State::default();
                    app.ui.show_groove = false;
                    app.ui.show_midi_editor = true;
                }
            }
            Err(err) => app.groove.error = err,
        }
    }
}
