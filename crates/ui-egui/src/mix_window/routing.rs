use super::plugin_menu::{MenuSearch, menu_row};
use super::{Route, SoundApp, Track, TrackId, Ui, json};

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => crate::localization::text("no output").to_string(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| crate::localization::text("Out 1-2").to_string(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
    }
}

pub(crate) fn input_name(app: &SoundApp, track: &Track) -> String {
    if track.kind.has_playlist() && track.mixer.input == Route::None {
        let channels = track.format.channels();
        if channels == 1 { crate::localized_format!("In {}", 1) } else { crate::localized_format!("In 1-{}", channels) }
    } else {
        route_name(app, &track.mixer.input)
    }
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    let popup = egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let width_key = popup.get_id().with("input_width");
    let was_open = popup.is_open();
    popup.show(|ui| {
        // Query only when opening the selector, never on each frame of the popup.
        let inputs = if input {
            let cached = was_open.then(|| ui.data(|data| data.get_temp(width_key))).flatten();
            cached.unwrap_or_else(|| {
                let channels = app.available_input_channels();
                ui.data_mut(|data| data.insert_temp(width_key, channels));
                channels
            })
        } else {
            0
        };
        route_menu(app, ui, id, input, inputs);
    });
}

/// Input or output menu with searchable hardware paths and buses.
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool, inputs: usize) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    let hint = crate::localization::text(if input { "Search inputs…" } else { "Search outputs…" });
    let search = MenuSearch::show(ui, egui::Id::unique(("route-search", id.0, input)), hint);
    let list_height = (ui.ctx().content_rect().height() - 105.0).clamp(48.0, 420.0);
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    // (label, the route the command is given)
    let mut fixed = vec![(crate::localization::text(if input { "no input" } else { "no output" }).to_string(), "none".to_string())];
    if input {
        let width = app.engine.session().track(id).map_or(1, |track| track.channels().max(1));
        let channels = if inputs == 0 { 8 } else { inputs };
        fixed.extend((width..=channels).map(|last| {
            let first = last + 1 - width;
            if width == 1 {
                (crate::localized_format!("In {}", first), format!("In {first}"))
            } else {
                (crate::localized_format!("In {}-{}", first, last), format!("In {first}-{last}"))
            }
        }));
        if let Some(track) = app.engine.session().track(id)
            && let Route::Hardware(name) = &track.mixer.input
            && !fixed.iter().any(|(_, value)| value.eq_ignore_ascii_case(name))
        {
            fixed.push((format!("{name} ({})", crate::localization::text("Unavailable")), name.clone()));
        }
    } else {
        fixed.push((main_name, "main".to_string()));
        fixed.extend(app.engine.session().outputs.iter().skip(1).map(|path| (path.name.clone(), path.name.clone())));
    }
    if search.typing() {
        let visible: Vec<_> =
            fixed.iter().map(|(l, v)| (l, v, false)).chain(busses.iter().map(|b| (b, b, true))).filter(|(l, _, _)| search.matches(l)).collect();
        let picked =
            search.rows(ui, visible.len(), list_height, |ui, i, highlighted| visible.get(i).is_some_and(|(l, _, _)| menu_row(ui, highlighted, l)));
        if let Some((_, value, create_bus)) = picked.and_then(|i| visible.get(i)) {
            let _ = app.run(cmd, json!({"track": id.0, key: value, "create_bus": create_bus}));
            ui.close();
        }
        return;
    }
    egui::ScrollArea::vertical().id_salt("route_paths").max_height(list_height).show(ui, |ui| {
        for (label, value) in &fixed {
            if ui.button(label).clicked() {
                let _ = app.run(cmd, json!({"track": id.0, key: value, "create_bus": false}));
                ui.close();
            }
        }
        ui.separator();
        ui.strong(crate::localization::text("Bus"));
        for bus in &busses {
            if menu_row(ui, false, bus) {
                let _ = app.run(cmd, json!({"track": id.0, key: bus}));
                ui.close();
            }
        }
    });
    if ui.button(crate::localization::text("new bus...")).clicked() {
        let mut n = busses.len() + 1;
        let session = app.engine.session();
        while busses.contains(&format!("Bus {n}"))
            || session.track_by_name(&format!("Bus {n}")).is_some()
            || session.outputs.iter().any(|path| path.name.eq_ignore_ascii_case(&format!("Bus {n}")))
        {
            n += 1;
        }
        let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{input_name, route_menu};
    use crate::{Services, SoundApp};
    use egui_kittest::{Harness, kittest::Queryable};
    use serde_json::json;

    #[test]
    fn stereo_track_defaults_to_a_stereo_input_pair() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "stereo"})).unwrap();
        let track = &app.engine.session().tracks[0];

        assert_eq!(input_name(&app, track), "In 1-2");
    }

    #[test]
    fn wide_projects_can_scroll_to_and_assign_high_input_and_output_paths() {
        let _palette = crate::theme::lock_test_palette();
        for ports in [32, 64] {
            let mut engine = soundcraft_engine::Engine::default();
            engine.execute("track.new", &json!({"format": "mono"})).unwrap();
            for index in 0..ports {
                engine
                    .execute(
                        "setup.io",
                        &json!({
                            "action": "create_output", "name": format!("USB {:02}", index + 1), "first_channel": index, "format": "mono",
                        }),
                    )
                    .unwrap();
            }
            let id = engine.session().tracks.first().unwrap().id;
            let app = SoundApp::new(engine, None, Services::default());
            let mut ready = false;
            let mut h = Harness::builder().with_size(egui::vec2(360.0, 290.0)).with_max_steps(32).build_ui_state(
                move |ui, app: &mut SoundApp| {
                    if !ready {
                        crate::fonts::install(ui.ctx(), true);
                        crate::theme::apply(ui.ctx(), false);
                        ready = true;
                    }
                    for (label, input) in [("Choose input", true), ("Choose output", false)] {
                        let button = ui.button(label);
                        egui::Popup::menu(&button)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| route_menu(app, ui, id, input, ports));
                    }
                },
                app,
            );
            h.run();
            h.get_by_label("Choose input").click();
            h.run();
            h.event(egui::Event::Text("in 3".into()));
            h.run();
            assert!(h.query_by_label("In 4").is_none(), "only the matches are listed");
            h.key_press(egui::Key::Enter);
            h.run();
            assert_eq!(h.state().engine.session().tracks[0].mixer.input, soundcraft_model::Route::Hardware("In 3".into()));

            for (button, first, last, input) in
                [("Choose input", "In 1", format!("In {ports}"), true), ("Choose output", "USB 01", format!("USB {ports}"), false)]
            {
                h.get_by_label(button).click();
                h.run();
                assert!(h.query_by_label(&last).is_none_or(|node| node.rect().top() >= 290.0), "{last} is beyond the initial viewport");
                h.hover_at(h.get_by_label(first).rect().center());
                h.run();
                h.event(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -100.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                });
                h.run();
                h.get_by_label(&last).click();
                h.run();
                let track = h.state().engine.session().track(id).unwrap();
                let actual = if input { &track.mixer.input } else { &track.mixer.output };
                assert_eq!(*actual, super::Route::Hardware(last));
            }
        }
    }
}
