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
        cmd!(noundo "setup.hardware", "Hardware...", ["Setup"], None, "{device?, buffer_size?: 16..8192} — audio interface settings (sample rate is the session's)", always, |e, p| engine_settings(e, p)),
        cmd!(noundo "setup.playback_engine", "Playback Engine...", ["Setup"], None, "{device?, buffer_size?: 16..8192, delay_compensation?: bool}", always, |e, p| {
            if let Some(v) = p.get("delay_compensation").and_then(Value::as_bool) {
                e.session_mut().edit.delay_compensation = v;
            }
            engine_settings(e, p)
        }),
        cmd!(noundo "setup.disk_allocation", "Disk Allocation...", ["Setup"], None, "{path?, track?} — the record folder for the session, or for one track", always, disk_allocation),
        cmd!(noundo "setup.peripherals", "Peripherals...", ["Setup"], None, "{settings?: {name: number|bool|string}} — control surface / sync device settings", always, peripherals),
        cmd!(
            "setup.io",
            "I/O...",
            ["Setup"],
            None,
            "{action?: list|create_bus|rename_bus|delete_bus|create_output|rename_output|delete_output, name?, new_name?, format?: stereo, first_channel?: 0}",
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

fn engine_settings(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(b) = p.get("buffer_size").and_then(Value::as_u64) {
        let b = b.clamp(16, 8192).next_power_of_two().min(8192);
        s.edit.values.insert("engine.buffer_size".into(), b as f64);
    }
    if let Some(d) = str_param(p, "device") {
        set_text(&mut s.edit, "engine.device", Some(d));
    }
    Ok(json!({
        "device": text(&s.edit, "engine.device"),
        "buffer_size": s.edit.value("engine.buffer_size", 1024.0),
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
            s.add_bus(&name, format);
        }
        "rename_bus" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            if s.bus_by_name(&to).is_some() {
                return Err(bad(id, format!("a bus named `{to}` exists")));
            }
            let b = s.busses.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            b.name = to;
        }
        "delete_bus" => {
            let bid: BusId = s.bus_by_name(&name).map(|b| b.id).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            s.busses.retain(|b| b.id != bid);
            unroute_bus(s, bid);
        }
        "create_output" => {
            if s.outputs.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
                return Err(bad(id, format!("an output named `{name}` exists")));
            }
            let first = i64_or(p, "first_channel", 0).clamp(0, 1023) as u16;
            s.outputs.push(OutputPath { name, first_channel: first, format });
        }
        "rename_output" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            let o = s.outputs.iter_mut().find(|o| o.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            let old = std::mem::replace(&mut o.name, to.clone());
            for t in &mut s.tracks {
                if t.mixer.output == Route::Hardware(old.clone()) {
                    t.mixer.output = Route::Hardware(to.clone());
                }
            }
        }
        "delete_output" => {
            let before = s.outputs.len();
            s.outputs.retain(|o| !o.name.eq_ignore_ascii_case(&name));
            if s.outputs.len() == before {
                return Err(bad(id, format!("no output `{name}`")));
            }
        }
        other => return Err(bad(id, format!("unknown action `{other}`"))),
    }
    Ok(io_state(s))
}

fn current_timecode(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.current_timecode";
    let tc = str_param(p, "timecode").ok_or_else(|| bad(id, "`timecode` required (HH:MM:SS:FF)"))?;
    let tc = Timecode::parse(tc).map_err(|err| bad(id, err.to_string()))?;
    let s = e.session_mut();
    let at = s.edit.selection.start.max(0);
    let abs = tc.to_samples(s.frame_rate, s.sample_rate);
    s.timecode_start = abs - at;
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
    let text_v = crate::io::read_text(&path)?;
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
        // Busses are matched by name (created when missing).
        let remap = |s: &mut Session, r: &Route| -> Route {
            match r {
                Route::Bus(b) => other.bus(*b).map_or(Route::None, |ob| Route::Bus(s.add_bus(&ob.name, ob.format))),
                x => x.clone(),
            }
        };
        tr.mixer.input = remap(s, &tr.mixer.input);
        tr.mixer.output = remap(s, &tr.mixer.output);
        for slot in tr.mixer.sends.iter_mut().flatten() {
            slot.target = remap(s, &slot.target);
        }
        let pos = s.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(s.tracks.len());
        out.push(tr.id);
        s.tracks.insert(pos, tr);
    }
    s.edit.selected_tracks = out.clone();
    Ok(json!({"tracks": out, "sources": map.len(), "missing": missing}))
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
        assert_eq!(r["buffer_size"], 512.0);
        assert_eq!(r["device"], "Interface");
        let r = e.execute("setup.midi_input_filter", &json!({"filter": ["sysex", "bogus"]})).unwrap();
        assert_eq!(r["filtered"], json!(["sysex"]));
        let r = e.execute("setup.midi_input_devices", &json!({"enable": ["Keys"]})).unwrap();
        assert_eq!(r["enabled"], json!(["Keys"]));
        let r = e.execute("setup.video_sync_offset", &json!({"frames": 2})).unwrap();
        assert_eq!(r["samples"], 3200.0);
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
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4_800]] };
        std::fs::write(&wav, soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap()).unwrap();
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
