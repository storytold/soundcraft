use super::plugin_menu::{MenuSearch, menu_row};
use super::{Route, SoundApp, Track, TrackId, TrackKind, Ui, json};

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => "no output".into(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| "Out 1-2".into(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
    }
}

/// Show the complete span consumed by a physical input, even when its route stores only the
/// first channel. Bus names are kept intact rather than interpreted as hardware labels.
pub(crate) fn input_name(app: &SoundApp, track: &Track) -> String {
    if let Some(width) = hardware_route_width(track, true) {
        let first = match &track.mixer.input {
            Route::None => Some(1),
            Route::Hardware(name) => name.strip_prefix("In ").and_then(|name| name.split('-').next()).and_then(|first| first.parse().ok()),
            _ => None,
        };
        if let Some(label) = first.and_then(|first| channel_span_label("In", first, width)) {
            return label;
        }
    }
    if track.mixer.input == Route::None { "no input".into() } else { route_name(app, &track.mixer.input) }
}

/// Live hardware capture feeds audio tracks only. Outputs use the renderer's bounded strip
/// width, including instruments' stereo minimum; speaker formats can advertise wider layouts.
fn hardware_route_width(track: &Track, input: bool) -> Option<usize> {
    if input {
        return (track.kind == TrackKind::Audio).then_some(track.format.channels());
    }
    if !matches!(track.kind, TrackKind::Audio | TrackKind::Aux | TrackKind::Instrument | TrackKind::Folder) {
        return None;
    }
    let minimum = if track.kind == TrackKind::Instrument { 2 } else { 1 };
    Some(track.format.channels().clamp(minimum, soundcraft_mix::MAX_CHANNELS))
}

/// Format one-based hardware spans without overflowing on a malformed persisted channel name.
fn channel_span_label(prefix: &str, first: usize, width: usize) -> Option<String> {
    if first == 0 {
        return None;
    }
    let last = first.checked_add(width.checked_sub(1)?)?;
    Some(if width == 1 { format!("{prefix} {first}") } else { format!("{prefix} {first}-{last}") })
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        // The popup content runs only while open. Reuse the recorder so the menu reflects
        // its effective configuration without probing hardware on every mixer frame.
        let routable = app.engine.session().track(id).and_then(|track| hardware_route_width(track, input)).is_some();
        let channels = if !routable {
            0
        } else if input {
            app.ensure_input();
            app.recorder.as_ref().map_or(0, |recorder| recorder.channels)
        } else {
            app.player.as_ref().map_or(0, |player| player.routable_output_channels())
        };
        route_menu(app, ui, id, input, channels);
    });
}

/// Hardware rows use the open stream's channel count, separately from the session's main format.
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool, channels: usize) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    if ui.button(if input { "no input" } else { "no output" }).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "none"}));
        ui.close();
    }
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    if !input && ui.button(format!("Main ({main_name})")).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "main"}));
        ui.close();
    }
    let track = app.engine.session().track(id);
    // Instruments render at least stereo even when their track format is mono. Master
    // faders process Main directly and cannot be assigned a separate hardware destination.
    let width = track.and_then(|track| hardware_route_width(track, input));
    let channels = if width.is_some() { channels } else { 0 };
    let width = width.unwrap_or(1);
    egui::ScrollArea::vertical().id_salt(("hardware-routes", id.0, input)).max_height(280.0).show_rows(
        ui,
        ui.spacing().interact_size.y,
        channels.saturating_sub(width.saturating_sub(1)),
        |ui, rows| {
            for row in rows {
                let channel = row + 1;
                let Some(label) = channel_span_label(if input { "In" } else { "Hardware Out" }, channel, width) else { continue };
                if ui.button(label).clicked() {
                    // An explicit hardware route must not create a bus. Store its first
                    // channel; the output width follows the track's format.
                    let route = format!("{} {channel}", if input { "In" } else { "Out" });
                    let _ = app.run(cmd, json!({"track": id.0, key: route, "create_bus": false, "hardware": true}));
                    ui.close();
                }
            }
        },
    );
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    ui.strong("Bus");
    let search = MenuSearch::show(ui, egui::Id::unique(("route-bus-search", id.0, input)), "Search buses…");
    let visible: Vec<&String> = busses.iter().filter(|b| search.matches(b)).collect();
    if let Some(i) = search.rows(ui, visible.len(), 420.0, |ui, i, highlighted| menu_row(ui, highlighted, visible[i])) {
        let _ = app.run(cmd, json!({"track": id.0, key: visible[i]}));
        ui.close();
    }
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.contains(&format!("Bus {n}")) || app.engine.session().track_by_name(&format!("Bus {n}")).is_some() {
            n += 1;
        }
        let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{hardware_route_width, input_name, route_menu};
    use crate::{Services, SoundApp};
    use egui_kittest::{Harness, kittest::Queryable};
    use serde_json::json;

    fn scroll_to_last_channels<State>(h: &mut Harness<State>, first: &str) {
        h.hover_at(h.get_by_label(first).rect().center());
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -10000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run();
    }

    #[test]
    fn stereo_track_defaults_to_a_stereo_input_pair() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "stereo"})).unwrap();
        let track = &app.engine.session().tracks[0];

        assert_eq!(input_name(&app, track), "In 1-2");
    }

    /// Only audio tracks can consume hardware capture choices; other track kinds retain bus routing.
    #[test]
    fn hardware_capture_choices_are_only_offered_to_audio_tracks() {
        for kind in soundcraft_model::TrackKind::ALL {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            let id = app.engine.session_mut().add_track(kind, soundcraft_model::ChannelFormat::Mono, None);
            let track = app.engine.session().track(id).unwrap();
            assert_eq!(hardware_route_width(track, true), (kind == soundcraft_model::TrackKind::Audio).then_some(1), "{kind:?}");
            let mut h = Harness::builder().build_ui_state(move |ui, app| route_menu(app, ui, id, true, 48), app);
            h.run();
            assert_eq!(h.query_by_label("In 1").is_some(), kind == soundcraft_model::TrackKind::Audio, "{kind:?}");
        }
    }

    /// Wide output labels and their last valid start match the bounded rendered strip width.
    #[test]
    fn wide_output_choices_match_the_rendered_strip_width() {
        for kind in [soundcraft_model::TrackKind::Audio, soundcraft_model::TrackKind::Aux, soundcraft_model::TrackKind::Instrument] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            let id = app.engine.session_mut().add_track(kind, soundcraft_model::ChannelFormat::Ambisonics(7), None);
            assert_eq!(hardware_route_width(app.engine.session().track(id).unwrap(), false), Some(soundcraft_mix::MAX_CHANNELS));
            let mut h = Harness::builder()
                .with_max_steps(32)
                .with_size(egui::vec2(600.0, 700.0))
                .build_ui_state(move |ui, app| route_menu(app, ui, id, false, 48), app);
            h.run();
            scroll_to_last_channels(&mut h, "Hardware Out 1-16");
            h.get_by_label("Hardware Out 33-48").click();
            h.run();
            assert_eq!(h.state().engine.session().track(id).unwrap().mixer.output, soundcraft_model::Route::Hardware("Out 33".into()));
            assert!(h.query_by_label("Hardware Out 34-49").is_none());
        }
    }

    /// Selected mono and multichannel inputs show their full spans without rewriting the route.
    #[test]
    fn selected_capture_labels_show_the_full_track_span_without_changing_the_stored_start() {
        for (format, start, expected) in [("mono", 48, "In 48"), ("stereo", 47, "In 47-48"), ("5.1", 3, "In 3-8")] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            app.run("track.new", json!({"format": format})).unwrap();
            let id = app.engine.session().tracks[0].id;
            let route = format!("In {start}");
            app.run("track.input", json!({"track": id.0, "input": route, "hardware": true})).unwrap();
            assert_eq!(input_name(&app, app.engine.session().track(id).unwrap()), expected);
            assert_eq!(app.engine.session().track(id).unwrap().mixer.input, soundcraft_model::Route::Hardware(route));
        }
    }

    /// Capture choices follow the open device width, with channel 48 reachable by scrolling.
    #[test]
    fn input_menu_matches_available_capture_channels() {
        for channels in [0, 1, 2, 8, 48] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            app.run("track.new", json!({"name": "Input", "format": "mono"})).unwrap();
            let id = app.engine.session().tracks[0].id;
            let mut h = Harness::builder()
                .with_max_steps(32)
                .with_size(egui::vec2(600.0, 700.0))
                .build_ui_state(move |ui, app| route_menu(app, ui, id, true, channels), app);
            h.run();
            for channel in 1..=channels.min(8) {
                assert!(h.query_by_label(&format!("In {channel}")).is_some());
            }
            if channels == 48 {
                scroll_to_last_channels(&mut h, "In 1");
                h.get_by_label("In 48").click();
                h.run();
                assert_eq!(h.state().engine.session().tracks[0].mixer.input, soundcraft_model::Route::Hardware("In 48".into()));
            }
            assert!(h.query_by_label(&format!("In {}", channels + 1)).is_none());
        }
    }

    /// Output choices distinguish Main and physical destinations from capture inputs.
    #[test]
    fn output_menu_does_not_list_capture_channels() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "mono"})).unwrap();
        let id = app.engine.session().tracks[0].id;
        let mut h = Harness::builder().build_ui_state(move |ui, app| route_menu(app, ui, id, false, 48), app);
        h.run();
        assert!(h.query_by_label("In 1").is_none());
        assert!(h.query_by_label("In 48").is_none());
        h.get_by_label("Main (Out 1-2)");
        h.get_by_label("Hardware Out 1");
    }

    /// Selecting a physical destination bypasses a same-named bus without creating another bus.
    #[test]
    fn hardware_output_selection_keeps_physical_channel_and_does_not_create_a_bus() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "mono"})).unwrap();
        app.run("mix.new_bus", json!({"name": "Out 48"})).unwrap();
        let id = app.engine.session().tracks[0].id;
        let buses = app.engine.session().busses.len();
        let mut h = Harness::builder()
            .with_max_steps(32)
            .with_size(egui::vec2(600.0, 700.0))
            .build_ui_state(move |ui, app| route_menu(app, ui, id, false, 48), app);
        h.run();
        scroll_to_last_channels(&mut h, "Hardware Out 1");
        h.get_by_label("Hardware Out 48").click();
        h.run();
        assert_eq!(h.state().engine.session().tracks[0].mixer.output, soundcraft_model::Route::Hardware("Out 48".into()));
        assert_eq!(h.state().engine.session().busses.len(), buses);
    }

    /// Stereo input and output menus offer only consecutive pairs that fit the device.
    #[test]
    fn stereo_routes_do_not_wrap_past_the_last_device_channel() {
        for input in [true, false] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            app.run("track.new", json!({"format": "stereo"})).unwrap();
            let id = app.engine.session().tracks[0].id;
            let mut h = Harness::builder()
                .with_max_steps(32)
                .with_size(egui::vec2(600.0, 700.0))
                .build_ui_state(move |ui, app| route_menu(app, ui, id, input, 48), app);
            h.run();
            scroll_to_last_channels(&mut h, if input { "In 1-2" } else { "Hardware Out 1-2" });
            h.get_by_label(if input { "In 47-48" } else { "Hardware Out 47-48" });
            assert!(h.query_by_label(if input { "In 48-49" } else { "Hardware Out 48-49" }).is_none());
        }
    }

    /// A smaller or unavailable device removes physical choices that it cannot address.
    #[test]
    fn output_menu_removes_channels_after_switching_to_a_smaller_device() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "mono"})).unwrap();
        let id = app.engine.session().tracks[0].id;
        let mut h = Harness::builder()
            .with_max_steps(32)
            .with_size(egui::vec2(600.0, 700.0))
            .build_ui_state(move |ui, (app, channels): &mut (SoundApp, usize)| route_menu(app, ui, id, false, *channels), (app, 48));
        h.run();
        scroll_to_last_channels(&mut h, "Hardware Out 1");
        h.get_by_label("Hardware Out 48");
        h.state_mut().1 = 2;
        h.run();
        h.get_by_label("Hardware Out 1");
        h.get_by_label("Hardware Out 2");
        assert!(h.query_by_label("Hardware Out 3").is_none());
        assert!(h.query_by_label("Hardware Out 48").is_none());
        h.state_mut().1 = 0;
        h.run();
        assert!(h.query_by_label("Hardware Out 1").is_none());
        h.get_by_label("Main (Out 1-2)");
    }

    /// Instrument choices respect stereo rendering; master faders cannot select physical outputs.
    #[test]
    fn hardware_output_menu_matches_instrument_and_master_rendering() {
        for kind in ["instrument", "master"] {
            let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
            app.run("track.new", json!({"kind": kind, "format": "mono"})).unwrap();
            let id = app.engine.session().tracks[0].id;
            let mut h = Harness::builder()
                .with_max_steps(32)
                .with_size(egui::vec2(600.0, 700.0))
                .build_ui_state(move |ui, app| route_menu(app, ui, id, false, 48), app);
            h.run();
            if kind == "master" {
                assert!(h.query_by_label("Hardware Out 1").is_none());
                assert!(h.query_by_label("Hardware Out 1-2").is_none());
            } else {
                h.get_by_label("Hardware Out 1-2");
                scroll_to_last_channels(&mut h, "Hardware Out 1-2");
                h.get_by_label("Hardware Out 47-48");
                assert!(h.query_by_label("Hardware Out 48").is_none());
            }
        }
    }
}
