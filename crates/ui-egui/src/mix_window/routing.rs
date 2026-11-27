use super::plugin_menu::{MenuSearch, menu_row};
use super::{Route, SoundApp, Track, TrackId, Ui, json};

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => crate::localization::text("no output").to_string(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| crate::localization::text("Out 1-2").to_string(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) | Route::Input(h) => h.clone(),
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

/// The first "Bus N" from the bus count on that no bus, track, input or output path is called, for New
/// Bus: a route names a bus or a path (`track.input`, `track.output`), so a bus never takes a
/// path's name.
pub(crate) fn free_bus_name(session: &soundcraft_model::Session) -> String {
    let taken = |name: &str| {
        session.busses.iter().any(|bus| bus.name.eq_ignore_ascii_case(name))
            || session.track_by_name(name).is_some()
            || session.inputs.iter().any(|path| path.name.eq_ignore_ascii_case(name))
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
    } else if let Route::Input(name) = &track.mixer.input
        && !app.engine.session().inputs.iter().any(|path| path.name.eq_ignore_ascii_case(name) && path.format == track.format)
    {
        format!("{name} ({})", crate::localization::text("Unavailable"))
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
        let channels = if input {
            let cached = was_open.then(|| ui.data(|data| data.get_temp(width_key))).flatten();
            cached.unwrap_or_else(|| {
                let channels = app.available_input_channels();
                ui.data_mut(|data| data.insert_temp(width_key, channels));
                channels
            })
        } else {
            output_channels(app)
        };
        route_menu(app, ui, id, input, channels);
    });
}

/// The first and last input (1-based) of a numbered input route: "In 7" or "In 7-8", in any case.
fn numbered_input(name: &str) -> Option<(usize, usize)> {
    let rest = name.get(..3).filter(|prefix| prefix.eq_ignore_ascii_case("in "))?;
    let range = &name[rest.len()..];
    let (first, last) = range.split_once('-').unwrap_or((range, range));
    let (first, last) = (first.trim().parse::<usize>().ok()?, last.trim().parse::<usize>().ok()?);
    (first >= 1 && last >= first).then_some((first, last))
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

/// Input or output menu with searchable hardware paths and buses. `channels` is the device's
/// input or output count (0 when unknown).
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool, channels: usize) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    let hint = crate::localization::text(if input { "Search inputs…" } else { "Search outputs…" });
    let search = MenuSearch::show(ui, egui::Id::unique(("route-search", id.0, input)), hint);
    let list_height = (ui.ctx().content_rect().height() - 105.0).clamp(48.0, 420.0);
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    // (label, the route the command is given)
    let mut fixed = vec![(crate::localization::text(if input { "no input" } else { "no output" }).to_string(), "none".to_string())];
    if input {
        let session = app.engine.session();
        let track = session.track(id);
        if let Some(track) = track.filter(|track| track.kind == soundcraft_model::TrackKind::Audio) {
            fixed.extend(
                session
                    .inputs
                    .iter()
                    .filter(|path| path.format == track.format)
                    .filter(|path| channels == 0 || usize::from(path.first_channel) + path.format.channels() <= channels)
                    .map(|path| (path.name.clone(), path.name.clone())),
            );
        }
        let width = track.map_or(1, |track| track.channels().max(1));
        let channels = if channels == 0 { 8 } else { channels };
        // Pro Tools' default input paths (Reference Guide, printed p. 58 and 82–83): every input
        // for a mono track, the pairs In 1-2, In 3-4 … for a stereo one (blocks of its width for a
        // wider one). A range off those that a track already uses (an older session's In 2-3)
        // stays listed, so the sessions that use it keep working and can share it.
        let mut firsts: Vec<usize> = (0..channels / width).map(|block| block * width + 1).collect();
        firsts.extend(app.engine.session().tracks.iter().filter(|t| t.channels().max(1) == width).filter_map(|t| match &t.mixer.input {
            Route::Hardware(name) => {
                numbered_input(name).filter(|&(first, last)| last + 1 - first == width && last <= channels).map(|(first, _)| first)
            }
            _ => None,
        }));
        firsts.sort_unstable();
        firsts.dedup();
        fixed.extend(firsts.into_iter().map(|first| {
            let last = first + width - 1;
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
        fixed.extend(output_path_choices(app, channels));
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
    use super::{input_name, route_menu};
    use crate::{Services, SoundApp};
    use egui_kittest::{Harness, kittest::Queryable};
    use serde_json::json;

    /// A stereo track's input selector offers Pro Tools' default stereo input paths, In 1-2, In 3-4
    /// and so on (Reference Guide, "Default I/O Settings" and "Restoring Default Paths and Path
    /// Names", printed p. 58 and 82–83), not every adjacent pair; a mono track's offers every input.
    /// A pair off the defaults that a track already uses (an older session's In 2-3) stays listed.
    #[test]
    fn the_stereo_input_selector_lists_the_default_pairs() {
        let _palette = crate::theme::lock_test_palette();
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &json!({"name": "Keys", "format": "stereo"})).unwrap();
        engine.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap();
        let keys = engine.session().track_by_name("Keys").unwrap().id;
        let vox = engine.session().track_by_name("Vox").unwrap().id;
        let app = SoundApp::new(engine, None, Services::default());
        let mut h = Harness::builder().with_size(egui::vec2(360.0, 2000.0)).with_max_steps(32).build_ui_state(
            move |ui, app: &mut SoundApp| {
                for (label, id) in [("Keys input", keys), ("Vox input", vox)] {
                    let button = ui.button(label);
                    egui::Popup::menu(&button)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| route_menu(app, ui, id, true, 30));
                }
            },
            app,
        );
        h.run();
        h.get_by_label("Keys input").click();
        h.run();
        for listed in ["In 1-2", "In 3-4", "In 7-8", "In 29-30"] {
            assert!(h.query_by_label(listed).is_some(), "{listed} is listed");
        }
        for missing in ["In 2-3", "In 28-29", "In 7"] {
            assert!(h.query_by_label(missing).is_none(), "{missing} is not listed");
        }
        h.get_by_label("Keys input").click();
        h.run();

        h.state_mut().run("track.input", json!({"track": keys.0, "input": "In 2-3"})).unwrap();
        h.get_by_label("Keys input").click();
        h.run();
        assert!(h.query_by_label("In 2-3").is_some(), "a pair in use stays selectable");
        h.get_by_label("Keys input").click();
        h.run();

        h.get_by_label("Vox input").click();
        h.run();
        for listed in ["In 1", "In 2", "In 3"] {
            assert!(h.query_by_label(listed).is_some(), "{listed} is listed for a mono track");
        }
    }

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
                    .show(|ui| route_menu(app, ui, id, false, 30));
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
            engine
                .execute("setup.io", &json!({"action": "create_input", "name": "Named last", "first_channel": ports - 1, "format": "mono"}))
                .unwrap();
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
                            .show(|ui| route_menu(app, ui, id, input, if input { ports } else { 0 }));
                    }
                },
                app,
            );
            h.run();
            h.get_by_label("Choose input").click();
            h.run();
            h.get_by_label("Named last").click();
            h.run();
            assert_eq!(h.state().engine.session().track(id).unwrap().mixer.input, super::Route::Input("Named last".into()));
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
