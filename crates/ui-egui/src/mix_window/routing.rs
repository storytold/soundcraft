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

/// The name an output or send selector shows for `r`. A route to an output path this session does
/// not have (a deleted path in an older session, or an imported track's), or to one that does not
/// fit the device, is marked unavailable: it plays nothing, as Pro Tools shows an inactive path in
/// italics (Reference Guide, "Making Paths Active or Inactive", printed p. 85).
pub(crate) fn output_name(app: &SoundApp, r: &Route) -> String {
    let name = route_name(app, r);
    let Route::Hardware(path) = r else { return name };
    let channels = output_channels(app);
    let available = app
        .engine
        .session()
        .outputs
        .iter()
        .enumerate()
        .find(|(_, p)| p.name.eq_ignore_ascii_case(path))
        .is_some_and(|(index, p)| index == 0 || channels == 0 || usize::from(p.first_channel) + p.format.channels() <= channels);
    if available { name } else { format!("{name} ({})", crate::localization::text("Unavailable")) }
}

/// The first "Bus N" from the bus count on that no bus, track or output path is called, for New
/// Bus: a route names a bus or an output path, so the two never share a name.
pub(crate) fn free_bus_name(session: &soundcraft_model::Session) -> String {
    let taken = |name: &str| {
        session.busses.iter().any(|bus| bus.name.eq_ignore_ascii_case(name))
            || session.track_by_name(name).is_some()
            || session.outputs.iter().any(|path| path.name.eq_ignore_ascii_case(name))
    };
    (session.busses.len() + 1..).map(|n| format!("Bus {n}")).find(|name| !taken(name)).unwrap_or_else(|| "Bus".into())
}

/// The input selector's text on a MIDI or instrument track.
pub(crate) fn midi_input_name(track: &Track) -> String {
    let input = soundcraft_model::MidiInput::of(&track.mixer.input);
    if input.device.is_none() && input.channel.is_none() { crate::localization::text("All MIDI").to_string() } else { input.label() }
}

/// The MIDI input menu of a MIDI or instrument track: a device (or all) and a channel (or all).
pub(crate) fn midi_input_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let Some(route) = app.engine.session().track(id).map(|t| t.mixer.input.clone()) else { return };
    let now = soundcraft_model::MidiInput::of(&route);
    let (device, channel) = (now.device.map(str::to_string), now.channel);
    let mut pick: Option<(Option<String>, Option<u8>)> = None;
    let mut devices: Vec<(String, bool)> =
        app.midi_inputs.as_ref().map(|m| m.ports().iter().filter(|p| p.enabled).map(|p| (p.name.clone(), p.open)).collect()).unwrap_or_default();
    // A device the track listens to stays in the list while it is unplugged.
    if let Some(d) = &device
        && !devices.iter().any(|(n, _)| n == d)
    {
        devices.push((d.clone(), false));
    }
    ui.strong(crate::localization::text("MIDI input"));
    if ui.selectable_label(device.is_none(), crate::localization::text("All devices")).clicked() {
        pick = Some((None, channel));
    }
    for (name, open) in &devices {
        let text = if *open { name.clone() } else { crate::localized_format!("{name} (not connected)", name = name) };
        if ui.selectable_label(device.as_deref() == Some(name.as_str()), text).clicked() {
            pick = Some((Some(name.clone()), channel));
        }
    }
    if devices.is_empty() {
        ui.weak(crate::localization::text("No MIDI input device found"));
    }
    ui.separator();
    ui.strong(crate::localization::text("Channel"));
    egui::Grid::new(("midi-channel", id.0)).spacing(egui::vec2(2.0, 2.0)).show(ui, |ui| {
        if ui.selectable_label(channel.is_none(), crate::localization::text("All")).clicked() {
            pick = Some((device.clone(), None));
        }
        ui.end_row();
        for ch in 0..16u8 {
            if ui.selectable_label(channel == Some(ch), format!("{:>2}", ch + 1)).clicked() {
                pick = Some((device.clone(), Some(ch)));
            }
            if ch % 8 == 7 {
                ui.end_row();
            }
        }
    });
    if let Some((device, channel)) = pick {
        let input = match (soundcraft_model::MidiInput { device: device.as_deref(), channel }).route() {
            Route::Hardware(h) => h,
            _ => "none".to_string(),
        };
        let _ = app.run("track.input", json!({"track": id.0, "input": input}));
    }
}

pub(crate) fn input_name(app: &SoundApp, track: &Track) -> String {
    if track.kind.is_midi() {
        midi_input_name(track)
    } else if track.kind.has_playlist() && track.mixer.input == Route::None {
        let channels = track.format.channels();
        if channels == 1 { crate::localized_format!("In {}", 1) } else { crate::localized_format!("In 1-{}", channels) }
    } else {
        route_name(app, &track.mixer.input)
    }
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    let midi = input && app.engine.session().track(id).is_some_and(|t| t.kind.is_midi());
    let popup = egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let width_key = popup.get_id().with("input_width");
    let was_open = popup.is_open();
    popup.show(|ui| {
        if midi {
            midi_input_menu(app, ui, id);
            return;
        }
        // Query only when opening the selector, never on each frame of the popup.
        let (channels, names) = if input {
            let cached = was_open.then(|| ui.data(|data| data.get_temp(width_key))).flatten();
            cached.unwrap_or_else(|| {
                let found = (app.available_input_channels(), app.available_input_channel_names());
                ui.data_mut(|data| data.insert_temp(width_key, found.clone()));
                found
            })
        } else {
            (output_channels(app), app.player.as_ref().map(|player| player.device_channel_names.clone()).unwrap_or_default())
        };
        route_menu(app, ui, id, input, channels, &names);
    });
}

/// The output paths a track output or send selector lists, as (label, the name `track.output` or
/// `mix.send` is given): the main path, the named paths, then every other pair of the interface's
/// `channels` outputs by the name Pro Tools' default paths give it ("Out 3-4" …); choosing a pair
/// makes its path. A pair a path already covers, or a bus of that name, is left out.
pub(crate) fn output_path_choices(app: &SoundApp, channels: usize) -> Vec<(String, String)> {
    let session = app.engine.session();
    let main_name = session.outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    let mut choices = vec![(main_name, "main".to_string())];
    choices.extend(session.outputs.iter().skip(1).map(|path| (path.name.clone(), path.name.clone())));
    choices.extend(default_output_pairs(session, channels).into_iter().map(|(name, _)| (name.clone(), name)));
    choices
}

/// The device's further pairs of `channels` outputs by their default path names ("Out 3-4" …) with
/// their first channel (0-based), leaving out a pair a path already covers or a bus of that name.
pub(crate) fn default_output_pairs(session: &soundcraft_model::Session, channels: usize) -> Vec<(String, usize)> {
    (2..channels.saturating_sub(1))
        .step_by(2)
        .map(|first| (format!("Out {}-{}", first + 1, first + 2), first))
        .filter(|(name, first)| {
            !session
                .outputs
                .iter()
                .any(|path| path.name.eq_ignore_ascii_case(name) || (usize::from(path.first_channel) == *first && path.format.channels() == 2))
                && !session.busses.iter().any(|bus| bus.name.eq_ignore_ascii_case(name))
        })
        .collect()
}

/// The device's output channels as the output and send selectors use them (0 when unknown).
pub(crate) fn output_channels(app: &SoundApp) -> usize {
    app.player.as_ref().map_or(0, |player| usize::from(player.device_channels))
}

/// The device's name for `width` channels from `first` (0-based): "ADAT 1", "Analog 1-2", or
/// "Analog 8–Mic 9" across two groups. `None` when the device leaves any of them unnamed or
/// names them only by number.
pub(crate) fn channel_span(names: &[String], first: usize, width: usize) -> Option<String> {
    let span = names.get(first..first.checked_add(width)?)?;
    fn named(name: &str) -> Option<&str> {
        let name = name.trim();
        (!name.is_empty() && !name.chars().all(|c| c.is_ascii_digit())).then_some(name)
    }
    let (a, b) = (named(span.first()?)?, named(span.last()?)?);
    if !span.iter().all(|name| named(name).is_some()) {
        return None;
    }
    if width == 1 {
        return Some(a.to_string());
    }
    Some(match (a.rsplit_once(' '), b.rsplit_once(' ')) {
        (Some((group, _)), Some((same, last))) if group == same => format!("{a}-{last}"),
        _ => format!("{a}–{b}"),
    })
}

/// `label` followed by the device's name for its channels: "In 15 (ADAT 1)". Only the menu and
/// tooltip text change; the route keeps the numbered name, so sessions open the same on any
/// interface.
fn with_channel_names(label: String, names: &[String], first: usize, width: usize) -> String {
    match channel_span(names, first, width) {
        Some(name) if !name.eq_ignore_ascii_case(&label) => format!("{label} ({name})"),
        _ => label,
    }
}

/// The route a selector shows, followed by the device's names for its channels: "In 15 (ADAT 1)",
/// "Cue (Phones 9-10)". The closed selector keeps the path name, as Pro Tools' path selectors do;
/// this is its tooltip, where Pro Tools gives a path's details. `None` for a bus, a MIDI input, or
/// when the device names none of the channels. `names` are the device's names in that direction.
pub(crate) fn route_details(app: &SoundApp, track: &Track, input: bool, names: &[String]) -> Option<String> {
    let session = app.engine.session();
    let (label, first, width) = if input {
        if track.kind.is_midi() {
            return None;
        }
        let (first, last) = match &track.mixer.input {
            Route::Hardware(route) => numbered_input(route)?,
            Route::None if track.kind.has_playlist() => (1, track.format.channels().max(1)),
            _ => return None,
        };
        (input_name(app, track), first - 1, last + 1 - first)
    } else {
        let label = route_name(app, &track.mixer.output);
        match &track.mixer.output {
            Route::Main => (label, usize::from(session.outputs.first()?.first_channel), session.main_format().channels()),
            Route::Hardware(name) => {
                let path = session.outputs.iter().find(|path| path.name.eq_ignore_ascii_case(name))?;
                (label, usize::from(path.first_channel), path.format.channels())
            }
            _ => return None,
        }
    };
    let detailed = with_channel_names(label.clone(), names, first, width);
    (detailed != label).then_some(detailed)
}

/// The channels of a numbered input route, 1-based: "In 15" is (15, 15), "In 8-9" is (8, 9).
fn numbered_input(route: &str) -> Option<(usize, usize)> {
    let rest = route.strip_prefix("In ")?;
    let (first, last) = rest.split_once('-').unwrap_or((rest, rest));
    let (first, last) = (first.trim().parse::<usize>().ok()?, last.trim().parse::<usize>().ok()?);
    (first >= 1 && last >= first).then_some((first, last))
}

/// The tooltip of a track's input or output selector at `rect`: [`route_details`] while the
/// pointer is over it, else `fallback` ("Input" or "Output", translated). The input's names are
/// read once per hover, since with no input open they are asked of the device (on Core Audio on
/// macOS only; other audio systems give no names, and the device is not asked).
pub(crate) fn selector_tooltip(app: &SoundApp, ui: &Ui, rect: egui::Rect, track: &Track, input: bool, fallback: &str) -> String {
    let key = egui::Id::unique(("route-selector-names", track.id.0, input));
    if !ui.rect_contains_pointer(rect) {
        ui.data_mut(|data| data.remove::<Vec<String>>(key));
        return fallback.to_string();
    }
    let names = if input {
        ui.data(|data| data.get_temp::<Vec<String>>(key)).unwrap_or_else(|| {
            let names = app.available_input_channel_names();
            ui.data_mut(|data| data.insert_temp(key, names.clone()));
            names
        })
    } else {
        app.player.as_ref().map(|player| player.device_channel_names.clone()).unwrap_or_default()
    };
    route_details(app, track, input, &names).unwrap_or_else(|| fallback.to_string())
}

/// Input or output menu with searchable hardware paths and buses. `channels` is the device's
/// input or output count (0 when unknown); `names` are the device's names for the channels in that
/// direction (see [`channel_span`]).
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool, channels: usize, names: &[String]) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    let hint = crate::localization::text(if input { "Search inputs…" } else { "Search outputs…" });
    let search = MenuSearch::show(ui, egui::Id::unique(("route-search", id.0, input)), hint);
    let list_height = (ui.ctx().content_rect().height() - 105.0).clamp(48.0, 420.0);
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    // (label, the route the command is given)
    let mut fixed = vec![(crate::localization::text(if input { "no input" } else { "no output" }).to_string(), "none".to_string())];
    if input {
        let width = app.engine.session().track(id).map_or(1, |track| track.channels().max(1));
        let channels = if channels == 0 { 8 } else { channels };
        fixed.extend((width..=channels).map(|last| {
            let first = last + 1 - width;
            let (label, value) = if width == 1 {
                (crate::localized_format!("In {}", first), format!("In {first}"))
            } else {
                (crate::localized_format!("In {}-{}", first, last), format!("In {first}-{last}"))
            };
            (with_channel_names(label, names, first - 1, width), value)
        }));
        if let Some(track) = app.engine.session().track(id)
            && let Route::Hardware(name) = &track.mixer.input
            && !fixed.iter().any(|(_, value)| value.eq_ignore_ascii_case(name))
        {
            fixed.push((format!("{name} ({})", crate::localization::text("Unavailable")), name.clone()));
        }
    } else {
        let session = app.engine.session();
        // The first channel and width of each choice's outputs, for the device's names.
        let span = |value: &str| -> Option<(usize, usize)> {
            if value == "main" {
                return session.outputs.first().map(|o| (usize::from(o.first_channel), session.main_format().channels()));
            }
            if let Some(path) = session.outputs.iter().find(|path| path.name.eq_ignore_ascii_case(value)) {
                return Some((usize::from(path.first_channel), path.format.channels()));
            }
            default_output_pairs(session, channels).into_iter().find(|(name, _)| name == value).map(|(_, first)| (first, 2))
        };
        fixed.extend(output_path_choices(app, channels).into_iter().map(|(label, value)| match span(&value) {
            Some((first, width)) => (with_channel_names(label, names, first, width), value),
            None => (label, value),
        }));
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
        let name = free_bus_name(app.engine.session());
        let _ = app.run(cmd, json!({"track": id.0, key: name}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{input_name, route_details, route_menu};
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

    /// A route to an output path the session does not have (a deleted path in an older session,
    /// or an imported track's) shows as unavailable, as an input route does.
    #[test]
    fn an_output_route_to_a_missing_path_shows_unavailable() {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("setup.io", &json!({"action": "create_output", "name": "Phones", "first_channel": 2})).unwrap();
        let app = SoundApp::new(engine, None, Services::default());
        assert_eq!(super::output_name(&app, &soundcraft_model::Route::Hardware("Phones".into())), "Phones");
        assert_eq!(super::output_name(&app, &soundcraft_model::Route::Hardware("Gone".into())), "Gone (Unavailable)");
        assert_eq!(super::output_name(&app, &soundcraft_model::Route::Main), "Out 1-2");
    }

    #[test]
    fn the_output_selector_lists_every_pair_of_interface_outputs() {
        let _palette = crate::theme::lock_test_palette();
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"format": "mono"})).unwrap();
        // A named path on channels 5-6 stands for that pair.
        engine.execute("setup.io", &json!({"action": "create_output", "name": "Cue", "first_channel": 4})).unwrap();
        let id = engine.session().tracks[0].id;
        let app = SoundApp::new(engine, None, Services::default());
        let mut h = Harness::builder().with_size(egui::vec2(360.0, 900.0)).with_max_steps(32).build_ui_state(
            move |ui, app: &mut SoundApp| {
                let button = ui.button("Choose output");
                egui::Popup::menu(&button)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| route_menu(app, ui, id, false, 30, &[]));
            },
            app,
        );
        h.run();
        h.get_by_label("Choose output").click();
        h.run();
        for listed in ["Out 1-2", "Out 3-4", "Cue", "Out 7-8", "Out 29-30"] {
            assert!(h.query_by_label(listed).is_some(), "{listed} is listed");
        }
        for missing in ["Out 5-6", "Out 31-32", "Out 2-3"] {
            assert!(h.query_by_label(missing).is_none(), "{missing} is not listed");
        }
        h.event(egui::Event::Text("9-10".into()));
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        let session = h.state().engine.session();
        assert_eq!(session.tracks[0].mixer.output, soundcraft_model::Route::Hardware("Out 9-10".into()));
        assert!(session.outputs.iter().any(|path| path.name == "Out 9-10" && path.first_channel == 8), "the path is made");
        assert!(session.busses.is_empty());
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
                            // Output pairs are left out (0 channels), so the named paths end the list.
                            .show(|ui| route_menu(app, ui, id, input, if input { ports } else { 0 }, &[]));
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

    #[test]
    fn the_interfaces_channel_names_find_a_path_that_keeps_its_numbered_route() {
        let _palette = crate::theme::lock_test_palette();
        // Named like a Fireface UFX: Analog 1-8, Mic (in) or Phones (out) 9-12, AES L/R, ADAT 1-16.
        let names = |middle: &str| -> Vec<String> {
            (1..=8)
                .map(|n| format!("Analog {n}"))
                .chain((9..=12).map(|n| format!("{middle} {n}")))
                .chain(["AES L".to_string(), "AES R".to_string()])
                .chain((1..=16).map(|n| format!("ADAT {n}")))
                .collect()
        };
        let (inputs, outputs) = (names("Mic"), names("Phones"));
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"format": "mono"})).unwrap();
        engine.execute("setup.io", &json!({"action": "create_output", "name": "Cue", "first_channel": 8, "format": "stereo"})).unwrap();
        let id = engine.session().tracks.first().unwrap().id;
        let app = SoundApp::new(engine, None, Services::default());
        let mut ready = false;
        let mut h = Harness::builder().with_size(egui::vec2(360.0, 400.0)).with_max_steps(32).build_ui_state(
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
                        .show(|ui| route_menu(app, ui, id, input, inputs.len(), if input { &inputs } else { &outputs }));
                }
            },
            app,
        );
        let mut pick = |button: &str, typed: &str| {
            h.run();
            h.get_by_label(button).click();
            h.run();
            h.event(egui::Event::Text(typed.into()));
            h.run();
            h.key_press(egui::Key::Enter);
            h.run();
            h.state().engine.session().track(id).unwrap().mixer.clone()
        };
        // The first input named ADAT 1 is the 15th; the routes stay numbered and named as before.
        assert_eq!(pick("Choose input", "adat 1").input, super::Route::Hardware("In 15".into()));
        assert_eq!(pick("Choose output", "phones").output, super::Route::Hardware("Cue".into()), "the path on channels 9-10 is found by their name");
        // A default pair is named too: outputs 17-18 are ADAT 3-4, and choosing it makes its path.
        assert_eq!(pick("Choose output", "adat 3").output, super::Route::Hardware("Out 17-18".into()));
    }

    #[test]
    fn the_selector_keeps_the_path_name_and_its_tooltip_names_the_channels() {
        // Named like a Fireface UFX inputs: Analog 1-8, Mic 9-12, AES L/R, ADAT 1-16.
        let names: Vec<String> = (1..=8)
            .map(|n| format!("Analog {n}"))
            .chain((9..=12).map(|n| format!("Mic {n}")))
            .chain(["AES L".to_string(), "AES R".to_string()])
            .chain((1..=16).map(|n| format!("ADAT {n}")))
            .collect();
        let outputs: Vec<String> = names.iter().map(|n| n.replace("Mic", "Phones")).collect();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let mono = app.run("track.new", json!({"format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        let stereo = app.run("track.new", json!({"format": "stereo"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("setup.io", json!({"action": "create_output", "name": "Cue", "first_channel": 8, "format": "stereo"})).unwrap();
        app.run("track.input", json!({"track": mono, "input": "In 15"})).unwrap();
        app.run("track.input", json!({"track": stereo, "input": "In 8-9"})).unwrap();
        app.run("track.output", json!({"track": mono, "output": "Cue"})).unwrap();
        app.run("track.output", json!({"track": stereo, "output": "Bus 1", "create_bus": true})).unwrap();
        let fresh = app.run("track.new", json!({"format": "stereo"})).unwrap()["tracks"][0].as_u64().unwrap();
        let track = |id: u64| app.engine.session().track(super::TrackId(id)).unwrap().clone();
        let (mono, stereo, fresh) = (track(mono), track(stereo), track(fresh));

        assert_eq!(input_name(&app, &mono), "In 15", "the closed selector keeps the path name");
        assert_eq!(route_details(&app, &mono, true, &names).as_deref(), Some("In 15 (ADAT 1)"));
        assert_eq!(route_details(&app, &stereo, true, &names).as_deref(), Some("In 8-9 (Analog 8–Mic 9)"));
        assert_eq!(route_details(&app, &mono, false, &outputs).as_deref(), Some("Cue (Phones 9-10)"));
        assert_eq!(route_details(&app, &stereo, false, &outputs), None, "a bus has no channels of the device");
        assert_eq!(route_details(&app, &mono, true, &[]), None, "a device that names nothing leaves the tooltip as it was");
        assert_eq!(route_details(&app, &fresh, true, &names).as_deref(), Some("In 1-2 (Analog 1-2)"), "the default input pair");
        assert_eq!(route_details(&app, &fresh, false, &outputs).as_deref(), Some("Out 1-2 (Analog 1-2)"), "the main path");
    }
}

#[cfg(test)]
mod midi_input_tests {
    use super::*;

    #[test]
    fn a_midi_track_names_its_device_and_channel_on_the_input_selector() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let midi = app.run("track.new", json!({"name": "Synth", "kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let audio = app.run("track.new", json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        let shown = |app: &SoundApp, id: u64| input_name(app, app.engine.session().track(TrackId(id)).unwrap());
        assert_eq!(shown(&app, midi), "All MIDI", "a new MIDI track listens to every device and channel");
        for (input, text) in [
            ("MIDI:10:Drum Pad", "Drum Pad ch 10"),
            ("MIDI:*:Keys", "Keys"),
            ("MIDI:2:*", "All ch 2"),
            ("MIDI:99:*", "All MIDI"),
            ("none", "All MIDI"),
        ] {
            app.run("track.input", json!({"tracks": [midi], "input": input})).unwrap();
            assert_eq!(shown(&app, midi), text, "{input}");
        }
        assert_eq!(shown(&app, audio), "In 1", "an audio track still shows its audio input");
    }
}
