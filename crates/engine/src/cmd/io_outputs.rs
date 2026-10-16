//! Setup › I/O › Output: the output path actions of `setup.io` (the command lives in
//! `setup_more.rs`). Output paths are named; tracks, sends and busses route to them by name, so
//! renaming or deleting one follows through. The monitor path carries the main mix.

use super::*;
use serde_json::json;
use soundcraft_model::{ChannelFormat, MAX_INPUT_CHANNEL, MAX_INPUT_PATHS, OutputPath, Route, Session};

const ID: &str = "setup.io";

/// Runs an output path action; `None` when `action` isn't one.
pub(crate) fn output_action(e: &mut Engine, action: &str, p: &Value) -> Option<Result<Value>> {
    Some(match action {
        "create_output" => create_output(e, p),
        "rename_output" => rename_output(e, p),
        "delete_output" => delete_output(e, p),
        "set_output_format" => set_format(e, p),
        "map_output" => map_output(e, p),
        "set_output_channel" => set_channel(e, p),
        "set_output_active" => set_active(e, p),
        "default_outputs" => default_outputs(e, p),
        "set_monitor_output" => set_named(e, p, true),
        "set_meter_output" => set_named(e, p, false),
        _ => return None,
    })
}

fn name_param(p: &Value, key: &str) -> Result<String> {
    let n = str_param(p, key).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(ID, format!("`{key}` required")))?;
    let mut n = n.to_string();
    soundcraft_model::truncate_chars(&mut n, soundcraft_model::MAX_NAME_CHARS);
    Ok(n)
}

fn format_param(p: &Value, default: ChannelFormat) -> Result<ChannelFormat> {
    match str_param(p, "format") {
        Some(f) => ChannelFormat::from_id(f).ok_or_else(|| bad(ID, format!("unknown format `{f}`"))),
        None => Ok(default),
    }
}

fn hardware_param(p: &Value, key: &str) -> Result<Option<u16>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| bad(ID, format!("`{key}` must be a channel number from 0 or null")))?;
            Ok(Some(
                u16::try_from(n)
                    .ok()
                    .filter(|n| *n <= MAX_INPUT_CHANNEL)
                    .ok_or_else(|| bad(ID, format!("`{key}` is at most {MAX_INPUT_CHANNEL}")))?,
            ))
        }
    }
}

fn ensure_free(s: &Session, name: &str) -> Result<()> {
    if s.find_output_path(name).is_some() { Err(bad(ID, format!("an output path named `{name}` exists"))) } else { Ok(()) }
}

fn path_mut<'a>(s: &'a mut Session, name: &str) -> Result<&'a mut OutputPath> {
    let i = s.find_output_path(name).ok_or_else(|| bad(ID, format!("no output path `{name}`")))?;
    s.outputs.get_mut(i).ok_or_else(|| bad(ID, format!("no output path `{name}`")))
}

fn next_free_channel(s: &Session) -> u16 {
    s.outputs.iter().flat_map(|o| o.channels.iter().flatten()).max().map_or(0, |c| c.saturating_add(1))
}

/// `{name, format?: stereo, count?: 1, assign?: true, first_channel?, device_channels?}`: new
/// output paths named `name` (`name 1`, `name 2`… for several), driving channels in order from
/// `first_channel` (default: after the last used channel; wrapping to channel 1 past
/// `device_channels` when given). Output paths may overlap; they have no sub-paths.
fn create_output(e: &mut Engine, p: &Value) -> Result<Value> {
    let base = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let count = usize::try_from(i64_or(p, "count", 1).clamp(1, 256)).unwrap_or(1);
    let assign = bool_or(p, "assign", true);
    let wrap = hardware_param(p, "device_channels")?.map(|n| n.max(1));
    let s = e.session_mut();
    if s.outputs.len().saturating_add(count) > MAX_INPUT_PATHS {
        return Err(bad(ID, format!("at most {MAX_INPUT_PATHS} output paths")));
    }
    let mut first = match hardware_param(p, "first_channel")? {
        Some(c) => c,
        None => next_free_channel(s),
    };
    let names: Vec<String> = (1..=count).map(|k| if count == 1 { base.clone() } else { format!("{base} {k}") }).collect();
    for n in &names {
        ensure_free(s, n)?;
    }
    let mut paths = Vec::new();
    for n in names {
        let mut path = OutputPath::new(n, format);
        if assign {
            let width = u16::try_from(format.channels()).unwrap_or(u16::MAX);
            if let Some(w) = wrap
                && first.saturating_add(width) > w
            {
                first = 0;
            }
            path.assign_from(first);
            first = first.saturating_add(width);
        }
        paths.push(path);
    }
    let created: Vec<String> = paths.iter().map(|o| o.name.clone()).collect();
    s.outputs.extend(paths);
    Ok(json!({"created": created, "outputs": s.outputs}))
}

/// Every place an output path is named, renamed from `old` to `new`.
fn follow_rename(s: &mut Session, old: &str, new: &str) {
    let same = |n: &str| n.eq_ignore_ascii_case(old);
    for t in &mut s.tracks {
        if let Route::Hardware(h) = &t.mixer.output
            && same(h)
        {
            t.mixer.output = Route::Hardware(new.to_string());
        }
        for snd in t.mixer.sends.iter_mut().flatten() {
            if let Route::Hardware(h) = &snd.target
                && same(h)
            {
                snd.target = Route::Hardware(new.to_string());
            }
        }
    }
    for b in &mut s.busses {
        if b.output.as_deref().is_some_and(same) {
            b.output = Some(new.to_string());
        }
    }
    for n in [&mut s.monitor_output, &mut s.meter_output] {
        if n.as_deref().is_some_and(same) {
            *n = Some(new.to_string());
        }
    }
}

/// `{name, new_name}`: renames a path and everything routed to it.
fn rename_output(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let to = name_param(p, "new_name")?;
    let s = e.session_mut();
    if !to.eq_ignore_ascii_case(&name) {
        ensure_free(s, &to)?;
    }
    let old = std::mem::replace(&mut path_mut(s, &name)?.name, to.clone());
    follow_rename(s, &old, &to);
    Ok(json!({"renamed": [[old, to]], "outputs": s.outputs}))
}

/// `{name}`: removes a path; tracks and sends on it get no output, busses mapped to it come off.
fn delete_output(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let s = e.session_mut();
    let i = s.find_output_path(&name).ok_or_else(|| bad(ID, format!("no output path `{name}`")))?;
    if i >= s.outputs.len() {
        return Err(bad(ID, format!("no output path `{name}`")));
    }
    let gone = s.outputs.remove(i).name;
    let same = |n: &str| n.eq_ignore_ascii_case(&gone);
    for t in &mut s.tracks {
        if let Route::Hardware(h) = &t.mixer.output
            && same(h)
        {
            t.mixer.output = Route::None;
        }
        for slot in &mut t.mixer.sends {
            if slot.as_ref().is_some_and(|x| matches!(&x.target, Route::Hardware(h) if same(h))) {
                *slot = None;
            }
        }
    }
    for b in &mut s.busses {
        if b.output.as_deref().is_some_and(same) {
            b.output = None;
        }
    }
    for n in [&mut s.monitor_output, &mut s.meter_output] {
        if n.as_deref().is_some_and(same) {
            *n = None;
        }
    }
    Ok(json!({"deleted": gone, "outputs": s.outputs}))
}

/// `{name, format}`: a path's channel width; the assignment is kept as far as it fits. The
/// monitor path's format is the main mix's: the master faders follow.
fn set_format(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let s = e.session_mut();
    if s.is_monitor_output(&name) {
        if !(2..=16).contains(&format.channels()) || format.is_ambisonic() {
            return Err(bad(ID, "the monitor path's format is the main mix's: 2 to 16 channels"));
        }
        super::surround::set_main_format(s, format);
    } else {
        let o = path_mut(s, &name)?;
        o.set_format(format);
        o.sanitize();
    }
    Ok(json!({"outputs": s.outputs}))
}

/// `{name, hardware: n|null}`: the grid click: the path drives hardware channel `n` (0-based)
/// and the channels after it; `null` clears the assignment.
fn map_output(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let hw = hardware_param(p, "hardware")?;
    let s = e.session_mut();
    let o = path_mut(s, &name)?;
    match hw {
        None => o.channels.iter_mut().for_each(|c| *c = None),
        Some(hw) => o.assign_from(hw),
    }
    o.sanitize();
    Ok(json!({"outputs": s.outputs}))
}

/// `{name, channel: k, hardware: n|null}`: one channel of the path drives hardware channel `n`
/// (another channel of the path that drove it is cleared), or nothing.
fn set_channel(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let k = usize::try_from(p.get("channel").and_then(Value::as_u64).ok_or_else(|| bad(ID, "`channel` (index in the path) required"))?)
        .unwrap_or(usize::MAX);
    let hw = hardware_param(p, "hardware")?;
    let s = e.session_mut();
    let o = path_mut(s, &name)?;
    if k >= o.width() {
        return Err(bad(ID, format!("`{}` has {} channels", o.name, o.width())));
    }
    for (idx, c) in o.channels.iter_mut().enumerate() {
        if idx == k {
            *c = hw;
        } else if hw.is_some() && *c == hw {
            *c = None;
        }
    }
    o.sanitize();
    Ok(json!({"outputs": s.outputs}))
}

/// `{name, active: bool}`: an inactive path drives nothing (the main mix moves to the next
/// active path when the monitor path goes inactive).
fn set_active(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let active = p.get("active").and_then(Value::as_bool).ok_or_else(|| bad(ID, "`active` (bool) required"))?;
    let s = e.session_mut();
    path_mut(s, &name)?.active = active;
    Ok(json!({"outputs": s.outputs}))
}

/// `{channels?: 2}`: the Default button: a stereo path per channel pair of the output device,
/// the first the monitor path. Routes keep their names.
fn default_outputs(e: &mut Engine, p: &Value) -> Result<Value> {
    let n = usize::try_from(i64_or(p, "channels", 2).clamp(1, i64::from(MAX_INPUT_CHANNEL) + 1)).unwrap_or(2);
    let s = e.session_mut();
    s.outputs = Session::default_output_paths(n);
    s.monitor_output = None;
    s.meter_output = None;
    Ok(json!({"outputs": s.outputs}))
}

/// `{name: path|null}`: the Monitor Path (the output path the main mix drives) or the Output
/// Meter Path (the one the main meter shows); `null` is the default (the first active path, or
/// the monitor path).
fn set_named(e: &mut Engine, p: &Value, monitor: bool) -> Result<Value> {
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(_) => Some(name_param(p, "name")?),
    };
    let s = e.session_mut();
    if let Some(n) = &name {
        let o = s.output_path(n).ok_or_else(|| bad(ID, format!("no output path `{n}`")))?;
        if monitor && !(2..=16).contains(&o.format.channels()) {
            return Err(bad(ID, "the monitor path carries the main mix: 2 to 16 channels"));
        }
    }
    if monitor {
        s.monitor_output = name;
        let fmt = s.main_format();
        super::surround::set_main_format(s, fmt);
    } else {
        s.meter_output = name;
    }
    Ok(json!({"monitor_output": s.monitor_output, "meter_output": s.meter_output}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::TrackId;

    fn io(e: &mut Engine, action: &str, rest: Value) -> Result<Value> {
        let mut p = rest;
        p["action"] = json!(action);
        e.execute("setup.io", &p)
    }

    fn channels(e: &Engine, name: &str) -> Vec<Option<u16>> {
        e.session().output_path(name).map(|o| o.channels.clone()).unwrap_or_default()
    }

    #[test]
    fn new_output_paths_are_numbered_and_assigned_after_the_last_channel() {
        let mut e = Engine::default();
        io(&mut e, "create_output", json!({"name": "Cue", "count": 2})).unwrap();
        assert_eq!(channels(&e, "Cue 1"), [Some(2), Some(3)]);
        assert_eq!(channels(&e, "Cue 2"), [Some(4), Some(5)]);
        assert_eq!(e.session().outputs[1].first_channel, 2);
        io(&mut e, "create_output", json!({"name": "Out 3-4", "first_channel": 2})).unwrap();
        assert_eq!(channels(&e, "Out 3-4"), [Some(2), Some(3)], "overlap is allowed for outputs");
        io(&mut e, "create_output", json!({"name": "Line", "count": 2, "first_channel": 2, "device_channels": 4, "format": "stereo"})).unwrap();
        assert_eq!((channels(&e, "Line 1"), channels(&e, "Line 2")), (vec![Some(2), Some(3)], vec![Some(0), Some(1)]));
        assert!(io(&mut e, "create_output", json!({"name": "Cue 1"})).is_err());
        assert!(io(&mut e, "create_output", json!({"name": "X", "format": "nope"})).is_err());
        assert!(e.undo());
    }

    #[test]
    fn the_grid_and_the_active_flag() {
        let mut e = Engine::default();
        io(&mut e, "create_output", json!({"name": "Cue", "assign": false})).unwrap();
        assert_eq!(channels(&e, "Cue"), [None, None]);
        io(&mut e, "map_output", json!({"name": "Cue", "hardware": 6})).unwrap();
        assert_eq!(channels(&e, "Cue"), [Some(6), Some(7)]);
        io(&mut e, "set_output_channel", json!({"name": "Cue", "channel": 0, "hardware": 7})).unwrap();
        assert_eq!(channels(&e, "Cue"), [Some(7), None], "the right channel that drove 7 is cleared");
        assert!(io(&mut e, "set_output_channel", json!({"name": "Cue", "channel": 5, "hardware": 0})).is_err());
        assert!(io(&mut e, "map_output", json!({"name": "Cue", "hardware": 99999})).is_err());
        io(&mut e, "map_output", json!({"name": "Cue", "hardware": null})).unwrap();
        assert_eq!(channels(&e, "Cue"), [None, None]);
        io(&mut e, "set_output_format", json!({"name": "Cue", "format": "5.1"})).unwrap();
        assert_eq!(channels(&e, "Cue").len(), 6);
        io(&mut e, "set_output_active", json!({"name": "Cue", "active": false})).unwrap();
        assert!(!e.session().output_path("Cue").unwrap().active);
        // The monitor path's format is the main mix's; the master fader follows.
        e.execute("track.new", &json!({"name": "M", "kind": "master", "format": "stereo"})).ok();
        io(&mut e, "set_output_format", json!({"name": "Out 1-2", "format": "5.1"})).unwrap();
        assert_eq!(e.session().main_format(), ChannelFormat::Surround51);
        assert_eq!(channels(&e, "Out 1-6"), [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)], "renamed with the width and assigned in order");
        assert!(io(&mut e, "set_output_format", json!({"name": "Out 1-6", "format": "mono"})).is_err(), "the main mix is at least stereo");
    }

    #[test]
    fn renaming_and_deleting_follow_through_and_the_monitor_and_meter_paths() {
        let mut e = Engine::default();
        io(&mut e, "default_outputs", json!({"channels": 6})).unwrap();
        assert_eq!(e.session().outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Out 1-2", "Out 3-4", "Out 5-6"]);
        let t = e.execute("track.new", &json!({"name": "Cue mix", "format": "stereo"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.output", &json!({"tracks": [t], "output": "Out 3-4"})).unwrap();
        let bus = e.execute("mix.new_bus", &json!({"name": "Stem"})).unwrap();
        assert!(bus["bus"].is_number());
        io(&mut e, "map_bus_output", json!({"name": "Stem", "output": "Out 3-4"})).unwrap();
        io(&mut e, "set_monitor_output", json!({"name": "Out 3-4"})).unwrap();
        io(&mut e, "set_meter_output", json!({"name": "Out 5-6"})).unwrap();
        io(&mut e, "rename_output", json!({"name": "Out 3-4", "new_name": "Cue"})).unwrap();
        let output = |e: &Engine| e.session().track(TrackId(t)).unwrap().mixer.output.clone();
        assert_eq!(output(&e), Route::Hardware("Cue".into()));
        assert_eq!(e.session().bus_by_name("Stem").and_then(|b| b.output.clone()), Some("Cue".into()));
        assert_eq!(e.session().monitor_output.as_deref(), Some("Cue"));
        assert!(e.session().is_monitor_output("cue"));
        assert!(io(&mut e, "rename_output", json!({"name": "Cue", "new_name": "out 5-6"})).is_err());
        assert!(io(&mut e, "set_monitor_output", json!({"name": "Nope"})).is_err());
        io(&mut e, "delete_output", json!({"name": "Cue"})).unwrap();
        assert_eq!(output(&e), Route::None);
        assert_eq!(e.session().bus_by_name("Stem").and_then(|b| b.output.clone()), None);
        assert_eq!(e.session().monitor_output, None);
        assert_eq!(e.session().monitor_output_path().map(|o| o.name.as_str()), Some("Out 1-2"));
        io(&mut e, "set_meter_output", json!({"name": null})).unwrap();
        assert_eq!(e.session().meter_output, None);
        assert!(io(&mut e, "delete_output", json!({"name": "Cue"})).is_err());
        assert!(e.undo() && e.undo());
        assert_eq!(e.session().meter_output.as_deref(), Some("Out 5-6"));
    }
}
