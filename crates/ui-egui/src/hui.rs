//! Eight HUI strips connected to the existing desktop transport and mixer commands.
use crate::{SoundApp, json};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

const NAME: &str = "SoundCraft HUI";
const STRIPS: usize = 8;
const TRANSPORT: u8 = 0x0e;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ports {
    input: String,
    output: String,
}

#[derive(Default)]
pub(crate) struct Surface {
    link: Option<Link>,
    opened_preferences: bool,
    show: bool,
    draft: Ports,
    enabled: bool,
    inputs: Vec<String>,
    outputs: Vec<String>,
    error: String,
}

struct Link {
    _input: MidiInputConnection<()>,
    output: MidiOutputConnection,
    packets: Receiver<[u8; 3]>,
    decoder: Decode,
    ping: Option<Instant>,
    reply: Option<Instant>,
    ready: bool,
    lamps: Option<[bool; STRIPS + 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Pong,
    Stop,
    Play,
    Record,
    Arm(usize),
    Fader(usize, u16),
}

#[derive(Default)]
struct Decode {
    zone: Option<u8>,
    coarse: [Option<u8>; STRIPS],
}

impl Decode {
    fn packet(&mut self, packet: [u8; 3]) -> Option<Event> {
        match packet {
            [0x90, 0, 0x7f] => Some(Event::Pong),
            [0xb0, 0x0f, zone @ 0..=127] => {
                self.zone = Some(zone);
                None
            }
            [0xb0, 0x2f, value] => match (self.zone.take()?, value) {
                (TRANSPORT, 0x43) => Some(Event::Stop),
                (TRANSPORT, 0x44) => Some(Event::Play),
                (TRANSPORT, 0x45) => Some(Event::Record),
                (strip @ 0..=7, 0x47) => Some(Event::Arm(usize::from(strip))),
                _ => None,
            },
            [0xb0, strip @ 0..=7, coarse @ 0..=127] => {
                *self.coarse.get_mut(usize::from(strip))? = Some(coarse);
                None
            }
            [0xb0, fine @ 0x20..=0x27, value @ 0..=127] => {
                let strip = usize::from(fine - 0x20);
                let coarse = self.coarse.get_mut(strip)?.take()?;
                Some(Event::Fader(strip, (u16::from(coarse) << 7) | u16::from(value)))
            }
            _ => None,
        }
    }
}

fn input_names() -> Result<Vec<String>, String> {
    let midi = MidiInput::new(NAME).map_err(|e| e.to_string())?;
    Ok(midi.ports().iter().filter_map(|port| midi.port_name(port).ok()).collect())
}

fn output_names() -> Result<Vec<String>, String> {
    let midi = MidiOutput::new(NAME).map_err(|e| e.to_string())?;
    Ok(midi.ports().iter().filter_map(|port| midi.port_name(port).ok()).collect())
}

impl Link {
    fn connect(ports: &Ports) -> Result<Self, String> {
        let input = MidiInput::new(NAME).map_err(|e| e.to_string())?;
        let output = MidiOutput::new(NAME).map_err(|e| e.to_string())?;
        let (tx, packets) = sync_channel(512);
        let in_port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).ok().as_deref() == Some(ports.input.as_str()))
            .ok_or_else(|| ports.input.clone())?;
        let out_port = output
            .ports()
            .into_iter()
            .find(|p| output.port_name(p).ok().as_deref() == Some(ports.output.as_str()))
            .ok_or_else(|| ports.output.clone())?;
        let output = output.connect(&out_port, NAME).map_err(|e| e.to_string())?;
        let input = input
            .connect(
                &in_port,
                NAME,
                move |_, bytes, _| {
                    if let Ok(packet) = <[u8; 3]>::try_from(bytes) {
                        let _ = tx.try_send(packet);
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self { _input: input, output, packets, decoder: Decode::default(), ping: None, reply: None, ready: false, lamps: None })
    }

    fn send(&mut self, packet: [u8; 3]) {
        if let Err(error) = self.output.send(&packet) {
            log::debug!("HUI MIDI output: {error}");
        }
    }

    fn online(&self) -> bool {
        self.reply.is_some_and(|at| at.elapsed() < Duration::from_secs(3))
    }
}

fn apply(app: &mut SoundApp, ports: Option<Ports>) -> Result<(), String> {
    app.hui.link = None;
    app.hui.opened_preferences = true;
    app.ui.hui = None;
    if let Some(p) = ports {
        let link = Link::connect(&p).inspect_err(|e| app.hui.error.clone_from(e))?;
        app.hui.link = Some(link);
        app.ui.hui = Some(p);
    }
    app.hui.error.clear();
    Ok(())
}

pub(crate) fn command(app: &mut SoundApp, id: &str, p: &Value) -> Result<Value, String> {
    if id == "setup.peripherals" {
        app.hui.show = p.get("value").and_then(Value::as_bool).unwrap_or(!app.hui.show);
        if app.hui.show {
            app.hui.inputs = input_names()?;
            app.hui.outputs = output_names()?;
            app.hui.enabled = app.ui.hui.is_some();
            app.hui.draft = app.ui.hui.clone().unwrap_or_default();
        }
        return Ok(json!({"value": app.hui.show}));
    }
    if p.get("list").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({"inputs": input_names()?, "outputs": output_names()?}));
    }
    if p.get("enabled").and_then(Value::as_bool) == Some(false) {
        apply(app, None)?;
    } else if let (Some(input), Some(output)) = (p.get("input").and_then(Value::as_str), p.get("output").and_then(Value::as_str)) {
        apply(app, Some(Ports { input: input.into(), output: output.into() }))?;
    }
    Ok(json!({
        "ports": app.ui.hui,
        "online": app.hui.link.as_ref().is_some_and(Link::online),
        "record_ready": app.hui.link.as_ref().is_some_and(|link| link.ready),
    }))
}

pub(crate) fn poll(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.hui.opened_preferences {
        app.hui.opened_preferences = true;
        if let Some(ports) = &app.ui.hui {
            match Link::connect(ports) {
                Ok(link) => app.hui.link = Some(link),
                Err(error) => app.hui.error = error,
            }
        }
    }
    let Some(mut link) = app.hui.link.take() else { return };
    ctx.request_repaint_after(Duration::from_millis(16));
    if link.ping.is_none_or(|at| at.elapsed() >= Duration::from_millis(900)) {
        link.send([0x90, 0, 0]);
        link.ping = Some(Instant::now());
    }
    if !link.online() {
        link.ready = false;
        link.lamps = None;
    }
    let mut faders = [None; STRIPS];
    for _ in 0..512 {
        let Ok(packet) = link.packets.try_recv() else { break };
        let Some(event) = link.decoder.packet(packet) else { continue };
        match event {
            Event::Pong => link.reply = Some(Instant::now()),
            Event::Stop => {
                link.ready = false;
                let _ = app.run("transport.stop", json!({}));
            }
            Event::Play => {
                let record = std::mem::take(&mut link.ready);
                let command = if record { "transport.record" } else { "transport.play" };
                if !app.engine.transport.recording {
                    let _ = app.run(command, json!({}));
                }
            }
            Event::Record if !app.is_playing() => link.ready = !link.ready,
            Event::Record => {}
            Event::Arm(strip) => {
                if let Some(track) = app.engine.session().tracks.get(strip) {
                    let _ = app.run("mix.record_arm", json!({"tracks":[track.id.0], "value": !track.mixer.record_arm}));
                }
            }
            Event::Fader(strip, value) => {
                if let Some(fader) = faders.get_mut(strip) {
                    *fader = Some(value);
                }
            }
        }
    }
    for (strip, value) in faders.into_iter().enumerate() {
        if let (Some(value), Some(track)) = (value, app.engine.session().tracks.get(strip)) {
            let db = soundcraft_model::fader_pos_to_db(f32::from(value) / 16_383.0);
            if (track.mixer.volume_db - db).abs() > 0.001 {
                let _ = app.run("mix.volume", json!({"tracks":[track.id.0], "db":db}));
            }
        }
    }
    app.hui.link = Some(link);
}

pub(crate) fn feedback(app: &mut SoundApp) {
    let Some(link) = app.hui.link.as_mut().filter(|link| link.online()) else { return };
    let transport = &app.engine.transport;
    let lamps: [bool; STRIPS + 3] = std::array::from_fn(|i| match i {
        8 => !transport.playing,
        9 => transport.playing,
        10 => transport.recording || link.ready,
        _ => app.engine.session().tracks.get(i).is_some_and(|t| t.mixer.record_arm),
    });
    let previous = link.lamps.replace(lamps);
    for (i, lit) in lamps.into_iter().enumerate() {
        if previous.is_some_and(|old| old.get(i) == Some(&lit)) {
            continue;
        }
        let (zone, port) = if i < STRIPS { (i as u8, 7) } else { (TRANSPORT, (i - STRIPS + 3) as u8) };
        link.send([0xb0, 0x0c, zone]);
        link.send([0xb0, 0x2c, port | if lit { 0x40 } else { 0 }]);
    }
}

fn port_picker(ui: &mut egui::Ui, id: &str, selected: &mut String, ports: &[String]) {
    craft_ui::ComboBox::from_id_salt(id).width(225.0).selected_text(selected.as_str()).show_ui(ui, |ui| {
        for name in ports {
            ui.selectable_value(selected, name.clone(), name);
        }
    });
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.hui.show;
    let mut pending = false;
    crate::windows::win(ctx, &mut open, "Peripherals", egui::vec2(390.0, 200.0), |ui| {
        ui.label(crate::localization::text("MIDI Controllers"));
        egui::Grid::new("hui-peripherals").show(ui, |ui| {
            ui.label(crate::localization::text("Type"));
            craft_ui::ComboBox::from_id_salt("hui-type")
                .selected_text(if app.hui.enabled { "HUI" } else { crate::localization::text("None") })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.hui.enabled, false, crate::localization::text("None"));
                    ui.selectable_value(&mut app.hui.enabled, true, "HUI");
                });
            ui.end_row();
            ui.label(crate::localization::text("Receive From"));
            port_picker(ui, "hui-input", &mut app.hui.draft.input, &app.hui.inputs);
            ui.end_row();
            ui.label(crate::localization::text("Send To"));
            port_picker(ui, "hui-output", &mut app.hui.draft.output, &app.hui.outputs);
            ui.end_row();
            ui.label(crate::localization::text("Channels"));
            ui.label(if app.hui.enabled { "8" } else { "0" });
            ui.end_row();
        });
        pending = ui
            .add_enabled(
                !app.hui.enabled || (!app.hui.draft.input.is_empty() && !app.hui.draft.output.is_empty()),
                egui::Button::new(crate::localization::text("Apply")),
            )
            .clicked();
        if !app.hui.error.is_empty() {
            ui.label(&app.hui.error);
        } else if let Some(link) = &app.hui.link {
            ui.label(crate::localization::text(if link.online() { "HUI connected" } else { "Waiting for HUI" }));
        }
    });
    app.hui.show = open;
    if pending {
        let _ = apply(app, app.hui.enabled.then(|| app.hui.draft.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_12_packets_select_transport_strip_buttons_and_faders() {
        let mut decoder = Decode::default();
        assert_eq!(decoder.packet([0x90, 0, 127]), Some(Event::Pong));
        for (port, action) in [(3, Event::Stop), (4, Event::Play), (5, Event::Record)] {
            assert_eq!(decoder.packet([0xb0, 0x0f, TRANSPORT]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(action));
            assert_eq!(decoder.packet([0xb0, 0x0f, TRANSPORT]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port]), None);
        }
        for strip in 0..8 {
            assert_eq!(decoder.packet([0xb0, 0x0f, strip]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, 0x47]), Some(Event::Arm(usize::from(strip))));
            assert_eq!(decoder.packet([0xb0, strip, 96]), None);
            assert_eq!(decoder.packet([0xb0, 0x20 + strip, 16]), Some(Event::Fader(usize::from(strip), 12_304)));
            assert_eq!(decoder.packet([0xb0, 0x20 + strip, 16]), None);
        }
    }
}
