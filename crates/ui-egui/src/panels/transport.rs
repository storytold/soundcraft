use super::{Align2, Rect, Sense, SoundApp, Tokens, json, mono, pos2, vec2};

pub(super) fn transport_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_transport;
    if !open {
        return;
    }
    let t = Tokens::current();
    egui::Window::new(crate::localization::text("Transport"))
        .id(egui::Id::unique("Transport"))
        .open(&mut open)
        .resizable(false)
        .default_pos(pos2(400.0, 500.0))
        .show(ctx, |ui| {
            let flags = app.engine.session().edit.flags.clone();
            let expanded = flags.contains("view.transport.expanded");
            ui.horizontal(|ui| {
                for (icon, cmd) in [
                    ("rtz", "transport.rtz"),
                    ("rewind", "transport.rewind"),
                    ("stop", "transport.stop"),
                    ("play", "transport.play"),
                    ("ffwd", "transport.fast_forward"),
                    ("end", "transport.go_to_end"),
                    ("record", "transport.record"),
                ] {
                    let mode = if cmd == "transport.record" { app.record_mode() } else { None };
                    // Loop Record shows a loop symbol on the Record button (Reference Guide p.771).
                    let icon = if mode.is_some() { "record_loop" } else { icon };
                    let resp = crate::widgets::icon_button(
                        ui,
                        vec2(34.0, 26.0),
                        icon,
                        (cmd == "transport.play" && app.is_playing()) || (cmd == "transport.record" && app.engine.transport.recording),
                        cmd,
                    );
                    let resp = crate::widgets::record_mode_hint(resp, mode);
                    if cmd == "transport.record"
                        && let Some(why) = app.record_hint()
                    {
                        egui::Tooltip::for_widget(&resp).show(|ui| ui.label(why));
                    }
                    if resp.clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                }
                if flags.contains("view.transport.output_meters") {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 26.0), Sense::hover());
                    let m = app.main_meter;
                    crate::widgets::meter(ui, Rect::from_min_size(r.min, vec2(10.0, 26.0)), m.level[0], m.hold[0], m.clip);
                    crate::widgets::meter(ui, Rect::from_min_size(pos2(r.min.x + 12.0, r.min.y), vec2(10.0, 26.0)), m.level[1], m.hold[1], m.clip);
                }
            });
            let s = app.engine.session();
            let fmt = |x: i64| s.format_position(x, s.edit.main_counter);
            if flags.contains("view.transport.counters") || expanded || flags.is_empty() {
                ui.label(egui::RichText::new(fmt(app.position())).font(mono(20.0)).color(t.counter_text));
            }
            if expanded {
                let e = s.edit.clone();
                egui::Grid::new("tx_exp").num_columns(4).show(ui, |ui| {
                    ui.label(crate::localization::text("Start"));
                    ui.label(egui::RichText::new(fmt(e.selection.start)).font(mono(11.0)));
                    ui.label(crate::localization::text("Pre-roll"));
                    ui.label(
                        egui::RichText::new(crate::localized_format!("{seconds:.2} s", seconds = s.sample_rate.seconds(e.pre_roll))).font(mono(11.0)),
                    );
                    ui.end_row();
                    ui.label(crate::localization::text("End"));
                    ui.label(egui::RichText::new(fmt(e.selection.end)).font(mono(11.0)));
                    ui.label(crate::localization::text("Post-roll"));
                    ui.label(
                        egui::RichText::new(crate::localized_format!("{seconds:.2} s", seconds = s.sample_rate.seconds(e.post_roll)))
                            .font(mono(11.0)),
                    );
                    ui.end_row();
                });
            }
            if flags.contains("view.transport.midi_controls") || expanded {
                let loop_pass = app.record_mode().is_some();
                let (click, countoff, merge, loop_rec, prepost) = {
                    let e = &app.engine.session().edit;
                    (e.click, e.countoff, e.midi_merge, e.loop_record, e.pre_post_roll)
                };
                ui.horizontal(|ui| {
                    for (label, on, cmd) in [
                        ("Click", click, "options.click"),
                        ("Count Off", countoff, "options.countoff"),
                        ("MIDI Merge", merge, "options.midi_merge"),
                        ("Loop Rec", loop_rec, "options.loop_record"),
                        ("Pre/Post", prepost, "options.pre_post_roll"),
                    ] {
                        // In Loop Record mode MIDI Merge has no effect, and the button is dimmed
                        // (Reference Guide p.813): for a Loop Record pass, and when stopped for
                        // the pass Record would start. A punch-in while playing records as a
                        // normal pass, where MIDI Merge counts (`SoundApp::record_mode`).
                        let dimmed = cmd == "options.midi_merge" && loop_pass;
                        let r = ui.add_enabled(!dimmed, egui::Button::selectable(on, crate::localization::text(label)));
                        let r = if cmd != "options.midi_merge" {
                            r
                        } else if dimmed {
                            r.on_disabled_hover_text(merge_tip("No effect while Loop Record is on"))
                        } else {
                            r.on_hover_text(merge_tip(MERGE_OR_REPLACE))
                        };
                        if r.clicked() {
                            let _ = app.run(cmd, json!({}));
                        }
                        if cmd == "options.midi_merge" {
                            // Right-click: MIDI Merge Release mode (p.814). Dimmed too, as the mode
                            // is a preference for later passes: a container menu opens on a
                            // right-click over a disabled widget, which a plain one does not see.
                            let current = app.ui.midi_merge_release;
                            r.container_context_menu(|ui| {
                                ui.label(crate::localization::text("MIDI Merge Release Mode"));
                                for mode in soundcraft_engine::io::MergeRelease::ALL {
                                    if ui.radio(current == mode, crate::localization::text(mode.label())).clicked() {
                                        let _ = app.run("ui.midi_merge_release", json!({"mode": mode.id()}));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                });
                let tick = app.engine.session().tempo.samples_to_ticks(app.position(), app.engine.session().sample_rate);
                let bpm = app.engine.session().tempo.tempo_at_tick(tick);
                let m = app.engine.session().tempo.meter_at_tick(tick);
                ui.label(
                    egui::RichText::new(crate::localized_format!("♩ = {bpm:.2}   {n}/{d}", bpm = bpm, n = m.numerator, d = m.denominator))
                        .font(mono(12.0))
                        .color(t.counter_text),
                );
            }
        });
    app.ui.show_transport = open;
}

pub(super) fn big_counter(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_big_counter;
    if !open {
        return;
    }
    egui::Window::new(crate::localization::text("Big Counter"))
        .id(egui::Id::unique("Big Counter"))
        .open(&mut open)
        .default_size(vec2(520.0, 110.0))
        .show(ctx, |ui| {
            let s = app.engine.session();
            let txt = s.format_position(app.position(), s.edit.main_counter);
            let r = ui.available_rect_before_wrap();
            let t = Tokens::current();
            ui.painter().rect_filled(r, 4.0, t.big_counter_bg);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, txt, mono((r.height() * 0.6).clamp(20.0, 120.0)), t.counter_text);
        });
    app.ui.show_big_counter = open;
}

/// What the MIDI Merge button switches (Reference Guide p.813).
const MERGE_OR_REPLACE: &str =
    "On (Merge): recorded MIDI is merged with the MIDI already on the track.\nOff (Replace): recorded MIDI replaces the MIDI in the recorded range.";

/// The MIDI Merge button's tooltip: `first`, then where the release mode is chosen (p.814).
fn merge_tip(first: &str) -> String {
    format!("{}\n{}", crate::localization::text(first), crate::localization::text("Right-click to choose the MIDI Merge Release Mode."))
}
