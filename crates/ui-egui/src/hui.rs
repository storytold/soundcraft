//! Eight MIDI surface strips connected to the existing desktop transport and mixer commands.
mod motion;

use crate::surface_protocol::{Decode, Event, Navigation, PONG, STRIPS, TRANSPORT, hui_fader, mcu};
use motion::{FADER_MAX, GESTURE_GAP, Motion};

use crate::{SoundApp, json};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soundcraft_model::{TrackId, TrackKind};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::{Duration, Instant};

const NAME: &str = "SoundCraft HUI";
const STRIP_LAMP_PORTS: [u8; 3] = [2, 3, 7];
const TRANSPORT_LAMP_PORTS: [u8; 3] = [3, 4, 5];
const PING_INTERVAL: Duration = Duration::from_millis(900);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const MIX_GESTURE: &str = "surface:mixer";
// The Model 12 sends about 50 single steps between the centre and either end.
const PAN_STEP: f32 = 0.02;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ports {
    input: String,
    output: String,
    #[serde(default, skip_serializing_if = "Protocol::is_hui")]
    protocol: Protocol,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    motorized: bool,
}

impl Ports {
    pub(crate) fn input(&self) -> &str {
        &self.input
    }
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
    context: Option<egui::Context>,
    motion: Motion,
    frame: Option<Instant>,
}

struct Link {
    input: Option<MidiInputConnection<()>>,
    output: Option<MidiOutputConnection>,
    packets: Receiver<[u8; 3]>,
    protocol: Protocol,
    failed: bool,
    check_at: Instant,
    motorized: bool,
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
        if let Some(mut link) = self.link.take() {
            link.check_ports(ports, now);
            if !link.needs_reopen(now) {
                return Some(link);
            }
        }
        self.motion = Motion::default();
        // A timed-out WinMM handle may stay invalid after USB has re-enumerated.
        // Drop it before opening both ports again by name.
        if self.connection_due(now) {
            match Link::connect(ports, Some(ctx.clone())) {
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

    pub(crate) fn set_context(&mut self, ctx: &egui::Context) {
        if self.context.is_none() {
            self.context = Some(ctx.clone());
        }
    }
}

fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(crate::localization::text("The system's MIDI service failed").into()))
}

fn input_names() -> Result<Vec<String>, String> {
    guarded(|| {
        let midi = MidiInput::new(NAME).map_err(|e| e.to_string())?;
        Ok(midi.ports().iter().filter_map(|port| midi.port_name(port).ok()).collect())
    })
}

fn output_names() -> Result<Vec<String>, String> {
    guarded(|| {
        let midi = MidiOutput::new(NAME).map_err(|e| e.to_string())?;
        Ok(midi.ports().iter().filter_map(|port| midi.port_name(port).ok()).collect())
    })
}

impl Link {
    fn connect(ports: &Ports, ctx: Option<egui::Context>) -> Result<Self, String> {
        guarded(|| Self::connect_midi(ports, ctx))
    }

    fn connect_midi(ports: &Ports, ctx: Option<egui::Context>) -> Result<Self, String> {
        let input = MidiInput::new(NAME).map_err(|e| e.to_string())?;
        let output = MidiOutput::new(NAME).map_err(|e| e.to_string())?;
        let (tx, packets) = sync_channel(512);
        let in_port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).ok().as_deref() == Some(ports.input.as_str()))
            .ok_or_else(|| missing_port(&ports.input))?;
        let out_port = output
            .ports()
            .into_iter()
            .find(|p| output.port_name(p).ok().as_deref() == Some(ports.output.as_str()))
            .ok_or_else(|| missing_port(&ports.output))?;
        let output = output.connect(&out_port, NAME).map_err(|e| e.to_string())?;
        let input = input
            .connect(
                &in_port,
                NAME,
                move |_, bytes, _| {
                    if let Ok(packet) = <[u8; 3]>::try_from(bytes)
                        && tx.try_send(packet).is_ok()
                        && let Some(ctx) = &ctx
                    {
                        ctx.request_repaint();
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            input: Some(input),
            output: Some(output),
            packets,
            protocol: ports.protocol,
            failed: false,
            check_at: Instant::now() + RETRY_INTERVAL,
            motorized: ports.motorized,
            decoder: Decode::default(),
            ping: None,
            heartbeat: Heartbeat::new(Instant::now()),
            ready: false,
            lamps: None,
        })
    }

    fn send(&mut self, packet: [u8; 3]) {
        if self.failed {
            return;
        }
        if let Some(output) = &mut self.output
            && let Err(error) = guarded(|| output.send(&packet).map_err(|error| error.to_string()))
        {
            log::debug!("{} MIDI output: {error}", self.protocol.label());
            self.failed = true;
        }
    }

    fn lamp(&mut self, strip: usize, index: usize, lit: bool) {
        let ports = if strip == STRIPS { TRANSPORT_LAMP_PORTS } else { STRIP_LAMP_PORTS };
        if strip > STRIPS {
            return;
        }
        match self.protocol {
            Protocol::Hui => {
                if let Some(port) = ports.get(index) {
                    let zone = if strip == STRIPS { TRANSPORT } else { strip as u8 };
                    self.send([0xb0, 0x0c, zone]);
                    self.send([0xb0, 0x2c, port | if lit { 0x40 } else { 0 }]);
                }
            }
            Protocol::Mcu => {
                if let Some(packet) = mcu::lamp(strip, index, lit) {
                    self.send(packet);
                }
            }
        }
    }

    fn check_ports(&mut self, ports: &Ports, now: Instant) {
        if now < self.check_at || self.failed {
            return;
        }
        self.check_at = now + RETRY_INTERVAL;
        self.failed = match input_names().and_then(|input| Ok((input, output_names()?))) {
            Ok((input, output)) => !ports_present(ports, &input, &output),
            Err(error) => {
                log::debug!("{} MIDI scan: {error}", self.protocol.label());
                true
            }
        };
        // Reassert the current Stop lamp to detect stale WinMM handles when a device went
        // away and came back between two scans, with no ordinary feedback changes.
        if self.protocol == Protocol::Mcu
            && let Some(lamps) = self.lamps
            && let Some(packet) = mcu::lamp(STRIPS, 0, lamps[STRIPS][0])
        {
            self.send(packet);
        }
    }

    fn needs_reopen(&self, now: Instant) -> bool {
        self.failed || (self.protocol.is_hui() && self.heartbeat.seen_reply && self.heartbeat.expired(now))
    }

    fn online(&self) -> bool {
        !self.failed && (!self.protocol.is_hui() || self.heartbeat.online(Instant::now()))
    }

    fn decode(&mut self, packet: [u8; 3], now: Instant) -> Option<Event> {
        match self.protocol {
            Protocol::Hui if packet == PONG || self.heartbeat.online(now) => self.decoder.packet(packet),
            Protocol::Hui => None,
            Protocol::Mcu => mcu::decode(packet),
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(lamps) = self.lamps.take() {
            for (strip, states) in lamps.into_iter().enumerate() {
                for (index, lit) in states.into_iter().enumerate() {
                    if lit {
                        self.lamp(strip, index, false);
                    }
                }
            }
        }
        let _ = guarded(|| {
            drop(self.input.take());
            Ok(())
        });
        let _ = guarded(|| {
            drop(self.output.take());
            Ok(())
        });
    }
}

fn ports_present(ports: &Ports, inputs: &[String], outputs: &[String]) -> bool {
    inputs.contains(&ports.input) && outputs.contains(&ports.output)
}

fn missing_port(name: &str) -> String {
    crate::localized_format!("MIDI port not found: {}", name)
}

fn mixer_tracks(app: &SoundApp) -> impl Iterator<Item = TrackId> + '_ {
    app.engine.session().tracks.iter().filter(|track| !track.hidden && track.kind != TrackKind::Video).map(|track| track.id)
}

fn mapped_tracks(app: &SoundApp, first: usize) -> [Option<TrackId>; STRIPS] {
    let mut tracks = mixer_tracks(app).skip(first);
    std::array::from_fn(|_| tracks.next())
}

fn master_fader_target(app: &SoundApp, protocol: Protocol) -> Option<TrackId> {
    if protocol != Protocol::Mcu {
        return None;
    }
    // SoundCraft can process several master tracks in series. A single physical
    // fader has an unambiguous target only when exactly one is active.
    let mut masters = app.engine.session().tracks.iter().filter(|t| t.kind == TrackKind::Master && !t.inactive);
    let master = masters.next()?;
    masters.next().is_none().then_some(master.id)
}

pub(crate) fn controlled_tracks(app: &SoundApp) -> [Option<TrackId>; STRIPS] {
    app.hui.link.as_ref().filter(|link| link.online()).map_or([None; STRIPS], |_| mapped_tracks(app, app.hui.first_track))
}

pub(crate) fn document_replaced(app: &mut SoundApp) {
    app.hui.first_track = 0;
    app.hui.motion = Motion::default();
    if let Some(link) = &mut app.hui.link {
        link.ready = false;
        link.decoder = Decode::default();
        link.lamps = None;
    }
}

pub(crate) fn gesture_active(app: &SoundApp) -> bool {
    app.engine.merge_key() == Some(MIX_GESTURE) && app.hui.motion.active(app.clock_now())
}

pub(crate) fn touching(app: &SoundApp, track: TrackId) -> bool {
    let (Some(frame), Some(link)) = (app.hui.frame, &app.hui.link) else { return false };
    link.online() && app.hui.motion.strips.iter().any(|strip| strip.track == Some(track) && strip.writing(frame, link.motorized))
}

fn navigate(app: &mut SoundApp, link: &mut Link, direction: Navigation) {
    let first = direction.target(app.hui.first_track, mixer_tracks(app).count());
    if first != app.hui.first_track {
        app.hui.first_track = first;
        link.decoder.coarse.fill(None);
        app.hui.motion.strips[..STRIPS].fill_with(Default::default);
        app.hui.motion.pan_until = None;
        if app.engine.merge_key() == Some(MIX_GESTURE) {
            app.engine.end_merge();
        }
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
    app.hui.motion = Motion::default();
    app.hui.retry_at = None;
    app.hui.first_track = 0;
    app.ui.hui = None;
    if let Some(inputs) = &mut app.midi_inputs {
        inputs.reserve_for_surface(ports.as_ref().map(Ports::input));
    }
    if let Some(p) = ports {
        let link = Link::connect(&p, app.hui.context.clone()).inspect_err(|e| {
            app.hui.error.clone_from(e);
            if let Some(inputs) = &mut app.midi_inputs {
                inputs.reserve_for_surface(None);
            }
        })?;
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
            app.hui.enabled = app.ui.hui.is_some();
            app.hui.draft = app.ui.hui.clone().unwrap_or_default();
            app.hui.inputs.clear();
            app.hui.outputs.clear();
            match input_names().and_then(|inputs| Ok((inputs, output_names()?))) {
                Ok((inputs, outputs)) => {
                    app.hui.inputs = inputs;
                    app.hui.outputs = outputs;
                    app.hui.error.clear();
                }
                Err(error) => app.hui.error = error,
            }
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
        let motorized = p.get("motorized").and_then(Value::as_bool).unwrap_or(false);
        apply(app, Some(Ports { input: input.into(), output: output.into(), protocol: protocol.unwrap_or_default(), motorized }))?;
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
    let now = app.clock_now();
    app.hui.frame = Some(now);
    let Some(mut link) = app.hui.take_link(ports, now, ctx) else { return };
    if link.protocol.is_hui() && link.ping.is_none_or(|at| now.saturating_duration_since(at) >= PING_INTERVAL) {
        link.send([0x90, 0, 0]);
        link.ping = Some(now);
    }
    let track_count = mixer_tracks(app).count();
    let last_bank = (track_count.saturating_sub(1) / STRIPS) * STRIPS;
    app.hui.first_track = app.hui.first_track.min(last_bank);
    let mut tracks = mapped_tracks(app, app.hui.first_track);
    let master = master_fader_target(app, link.protocol);
    app.hui.motion.map(tracks, master);
    let mut faders = [None; mcu::FADERS];
    let mut pans = [None; STRIPS];
    let mut jog_steps = 0;
    for index in 0..512 {
        let Ok(packet) = link.packets.try_recv() else { break };
        if index == 511 {
            ctx.request_repaint();
        }
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
                apply_faders(app, std::mem::take(&mut faders));
                apply_pans(app, tracks, std::mem::take(&mut pans));
                navigate(app, &mut link, direction);
                tracks = mapped_tracks(app, app.hui.first_track);
                app.hui.motion.map(tracks, master);
            }
            Event::Mute(strip) | Event::Solo(strip) => {
                if let Some(id) = tracks.get(strip).copied().flatten() {
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
                if let Some(fader) = faders.get_mut(strip)
                    && let Some(control) = app.hui.motion.strips.get_mut(strip)
                    && let Some(track) = control.track.and_then(|id| app.engine.session().track(id))
                    && control.accept(value, motion::position(track.mixer.volume_db), link.motorized, now)
                {
                    *fader = Some(value);
                }
            }
            Event::Touch(strip, held) => {
                if let Some(control) = app.hui.motion.strips.get_mut(strip) {
                    control.touch(held);
                }
            }
            Event::Pan(strip, steps) => {
                if let Some(pan) = pans.get_mut(strip)
                    && let Some(track) = tracks.get(strip).copied().flatten().and_then(|id| app.engine.session().track(id))
                    && let Some(current) = track.mixer.pan.first()
                {
                    *pan = Some((pan.unwrap_or(*current) + f32::from(steps) * PAN_STEP).clamp(-1.0, 1.0));
                    app.hui.motion.pan_until = Some(now + GESTURE_GAP);
                }
            }
        }
    }
    jog(app, jog_steps);
    apply_faders(app, faders);
    apply_pans(app, tracks, pans);
    for at in [Some(link.check_at), link.ping.map(|at| at + PING_INTERVAL), app.hui.motion.deadline(now)].into_iter().flatten() {
        ctx.request_repaint_after(at.saturating_duration_since(now));
    }
    if link.failed {
        ctx.request_repaint();
    }
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
            && app.engine.session().track(id).and_then(|t| t.mixer.pan.first()).is_some_and(|old| (*old - pan).abs() > 0.000_01)
        {
            let _ = app.engine.execute_merged("mix.pan", &json!({"tracks":[id.0], "pan":pan}), MIX_GESTURE);
        }
    }
}

fn apply_faders(app: &mut SoundApp, faders: [Option<u16>; mcu::FADERS]) {
    for (strip, value) in app.hui.motion.strips.iter().zip(faders) {
        if let (Some(value), Some(track)) = (value, strip.track.and_then(|id| app.engine.session().track(id))) {
            let db = soundcraft_model::fader_pos_to_db(f32::from(value) / FADER_MAX);
            if (track.mixer.volume_db - db).abs() > 0.001 {
                let _ = app.engine.execute_merged("mix.volume", &json!({"tracks":[track.id.0], "db":db}), MIX_GESTURE);
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
            tracks
                .get(i)
                .copied()
                .flatten()
                .and_then(|id| app.engine.session().track(id))
                .map_or([false; 3], |t| [t.mixer.mute, t.mixer.solo, t.mixer.record_arm])
        }
    });
    let previous = link.lamps.replace(lamps);
    if previous.is_none() && link.protocol == Protocol::Mcu {
        for note in 0x18..=0x1f {
            link.send([0x90, note, 0]);
        }
    }
    for (i, states) in lamps.into_iter().enumerate() {
        for (index, lit) in states.into_iter().enumerate() {
            if previous.is_some_and(|old| old[i][index] == lit) {
                continue;
            }
            link.lamp(i, index, lit);
        }
    }
    if link.motorized
        && let Some(frame) = app.hui.frame
    {
        for (index, strip) in app.hui.motion.strips.iter_mut().enumerate() {
            let Some(track) = strip.track.and_then(|id| app.engine.session().track(id)) else { continue };
            let position = motion::position(track.mixer.volume_db);
            if !strip.should_send(position, frame) {
                continue;
            }
            match link.protocol {
                Protocol::Hui => {
                    if let Some(packets) = hui_fader(index, position) {
                        for packet in packets {
                            link.send(packet);
                        }
                    }
                }
                Protocol::Mcu => {
                    if let Some(packet) = mcu::fader(index, position) {
                        link.send(packet);
                    }
                }
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
            ui.label(crate::localization::text("Faders"));
            ui.checkbox(&mut app.hui.draft.motorized, crate::localization::text("Motorized faders"));
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
    fn midi_failures_are_contained_and_missing_either_configured_port_requires_reopening() {
        assert_eq!(guarded(|| Ok(1)), Ok(1));
        assert!(guarded::<()>(|| panic!("WinMM test failure")).unwrap_err().contains("MIDI service failed"));
        let ports = Ports { input: "in".into(), output: "out".into(), protocol: Protocol::Mcu, motorized: false };
        assert!(ports_present(&ports, &["in".into()], &["out".into()]));
        assert!(!ports_present(&ports, &[], &["out".into()]));
        assert!(!ports_present(&ports, &["in".into()], &[]));
        assert_eq!(missing_port("in"), "MIDI port not found: in");
        let start = Instant::now();
        let (mut link, _) = test_link(Protocol::Hui, start);
        assert!(!link.needs_reopen(start + HEARTBEAT_TIMEOUT));
        link.heartbeat.receive(start);
        assert!(link.needs_reopen(start + HEARTBEAT_TIMEOUT));
    }

    fn test_link(protocol: Protocol, now: Instant) -> (Link, std::sync::mpsc::SyncSender<[u8; 3]>) {
        let (send, packets) = sync_channel(512);
        (
            Link {
                input: None,
                output: None,
                packets,
                protocol,
                failed: false,
                check_at: now + Duration::from_secs(60),
                motorized: false,
                decoder: Decode::default(),
                ping: None,
                heartbeat: Heartbeat::new(now),
                ready: false,
                lamps: None,
            },
            send,
        )
    }

    fn test_app() -> (SoundApp, TrackId, std::sync::mpsc::SyncSender<[u8; 3]>) {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.advance_test_clock(Duration::ZERO);
        let track = app.run("track.new", json!({"name":"Fader","kind":"audio","format":"mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.ui.hui = Some(Ports { protocol: Protocol::Mcu, ..Ports::default() });
        let (link, packets) = test_link(Protocol::Mcu, app.clock_now());
        app.hui.link = Some(link);
        (app, TrackId(track), packets)
    }

    #[test]
    fn ten_frames_of_fader_or_pan_make_one_undo_and_end_after_idle() {
        for is_fader in [true, false] {
            let (mut app, id, packets) = test_app();
            let ctx = egui::Context::default();
            let undo = app.engine.undo_history().len();
            if is_fader {
                packets.send(mcu::fader(0, motion::position(0.0)).unwrap()).unwrap();
            }
            for step in 1..=10 {
                app.finish_idle_mixer_gesture();
                let packet = if is_fader { mcu::fader(0, motion::position(0.0) - step * 200).unwrap() } else { [0xb0, 0x10, 1] };
                packets.send(packet).unwrap();
                poll(&mut app, &ctx);
                app.advance_test_clock(Duration::from_millis(16));
            }
            assert_eq!(app.engine.undo_history().len(), undo + 1);
            app.advance_test_clock(GESTURE_GAP);
            app.finish_idle_mixer_gesture();
            assert!(app.engine.merge_key().is_none());
            app.run("edit.undo", json!({})).unwrap();
            let track = app.engine.session().track(id).unwrap();
            assert_eq!(track.mixer.volume_db, 0.0);
            assert_eq!(track.mixer.pan[0], 0.0);
        }
    }

    #[test]
    fn mcu_master_fader_follows_the_main_mix_across_banks() {
        let (mut app, _, packets) = test_app();
        app.run("track.new", json!({"count": 15, "kind":"audio","format":"mono"})).unwrap();
        let ctx = egui::Context::default();
        let move_to = |position: u16| packets.send([0xe8, (position & 127) as u8, (position >> 7) as u8]).unwrap();
        move_to(16_383);
        poll(&mut app, &ctx);
        assert!(app.engine.session().tracks.iter().all(|t| t.mixer.volume_db == 0.0));

        let add_master =
            |app: &mut SoundApp| TrackId(app.run("track.new", json!({"kind":"master","format":"stereo"})).unwrap()["tracks"][0].as_u64().unwrap());
        let master = add_master(&mut app);
        move_to(0);
        poll(&mut app, &ctx);
        assert_eq!(app.engine.session().track(master).unwrap().mixer.volume_db, 0.0);
        move_to(16_383);
        poll(&mut app, &ctx);
        assert_eq!(app.engine.session().track(master).unwrap().mixer.volume_db, 12.0);
        packets.send([0x90, 0x2f, 127]).unwrap();
        move_to(0);
        poll(&mut app, &ctx);
        assert_eq!(app.hui.first_track, 8);
        assert_eq!(app.engine.session().track(master).unwrap().mixer.volume_db, -144.0);
        assert!(app.engine.session().tracks.iter().filter(|t| t.id != master).all(|t| t.mixer.volume_db == 0.0));

        let second = add_master(&mut app);
        move_to(16_383);
        poll(&mut app, &ctx);
        assert_eq!(app.engine.session().track(master).unwrap().mixer.volume_db, -144.0);
        assert_eq!(app.engine.session().track(second).unwrap().mixer.volume_db, 0.0);
    }

    #[test]
    fn bank_change_ignores_the_old_fader_position_until_pickup() {
        let (mut app, first, packets) = test_app();
        app.run("track.new", json!({"count": 8, "kind":"audio","format":"mono"})).unwrap();
        let ninth = app.engine.session().tracks.last().unwrap().id;
        let ctx = egui::Context::default();
        for packet in [mcu::fader(0, motion::position(0.0)).unwrap(), mcu::fader(0, motion::position(-20.0)).unwrap()] {
            packets.send(packet).unwrap();
        }
        poll(&mut app, &ctx);
        let first_level = app.engine.session().track(first).unwrap().mixer.volume_db;
        assert!((first_level + 20.0).abs() < 0.01);
        for packet in [[0x90, 0x2f, 127], mcu::fader(0, motion::position(-18.0)).unwrap()] {
            packets.send(packet).unwrap();
        }
        poll(&mut app, &ctx);
        assert_eq!(app.engine.session().track(ninth).unwrap().mixer.volume_db, 0.0);
        for packet in [mcu::fader(0, motion::position(0.0)).unwrap(), mcu::fader(0, motion::position(3.0)).unwrap()] {
            packets.send(packet).unwrap();
        }
        poll(&mut app, &ctx);
        assert!((app.engine.session().track(ninth).unwrap().mixer.volume_db - 3.0).abs() < 0.01);
        assert_eq!(app.engine.session().track(first).unwrap().mixer.volume_db, first_level);
    }

    #[test]
    fn surface_touch_and_latch_write_volume_and_share_the_pass_undo() {
        use soundcraft_model::{AutoParam, AutomationMode};
        for (mode, continues) in [(AutomationMode::Touch, false), (AutomationMode::Latch, true)] {
            let (mut app, id, packets) = test_app();
            app.engine.session_mut().track_mut(id).unwrap().mixer.automation_mode = mode;
            app.engine.transport.playing = true;
            let ctx = egui::Context::default();
            let undo = app.engine.undo_history().len();
            packets.send([0x90, 0x68, 127]).unwrap();
            packets.send(mcu::fader(0, motion::position(0.0)).unwrap()).unwrap();
            for (position, level) in [(1_000, -3.0), (2_000, -6.0)] {
                app.engine.transport.position = position;
                app.hold_automation_pass();
                packets.send(mcu::fader(0, motion::position(level)).unwrap()).unwrap();
                poll(&mut app, &ctx);
                app.write_automation();
                app.advance_test_clock(Duration::from_millis(25));
            }
            let lane = |app: &SoundApp| {
                app.engine.session().track(id).unwrap().automation.iter().find(|lane| lane.param == AutoParam::Volume).unwrap().points.clone()
            };
            let before_release = lane(&app);
            assert_eq!(before_release.last().unwrap().at, 2_000);
            assert_eq!(app.engine.undo_history().len(), undo + 1);
            packets.send([0x90, 0x68, 0]).unwrap();
            app.engine.transport.position = 3_000;
            poll(&mut app, &ctx);
            app.write_automation();
            if continues {
                assert_eq!(lane(&app).last().unwrap().at, 3_000);
            } else {
                assert_eq!(lane(&app), before_release);
            }
            assert_eq!(app.engine.undo_history().len(), undo + 1);
        }
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
}
