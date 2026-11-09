//! The Output window, as in Pro Tools: one track's volume fader and readout, pan, Solo and Mute,
//! floating over the Edit window. The fader button beside a track's Output selector opens it.

use super::routing::route_name;
use super::sends::{Readout, editable_readout, parse_db, parse_pan};
use super::{SoundApp, Tokens, TrackId, TrackKind, db_text, fader, json, pan_knob, pan_text, pos2, surround_out, text_toggle, vec2};

/// Open the Output window on `track`, or close it when it already shows that track.
pub(crate) fn toggle_output_window(app: &mut SoundApp, track: TrackId) {
    app.ui.output_window = (app.ui.output_window != Some(track)).then_some(track);
}

/// Draw the Output window; Escape closes it, unless a menu, a text field or a dialog takes it.
pub(crate) fn output_window(app: &mut SoundApp, ctx: &egui::Context) {
    // As for the Send window: whether this Escape was a menu's or a text field's comes from the
    // last frame, because egui closes them before this runs.
    let busy_id = egui::Id::unique("output_window_escape");
    let busy = ctx.data(|d| d.get_temp(busy_id)).unwrap_or(false) || app.dialogs.open.is_some() || app.ui.show_search;
    if !busy && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.output_window = None;
    }
    output_window_contents(app, ctx);
    let busy = egui::Popup::is_any_open(ctx) || ctx.text_edit_focused();
    ctx.data_mut(|d| d.insert_temp(busy_id, busy));
}

/// The window for the remembered track; closes it when that track no longer exists.
fn output_window_contents(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(id) = app.ui.output_window else { return };
    let Some(track) = app.engine.session().track(id).cloned() else {
        app.ui.output_window = None;
        return;
    };
    let t = Tokens::current();
    let name = track.name.clone();
    let mut open = true;
    egui::Window::new(format!("{name} · Output"))
        .id(egui::Id::unique("output_window"))
        .open(&mut open)
        .auto_sized()
        .collapsible(false)
        .default_pos(ctx.pointer_latest_pos().map_or(pos2(200.0, 200.0), |p| p + vec2(24.0, -160.0)))
        .show(ctx, |ui| {
            ui.set_width(150.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(route_name(app, &track.mixer.output)).color(t.counter_text));
                if track.kind != TrackKind::Master {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.add_space(((ui.available_width() - 48.0) / 2.0).max(0.0));
                        if text_toggle(ui, vec2(22.0, 18.0), "S", track.mixer.solo, t.solo, &format!("{name} output solo")).clicked() {
                            let _ = app.run("mix.solo", json!({"track": id.0}));
                        }
                        if text_toggle(ui, vec2(22.0, 18.0), "M", track.mixer.mute, t.mute, &format!("{name} output mute")).clicked() {
                            let _ = app.run("mix.mute", json!({"track": id.0}));
                        }
                    });
                }
                // The stereo knobs, as on the track's strip; a surround panner stays in the Mix window.
                if !track.mixer.pan.is_empty() && track.kind != TrackKind::Vca && !surround_out(app, &track) {
                    ui.horizontal(|ui| {
                        let n = track.mixer.pan.len().min(2);
                        ui.add_space(((ui.available_width() - 36.0 * n as f32) / 2.0).max(0.0));
                        for i in 0..n {
                            ui.vertical(|ui| {
                                let mut v = track.mixer.pan.get(i).copied().unwrap_or(0.0);
                                let before = v;
                                pan_knob(ui, 32.0, &mut v, &format!("{name} output pan {} (drag; double-click centres)", i + 1));
                                if (v - before).abs() > f32::EPSILON {
                                    let _ = app.engine.execute_merged(
                                        "mix.pan",
                                        &json!({"track": id.0, "pan": v, "index": i}),
                                        &format!("pan:{}:{i}", id.0),
                                    );
                                }
                                let (pr, _) = ui.allocate_exact_size(vec2(32.0, 13.0), egui::Sense::hover());
                                ui.painter().rect_filled(pr, 1.0, t.counter_bg);
                                let (_, typed) = editable_readout(
                                    ui,
                                    pr,
                                    ui.scope_id().with(("output_pan_value", i)),
                                    Readout {
                                        name: &format!("{name} output pan {} value", i + 1),
                                        display: &pan_text(v),
                                        edit_initial: &format!("{:.0}", v * 100.0),
                                        parse: parse_pan,
                                        reset: 0.0,
                                    },
                                );
                                if let Some(v) = typed {
                                    let _ = app.run("mix.pan", json!({"track": id.0, "pan": v, "index": i}));
                                }
                            });
                        }
                    });
                }
                let (fr, _) = ui.allocate_exact_size(vec2(56.0, 220.0), egui::Sense::hover());
                // Distinct from the strip's "<track> level", so a label query while the Mix window
                // is open still picks one node.
                if let Some(db) = fader(ui, fr, track.mixer.volume_db, ui.scope_id().with("output_fader"), &format!("{name} output level")) {
                    app.gesture = Some(crate::Gesture::Fader { track: id, start_db: track.mixer.volume_db });
                    // The strip's merge key: one undo step for a drag, whichever fader it uses.
                    let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &format!("fader:{}", id.0));
                }
                let (rr, _) = ui.allocate_exact_size(vec2(64.0, 15.0), egui::Sense::hover());
                ui.painter().rect_filled(rr, 1.0, t.counter_bg);
                let (_, typed) = editable_readout(
                    ui,
                    rr,
                    ui.scope_id().with("output_volume_value"),
                    Readout {
                        name: &format!("{name} output volume"),
                        display: &db_text(track.mixer.volume_db),
                        edit_initial: &format!("{:.1}", track.mixer.volume_db),
                        parse: parse_db,
                        reset: 0.0,
                    },
                );
                if let Some(db) = typed {
                    let _ = app.run("mix.volume", json!({"track": id.0, "db": db}));
                }
            });
        });
    if !open {
        app.ui.output_window = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MainWindow, Services};
    use egui_kittest::{Harness, kittest::Queryable};

    /// The demo session in the Edit window with the I/O column shown and Kick tall enough for its
    /// Output row; returns the harness and Kick's id.
    fn edit_window_with_io() -> (Harness<'static, SoundApp>, TrackId) {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("view.edit_io", json!({})).unwrap();
        app.run("track.height", json!({"tracks": ["Kick"], "height": "medium"})).unwrap();
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(4);
        (h, kick)
    }

    fn kick_mixer(h: &Harness<'_, SoundApp>, kick: TrackId) -> soundcraft_model::Mixer {
        h.state().engine.session().track(kick).unwrap().mixer.clone()
    }

    /// Drags the pointer from `from` by `dy` points with the primary button held.
    fn drag(h: &mut Harness<'_, SoundApp>, from: egui::Pos2, dy: f32) {
        use egui::{Event, Modifiers, PointerButton};
        h.input_mut().events.push(Event::PointerMoved(from));
        h.step();
        h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for i in 1..=10 {
            h.input_mut().events.push(Event::PointerMoved(from + egui::vec2(0.0, dy * i as f32 / 10.0)));
            h.step();
        }
        let to = from + egui::vec2(0.0, dy);
        h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
    }

    /// #1392: the fader button beside Kick's Output selector opens Kick's Output window, whose
    /// controls edit Kick; it stays in the Edit window, and a second click closes the window.
    #[test]
    fn the_output_fader_button_opens_the_tracks_output_window() {
        let _palette = crate::theme::lock_test_palette();
        let (mut h, kick) = edit_window_with_io();
        assert!(h.query_by_label("Kick output level").is_none(), "no Output window before the click");
        h.get_by_label("Output window for Kick").click();
        h.run_steps(2);
        assert_eq!(h.state().ui.output_window, Some(kick));
        assert_eq!(h.state().ui.window, MainWindow::Edit, "the Mix window did not take over");

        let before = kick_mixer(&h, kick);
        let fader = h.get_by_label("Kick output level").rect();
        drag(&mut h, fader.center(), -40.0);
        let after = kick_mixer(&h, kick);
        assert!(after.volume_db > before.volume_db + 0.5, "dragging the fader up raised Kick: {} -> {}", before.volume_db, after.volume_db);

        h.get_by_label("Kick output mute").click();
        h.run_steps(2);
        assert_eq!(kick_mixer(&h, kick).mute, !before.mute, "Mute toggles Kick");
        h.get_by_label("Kick output solo").click();
        h.run_steps(2);
        assert_eq!(kick_mixer(&h, kick).solo, !before.solo, "Solo toggles Kick");

        h.get_by_label("Output window for Kick").click();
        h.run_steps(2);
        assert_eq!(h.state().ui.output_window, None, "a second click closes it");
        assert!(h.query_by_label("Kick output level").is_none());
    }

    /// Escape closes the Output window, and so does deleting its track.
    #[test]
    fn escape_or_deleting_the_track_closes_the_output_window() {
        let _palette = crate::theme::lock_test_palette();
        let (mut h, kick) = edit_window_with_io();
        h.get_by_label("Output window for Kick").click();
        h.run_steps(2);
        assert_eq!(h.state().ui.output_window, Some(kick));
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(h.state().ui.output_window, None, "Escape closes it");

        h.get_by_label("Output window for Kick").click();
        h.run_steps(2);
        h.state_mut().run("track.delete", json!({"tracks": [kick.0]})).unwrap();
        h.run_steps(2);
        assert_eq!(h.state().ui.output_window, None, "the window closes with its track");
    }
}
