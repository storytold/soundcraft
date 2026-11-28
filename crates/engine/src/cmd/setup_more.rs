//! More Options toggles, the Setup menu (hardware, playback engine, I/O, timecode, click,
//! shortcuts, MIDI setup) and File › Import › Session Data.
use super::{CommandSpec, always, bad, command_specs, i64_or, str_param, track_param, tracks_param};

use super::more_util::{set_text, store_value, text, toggle_flag, values_with_prefix};
use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{BusId, ChannelFormat, ClipContent, OutputPath, Route, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::Timecode;
use std::path::Path;

macro_rules! option {
    ($id:literal, $label:literal, $flag:literal) => {
        cmd!(noundo $id, $label, ["Options"], None, "{value?: bool} (toggles when omitted)", always, |e, p| toggle_flag(e, p, $flag))
    };
}

const MIDI_FILTER_KINDS: [&str; 8] = ["notes", "pitch_bend", "mono_aftertouch", "poly_aftertouch", "program", "cc", "sysex", "realtime"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        option!("options.destructive_punch", "DestructivePunch", "options.destructive_punch"),
        option!("options.transport_online", "Transport Online", "options.transport_online"),
        option!("options.dynamic_transport", "Dynamic Transport", "options.dynamic_transport"),
        option!(
            "options.marker_target_follows_selection",
            "Marker Target Follows First Selected Track",
            "options.marker_target_follows_first_selected_track"
        ),
        option!("options.midi_thru", "MIDI Thru", "options.midi_thru"),
        option!("options.auto_spot_clips", "Auto-Spot Clips", "options.auto_spot_clips"),
        option!("options.keyboard_lock", "Edit/Tool Mode Keyboard Lock", "options.edit_tool_mode_keyboard_lock"),
        option!("options.calibration_mode", "Calibration Mode", "options.calibration_mode"),
        option!("options.midi_live_mode", "MIDI Live Mode", "options.midi_live_mode"),
        option!("options.low_latency_monitoring", "Low Latency Monitoring", "options.low_latency_monitoring"),
        cmd!(
            "options.prepare_dpe_tracks",
            "Prepare DPE Tracks",
            ["Options"],
            None,
            "{tracks?} — consolidates each record-armed (or given) audio track into one continuous clip for destructive punch",
            always,
            prepare_dpe
        ),
        cmd!(
            noundo "setup.hardware",
            "Hardware...",
            ["Setup"],
            None,
            "{device?: output name, buffer_size?: 16..=8192 or null} — output device and buffer. Applied only in the desktop app (Playback Engine window / ui.audio_device), which returns the open device. Headless stores the request and returns applied: false; buffer_size is null when unset. Any integer in range, not rounded to the window's power-of-two sizes. Sample rate is the session's",
            always,
            |e, p| engine_settings(e, p, "setup.hardware")
        ),
        cmd!(
            noundo "setup.playback_engine",
            "Playback Engine...",
            ["Setup"],
            None,
            "{device?: output name, buffer_size?: 16..=8192 or null, delay_compensation?: bool} — device and buffer as setup.hardware (applied only in the desktop app, or via ui.audio_device); delay_compensation is applied here too. Headless returns applied: false and does not invent a buffer",
            always,
            |e, p| engine_settings(e, p, "setup.playback_engine")
        ),
        cmd!(noundo "setup.disk_allocation", "Disk Allocation...", ["Setup"], None, "{path?, track?} — the record folder for the session, or for one track", always, disk_allocation),
        cmd!(noundo "setup.peripherals", "Peripherals...", ["Setup"], None, "{settings?: {name: number|bool|string}} — control surface / sync device settings", always, peripherals),
        cmd!(
            "setup.io",
            "I/O...",
            ["Setup"],
            None,
            "{action?: list|create_bus|rename_bus|delete_bus|create_output|set_output|rename_output|delete_output|default_outputs, name?, new_name?, format?: stereo, first_channel?: 0, channels?: device outputs (default_outputs)}",
            always,
            io_setup
        ),
        cmd!(
            "setup.current_timecode",
            "Current Timecode Position...",
            ["Setup"],
            None,
            "{timecode: 'HH:MM:SS:FF'} — sets the session start so the insertion point reads this timecode",
            always,
            current_timecode
        ),
        cmd!(noundo "setup.current_feet_frames", "Current Feet+Frames Position...", ["Setup"], None, "{feet_frames: 'F+FF'} — the feet+frames reading at the insertion point", always, current_feet_frames),
        cmd!(noundo "setup.external_timecode_offset", "External Timecode Offset...", ["Setup"], None, "{frames?: ±, samples?: ±} — offset applied to incoming timecode", always, |e, p| offset_setting(e, p, "sync.external_offset")),
        cmd!(noundo "setup.video_sync_offset", "Video Sync Offset...", ["Setup"], None, "{frames?: ±, samples?: ±} — offset between audio and video playback", always, |e, p| offset_setting(e, p, "video.sync_offset")),
        cmd!(noundo "setup.click_countoff", "Click/Countoff...", ["Setup"], None, "{volume_db?: -60..12, accent_note?: 0..127, normal_note?: 0..127, accent_velocity?, normal_velocity?, countoff?: bool, countoff_bars?: 1..16, only_during_record?: bool}", always, click_countoff),
        cmd!(query "setup.keyboard_shortcuts", "Keyboard Shortcuts...", ["Setup"], None, "{filter?} — every command shortcut", always, |_, p| {
            let f = str_param(p, "filter").map(str::to_ascii_lowercase);
            let rows: Vec<Value> = command_specs()
                .iter()
                .filter_map(|c| c.shortcut.map(|sc| (c, sc)))
                .filter(|(c, _)| f.as_ref().is_none_or(|f| c.id.contains(f.as_str()) || c.label.to_ascii_lowercase().contains(f.as_str())))
                .map(|(c, sc)| json!({"id": c.id, "label": c.label, "menu": c.menu.join(" > "), "shortcut": sc}))
                .collect();
            Ok(json!({"shortcuts": rows}))
        }),
        cmd!(noundo "setup.midi_studio", "MIDI Studio...", ["Setup", "MIDI"], None, "{devices?: [names]} — the MIDI devices in the studio", always, |e, p| {
            let s = e.session_mut();
            if let Some(a) = p.get("devices").and_then(Value::as_array) {
                let names: Vec<&str> = a.iter().filter_map(Value::as_str).take(64).collect();
                set_text(&mut s.edit, "midi.studio.devices", Some(&names.join("|")));
            }
            let devices: Vec<String> = text(&s.edit, "midi.studio.devices").map(|t| t.split('|').map(str::to_string).collect()).unwrap_or_default();
            Ok(json!({"devices": devices}))
        }),
        cmd!(noundo "setup.midi_beat_clock", "MIDI Beat Clock...", ["Setup", "MIDI"], None, "{enabled?: bool, offset?: samples, destination?: name}", always, |e, p| {
            let s = e.session_mut();
            if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
                s.edit.set_flag("midi.beat_clock", v);
            }
            store_value(&mut s.edit, p, "offset", "midi.beat_clock.offset", -1e7, 1e7);
            if let Some(d) = str_param(p, "destination") {
                set_text(&mut s.edit, "midi.beat_clock.destination", Some(d));
            }
            Ok(json!({"enabled": s.edit.flag("midi.beat_clock"), "offset": s.edit.value("midi.beat_clock.offset", 0.0), "destination": text(&s.edit, "midi.beat_clock.destination")}))
        }),
        cmd!(noundo "setup.midi_input_filter", "MIDI Input Filter...", ["Setup", "MIDI"], None, "{filter?: [notes|pitch_bend|mono_aftertouch|poly_aftertouch|program|cc|sysex|realtime], allow?: [...]} — filtered kinds are not recorded", always, midi_filter),
        cmd!(noundo "setup.midi_input_devices", "MIDI Input Devices...", ["Setup", "MIDI"], None, "{enable?: [names], disable?: [names]}", always, midi_inputs),
        cmd!(noundo "setup.transcription_settings", "Transcription Settings...", ["Setup"], None, "{enabled?: bool, language?: 'en'}", always, |e, p| {
            let s = e.session_mut();
            if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
                s.edit.set_flag("transcription.enabled", v);
            }
            if let Some(l) = str_param(p, "language") {
                set_text(&mut s.edit, "transcription.language", Some(l));
            }
            Ok(json!({"enabled": s.edit.flag("transcription.enabled"), "language": text(&s.edit, "transcription.language").unwrap_or_else(|| "en".into())}))
        }),
        cmd!(
            "file.import_session_data",
            "Session Data...",
            ["File", "Import"],
            Some("Alt+Shift+I"),
            "{path, tracks?: [names]} — imports tracks (clips, playlists, mixer, automation, audio) from another session",
            always,
            import_session_data
        ),
    ]
}

fn prepare_dpe(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "options.prepare_dpe_tracks";
    let s = e.session();
    let tracks: Vec<TrackId> = if p.get("tracks").is_some() || p.get("track").is_some() {
        tracks_param(e, id, p)?
    } else {
        s.tracks.iter().filter(|t| t.mixer.record_arm).map(|t| t.id).collect()
    };
    let tracks: Vec<TrackId> = tracks.into_iter().filter(|t| s.track(*t).is_some_and(|x| x.kind == TrackKind::Audio)).collect();
    if tracks.is_empty() {
        return Err(bad(id, "record-enable (or pass) the audio tracks to prepare"));
    }
    let end = s.content_end();
    if end <= 0 {
        return Err(bad(id, "there is no audio to prepare"));
    }
    let r = soundcraft_time::Range::new(0, end);
    if tracks.iter().any(|&t| crate::edit::holds_locked(e.session(), t, crate::io::bounded_to_track(e.session(), t, r))) {
        return Err(bad(id, crate::edit::LOCKED_IN_RANGE));
    }
    let mut n = 0;
    for t in tracks {
        // Each track keeps its own length (its material plus a second), as before.
        let tr = crate::io::bounded_to_track(e.session(), t, r);
        if crate::io::consolidate(e, id, t, tr)? {
            n += 1;
        }
    }
    e.session_mut().edit.set_flag("options.dpe_prepared", true);
    Ok(json!({"prepared": n}))
}

/// A `buffer_size` argument: absent, cleared (`null`), or an integer in 16..=8192.
enum BufferParam {
    Skip,
    Clear,
    Set(u64),
}

/// Same range as `ui.audio_device`. Out of range and non-numbers error; there is no clamp and no
/// power-of-two rounding (the Playback Engine window lists those sizes, the command keeps the number).
fn buffer_param(id: &str, p: &Value) -> Result<BufferParam> {
    match p.get("buffer_size") {
        None => Ok(BufferParam::Skip),
        Some(Value::Null) => Ok(BufferParam::Clear),
        Some(v) => {
            let n = v.as_u64().filter(|n| (16..=8192).contains(n)).ok_or_else(|| bad(id, "`buffer_size` must be a number in 16..=8192 or null"))?;
            Ok(BufferParam::Set(n))
        }
    }
}

/// `device` as `ui.audio_device`'s `output`: a name, or `null`/empty to clear. Absent leaves it.
fn device_param<'a>(id: &str, p: &'a Value) -> Result<Option<Option<&'a str>>> {
    match p.get("device") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(name)) => Ok(Some((!name.is_empty()).then_some(name.as_str()))),
        Some(_) => Err(bad(id, "`device` must be a string or null")),
    }
}

/// Headless `setup.hardware` / `setup.playback_engine`. Playback never reads `engine.device` or
/// `engine.buffer_size` (only this command does); they are kept so a later call can report the
/// request. The result says `applied: false` and leaves `buffer_size` null until one was stored.
fn engine_settings(e: &mut Engine, p: &Value, id: &str) -> Result<Value> {
    let buffer = buffer_param(id, p)?;
    let device = device_param(id, p)?;
    if id == "setup.playback_engine"
        && let Some(on) = p.get("delay_compensation").and_then(Value::as_bool)
    {
        e.session_mut().edit.delay_compensation = on;
    }
    let s = e.session_mut();
    match buffer {
        BufferParam::Skip => {}
        BufferParam::Clear => {
            s.edit.values.remove("engine.buffer_size");
        }
        BufferParam::Set(n) => {
            s.edit.values.insert("engine.buffer_size".into(), n as f64);
        }
    }
    if let Some(name) = device {
        set_text(&mut s.edit, "engine.device", name);
    }
    // Exact integers only: a hand-edited fraction is not reported as a buffer size.
    let buffer_size = s.edit.values.get("engine.buffer_size").copied().and_then(|v| {
        let n = v as u64;
        (v.is_finite() && (n as f64) == v).then_some(n)
    });
    Ok(json!({
        "applied": false,
        "device": text(&s.edit, "engine.device"),
        "buffer_size": buffer_size,
        "sample_rate": s.sample_rate.hz(),
        "bit_depth": s.bit_depth,
        "delay_compensation": s.edit.delay_compensation,
    }))
}

fn disk_allocation(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.disk_allocation";
    let t = track_param(e, id, p, "track")?;
    let s = e.session_mut();
    if let Some(path) = str_param(p, "path") {
        let key = t.map_or_else(|| "disk.root".to_string(), |t| format!("disk.track.{}", t.0));
        set_text(&mut s.edit, &key, Some(path));
    }
    let root = text(&s.edit, "disk.root").unwrap_or_else(|| "Audio Files".into());
    let rows: Vec<Value> = s
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| json!({"track": t.id, "name": t.name, "path": text(&s.edit, &format!("disk.track.{}", t.id.0)).unwrap_or_else(|| root.clone())}))
        .collect();
    Ok(json!({"root": root, "tracks": rows}))
}

fn peripherals(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(m) = p.get("settings").and_then(Value::as_object) {
        for (k, v) in m.iter().take(256) {
            let key: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).take(64).collect();
            if key.is_empty() {
                continue;
            }
            let key = format!("peripherals.{key}");
            match v {
                Value::Bool(b) => s.edit.set_flag(&key, *b),
                Value::Number(n) => {
                    if let Some(x) = n.as_f64().filter(|x| x.is_finite()) {
                        s.edit.values.insert(key, x);
                    }
                }
                Value::String(t) => set_text(&mut s.edit, &key, Some(t)),
                _ => {}
            }
        }
    }
    let flags: Vec<&String> = s.edit.flags.iter().filter(|f| f.starts_with("peripherals.")).collect();
    Ok(json!({"values": values_with_prefix(&s.edit, "peripherals."), "flags": flags}))
}

fn io_state(s: &Session) -> Value {
    json!({"busses": s.busses, "outputs": s.outputs})
}

/// Disconnect every track input, output and send that used bus `bid`.
fn unroute_bus(s: &mut Session, bid: BusId) {
    for t in &mut s.tracks {
        if t.mixer.input == Route::Bus(bid) {
            t.mixer.input = Route::None;
        }
        if t.mixer.output == Route::Bus(bid) {
            t.mixer.output = Route::None;
        }
        for slot in &mut t.mixer.sends {
            if slot.as_ref().is_some_and(|x| x.target == Route::Bus(bid)) {
                *slot = None;
            }
        }
    }
}

fn io_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.io";
    let action = str_param(p, "action").unwrap_or("list");
    if action == "list" {
        return Ok(io_state(e.session()));
    }
    if action == "default_outputs" {
        let channels = usize::try_from(i64_or(p, "channels", 0).clamp(0, soundcraft_mix::MAX_HARDWARE_CHANNELS as i64)).unwrap_or(0);
        default_outputs(e.session_mut(), channels);
        return Ok(io_state(e.session()));
    }
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(id, "`name` required"))?.to_string();
    let new_name = str_param(p, "new_name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let format = match str_param(p, "format") {
        Some(f) => ChannelFormat::from_id(f).ok_or_else(|| bad(id, format!("unknown format `{f}`")))?,
        None => ChannelFormat::Stereo,
    };
    let s = e.session_mut();
    match action {
        "create_bus" => {
            if s.bus_by_name(&name).is_some() {
                return Err(bad(id, format!("a bus named `{name}` exists")));
            }
            check_name_free_of_outputs(s, &name)?;
            s.add_bus(&name, format);
        }
        "rename_bus" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            if s.bus_by_name(&to).is_some() {
                return Err(bad(id, format!("a bus named `{to}` exists")));
            }
            check_name_free_of_outputs(s, &to)?;
            let b = s.busses.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            b.name = to;
        }
        "delete_bus" => {
            let bid: BusId = s.bus_by_name(&name).map(|b| b.id).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            s.busses.retain(|b| b.id != bid);
            unroute_bus(s, bid);
        }
        "create_output" => {
            check_output_name(&name)?;
            if s.outputs.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
                return Err(bad(id, format!("an output named `{name}` exists")));
            }
            check_name_free_of_busses(s, &name)?;
            let first = output_first_channel(i64_or(p, "first_channel", 0), format)?;
            s.outputs.push(OutputPath { name, first_channel: first, format });
        }
        "rename_output" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            if s.outputs.iter().any(|o| o.name.eq_ignore_ascii_case(&to) && !o.name.eq_ignore_ascii_case(&name)) {
                return Err(bad(id, format!("an output named `{to}` exists")));
            }
            check_name_free_of_busses(s, &to)?;
            let index = s.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            if index != 0 {
                check_output_name(&to)?;
            }
            let o = &mut s.outputs[index];
            let old = std::mem::replace(&mut o.name, to.clone());
            rename_output_routes(s, &old, Some(&to));
        }
        "set_output" => {
            let (index, output) = s
                .outputs
                .iter_mut()
                .enumerate()
                .find(|(_, o)| o.name.eq_ignore_ascii_case(&name))
                .ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            let first = i64_or(p, "first_channel", i64::from(output.first_channel));
            let format = if p.get("format").is_some() { format } else { output.format };
            if index == 0 && format != output.format {
                return Err(bad(id, "use Main output format to change the main mix width"));
            }
            output.first_channel = output_first_channel(first, format)?;
            output.format = format;
        }
        "delete_output" => {
            let index = s.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            if index == 0 {
                return Err(bad(id, "the main output path cannot be deleted"));
            }
            let path = s.outputs.remove(index);
            // As Pro Tools does (Reference Guide, "Deleting Paths", printed p. 85), the track
            // outputs and sends on the path are reset to No Output.
            rename_output_routes(s, &path.name, None);
        }
        other => return Err(bad(id, format!("unknown action `{other}`"))),
    }
    Ok(io_state(s))
}

/// Whether a track output or send uses the output path `name`.
fn output_in_use(s: &Session, name: &str) -> bool {
    let uses = |route: &Route| matches!(route, Route::Hardware(path) if path.eq_ignore_ascii_case(name));
    s.tracks.iter().any(|t| uses(&t.mixer.output) || t.mixer.sends.iter().flatten().any(|send| uses(&send.target)))
}

/// Point every track output and send on the output path `old` at `new`, or at no output.
fn rename_output_routes(s: &mut Session, old: &str, new: Option<&str>) {
    for t in &mut s.tracks {
        let routes = std::iter::once(&mut t.mixer.output).chain(t.mixer.sends.iter_mut().flatten().map(|send| &mut send.target));
        for route in routes {
            if matches!(route, Route::Hardware(path) if path.eq_ignore_ascii_case(old)) {
                *route = new.map_or(Route::None, |new| Route::Hardware(new.to_string()));
            }
        }
    }
}

/// The Output page's Default, as in Pro Tools (Reference Guide, "Restoring Default Paths and Path
/// Names", printed p. 82–83): delete the paths no track output or send uses, give a path in use on
/// an interface pair that pair's default name ("Out 5-6"), put the main path back on channels 1-2
/// as "Out 1-2", and make a stereo path for every further pair of the device's `channels`
/// outputs. A path in use with no default name (a mono or wider path, or one off the pairs) stays.
/// The paths are then in channel order, the main path first.
fn default_outputs(s: &mut Session, channels: usize) {
    let mut kept = Vec::new();
    let paths = std::mem::take(&mut s.outputs);
    for (index, path) in paths.into_iter().enumerate() {
        if index == 0 {
            kept.push(OutputPath { name: "Out 1-2".into(), first_channel: 0, ..path });
        } else if output_in_use(s, &path.name) {
            kept.push(path);
        }
    }
    s.outputs = kept;
    for index in 1..s.outputs.len() {
        let path = &s.outputs[index];
        let first = usize::from(path.first_channel);
        if path.format.channels() != 2 || first % 2 != 0 {
            continue;
        }
        let name = format!("Out {}-{}", first + 1, first + 2);
        if s.outputs.iter().any(|other| other.name.eq_ignore_ascii_case(&name)) || s.bus_by_name(&name).is_some() {
            continue;
        }
        let old = std::mem::replace(&mut s.outputs[index].name, name.clone());
        rename_output_routes(s, &old, Some(&name));
    }
    for first in (2..channels.saturating_sub(1)).step_by(2) {
        let name = format!("Out {}-{}", first + 1, first + 2);
        let covered = s
            .outputs
            .iter()
            .any(|path| path.name.eq_ignore_ascii_case(&name) || (usize::from(path.first_channel) == first && path.format.channels() == 2));
        if !covered && s.bus_by_name(&name).is_none() {
            s.outputs.push(OutputPath { name, first_channel: first as u16, format: ChannelFormat::Stereo });
        }
    }
    if let Some(paths) = s.outputs.get_mut(1..) {
        paths.sort_by_key(|path| path.first_channel);
    }
}

/// Routes name busses and output paths alike (`track.output`), so one name cannot be both.
fn check_name_free_of_busses(s: &Session, name: &str) -> Result<()> {
    if s.bus_by_name(name).is_some() {
        return Err(bad("setup.io", format!("a bus named `{name}` exists")));
    }
    Ok(())
}

fn check_name_free_of_outputs(s: &Session, name: &str) -> Result<()> {
    if s.outputs.iter().any(|o| o.name.eq_ignore_ascii_case(name)) {
        return Err(bad("setup.io", format!("an output named `{name}` exists")));
    }
    Ok(())
}

fn check_output_name(name: &str) -> Result<()> {
    if ["none", "no input", "no output", "main", "out 1-2", "master"].iter().any(|word| name.eq_ignore_ascii_case(word)) {
        return Err(bad("setup.io", format!("`{name}` is reserved for routing")));
    }
    Ok(())
}

fn output_first_channel(first: i64, format: ChannelFormat) -> Result<u16> {
    let width = format.channels();
    if first < 0
        || first.saturating_add(width as i64) > soundcraft_mix::MAX_HARDWARE_CHANNELS as i64
        || !(1..=soundcraft_mix::MAX_CHANNELS).contains(&width)
    {
        return Err(bad("setup.io", "output path is outside the supported channel range"));
    }
    Ok(first as u16)
}

fn current_timecode(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.current_timecode";
    let tc = str_param(p, "timecode").ok_or_else(|| bad(id, "`timecode` required (HH:MM:SS:FF)"))?;
    let tc = Timecode::parse(tc).map_err(|err| bad(id, err.to_string()))?;
    let s = e.session_mut();
    let at = s.edit.selection.start.max(0);
    let abs = tc.to_samples(s.frame_rate, s.sample_rate);
    s.timecode_start = abs.saturating_sub(at).clamp(-soundcraft_time::MAX_POSITION, soundcraft_time::MAX_POSITION);
    Ok(json!({"timecode_start": s.timecode_start, "at": at}))
}

fn current_feet_frames(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.current_feet_frames";
    let text_v = str_param(p, "feet_frames").ok_or_else(|| bad(id, "`feet_frames` required (F+FF)"))?;
    let (ft, fr) = text_v.split_once('+').ok_or_else(|| bad(id, "use F+FF, e.g. 12+08"))?;
    let ft: i64 = ft.trim().parse().map_err(|_| bad(id, "bad feet"))?;
    let fr: i64 = fr.trim().parse().map_err(|_| bad(id, "bad frames"))?;
    if !(0..=10_000_000).contains(&ft) || !(0..16).contains(&fr) {
        return Err(bad(id, "feet must be 0.. and frames 0..15"));
    }
    let s = e.session_mut();
    let at = s.edit.selection.start.max(0);
    let frames_at = s.frame_rate.frames_at(at, s.sample_rate);
    let start = ft * 16 + fr - frames_at;
    s.edit.values.insert(soundcraft_model::FEET_FRAMES_START.into(), start as f64);
    Ok(json!({"feet_frames_start": start}))
}

fn offset_setting(e: &mut Engine, p: &Value, key: &str) -> Result<Value> {
    let s = e.session_mut();
    let sr = s.sample_rate;
    let per_frame = s.frame_rate.samples_per_frame(sr);
    if let Some(f) = p.get("frames").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        s.edit.values.insert(key.to_string(), (f.clamp(-10_000.0, 10_000.0) * per_frame).round());
    } else if let Some(n) = p.get("samples").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        let lim = sr.as_f64() * 3600.0;
        s.edit.values.insert(key.to_string(), n.clamp(-lim, lim).round());
    }
    let samples = s.edit.value(key, 0.0);
    Ok(json!({"samples": samples, "frames": samples / per_frame.max(1e-9)}))
}

fn click_countoff(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    store_value(&mut s.edit, p, "volume_db", "click.volume_db", -60.0, 12.0);
    store_value(&mut s.edit, p, "accent_note", "click.accent_note", 0.0, 127.0);
    store_value(&mut s.edit, p, "normal_note", "click.normal_note", 0.0, 127.0);
    store_value(&mut s.edit, p, "accent_velocity", "click.accent_velocity", 1.0, 127.0);
    store_value(&mut s.edit, p, "normal_velocity", "click.normal_velocity", 1.0, 127.0);
    if let Some(v) = p.get("countoff").and_then(Value::as_bool) {
        s.edit.countoff = v;
    }
    if let Some(b) = p.get("countoff_bars").and_then(Value::as_u64) {
        s.edit.countoff_bars = b.clamp(1, 16) as u32;
    }
    if let Some(v) = p.get("only_during_record").and_then(Value::as_bool) {
        s.edit.set_flag("click.only_during_record", v);
    }
    Ok(json!({
        "volume_db": s.edit.value("click.volume_db", -6.0),
        "accent_note": s.edit.value("click.accent_note", 84.0),
        "normal_note": s.edit.value("click.normal_note", 72.0),
        "accent_velocity": s.edit.value("click.accent_velocity", 127.0),
        "normal_velocity": s.edit.value("click.normal_velocity", 100.0),
        "countoff": s.edit.countoff,
        "countoff_bars": s.edit.countoff_bars,
        "only_during_record": s.edit.flag("click.only_during_record"),
    }))
}

fn kinds(p: &Value, key: &str) -> Vec<&'static str> {
    p.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(|k| MIDI_FILTER_KINDS.iter().copied().find(|x| *x == k)).collect())
        .unwrap_or_default()
}

fn midi_filter(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    for k in kinds(p, "filter") {
        s.edit.set_flag(&format!("midi.filter.{k}"), true);
    }
    for k in kinds(p, "allow") {
        s.edit.set_flag(&format!("midi.filter.{k}"), false);
    }
    let filtered: Vec<&str> = MIDI_FILTER_KINDS.iter().copied().filter(|k| s.edit.flag(&format!("midi.filter.{k}"))).collect();
    Ok(json!({"filtered": filtered}))
}

fn device_key(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control() && *c != '=').take(64).collect();
    format!("midi.input.{}", n.trim())
}

fn midi_inputs(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    for (key, on) in [("enable", true), ("disable", false)] {
        for n in p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(64).collect::<Vec<_>>()).unwrap_or_default() {
            if !n.trim().is_empty() {
                s.edit.set_flag(&device_key(n), on);
            }
        }
    }
    let enabled: Vec<&str> = s.edit.flags.iter().filter_map(|f| f.strip_prefix("midi.input.")).collect();
    Ok(json!({"enabled": enabled}))
}

fn import_session_data(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "file.import_session_data";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let text_v = crate::io::read_session_text(&path)?;
    let other = Session::from_json(&text_v).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    let names: Option<Vec<String>> =
        p.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let picked: Vec<soundcraft_model::Track> = match &names {
        Some(ns) => {
            let mut v = Vec::new();
            for n in ns {
                v.push(other.track_by_name(n).cloned().ok_or_else(|| bad(id, format!("`{path}` has no track named `{n}`")))?);
            }
            v
        }
        None => other.tracks.iter().filter(|t| t.kind != TrackKind::Master).cloned().collect(),
    };
    if picked.is_empty() {
        return Err(bad(id, "nothing to import"));
    }
    let mut needed: Vec<SourceId> =
        picked.iter().flat_map(|t| t.playlists.iter().flat_map(|pl| pl.clips.iter().filter_map(|c| c.source()))).collect();
    // A Sampler's sample is session audio too.
    needed.extend(picked.iter().filter_map(|t| t.instrument.as_ref().and_then(|i| i.sample)));
    needed.sort_by_key(|x| x.0);
    needed.dedup();
    let sr = e.session().sample_rate.hz();
    let s = e.session_mut();
    let mut files = crate::io::DecodedFiles::new();
    let mut map: std::collections::BTreeMap<u64, SourceId> = Default::default();
    let mut missing = Vec::new();
    for src in needed.iter().filter_map(|sid| other.source(*sid)) {
        let mut ns = src.clone();
        ns.id = SourceId(s.alloc());
        match crate::io::stored_path(&src.path, Path::new(&path)) {
            Ok(abs) => {
                ns.path = abs.to_string_lossy().into_owned();
                match crate::io::decode_file(&mut files, &abs, sr) {
                    Ok(audio) => {
                        ns.frames = audio.buffer.frames() as u64;
                        ns.sample_rate = sr;
                        ns.unsaved = true;
                        s.pool.insert(ns.id, audio);
                    }
                    Err(_) => {
                        ns.unsaved = false;
                        missing.push(ns.path.clone());
                    }
                }
            }
            Err(why) => {
                ns.unsaved = false;
                missing.push(format!("{}: {why}", ns.path));
            }
        }
        map.insert(src.id.0, ns.id);
        s.sources.push(ns);
    }
    let mut out = Vec::new();
    // The donor's Auto Rename schemes come along under new keys, shared as they were.
    let mut names: std::collections::BTreeMap<u64, u64> = Default::default();
    for mut tr in picked {
        tr.id = TrackId(s.alloc());
        tr.name = s.unique_track_name(&tr.name);
        tr.folder = None;
        tr.mixer.vca = None;
        for pl in &mut tr.playlists {
            for c in &mut pl.clips {
                c.id = s.new_clip_id();
                if let ClipContent::Audio { source, .. } = &mut c.content
                    && let Some(n) = map.get(&source.0)
                {
                    *source = *n;
                }
                c.auto_name = c.auto_name.and_then(|k| {
                    let scheme = other.auto_names.get(&k)?;
                    Some(*names.entry(k).or_insert_with(|| {
                        let (key, id) = (s.alloc(), crate::new_scheme_id(s));
                        s.auto_names.insert(key, soundcraft_model::AutoName { id, ..scheme.clone() });
                        key
                    }))
                });
            }
        }
        // The donor's source ids mean nothing here: remap the sample, or drop it when it didn't come along.
        if let Some(ins) = tr.instrument.as_mut() {
            ins.sample = ins.sample.and_then(|sid| map.get(&sid.0).copied());
        }
        // Busses are matched by name (created when missing); output paths by `import_output_route`.
        let remap = |s: &mut Session, r: &Route| -> Route {
            match r {
                Route::Bus(b) => other.bus(*b).map_or(Route::None, |ob| Route::Bus(s.add_bus(&ob.name, ob.format))),
                x => x.clone(),
            }
        };
        tr.mixer.input = remap(s, &tr.mixer.input);
        let output = remap(s, &tr.mixer.output);
        tr.mixer.output = import_output_route(s, &other, &tr.name, output, &mut missing);
        let track = tr.name.clone();
        for slot in tr.mixer.sends.iter_mut().flatten() {
            let target = remap(s, &slot.target);
            slot.target = import_output_route(s, &other, &track, target, &mut missing);
        }
        let pos = s.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(s.tracks.len());
        out.push(tr.id);
        s.tracks.insert(pos, tr);
    }
    s.edit.selected_tracks = out.clone();
    Ok(json!({"tracks": out, "sources": map.len(), "missing": missing}))
}

/// A donor track's route to the output path `name` in this session, as Import Session Data
/// brings its busses: the donor's main path is the main output here, the same path here is used,
/// a path missing here is copied from the donor, and anything else (a different path of that
/// name, a bus of that name, or a name the donor has no path for) is reported in `missing` and
/// becomes no output, so it is never routed silently.
fn import_output_route(s: &mut Session, donor: &Session, track: &str, route: Route, missing: &mut Vec<String>) -> Route {
    let Route::Hardware(name) = &route else { return route };
    if donor.outputs.first().is_some_and(|main| main.name.eq_ignore_ascii_case(name)) {
        return Route::Main;
    }
    let donor_path = donor.outputs.iter().skip(1).find(|path| path.name.eq_ignore_ascii_case(name));
    let local = s.outputs.iter().find(|path| path.name.eq_ignore_ascii_case(name));
    match (local, donor_path) {
        (Some(local), Some(donor)) if local.first_channel == donor.first_channel && local.format == donor.format => {
            return Route::Hardware(local.name.clone());
        }
        (None, Some(donor)) if s.bus_by_name(name).is_none() && check_output_name(name).is_ok() => {
            s.outputs.push(donor.clone());
            return Route::Hardware(donor.name.clone());
        }
        _ => {}
    }
    missing.push(format!("{track}: output path `{name}` is unavailable or different in this session"));
    Route::None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn option_flags_toggle() {
        let mut e = Engine::default();
        assert_eq!(e.execute("options.midi_thru", &json!({})).unwrap()["value"], true);
        assert!(e.session().edit.flag("options.midi_thru"));
        e.execute("options.midi_thru", &json!({"value": false})).unwrap();
        assert!(!e.session().edit.flag("options.midi_thru"));
    }

    #[test]
    fn io_bus_lifecycle() {
        let mut e = crate::demo::demo_engine();
        e.execute("setup.io", &json!({"action": "create_bus", "name": "Drums"})).unwrap();
        e.execute("setup.io", &json!({"action": "rename_bus", "name": "Verb", "new_name": "Hall"})).unwrap();
        assert!(e.session().bus_by_name("Hall").is_some());
        e.execute("setup.io", &json!({"action": "delete_bus", "name": "Hall"})).unwrap();
        assert!(
            e.session().tracks.iter().all(|t| t
                .mixer
                .sends
                .iter()
                .flatten()
                .all(|s| matches!(s.target, Route::None | Route::Main | Route::Hardware(_))))
        );
        assert!(e.session().tracks.iter().all(|t| {
            !matches!(t.mixer.input, Route::Bus(_))
                || e.session()
                    .bus(match t.mixer.input {
                        Route::Bus(b) => b,
                        _ => BusId(0),
                    })
                    .is_some()
        }));
        e.execute("setup.io", &json!({"action": "create_output", "name": "Out 3-4", "first_channel": 2})).unwrap();
        assert_eq!(e.session().outputs.len(), 2);
        assert!(e.execute("setup.io", &json!({"action": "nope", "name": "x"})).is_err());
        assert!(e.can_undo());
    }

    /// Default on the Output page, as in Pro Tools (Reference Guide, "Restoring Default Paths and
    /// Path Names", printed p. 82–83): paths not in use are deleted, paths in use on an interface
    /// pair get that pair's default name back, the main path is "Out 1-2" on channels 1-2 again,
    /// and a stereo path is made for every further pair of the device's outputs. One undo step.
    #[test]
    fn default_restores_the_output_paths_of_the_device() {
        let mut e = Engine::default();
        for name in ["Vox", "Keys"] {
            e.execute("track.new", &json!({"name": name, "format": "mono"})).unwrap();
        }
        let io = |e: &mut Engine, args: Value| e.execute("setup.io", &args).unwrap();
        io(&mut e, json!({"action": "rename_output", "name": "Out 1-2", "new_name": "Monitors"}));
        io(&mut e, json!({"action": "set_output", "name": "Monitors", "first_channel": 2}));
        io(&mut e, json!({"action": "create_output", "name": "Phones", "first_channel": 4}));
        io(&mut e, json!({"action": "create_output", "name": "Spare", "first_channel": 6}));
        io(&mut e, json!({"action": "create_output", "name": "Talkback", "format": "mono", "first_channel": 9}));
        e.execute("track.output", &json!({"track": "Vox", "output": "Phones"})).unwrap();
        e.execute("mix.send", &json!({"track": "Keys", "slot": 0, "output": "Talkback"})).unwrap();
        let before = e.session().outputs.clone();

        let r = io(&mut e, json!({"action": "default_outputs", "channels": 10}));
        let names: Vec<String> = e.session().outputs.iter().map(|path| path.name.clone()).collect();
        assert_eq!(names, ["Out 1-2", "Out 3-4", "Out 5-6", "Out 7-8", "Out 9-10", "Talkback"], "in channel order: {r}");
        assert_eq!(e.session().outputs[0].first_channel, 0, "the main path is back on channels 1-2");
        let path = |e: &Engine, name: &str| e.session().outputs.iter().find(|path| path.name == name).cloned().unwrap();
        assert_eq!((path(&e, "Out 9-10").first_channel, path(&e, "Out 9-10").format), (8, ChannelFormat::Stereo));
        assert_eq!(e.session().track_by_name("Vox").unwrap().mixer.output, Route::Hardware("Out 5-6".into()), "renamed with its tracks");
        assert_eq!(
            e.session().track_by_name("Keys").unwrap().mixer.sends[0].as_ref().map(|send| send.target.clone()),
            Some(Route::Hardware("Talkback".into())),
            "a mono path in use has no default name and stays"
        );
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().outputs, before, "one undo step");
        // With no device known, only the paths change; no pair paths are made.
        io(&mut e, json!({"action": "default_outputs"}));
        assert_eq!(e.session().outputs.len(), 3);
    }

    /// Deleting an output path resets the track outputs and sends on it to No Output, as in Pro
    /// Tools (Reference Guide, "Deleting Paths", printed p. 85), so they neither stay silent under
    /// a name nothing lists nor come back on a later path of that name. One undo step.
    #[test]
    fn deleting_an_output_path_resets_its_tracks_and_sends_to_no_output() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap();
        e.execute("setup.io", &json!({"action": "create_output", "name": "Phones", "first_channel": 2})).unwrap();
        e.execute("track.output", &json!({"track": "Vox", "output": "Phones"})).unwrap();
        e.execute("mix.send", &json!({"track": "Vox", "output": "Phones", "level_db": -6.0})).unwrap();
        e.execute("setup.io", &json!({"action": "delete_output", "name": "phones"})).unwrap();
        let vox = e.session().track_by_name("Vox").unwrap();
        assert_eq!(vox.mixer.output, Route::None);
        let send = vox.mixer.sends[0].as_ref().expect("the send stays, with no output");
        assert_eq!((&send.target, send.level_db), (&Route::None, -6.0));
        e.execute("setup.io", &json!({"action": "create_output", "name": "Phones", "first_channel": 4})).unwrap();
        assert_eq!(e.session().track_by_name("Vox").unwrap().mixer.output, Route::None);
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        let vox = e.session().track_by_name("Vox").unwrap();
        assert_eq!(vox.mixer.output, Route::Hardware("Phones".into()));
        assert_eq!(vox.mixer.sends[0].as_ref().unwrap().target, Route::Hardware("Phones".into()));
    }

    /// A route names a bus or an output path, so New Bus refuses an output path's name.
    #[test]
    fn a_new_bus_cannot_take_an_output_path_name() {
        let mut e = Engine::default();
        e.execute("setup.io", &json!({"action": "create_output", "name": "Bus 1", "first_channel": 2})).unwrap();
        assert!(e.execute("mix.new_bus", &json!({"name": "BUS 1"})).is_err());
        assert!(e.session().busses.is_empty());
    }

    /// Import Session Data brings a donor track's output path along, as it does its busses: a
    /// path missing here is copied, the same path here is used, and a different one, or a name
    /// the donor has no path for, is reported and left with no output, never routed silently.
    #[test]
    fn import_session_data_carries_output_paths_and_reports_conflicts() {
        let dir = std::env::temp_dir().join(format!("sc-import-outputs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        let mut donor = Engine::default();
        for name in ["Vox", "Keys", "Gtr"] {
            donor.execute("track.new", &json!({"name": name, "format": "mono"})).unwrap();
        }
        donor.execute("setup.io", &json!({"action": "create_output", "name": "Phones", "first_channel": 2})).unwrap();
        donor.execute("setup.io", &json!({"action": "create_output", "name": "Cue", "first_channel": 4})).unwrap();
        donor.execute("track.output", &json!({"track": "Vox", "output": "Phones"})).unwrap();
        donor.execute("mix.send", &json!({"track": "Vox", "output": "Out 9-10"})).unwrap();
        donor.execute("track.output", &json!({"track": "Keys", "output": "Cue"})).unwrap();
        donor.execute("track.output", &json!({"track": "Gtr", "output": "Phones"})).unwrap();
        crate::io::save_session(&mut donor, &path).unwrap();
        // A donor saved by an older build can name a path it does not have (Vox's output).
        let text = std::fs::read_to_string(&path).unwrap().replacen("\"Hardware\": \"Phones\"", "\"Hardware\": \"Gone\"", 1);
        std::fs::write(&path, &text).unwrap();
        let mut e = Engine::default();
        e.execute("setup.io", &json!({"action": "create_output", "name": "Cue", "first_channel": 6})).unwrap();
        let r = e.execute("file.import_session_data", &json!({"path": path})).unwrap();
        let s = e.session();
        let output = |name: &str| s.track_by_name(name).unwrap().mixer.output.clone();
        assert_eq!(output("Vox"), Route::None, "the donor has no path `Gone`");
        let send = s.track_by_name("Vox").unwrap().mixer.sends[0].clone().unwrap();
        assert_eq!(send.target, Route::Hardware("Out 9-10".into()));
        assert_eq!(output("Keys"), Route::None, "`Cue` is on other channels here");
        assert_eq!(output("Gtr"), Route::Hardware("Phones".into()));
        let paths: Vec<_> = s.outputs.iter().map(|path| (path.name.as_str(), path.first_channel)).collect();
        assert_eq!(paths, [("Out 1-2", 0), ("Cue", 6), ("Out 9-10", 8), ("Phones", 2)]);
        let missing = r["missing"].to_string();
        assert!(missing.contains("Gone") && missing.contains("Cue"), "{missing}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_timecode_sets_start() {
        let mut e = Engine::default();
        e.execute("edit.select", &json!({"start": 48_000})).unwrap();
        e.execute("setup.current_timecode", &json!({"timecode": "01:00:00:00"})).unwrap();
        let s = e.session();
        let shown = s.format_position(48_000, soundcraft_time::TimeFormat::Timecode);
        assert_eq!(shown, "01:00:00:00");
    }

    #[test]
    fn current_timecode_out_of_range_is_clamped() {
        use soundcraft_time::MAX_POSITION;
        for tc in ["-9223372036854775807:00:00:00", "9999999:00:00:00"] {
            let mut e = Engine::default();
            e.execute("edit.select", &json!({"start": 48_000})).unwrap();
            e.execute("setup.current_timecode", &json!({"timecode": tc})).unwrap();
            let start = e.session().timecode_start;
            assert!((-MAX_POSITION..=MAX_POSITION).contains(&start), "{tc}: start {start}");
            let reloaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
            assert_eq!(reloaded.timecode_start, start, "{tc}");
        }
    }

    #[test]
    fn click_countoff_settings() {
        let mut e = Engine::default();
        let r = e.execute("setup.click_countoff", &json!({"volume_db": -12.0, "countoff_bars": 4, "countoff": true})).unwrap();
        assert_eq!(r["volume_db"], -12.0);
        assert_eq!(e.session().edit.countoff_bars, 4);
        assert!(e.session().edit.countoff);
    }

    #[test]
    fn shortcuts_listed() {
        let mut e = Engine::default();
        let r = e.execute("setup.keyboard_shortcuts", &json!({})).unwrap();
        assert!(r["shortcuts"].as_array().unwrap().iter().any(|x| x["id"] == "edit.undo"));
    }

    #[test]
    fn engine_and_midi_settings() {
        let mut e = Engine::default();
        let r = e.execute("setup.playback_engine", &json!({"buffer_size": 300, "device": "Interface"})).unwrap();
        assert_eq!(r["applied"], false);
        assert_eq!(r["buffer_size"], 300);
        assert_eq!(r["device"], "Interface");
        let r = e.execute("setup.midi_input_filter", &json!({"filter": ["sysex", "bogus"]})).unwrap();
        assert_eq!(r["filtered"], json!(["sysex"]));
        let r = e.execute("setup.midi_input_devices", &json!({"enable": ["Keys"]})).unwrap();
        assert_eq!(r["enabled"], json!(["Keys"]));
        let r = e.execute("setup.video_sync_offset", &json!({"frames": 2})).unwrap();
        assert_eq!(r["samples"], 3200.0);
    }

    #[test]
    fn playback_engine_does_not_pretend_the_device_changed() {
        let mut e = Engine::default();
        let r = e.execute("setup.playback_engine", &json!({})).unwrap();
        assert_eq!(r["applied"], false);
        assert!(r["buffer_size"].is_null(), "no invented default buffer: {r}");
        assert!(r["device"].is_null());
        assert_eq!(r["delay_compensation"], true, "the session default, not a device claim");

        let r = e.execute("setup.hardware", &json!({"buffer_size": 300, "device": "Interface"})).unwrap();
        assert_eq!(r["applied"], false);
        assert_eq!(r["buffer_size"], 300, "kept, not rounded to a power of two");
        assert_eq!(r["device"], "Interface");
        assert_eq!(e.session().edit.value("engine.buffer_size", -1.0), 300.0);

        let r = e.execute("setup.playback_engine", &json!({"delay_compensation": false})).unwrap();
        assert_eq!(r["applied"], false);
        assert_eq!(r["delay_compensation"], false);
        assert_eq!(r["buffer_size"], 300, "delay compensation does not clear a stored buffer");
        assert!(!e.session().edit.delay_compensation);

        let err = e.execute("setup.playback_engine", &json!({"buffer_size": 8, "delay_compensation": true, "device": "Other"})).unwrap_err();
        assert!(err.to_string().contains("16..=8192"), "{err}");
        assert!(!e.session().edit.delay_compensation, "an invalid buffer changes nothing");
        assert_eq!(e.session().edit.value("engine.buffer_size", -1.0), 300.0);
        assert_eq!(text(&e.session().edit, "engine.device").as_deref(), Some("Interface"));
        assert!(e.execute("setup.hardware", &json!({"buffer_size": "big"})).is_err());

        let err = e.execute("setup.playback_engine", &json!({"device": 42, "delay_compensation": true})).unwrap_err();
        assert!(err.to_string().contains("`device` must be a string or null"), "{err}");
        assert!(!e.session().edit.delay_compensation, "an invalid device changes nothing");
        assert_eq!(text(&e.session().edit, "engine.device").as_deref(), Some("Interface"));

        let r = e.execute("setup.hardware", &json!({"buffer_size": null, "device": ""})).unwrap();
        assert_eq!(r["applied"], false);
        assert!(r["buffer_size"].is_null());
        assert!(r["device"].is_null());
    }

    #[test]
    fn import_session_data_brings_tracks_and_audio() {
        let dir = std::env::temp_dir().join(format!("sc-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        let mut donor = crate::demo::demo_engine();
        crate::io::save_session(&mut donor, &path).unwrap();
        let mut e = Engine::default();
        let r = e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Kick", "Keys"]})).unwrap();
        assert_eq!(r["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(r["missing"], json!([]));
        let kick = e.session().track_by_name("Kick").unwrap();
        let src = kick.clips()[0].source().unwrap();
        assert!(e.session().pool.get(src).is_some());
        assert_eq!(e.session().track_by_name("Keys").unwrap().clips().len(), 1);
        assert!(e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Nope"]})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_session_data_brings_auto_rename_schemes_under_new_keys() {
        let dir = std::env::temp_dir().join(format!("sc-import-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        let mut donor = crate::demo::demo_engine();
        let kick: Vec<_> = donor.session().track_by_name("Kick").unwrap().clips().iter().map(|c| c.id.0).collect();
        donor.execute("clip.auto_rename", &json!({"clips": kick, "name": "Hit", "number": 5})).unwrap();
        crate::io::save_session(&mut donor, &path).unwrap();
        let mut e = crate::demo::demo_engine();
        let bass = e.session().track_by_name("Bass").unwrap().clips()[0].id;
        e.execute("clip.auto_rename", &json!({"clips": [bass], "name": "Low"})).unwrap();
        e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Kick"]})).unwrap();
        let s = e.session();
        assert_eq!(s.auto_names.len(), 2, "the donor's scheme comes in beside this session's");
        let imported = s.tracks.iter().rfind(|t| t.name.starts_with("Kick")).unwrap();
        let keys: std::collections::BTreeSet<_> = imported.clips().iter().map(|c| c.auto_name).collect();
        assert_eq!(keys.len(), 1, "the imported clips still share one scheme");
        let key = keys.into_iter().next().flatten().unwrap();
        assert_eq!((s.auto_names[&key].name.as_str(), s.auto_names[&key].number), ("Hit", 5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_session_data_brings_a_samplers_sample() {
        let dir = std::env::temp_dir().join(format!("sc-import-sampler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("hit.wav");
        let buf = craft_audio::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4_800]] };
        std::fs::write(&wav, craft_audio::encode(&buf, &craft_audio::EncodeOptions::default()).unwrap()).unwrap();
        let mut donor = Engine::default();
        donor.execute("track.new", &json!({"count": 1, "name": "Pads", "kind": "instrument"})).unwrap();
        donor.execute("mix.sampler_load", &json!({"track": "Pads", "path": wav.to_string_lossy()})).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        crate::io::save_session(&mut donor, &path).unwrap();
        let mut e = crate::demo::demo_engine();
        e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Pads"]})).unwrap();
        let sample = e.session().track_by_name("Pads").and_then(|t| t.instrument.as_ref()).and_then(|i| i.sample).unwrap();
        let src = e.session().source(sample).unwrap();
        assert_eq!(src.name, "hit");
        assert!(e.session().pool.get(sample).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
