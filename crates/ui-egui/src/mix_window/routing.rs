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
        let text = if *open { name.clone() } else { crate::localized_format!("{name} (unavailable)", name = name) };
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

/// The MIDI output selector of a MIDI or instrument track, as the Mix strip and the Edit
/// window's I/O column draw it.
pub(crate) struct MidiOutputSelector {
    /// "UM-ONE ch 1", with "(unavailable)" when that port is missing, or "No MIDI output".
    pub text: String,
    /// The chosen port is unavailable: the selector takes the warning colour the Playback Engine
    /// window gives a MIDI device that could not be opened, so it shows without opening the menu.
    pub missing: bool,
    /// The full text on hover, for a strip too narrow to show it: "MIDI output: Gone ch 3
    /// (unavailable)".
    pub tooltip: String,
}

pub(crate) fn midi_output_selector(app: &SoundApp, track: &Track) -> MidiOutputSelector {
    let out = soundcraft_model::MidiOutput::of(&app.engine.session().edit, track.id);
    let missing = out.is_some_and(|out| app.midi_output_missing(out.device) == Some(true));
    let text = match out {
        None => crate::localization::text("No MIDI output").to_string(),
        Some(out) if missing => crate::localized_format!("{name} (unavailable)", name = out.label()),
        Some(out) => out.label(),
    };
    let tooltip = if out.is_some() {
        crate::localized_format!("MIDI output: {output}", output = text)
    } else {
        crate::localization::text("MIDI output").to_string()
    };
    MidiOutputSelector { text, missing, tooltip }
}

/// The MIDI output menu of a MIDI or instrument track: no output, or one port and one of its 16
/// channels. A channel another track already plays to on that port says "in use".
pub(crate) fn midi_output_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let session = app.engine.session_arc();
    let now = soundcraft_model::MidiOutput::of(&session.edit, id);
    let (device, channel) = (now.map(|o| o.device.to_string()), now.map(|o| o.channel));
    let mut pick: Option<Option<(String, u8)>> = None;
    let listed = app.midi_output_ports.clone();
    let mut ports: Vec<(String, bool)> =
        listed.as_ref().and_then(|l| l.as_ref().ok()).map(|l| l.iter().map(|n| (n.clone(), true)).collect()).unwrap_or_default();
    // A port the track plays to stays in the list while it is unplugged.
    if let Some(d) = &device
        && !ports.iter().any(|(n, _)| n == d)
    {
        ports.push((d.clone(), false));
    }
    ui.strong(crate::localization::text("MIDI output"));
    if ui.selectable_label(device.is_none(), crate::localization::text("No MIDI output")).clicked() {
        pick = Some(None);
    }
    for (name, open) in &ports {
        let text = if *open { name.clone() } else { crate::localized_format!("{name} (unavailable)", name = name) };
        if ui.selectable_label(device.as_deref() == Some(name.as_str()), text).clicked() {
            pick = Some(Some((name.clone(), channel.unwrap_or(0))));
        }
    }
    match &listed {
        Some(Err(why)) => {
            ui.weak(crate::localization::error_text(why));
        }
        _ if ports.is_empty() => {
            ui.weak(crate::localization::text("No MIDI output port found"));
        }
        _ => {}
    }
    ui.separator();
    ui.strong(crate::localization::text("Channel"));
    // Channels other tracks already play to on this port.
    let used: Vec<u8> = soundcraft_model::MidiOutput::all(&session.edit)
        .filter(|(t, out)| *t != id && Some(out.device) == device.as_deref() && session.track(*t).is_some_and(|t| t.kind.is_midi()))
        .map(|(_, out)| out.channel)
        .collect();
    ui.add_enabled_ui(device.is_some(), |ui| {
        egui::Grid::new(("midi-output-channel", id.0)).spacing(egui::vec2(2.0, 2.0)).show(ui, |ui| {
            for ch in 0..16u8 {
                let text = if used.contains(&ch) { crate::localized_format!("{n} (in use)", n = ch + 1) } else { format!("{:>2}", ch + 1) };
                if ui.selectable_label(channel == Some(ch), text).clicked()
                    && let Some(d) = &device
                {
                    pick = Some(Some((d.clone(), ch)));
                }
                if ch % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    });
    if let Some(choice) = pick {
        let output = choice.map_or_else(|| "none".to_string(), |(d, ch)| soundcraft_model::MidiOutput { device: &d, channel: ch }.route());
        let _ = app.run("track.midi_output", json!({"track": id.0, "output": output}));
    }
}

/// The MIDI output menu under `resp`.
pub(crate) fn midi_output_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| midi_output_menu(app, ui, id));
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
    // The input of a MIDI or instrument track is a MIDI device and channel.
    let midi = input && app.engine.session().track(id).is_some_and(|t| t.kind.is_midi());
    egui::Popup::menu(resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| if midi { midi_input_menu(app, ui, id) } else { route_menu(app, ui, id, input) });
}

/// Input or output menu: no route and buses on both, plus the main output (outputs) or inputs 1-8
/// (inputs). Its search field takes typing at once and finds any of them, not just the buses.
fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    let hint = crate::localization::text(if input { "Search inputs…" } else { "Search outputs…" });
    let search = MenuSearch::show(ui, egui::Id::unique(("route-search", id.0, input)), hint);
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    // (label, the route the command is given)
    let mut fixed = vec![(crate::localization::text(if input { "no input" } else { "no output" }).to_string(), "none".to_string())];
    if input {
        fixed.extend((1..=8).map(|i| (crate::localized_format!("In {}", i), format!("In {i}"))));
    } else {
        fixed.push((main_name, "main".to_string()));
    }
    if search.typing() {
        let visible: Vec<(&String, &String)> =
            fixed.iter().map(|(l, v)| (l, v)).chain(busses.iter().map(|b| (b, b))).filter(|(l, _)| search.matches(l)).collect();
        let picked = search.rows(ui, visible.len(), 420.0, |ui, i, highlighted| visible.get(i).is_some_and(|(l, _)| menu_row(ui, highlighted, l)));
        if let Some((_, value)) = picked.and_then(|i| visible.get(i)) {
            let _ = app.run(cmd, json!({"track": id.0, key: value}));
            ui.close();
        }
        return;
    }
    for (label, value) in &fixed {
        if ui.button(label).clicked() {
            let _ = app.run(cmd, json!({"track": id.0, key: value}));
            ui.close();
        }
    }
    ui.separator();
    ui.strong(crate::localization::text("Bus"));
    if let Some(bus) = search
        .rows(ui, busses.len(), 420.0, |ui, i, highlighted| busses.get(i).is_some_and(|b| menu_row(ui, highlighted, b)))
        .and_then(|i| busses.get(i))
    {
        let _ = app.run(cmd, json!({"track": id.0, key: bus}));
        ui.close();
    }
    if ui.button(crate::localization::text("new bus...")).clicked() {
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
    use super::input_name;
    use crate::{Services, SoundApp};
    use serde_json::json;

    #[test]
    fn stereo_track_defaults_to_a_stereo_input_pair() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"format": "stereo"})).unwrap();
        let track = &app.engine.session().tracks[0];

        assert_eq!(input_name(&app, track), "In 1-2");
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
