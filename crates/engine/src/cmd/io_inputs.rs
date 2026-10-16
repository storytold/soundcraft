//! Setup › I/O › Input: the input path actions of `setup.io` (the command lives in
//! `setup_more.rs`). Paths are named; tracks route to them by name, so a name is unique across
//! main and sub-paths, and renaming or deleting a path follows through to the tracks.

use super::*;
use serde_json::json;
use soundcraft_model::{ChannelFormat, InputPath, MAX_INPUT_CHANNEL, MAX_INPUT_PATHS, MAX_SUB_PATHS, Route, Session, default_sub_paths};

const ID: &str = "setup.io";

/// Runs an input path action; `None` when `action` isn't one.
pub(crate) fn input_action(e: &mut Engine, action: &str, p: &Value) -> Option<Result<Value>> {
    Some(match action {
        "create_input" => create_input(e, p),
        "create_input_subpath" => create_subpath(e, p),
        "rename_input" => rename_input(e, p),
        "delete_input" => delete_input(e, p),
        "set_input_format" => set_format(e, p),
        "map_input" => map_input(e, p),
        "set_input_channel" => set_channel(e, p),
        "set_input_active" => set_active(e, p),
        "default_inputs" => default_inputs(e, p),
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

/// A hardware channel number from `key` (0-based), or `None` when the key is absent or null.
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

/// Path names are unique across main and sub-paths: tracks name them.
fn ensure_free(s: &Session, name: &str) -> Result<()> {
    if s.find_input_path(name).is_some() { Err(bad(ID, format!("an input path named `{name}` exists"))) } else { Ok(()) }
}

fn main_mut<'a>(s: &'a mut Session, name: &str) -> Result<&'a mut InputPath> {
    match s.find_input_path(name) {
        Some((i, None)) => s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`"))),
        Some((_, Some(_))) => Err(bad(ID, format!("`{name}` is a sub-path; this needs a main path"))),
        None => Err(bad(ID, format!("no input path `{name}`"))),
    }
}

/// The hardware channel after the last one any path uses: where new paths are assigned.
fn next_free_channel(s: &Session) -> u16 {
    s.inputs.iter().flat_map(|m| m.channels.iter().flatten()).max().map_or(0, |c| c.saturating_add(1))
}

/// `{name, format?: stereo, count?: 1, assign?: true, sub_paths?: true, first_channel?, device_channels?}`:
/// new main paths named `name` (`name 1`, `name 2`… for several), assigned in order from
/// `first_channel` (default: after the last used channel) and wrapping to channel 1 past
/// `device_channels` when given, as the New Paths dialog's "Add Default Channel Assignments"
/// does; `sub_paths` makes the default sub-paths for the format.
fn create_input(e: &mut Engine, p: &Value) -> Result<Value> {
    let base = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let count = usize::try_from(i64_or(p, "count", 1).clamp(1, 256)).unwrap_or(1);
    let assign = bool_or(p, "assign", true);
    let subs = bool_or(p, "sub_paths", true);
    let wrap = hardware_param(p, "device_channels")?.map(|n| n.max(1));
    let s = e.session_mut();
    if s.inputs.len().saturating_add(count) > MAX_INPUT_PATHS {
        return Err(bad(ID, format!("at most {MAX_INPUT_PATHS} input paths")));
    }
    let mut first = match hardware_param(p, "first_channel")? {
        Some(c) => c,
        None => next_free_channel(s),
    };
    let names: Vec<String> = (1..=count).map(|k| if count == 1 { base.clone() } else { format!("{base} {k}") }).collect();
    let mut paths = Vec::new();
    for n in names {
        let mut path = InputPath::new(n, format);
        if assign {
            if let Some(w) = wrap
                && first.saturating_add(u16::try_from(format.channels()).unwrap_or(u16::MAX)) > w
            {
                first = 0;
            }
            path.assign_from(first);
            first = first.saturating_add(u16::try_from(format.channels()).unwrap_or(u16::MAX));
        }
        if subs {
            path.sub_paths = default_sub_paths(&path);
        }
        paths.push(path);
    }
    // Every new name, main and sub, must be free and distinct before anything changes.
    let mut new_names: Vec<String> = Vec::new();
    for path in &paths {
        for n in std::iter::once(&path.name).chain(path.sub_paths.iter().map(|s| &s.name)) {
            ensure_free(s, n)?;
            if new_names.iter().any(|m| m.eq_ignore_ascii_case(n)) {
                return Err(bad(ID, format!("`{n}` would be made twice")));
            }
            new_names.push(n.clone());
        }
    }
    let created: Vec<String> = paths.iter().map(|p| p.name.clone()).collect();
    s.inputs.extend(paths);
    Ok(json!({"created": created, "inputs": s.inputs}))
}

/// `{path, name?, format?: mono, assign?: true}`: a sub-path of the main path `path`, reading its
/// first channels (unassigned with `assign: false`; the grid assigns it). Named `<path>.<n>` by default.
fn create_subpath(e: &mut Engine, p: &Value) -> Result<Value> {
    let main_name = name_param(p, "path")?;
    let format = format_param(p, ChannelFormat::Mono)?;
    let assign = bool_or(p, "assign", true);
    let given = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let s = e.session_mut();
    let main = main_mut(s, &main_name)?;
    if main.sub_paths.len() >= MAX_SUB_PATHS {
        return Err(bad(ID, format!("at most {MAX_SUB_PATHS} sub-paths per path")));
    }
    if format.channels() > main.width() {
        return Err(bad(ID, format!("a {} sub-path does not fit in the {} path `{}`", format.label(), main.format.label(), main.name)));
    }
    let main_name = main.name.clone();
    let name = match given {
        Some(mut n) => {
            soundcraft_model::truncate_chars(&mut n, soundcraft_model::MAX_NAME_CHARS);
            n
        }
        None => (1..).map(|k| format!("{main_name}.{k}")).find(|n| s.find_input_path(n).is_none()).unwrap_or_default(),
    };
    ensure_free(s, &name)?;
    let main = main_mut(s, &main_name)?;
    let mut sp = InputPath::new(name.clone(), format);
    if assign {
        for (k, c) in sp.channels.iter_mut().enumerate() {
            *c = main.hardware(k);
        }
    }
    main.sub_paths.push(sp);
    Ok(json!({"created": name, "inputs": s.inputs}))
}

/// `{name, new_name}`: renames a path and the tracks on it; a main path's sub-paths named
/// `<name>.x` follow.
fn rename_input(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let to = name_param(p, "new_name")?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    if !to.eq_ignore_ascii_case(&name) {
        ensure_free(s, &to)?;
    }
    let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let mut renamed: Vec<(String, String)> = Vec::new();
    match j {
        Some(j) => {
            let sp = main.sub_paths.get_mut(j).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
            renamed.push((std::mem::replace(&mut sp.name, to.clone()), to.clone()));
        }
        None => {
            let old = std::mem::replace(&mut main.name, to.clone());
            let prefix = format!("{old}.");
            for sp in &mut main.sub_paths {
                if sp.name.len() > prefix.len() && sp.name.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)) {
                    let suffix = sp.name.get(prefix.len()..).unwrap_or_default().to_string();
                    renamed.push((std::mem::replace(&mut sp.name, format!("{to}.{suffix}")), format!("{to}.{suffix}")));
                }
            }
            renamed.push((old, to.clone()));
        }
    }
    for t in &mut s.tracks {
        if let Route::Hardware(h) = &t.mixer.input
            && let Some((_, new)) = renamed.iter().find(|(old, _)| old.eq_ignore_ascii_case(h))
        {
            t.mixer.input = Route::Hardware(new.clone());
        }
    }
    Ok(json!({"renamed": renamed, "inputs": s.inputs}))
}

/// `{name}`: removes a path (a main path with its sub-paths); tracks on it get no input.
fn delete_input(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let gone: Vec<String> = match j {
        Some(j) => {
            let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
            if j >= main.sub_paths.len() {
                return Err(bad(ID, format!("no input path `{name}`")));
            }
            vec![main.sub_paths.remove(j).name]
        }
        None => {
            if i >= s.inputs.len() {
                return Err(bad(ID, format!("no input path `{name}`")));
            }
            let main = s.inputs.remove(i);
            std::iter::once(main.name).chain(main.sub_paths.into_iter().map(|s| s.name)).collect()
        }
    };
    for t in &mut s.tracks {
        if let Route::Hardware(h) = &t.mixer.input
            && gone.iter().any(|g| g.eq_ignore_ascii_case(h))
        {
            t.mixer.input = Route::None;
        }
    }
    Ok(json!({"deleted": gone, "inputs": s.inputs}))
}

/// `{name, format}`: a path's channel width. The assignment is kept as far as it fits; a main
/// path's sub-paths lose channels that left it.
fn set_format(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    match j {
        Some(j) => {
            if format.channels() > main.width() {
                return Err(bad(ID, format!("a {} sub-path does not fit in the {} path `{}`", format.label(), main.format.label(), main.name)));
            }
            main.sub_paths.get_mut(j).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?.set_format(format);
        }
        None => main.set_format(format),
    }
    main.sanitize(true);
    Ok(json!({"inputs": s.inputs}))
}

/// `{name, hardware: n|null}`: the grid click: the path reads hardware channel `n` (0-based) and
/// the channels after it, left to right ("clicking under channel 1 fills both channel 1 and 2");
/// `null` clears the assignment. A sub-path reads its main path's channels only.
fn map_input(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let hw = hardware_param(p, "hardware")?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    match (j, hw) {
        (None, None) => main.channels.iter_mut().for_each(|c| *c = None),
        (None, Some(hw)) => main.assign_from(hw),
        (Some(j), hw) => {
            let within: Vec<Option<u16>> = main.channels.clone();
            let main_name = main.name.clone();
            let sp = main.sub_paths.get_mut(j).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
            match hw {
                None => sp.channels.iter_mut().for_each(|c| *c = None),
                Some(hw) => {
                    let wanted: Vec<u16> = (0..sp.width()).map(|k| hw.saturating_add(u16::try_from(k).unwrap_or(u16::MAX))).collect();
                    if let Some(c) = wanted.iter().find(|c| !within.contains(&Some(**c))) {
                        return Err(bad(
                            ID,
                            format!("channel {} is not in `{main_name}`: a sub-path reads its main path's channels", c.saturating_add(1)),
                        ));
                    }
                    for (c, w) in sp.channels.iter_mut().zip(wanted) {
                        *c = Some(w);
                    }
                }
            }
        }
    }
    main.sanitize(true);
    Ok(json!({"inputs": s.inputs}))
}

/// `{name, channel: k, hardware: n|null}`: one channel of the path reads hardware channel `n`
/// (another channel of the path that read it is cleared), or nothing.
fn set_channel(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let k = usize::try_from(p.get("channel").and_then(Value::as_u64).ok_or_else(|| bad(ID, "`channel` (index in the path) required"))?)
        .unwrap_or(usize::MAX);
    let hw = hardware_param(p, "hardware")?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let within: Option<Vec<Option<u16>>> = j.map(|_| main.channels.clone());
    let main_name = main.name.clone();
    let path = match j {
        Some(j) => main.sub_paths.get_mut(j).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?,
        None => main,
    };
    if k >= path.width() {
        return Err(bad(ID, format!("`{}` has {} channels", path.name, path.width())));
    }
    if let (Some(hw), Some(within)) = (hw, &within)
        && !within.contains(&Some(hw))
    {
        return Err(bad(ID, format!("channel {} is not in `{main_name}`: a sub-path reads its main path's channels", hw.saturating_add(1))));
    }
    for (idx, c) in path.channels.iter_mut().enumerate() {
        if idx == k {
            *c = hw;
        } else if hw.is_some() && *c == hw {
            *c = None;
        }
    }
    s.inputs.iter_mut().for_each(|m| m.sanitize(true));
    Ok(json!({"inputs": s.inputs}))
}

/// `{name, active: bool}`: an inactive path stays but feeds nothing (nor do its sub-paths).
fn set_active(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let active = p.get("active").and_then(Value::as_bool).ok_or_else(|| bad(ID, "`active` (bool) required"))?;
    let s = e.session_mut();
    let (i, j) = s.find_input_path(&name).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    let main = s.inputs.get_mut(i).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?;
    match j {
        Some(j) => main.sub_paths.get_mut(j).ok_or_else(|| bad(ID, format!("no input path `{name}`")))?.active = active,
        None => main.active = active,
    }
    Ok(json!({"inputs": s.inputs}))
}

/// `{channels?: 8}`: the Default button: a stereo path per channel pair with mono sub-paths, for
/// an interface with `channels` inputs. Tracks keep their input names.
fn default_inputs(e: &mut Engine, p: &Value) -> Result<Value> {
    let n = usize::try_from(i64_or(p, "channels", 8).clamp(1, i64::from(MAX_INPUT_CHANNEL) + 1)).unwrap_or(1);
    let s = e.session_mut();
    s.inputs = Session::default_input_paths(n);
    Ok(json!({"inputs": s.inputs}))
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

    fn names(e: &Engine) -> Vec<String> {
        e.session().input_paths_flat().iter().map(|(p, _)| p.name.clone()).collect()
    }

    fn channels(e: &Engine, name: &str) -> Vec<Option<u16>> {
        e.session().input_path(name).map(|p| p.channels.clone()).unwrap_or_default()
    }

    #[test]
    fn new_paths_are_numbered_assigned_after_the_last_channel_and_get_sub_paths() {
        let mut e = Engine::default();
        io(&mut e, "create_input", json!({"name": "Mic", "format": "mono", "count": 2})).unwrap();
        assert_eq!(channels(&e, "Mic 1"), [Some(8)], "after the default paths' eight channels");
        assert_eq!(channels(&e, "Mic 2"), [Some(9)]);
        io(&mut e, "create_input", json!({"name": "Surr", "format": "5.1"})).unwrap();
        assert_eq!(channels(&e, "Surr"), [Some(10), Some(11), Some(12), Some(13), Some(14), Some(15)]);
        assert_eq!(channels(&e, "Surr.LR"), [Some(10), Some(11)]);
        assert_eq!(channels(&e, "Surr.LFE"), [Some(13)]);
        io(&mut e, "create_input", json!({"name": "Bare", "assign": false, "sub_paths": false})).unwrap();
        assert_eq!(channels(&e, "Bare"), [None, None]);
        assert!(e.session().input_path("Bare").unwrap().sub_paths.is_empty());
        // Wrapping at the device's channel count, as the New Paths dialog does.
        io(&mut e, "create_input", json!({"name": "Line", "count": 3, "first_channel": 4, "device_channels": 8, "sub_paths": false})).unwrap();
        assert_eq!(
            (channels(&e, "Line 1"), channels(&e, "Line 2"), channels(&e, "Line 3")),
            (vec![Some(4), Some(5)], vec![Some(6), Some(7)], vec![Some(0), Some(1)])
        );
        assert!(io(&mut e, "create_input", json!({"name": "Mic 1"})).is_err(), "names are unique");
        assert!(io(&mut e, "create_input", json!({"name": "Surr", "format": "stereo"})).is_err(), "even against a sub-path's name");
        assert!(io(&mut e, "create_input", json!({"name": "X", "format": "11.1"})).is_err());
        assert!(e.undo());
    }

    #[test]
    fn sub_paths_live_inside_their_main_path() {
        let mut e = Engine::default();
        io(&mut e, "create_input_subpath", json!({"path": "In 3-4"})).unwrap();
        assert_eq!(channels(&e, "In 3-4.1"), [Some(2)]);
        io(&mut e, "create_input_subpath", json!({"path": "In 3-4", "name": "Pair", "format": "stereo"})).unwrap();
        assert_eq!(channels(&e, "Pair"), [Some(2), Some(3)]);
        assert!(io(&mut e, "create_input_subpath", json!({"path": "In 3-4", "format": "5.1"})).is_err(), "wider than the main path");
        assert!(io(&mut e, "create_input_subpath", json!({"path": "In 3"})).is_err(), "a sub-path has no sub-paths");
        // The grid click: a sub-path may only read its main path's channels.
        assert!(io(&mut e, "map_input", json!({"name": "Pair", "hardware": 3})).is_err());
        io(&mut e, "map_input", json!({"name": "In 3-4.1", "hardware": 3})).unwrap();
        assert_eq!(channels(&e, "In 3-4.1"), [Some(3)]);
        io(&mut e, "map_input", json!({"name": "In 3-4.1", "hardware": null})).unwrap();
        assert_eq!(channels(&e, "In 3-4.1"), [None]);
        assert!(io(&mut e, "set_input_channel", json!({"name": "Pair", "channel": 0, "hardware": 7})).is_err());
        assert!(io(&mut e, "set_input_channel", json!({"name": "Pair", "channel": 2, "hardware": 2})).is_err(), "no such channel");
        // Moving the main path moves what the sub-paths may read: channels that left it clear.
        io(&mut e, "map_input", json!({"name": "In 3-4", "hardware": 10})).unwrap();
        assert_eq!(channels(&e, "In 3-4"), [Some(10), Some(11)]);
        assert_eq!(channels(&e, "Pair"), [None, None]);
        assert_eq!(channels(&e, "In 3"), [None]);
    }

    #[test]
    fn one_channel_at_a_time_and_a_channel_used_once_per_path() {
        let mut e = Engine::default();
        io(&mut e, "set_input_channel", json!({"name": "In 1-2", "channel": 1, "hardware": 0})).unwrap();
        assert_eq!(channels(&e, "In 1-2"), [None, Some(0)], "the left channel that read 0 is cleared");
        io(&mut e, "set_input_channel", json!({"name": "In 1-2", "channel": 0, "hardware": 5})).unwrap();
        assert_eq!(channels(&e, "In 1-2"), [Some(5), Some(0)]);
        assert!(io(&mut e, "set_input_channel", json!({"name": "In 1-2", "channel": 0, "hardware": 5000})).is_err());
        assert!(io(&mut e, "set_input_channel", json!({"name": "In 1-2", "channel": 0, "hardware": -1})).is_err());
        io(&mut e, "set_input_format", json!({"name": "In 1-2", "format": "LCR"})).unwrap();
        assert_eq!(channels(&e, "In 1-2"), [Some(5), Some(0), None], "the assignment is kept as far as it fits");
        io(&mut e, "set_input_format", json!({"name": "In 1-2", "format": "mono"})).unwrap();
        assert_eq!(channels(&e, "In 1-2"), [Some(5)]);
        assert_eq!(channels(&e, "In 2"), [None], "channel 1 left the main path");
        assert!(io(&mut e, "set_input_format", json!({"name": "In 1", "format": "stereo"})).is_err(), "a sub-path within a mono path");
    }

    #[test]
    fn renaming_and_deleting_follow_through_to_the_tracks() {
        let mut e = Engine::default();
        let vox = e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        let pad = e.execute("track.new", &json!({"name": "Pad", "format": "stereo"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [vox], "input": "In 3"})).unwrap();
        e.execute("track.input", &json!({"tracks": [pad], "input": "In 3-4"})).unwrap();
        io(&mut e, "rename_input", json!({"name": "In 3-4", "new_name": "Keys"})).unwrap();
        let input = |e: &Engine, id: u64| e.session().track(TrackId(id)).unwrap().mixer.input.clone();
        assert_eq!(input(&e, pad), Route::Hardware("Keys".into()));
        assert_eq!(input(&e, vox), Route::Hardware("In 3".into()), "a sub-path not named after its main path keeps its name");
        io(&mut e, "create_input", json!({"name": "Drums", "format": "quad", "assign": false})).unwrap();
        io(&mut e, "rename_input", json!({"name": "Drums", "new_name": "Kit"})).unwrap();
        assert!(names(&e).iter().any(|n| n == "Kit.LR") && names(&e).iter().any(|n| n == "Kit.Ls"), "the `.x` sub-paths follow: {:?}", names(&e));
        assert!(io(&mut e, "rename_input", json!({"name": "Kit", "new_name": "in 5-6"})).is_err(), "names stay unique");
        io(&mut e, "rename_input", json!({"name": "Kit", "new_name": "KIT"})).unwrap();
        io(&mut e, "delete_input", json!({"name": "In 3"})).unwrap();
        assert_eq!(input(&e, vox), Route::None);
        assert_eq!(input(&e, pad), Route::Hardware("Keys".into()));
        io(&mut e, "delete_input", json!({"name": "Keys"})).unwrap();
        assert_eq!(input(&e, pad), Route::None);
        assert!(!names(&e).iter().any(|n| n == "In 4"), "the main path's sub-paths went with it");
        assert!(io(&mut e, "delete_input", json!({"name": "Keys"})).is_err());
        assert!(e.undo() && e.undo());
        assert_eq!(input(&e, vox), Route::Hardware("In 3".into()));
    }

    #[test]
    fn active_default_and_the_listing() {
        let mut e = Engine::default();
        io(&mut e, "set_input_active", json!({"name": "In 1-2", "active": false})).unwrap();
        assert_eq!(e.session().input_route_channels(&Route::Hardware("In 1".into()), 1), [None]);
        assert!(io(&mut e, "set_input_active", json!({"name": "In 1-2", "active": "yes"})).is_err());
        let listed = io(&mut e, "list", json!({})).unwrap();
        assert_eq!(listed["inputs"].as_array().map(Vec::len), Some(4));
        io(&mut e, "default_inputs", json!({"channels": 3})).unwrap();
        assert_eq!(names(&e), ["In 1-2", "In 1", "In 2", "In 3"]);
        assert!(e.session().inputs.iter().all(|p| p.active));
        io(&mut e, "default_inputs", json!({"channels": -5})).unwrap();
        assert_eq!(names(&e), ["In 1"]);
        assert!(io(&mut e, "no_such_input_action", json!({"name": "x"})).is_err());
    }
}
