//! Eight MIDI surface strips connected to the existing desktop transport and mixer commands.
mod mcu;

use crate::{SoundApp, json};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soundcraft_model::{TrackId, TrackKind};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

const NAME: &str = "SoundCraft HUI";
const STRIPS: usize = 8;
const TRANSPORT: u8 = 0x0e;
const NAVIGATION: u8 = 0x0a;
const LOOP: u8 = 0x0f;
const MARKERS: u8 = 0x14;
const STRIP_LAMP_PORTS: [u8; 3] = [2, 3, 7];
const TRANSPORT_LAMP_PORTS: [u8; 3] = [3, 4, 5];
const PONG: [u8; 3] = [0x90, 0, 0x7f];
const PING_INTERVAL: Duration = Duration::from_millis(900);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
// The Model 12 sends about 50 single steps between the centre and either end.
const PAN_STEP: f32 = 0.02;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ports {
    input: String,
    output: String,
    #[serde(default, skip_serializing_if = "Protocol::is_hui")]
    protocol: Protocol,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Protocol {
    #[default]
    Hui,
    Mcu,
}

impl Protocol {
    fn is_hui(&self) -> bool {
        *self == Self::Hui
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hui => "HUI",
            Self::Mcu => "MCU",
        }
    }

    fn status(self, online: bool) -> &'static str {
        match (self, online) {
            (Self::Hui, true) => "HUI connected",
            (Self::Hui, false) => "Waiting for HUI",
            (Self::Mcu, true) => "MCU connected",
            (Self::Mcu, false) => "Waiting for MCU",
        }
    }
}

#[derive(Default)]
pub(crate) struct Surface {
    link: Option<Link>,
    retry_at: Option<Instant>,
    first_track: usize,
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
    protocol: Protocol,
    mcu_output_failed: bool,
    decoder: Decode,
    ping: Option<Instant>,
    heartbeat: Heartbeat,
    ready: bool,
    lamps: Option<[[bool; 3]; STRIPS + 1]>,
}

struct Heartbeat {
    expires_at: Instant,
    seen_reply: bool,
}

impl Heartbeat {
    fn new(now: Instant) -> Self {
        Self { expires_at: now + HEARTBEAT_TIMEOUT, seen_reply: false }
    }

    fn receive(&mut self, now: Instant) {
        self.seen_reply = true;
        self.expires_at = now + HEARTBEAT_TIMEOUT;
    }

    fn expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    fn online(&self, now: Instant) -> bool {
        self.seen_reply && !self.expired(now)
    }
}

impl Surface {
    fn connection_due(&mut self, now: Instant) -> bool {
        if self.retry_at.is_some_and(|retry_at| now < retry_at) {
            return false;
        }
        self.retry_at = Some(now + RETRY_INTERVAL);
        true
    }

    fn take_link(&mut self, ports: &Ports, now: Instant, ctx: &egui::Context) -> Option<Link> {
        if let Some(link) = self.link.take()
            && !link.needs_reopen(now)
        {
            return Some(link);
        }
        // A timed-out WinMM handle may stay invalid after USB has re-enumerated.
        // Drop it before opening both ports again by name.
        if self.connection_due(now) {
            match Link::connect(ports) {
                Ok(link) => {
                    self.error.clear();
                    return Some(link);
                }
                Err(error) => self.error = error,
            }
        }
        if let Some(retry_at) = self.retry_at {
            ctx.request_repaint_after(retry_at.saturating_duration_since(now));
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Pong,
    Stop,
    Play,
    Record,
    Rewind,
    FastForward,
    Cycle,
    SetMarker,
    Jog(i8),
    Navigate(Navigation),
    Mute(usize),
    Solo(usize),
    Arm(usize),
    Pan(usize, i8),
    Fader(usize, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Navigation {
    BankLeft,
    BankRight,
    ChannelLeft,
    ChannelRight,
}

impl Navigation {
    fn target(self, first: usize, count: usize) -> usize {
        let last_bank = (count.saturating_sub(1) / STRIPS) * STRIPS;
        match self {
            Self::BankLeft => first.saturating_sub(STRIPS),
            Self::BankRight => first.saturating_add(STRIPS).min(last_bank),
            Self::ChannelLeft => first.saturating_sub(1),
            Self::ChannelRight => first.saturating_add(1).min(last_bank),
        }
    }
}

#[derive(Default)]
struct Decode {
    zone: Option<u8>,
    coarse: [Option<u8>; STRIPS],
}

impl Decode {
    fn packet(&mut self, packet: [u8; 3]) -> Option<Event> {
        match packet {
            PONG => Some(Event::Pong),
            [0xb0, 0x0f, zone @ 0..=127] => {
                self.zone = Some(zone);
                None
            }
            [0xb0, 0x2f, value] => match (self.zone.take()?, value) {
                (TRANSPORT, 0x41) => Some(Event::Rewind),
                (TRANSPORT, 0x42) => Some(Event::FastForward),
                (TRANSPORT, 0x43) => Some(Event::Stop),
                (TRANSPORT, 0x44) => Some(Event::Play),
                (TRANSPORT, 0x45) => Some(Event::Record),
                (LOOP, 0x43) => Some(Event::Cycle),
                (MARKERS, 0x40) => Some(Event::SetMarker),
                (NAVIGATION, 0x40) => Some(Event::Navigate(Navigation::ChannelLeft)),
                (NAVIGATION, 0x41) => Some(Event::Navigate(Navigation::BankLeft)),
                (NAVIGATION, 0x42) => Some(Event::Navigate(Navigation::ChannelRight)),
                (NAVIGATION, 0x43) => Some(Event::Navigate(Navigation::BankRight)),
                (strip @ 0..=7, 0x42) => Some(Event::Mute(usize::from(strip))),
                (strip @ 0..=7, 0x43) => Some(Event::Solo(usize::from(strip))),
                (strip @ 0..=7, 0x47) => Some(Event::Arm(usize::from(strip))),
                _ => None,
            },
            [0xb0, 0x0d, value] => relative_steps(value).map(Event::Jog),
            [0xb0, knob @ 0x40..=0x47, value] => relative_steps(value).map(|steps| Event::Pan(usize::from(knob - 0x40), steps)),
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

fn relative_steps(value: u8) -> Option<i8> {
    let magnitude = (value & 0x3f) as i8;
    match value {
        0 | 0x40 | 0x80..=0xff => None,
        1..=0x3f => Some(-magnitude),
        _ => Some(magnitude),
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
        Ok(Self {
            _input: input,
            output,
            packets,
            protocol: ports.protocol,
            mcu_output_failed: false,
            decoder: Decode::default(),
            ping: None,
            heartbeat: Heartbeat::new(Instant::now()),
            ready: false,
            lamps: None,
        })
    }

    fn send(&mut self, packet: [u8; 3]) {
        if self.mcu_output_failed {
            return;
        }
        if let Err(error) = self.output.send(&packet) {
            log::debug!("{} MIDI output: {error}", self.protocol.label());
            self.mcu_output_failed = self.protocol == Protocol::Mcu;
        }
    }

    fn needs_reopen(&self, now: Instant) -> bool {
        match self.protocol {
            Protocol::Hui => self.heartbeat.expired(now),
            Protocol::Mcu => self.mcu_output_failed,
        }
    }

    fn online(&self) -> bool {
        match self.protocol {
            Protocol::Hui => self.heartbeat.online(Instant::now()),
            Protocol::Mcu => !self.mcu_output_failed,
        }
    }

    fn decode(&mut self, packet: [u8; 3], now: Instant) -> Option<Event> {
        match self.protocol {
            Protocol::Hui if packet == PONG || self.heartbeat.online(now) => self.decoder.packet(packet),
            Protocol::Hui => None,
            Protocol::Mcu => mcu::decode(packet),
        }
    }
}

fn mixer_tracks(app: &SoundApp) -> impl Iterator<Item = TrackId> + '_ {
    app.engine.session().tracks.iter().filter(|track| !track.hidden && track.kind != TrackKind::Video).map(|track| track.id)
}

fn mapped_tracks(app: &SoundApp, first: usize) -> [Option<TrackId>; STRIPS] {
    let mut tracks = mixer_tracks(app).skip(first);
    std::array::from_fn(|_| tracks.next())
}

pub(crate) fn controlled_tracks(app: &SoundApp) -> [Option<TrackId>; STRIPS] {
    app.hui.link.as_ref().filter(|link| link.online()).map_or([None; STRIPS], |_| mapped_tracks(app, app.hui.first_track))
}

pub(crate) fn document_replaced(app: &mut SoundApp) {
    app.hui.first_track = 0;
    if let Some(link) = &mut app.hui.link {
        link.ready = false;
        link.decoder = Decode::default();
        link.lamps = None;
    }
}

fn navigate(app: &mut SoundApp, link: &mut Link, direction: Navigation) {
    let first = direction.target(app.hui.first_track, mixer_tracks(app).count());
    if first != app.hui.first_track {
        app.hui.first_track = first;
        link.decoder.coarse.fill(None);
        let first_id = mixer_tracks(app).nth(first);
        app.mix_scroll_into_view = first_id;
    }
}

fn navigate_from_host(app: &mut SoundApp, direction: Navigation) {
    if let Some(mut link) = app.hui.link.take() {
        navigate(app, &mut link, direction);
        app.hui.link = Some(link);
    }
}

fn apply(app: &mut SoundApp, ports: Option<Ports>) -> Result<(), String> {
    app.hui.link = None;
    app.hui.retry_at = None;
    app.hui.first_track = 0;
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
        let protocol = p.get("protocol").map(|value| serde_json::from_value(value.clone())).transpose().map_err(|error| error.to_string())?;
        apply(app, Some(Ports { input: input.into(), output: output.into(), protocol: protocol.unwrap_or_default() }))?;
    }
    if let Some(action) = p.get("navigate").and_then(Value::as_str) {
        let direction = match action {
            "bank_left" => Navigation::BankLeft,
            "bank_right" => Navigation::BankRight,
            "channel_left" => Navigation::ChannelLeft,
            "channel_right" => Navigation::ChannelRight,
            _ => return Err(format!("Unknown HUI navigation: {action}")),
        };
        navigate_from_host(app, direction);
    }
    let tracks = controlled_tracks(app);
    Ok(json!({
        "tracks": tracks.map(|id| id.map(|id| id.0)),
        "first_track": tracks[0].map(|_| app.hui.first_track + 1),
        "ports": app.ui.hui,
        "online": app.hui.link.as_ref().is_some_and(Link::online),
        "record_ready": app.hui.link.as_ref().is_some_and(|link| link.ready),
    }))
}

pub(crate) fn poll(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(ports) = &app.ui.hui else { return };
    let now = Instant::now();
    let Some(mut link) = app.hui.take_link(ports, now, ctx) else { return };
    ctx.request_repaint_after(Duration::from_millis(16));
    if link.protocol.is_hui() && link.ping.is_none_or(|at| now.saturating_duration_since(at) >= PING_INTERVAL) {
        link.send([0x90, 0, 0]);
        link.ping = Some(now);
    }
    let track_count = mixer_tracks(app).count();
    let last_bank = (track_count.saturating_sub(1) / STRIPS) * STRIPS;
    app.hui.first_track = app.hui.first_track.min(last_bank);
    let mut tracks = mapped_tracks(app, app.hui.first_track);
    let mut faders = [None; STRIPS];
    let mut pans = [None; STRIPS];
    let mut jog_steps = 0;
    for _ in 0..512 {
        let Ok(packet) = link.packets.try_recv() else { break };
        let Some(event) = link.decode(packet, now) else { continue };
        if let Event::Jog(steps) = event {
            jog_steps += i32::from(steps);
            continue;
        }
        if event != Event::Pong {
            jog(app, std::mem::take(&mut jog_steps));
        }
        match event {
            Event::Pong => link.heartbeat.receive(now),
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
            Event::Rewind | Event::FastForward | Event::Cycle | Event::SetMarker => transport_control(app, event),
            Event::Jog(_) => {}
            Event::Navigate(direction) => {
                apply_faders(app, tracks, std::mem::take(&mut faders));
                apply_pans(app, tracks, std::mem::take(&mut pans));
                navigate(app, &mut link, direction);
                tracks = mapped_tracks(app, app.hui.first_track);
            }
            Event::Mute(strip) | Event::Solo(strip) => {
                if let Some(id) = tracks[strip] {
                    let command = if matches!(event, Event::Mute(_)) { "mix.mute" } else { "mix.solo" };
                    let _ = app.run(command, json!({"tracks":[id.0]}));
                }
            }
            Event::Arm(strip) => {
                if let Some(track) = tracks.get(strip).and_then(|id| id.and_then(|id| app.engine.session().track(id))) {
                    let _ = app.run("mix.record_arm", json!({"tracks":[track.id.0], "value": !track.mixer.record_arm}));
                }
            }
            Event::Fader(strip, value) => {
                if let Some(fader) = faders.get_mut(strip) {
                    *fader = Some(value);
                }
            }
            Event::Pan(strip, steps) => {
                if let Some(track) = tracks[strip].and_then(|id| app.engine.session().track(id)) {
                    let pan = pans[strip].unwrap_or(track.mixer.pan[0]);
                    pans[strip] = Some((pan + f32::from(steps) * PAN_STEP).clamp(-1.0, 1.0));
                }
            }
        }
    }
    jog(app, jog_steps);
    apply_faders(app, tracks, faders);
    apply_pans(app, tracks, pans);
    app.hui.link = Some(link);
}

fn playhead(app: &SoundApp) -> i64 {
    if app.engine.transport.playing { app.engine.transport.position } else { app.engine.session().edit.playhead }
}

fn jog(app: &mut SoundApp, steps: i32) {
    if steps == 0 || app.engine.transport.recording {
        return;
    }
    let session = app.engine.session();
    let samples = (session.frame_rate.samples_per_frame(session.sample_rate) * f64::from(steps)).round() as i64;
    let at = playhead(app).saturating_add(samples).max(0);
    let _ = app.run("transport.locate", json!({"at":at}));
}

fn transport_control(app: &mut SoundApp, event: Event) {
    match event {
        Event::Cycle => {
            let _ = app.run("options.loop_playback", json!({}));
        }
        Event::SetMarker => {
            let _ = app.run("markers.add", json!({"at":playhead(app)}));
        }
        Event::Rewind | Event::FastForward if !app.engine.transport.recording => {
            let seconds = i64::from(app.engine.session().sample_rate.hz());
            let offset = if event == Event::Rewind { -seconds } else { seconds };
            let _ = app.run("transport.locate", json!({"at":playhead(app).saturating_add(offset).max(0)}));
        }
        _ => {}
    }
}

fn apply_pans(app: &mut SoundApp, tracks: [Option<TrackId>; STRIPS], pans: [Option<f32>; STRIPS]) {
    for (id, pan) in tracks.into_iter().zip(pans) {
        if let (Some(id), Some(pan)) = (id, pan)
            && app.engine.session().track(id).is_some_and(|t| (t.mixer.pan[0] - pan).abs() > 0.000_01)
        {
            let _ = app.engine.execute_merged("mix.pan", &json!({"tracks":[id.0], "pan":pan}), &format!("pan:{}:0", id.0));
        }
    }
}

fn apply_faders(app: &mut SoundApp, tracks: [Option<TrackId>; STRIPS], faders: [Option<u16>; STRIPS]) {
    for (id, value) in tracks.into_iter().zip(faders) {
        if let (Some(value), Some(track)) = (value, id.and_then(|id| app.engine.session().track(id))) {
            let db = soundcraft_model::fader_pos_to_db(f32::from(value) / 16_383.0);
            if (track.mixer.volume_db - db).abs() > 0.001 {
                let _ = app.run("mix.volume", json!({"tracks":[track.id.0], "db":db}));
            }
        }
    }
}

pub(crate) fn feedback(app: &mut SoundApp) {
    let tracks = controlled_tracks(app);
    let Some(link) = app.hui.link.as_mut().filter(|link| link.online()) else { return };
    let transport = &app.engine.transport;
    let lamps: [[bool; 3]; STRIPS + 1] = std::array::from_fn(|i| {
        if i == STRIPS {
            [!transport.playing, transport.playing, transport.recording || link.ready]
        } else {
            tracks[i].and_then(|id| app.engine.session().track(id)).map_or([false; 3], |t| [t.mixer.mute, t.mixer.solo, t.mixer.record_arm])
        }
    });
    let previous = link.lamps.replace(lamps);
    for (i, states) in lamps.into_iter().enumerate() {
        let (zone, ports) = if i < STRIPS { (i as u8, STRIP_LAMP_PORTS) } else { (TRANSPORT, TRANSPORT_LAMP_PORTS) };
        for (index, (port, lit)) in ports.into_iter().zip(states).enumerate() {
            if previous.is_some_and(|old| old[i][index] == lit) {
                continue;
            }
            match link.protocol {
                Protocol::Hui => {
                    link.send([0xb0, 0x0c, zone]);
                    link.send([0xb0, 0x2c, port | if lit { 0x40 } else { 0 }]);
                }
                Protocol::Mcu => link.send(mcu::lamp(i, index, lit)),
            }
        }
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
    let mut movement = None;
    crate::windows::win(ctx, &mut open, "Peripherals", egui::vec2(390.0, 260.0), |ui| {
        ui.label(crate::localization::text("MIDI Controllers"));
        egui::Grid::new("hui-peripherals").show(ui, |ui| {
            ui.label(crate::localization::text("Type"));
            craft_ui::ComboBox::from_id_salt("hui-type")
                .selected_text(if app.hui.enabled { app.hui.draft.protocol.label() } else { crate::localization::text("None") })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.hui.enabled, false, crate::localization::text("None"));
                    for protocol in [Protocol::Hui, Protocol::Mcu] {
                        let selected = app.hui.enabled && app.hui.draft.protocol == protocol;
                        if ui.selectable_label(selected, protocol.label()).clicked() {
                            app.hui.enabled = true;
                            app.hui.draft.protocol = protocol;
                        }
                    }
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
        }
        if let Some(link) = &app.hui.link {
            ui.label(crate::localization::text(link.protocol.status(link.online())));
            if link.online() {
                let count = mixer_tracks(app).count();
                let first = app.hui.first_track;
                if count > first {
                    ui.label(crate::localized_format!("Tracks {first} to {last}", first = first + 1, last = (first + STRIPS).min(count)));
                }
                for (label, left, right) in [
                    (crate::localization::text("Bank"), Navigation::BankLeft, Navigation::BankRight),
                    (crate::localization::text("Channel"), Navigation::ChannelLeft, Navigation::ChannelRight),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        if ui.add_enabled(left.target(first, count) != first, egui::Button::new("<")).clicked() {
                            movement = Some(left);
                        }
                        if ui.add_enabled(right.target(first, count) != first, egui::Button::new(">")).clicked() {
                            movement = Some(right);
                        }
                    });
                }
            }
        } else if let Some(ports) = &app.ui.hui {
            ui.label(crate::localization::text(ports.protocol.status(false)));
        }
    });
    app.hui.show = open;
    if let Some(direction) = movement {
        navigate_from_host(app, direction);
    }
    if pending {
        let _ = apply(app, app.hui.enabled.then(|| app.hui.draft.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_hui_preferences_keep_their_protocol_and_mcu_round_trips() {
        let old = json!({"input":"in","output":"out"});
        let mut ports: Ports = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(ports.protocol, Protocol::Hui);
        assert_eq!(serde_json::to_value(&ports).unwrap(), old);
        ports.protocol = Protocol::Mcu;
        let saved = serde_json::to_value(&ports).unwrap();
        assert_eq!(saved["protocol"], "mcu");
        assert_eq!(serde_json::from_value::<Ports>(saved).unwrap().protocol, Protocol::Mcu);
    }

    #[test]
    fn hui_heartbeat_expires_during_handshake_and_after_usb_loss() {
        let start = Instant::now();
        let mut beat = Heartbeat::new(start);
        assert!(!beat.online(start));
        assert!(beat.expired(start + HEARTBEAT_TIMEOUT));
        let reply = start + PING_INTERVAL;
        beat.receive(reply);
        assert!(beat.online(start + HEARTBEAT_TIMEOUT));
        assert!(beat.expired(reply + HEARTBEAT_TIMEOUT));
        assert!(!beat.online(reply + HEARTBEAT_TIMEOUT));
        let fresh = Heartbeat::new(reply + HEARTBEAT_TIMEOUT);
        assert!(!fresh.online(reply + HEARTBEAT_TIMEOUT));
    }

    #[test]
    fn hui_port_reopen_is_rate_limited_without_changing_bank() {
        let start = Instant::now();
        let mut surface = Surface { first_track: 8, ..Surface::default() };
        assert!(surface.connection_due(start));
        assert!(!surface.connection_due(start + PING_INTERVAL));
        assert!(surface.connection_due(start + RETRY_INTERVAL));
        assert_eq!(surface.first_track, 8);
    }

    #[test]
    fn model_12_mixing_and_locate_packets_act_only_on_button_press() {
        let mut decoder = Decode::default();
        for (zone, port, event) in [
            (0x0f, 3, Event::Cycle),
            (0, 2, Event::Mute(0)),
            (7, 3, Event::Solo(7)),
            (0x14, 0, Event::SetMarker),
            (0x0e, 1, Event::Rewind),
            (0x0e, 2, Event::FastForward),
        ] {
            assert_eq!(decoder.packet([0xb0, 0x0f, zone]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(event));
            assert_eq!(decoder.packet([0xb0, 0x0f, zone]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port]), None);
        }
        assert_eq!(decoder.packet([0xb0, 0x40, 0x3f]), Some(Event::Pan(0, -63)));
        assert_eq!(decoder.packet([0xb0, 0x47, 0x7f]), Some(Event::Pan(7, 63)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x41]), Some(Event::Jog(1)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x01]), Some(Event::Jog(-1)));
        assert_eq!(decoder.packet([0xb0, 0x0d, 0x40]), None);
    }

    #[test]
    fn hui_locates_adds_marker_at_playhead_and_toggles_loop() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let session = app.engine.session();
        let second = i64::from(session.sample_rate.hz());
        let three_frames = (session.frame_rate.samples_per_frame(session.sample_rate) * 3.0).round() as i64;
        let initial = second * 2;
        let looped = session.edit.loop_playback;
        transport_control(&mut app, Event::Cycle);
        assert_eq!(app.engine.session().edit.loop_playback, !looped);
        transport_control(&mut app, Event::Cycle);
        assert_eq!(app.engine.session().edit.loop_playback, looped);
        app.run("transport.locate", json!({"at":initial})).unwrap();
        jog(&mut app, 3);
        assert_eq!(playhead(&app), initial + three_frames);
        transport_control(&mut app, Event::SetMarker);
        assert_eq!(app.engine.session().markers.last().unwrap().start, initial + three_frames);
        transport_control(&mut app, Event::Rewind);
        assert_eq!(playhead(&app), initial + three_frames - second);
        transport_control(&mut app, Event::FastForward);
        assert_eq!(playhead(&app), initial + three_frames);
        app.engine.transport.recording = true;
        jog(&mut app, -3);
        transport_control(&mut app, Event::Rewind);
        assert_eq!(playhead(&app), initial + three_frames);
    }

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
        for (port, direction) in [(0, Navigation::ChannelLeft), (1, Navigation::BankLeft), (2, Navigation::ChannelRight), (3, Navigation::BankRight)]
        {
            assert_eq!(decoder.packet([0xb0, 0x0f, NAVIGATION]), None);
            assert_eq!(decoder.packet([0xb0, 0x2f, port | 0x40]), Some(Event::Navigate(direction)));
            assert_eq!(decoder.packet([0xb0, 0x0f, NAVIGATION]), None);
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

#[cfg(test)]
#[test]
fn hui_navigation_steps_eight_or_one_and_stops_at_the_last_bank() {
    use Navigation::*;
    assert_eq!(BankRight.target(0, 16), 8);
    assert_eq!(BankRight.target(8, 16), 8);
    assert_eq!(ChannelRight.target(8, 16), 8);
    assert_eq!(ChannelLeft.target(8, 16), 7);
    assert_eq!(ChannelRight.target(7, 16), 8);
    assert_eq!(BankLeft.target(7, 16), 0);
    assert_eq!(BankRight.target(8, 17), 16);
    assert_eq!(BankRight.target(0, 1), 0);
    assert_eq!(ChannelRight.target(0, 0), 0);
}
