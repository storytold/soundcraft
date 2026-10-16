//! Setup › Export / Import I/O Settings: the I/O Setup (input and output paths, busses with
//! their sub-paths and mapping, the monitor and meter paths) as a JSON settings file, as Pro
//! Tools' `.pio` files carry it between sessions and systems. An import merges by name (a bus
//! keeps its id, so tracks stay routed), or with `replace` first drops the paths nothing uses.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Bus, BusSubId, BusSubPath, InputPath, MAX_INPUT_PATHS, OutputPath, Route, Session};
use std::path::Path;

/// The settings file's extension.
pub const IO_SETTINGS_EXTENSION: &str = "scio";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "setup.io_export",
            "Export I/O Settings...",
            ["Setup"],
            None,
            "{path, tabs?: [input|output|bus] (all three unless given)} — writes the I/O Setup as a .scio settings file",
            always,
            export_io
        ),
        cmd!(
            "setup.io_import",
            "Import I/O Settings...",
            ["Setup"],
            None,
            "{path, tabs?: [input|output|bus], replace?: false (first drop the paths and busses nothing uses)} — merges a .scio settings file into the I/O Setup by name",
            always,
            import_io
        ),
    ]
}

/// What a settings file holds. Every field is optional on reading.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct IoSettings {
    format: String,
    version: u32,
    #[serde(default)]
    inputs: Option<Vec<InputPath>>,
    #[serde(default)]
    outputs: Option<Vec<OutputPath>>,
    #[serde(default)]
    busses: Option<Vec<Bus>>,
    #[serde(default)]
    monitor_output: Option<String>,
    #[serde(default)]
    meter_output: Option<String>,
}

const FORMAT: &str = "soundcraft-io-settings";

/// Which tabs a call means: `tabs` (an array of input/output/bus), else all three.
fn tabs(p: &Value, id: &str) -> Result<(bool, bool, bool)> {
    let Some(arr) = p.get("tabs") else { return Ok((true, true, true)) };
    let arr = arr.as_array().ok_or_else(|| bad(id, "`tabs` must be an array of input, output or bus"))?;
    let (mut i, mut o, mut b) = (false, false, false);
    for t in arr {
        match t.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("input") | Some("inputs") => i = true,
            Some("output") | Some("outputs") => o = true,
            Some("bus") | Some("busses") | Some("buses") => b = true,
            _ => return Err(bad(id, format!("unknown tab {t}: input, output or bus"))),
        }
    }
    Ok((i, o, b))
}

fn path_param(p: &Value, id: &str) -> Result<std::path::PathBuf> {
    let path = str_param(p, "path").map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(id, "`path` required"))?;
    let mut path = std::path::PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension(IO_SETTINGS_EXTENSION);
    }
    Ok(path)
}

fn export_io(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.io_export";
    let path = path_param(p, id)?;
    let (inputs, outputs, busses) = tabs(p, id)?;
    let s = e.session();
    let settings = IoSettings {
        format: FORMAT.into(),
        version: 1,
        inputs: inputs.then(|| s.inputs.clone()),
        outputs: outputs.then(|| s.outputs.clone()),
        busses: busses.then(|| s.busses.clone()),
        monitor_output: if outputs { s.monitor_output.clone() } else { None },
        meter_output: if outputs { s.meter_output.clone() } else { None },
    };
    let text = serde_json::to_string_pretty(&settings).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_file(&path.to_string_lossy(), text.as_bytes())?;
    Ok(json!({"path": path, "inputs": inputs, "outputs": outputs, "busses": busses}))
}

/// Reads and parses a settings file through the checked reader (inside the read root, a regular
/// file), refusing anything that is not a settings file.
fn read_settings(path: &Path, id: &str) -> Result<IoSettings> {
    let text = crate::io::read_text(path)?;
    let settings: IoSettings =
        serde_json::from_str(&text).map_err(|err| bad(id, format!("{} is not an I/O settings file: {err}", path.display())))?;
    if settings.format != FORMAT {
        return Err(bad(id, format!("{} is not an I/O settings file (format `{}`)", path.display(), settings.format)));
    }
    Ok(settings)
}

/// Whether any track or send routes to a bus (or one of its sub-paths) or names a hardware path.
fn bus_used(s: &Session, b: &Bus) -> bool {
    let hit = |r: &Route| match r {
        Route::Bus(id) => *id == b.id,
        Route::BusSub(id) => b.sub_paths.iter().any(|sp| sp.id == *id),
        _ => false,
    };
    s.tracks.iter().any(|t| hit(&t.mixer.input) || hit(&t.mixer.output) || t.mixer.sends.iter().flatten().any(|snd| hit(&snd.target)))
}

fn hardware_used(s: &Session, name: &str, input: bool) -> bool {
    let hit = |r: &Route| matches!(r, Route::Hardware(h) if h.eq_ignore_ascii_case(name));
    s.tracks
        .iter()
        .any(|t| if input { hit(&t.mixer.input) } else { hit(&t.mixer.output) || t.mixer.sends.iter().flatten().any(|snd| hit(&snd.target)) })
        || (!input && (s.busses.iter().any(|b| b.output.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(name)))))
}

fn import_io(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.io_import";
    let path = path_param(p, id)?;
    let (want_in, want_out, want_bus) = tabs(p, id)?;
    let replace = bool_or(p, "replace", false);
    let settings = read_settings(&path, id)?;
    let s = e.session_mut();
    let mut report = json!({});
    if want_in && let Some(mut paths) = settings.inputs {
        paths.truncate(MAX_INPUT_PATHS);
        paths.iter_mut().for_each(|p| p.sanitize(true));
        if replace {
            let keep: Vec<String> = paths.iter().map(|p| p.name.clone()).collect();
            let unused: Vec<String> = s
                .inputs
                .iter()
                .filter(|m| {
                    !keep.iter().any(|k| k.eq_ignore_ascii_case(&m.name))
                        && !hardware_used(s, &m.name, true)
                        && !m.sub_paths.iter().any(|sp| hardware_used(s, &sp.name, true))
                })
                .map(|m| m.name.clone())
                .collect();
            s.inputs.retain(|m| !unused.contains(&m.name));
        }
        let mut added = 0;
        let mut replaced = 0;
        for path in paths {
            match s.inputs.iter().position(|m| m.name.eq_ignore_ascii_case(&path.name)) {
                Some(i) => {
                    if let Some(slot) = s.inputs.get_mut(i) {
                        *slot = path;
                        replaced += 1;
                    }
                }
                None => {
                    if s.inputs.len() < MAX_INPUT_PATHS {
                        s.inputs.push(path);
                        added += 1;
                    }
                }
            }
        }
        report["inputs"] = json!({"added": added, "replaced": replaced});
    }
    if want_out && let Some(mut paths) = settings.outputs {
        paths.truncate(MAX_INPUT_PATHS);
        paths.iter_mut().for_each(OutputPath::sanitize);
        if replace {
            let keep: Vec<String> = paths.iter().map(|p| p.name.clone()).collect();
            let unused: Vec<String> = s
                .outputs
                .iter()
                .filter(|o| !keep.iter().any(|k| k.eq_ignore_ascii_case(&o.name)) && !hardware_used(s, &o.name, false))
                .map(|o| o.name.clone())
                .collect();
            s.outputs.retain(|o| !unused.contains(&o.name));
        }
        let mut added = 0;
        let mut replaced = 0;
        for path in paths {
            match s.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(&path.name)) {
                Some(i) => {
                    if let Some(slot) = s.outputs.get_mut(i) {
                        *slot = path;
                        replaced += 1;
                    }
                }
                None => {
                    if s.outputs.len() < MAX_INPUT_PATHS {
                        s.outputs.push(path);
                        added += 1;
                    }
                }
            }
        }
        for (name, slot) in [(settings.monitor_output, &mut s.monitor_output), (settings.meter_output, &mut s.meter_output)] {
            if let Some(n) = name {
                *slot = Some(n);
            }
        }
        report["outputs"] = json!({"added": added, "replaced": replaced});
    }
    if want_bus && let Some(mut busses) = settings.busses {
        busses.truncate(MAX_INPUT_PATHS);
        busses.iter_mut().for_each(Bus::sanitize);
        if replace {
            let keep: Vec<String> = busses.iter().map(|b| b.name.clone()).collect();
            let unused: Vec<soundcraft_model::BusId> =
                s.busses.iter().filter(|b| !keep.iter().any(|k| k.eq_ignore_ascii_case(&b.name)) && !bus_used(s, b)).map(|b| b.id).collect();
            s.busses.retain(|b| !unused.contains(&b.id));
        }
        let mut added = 0;
        let mut replaced = 0;
        for bus in busses {
            // Names stay unique across busses and sub-paths; an imported sub-path whose name
            // another bus owns is dropped.
            match s.busses.iter().position(|b| b.name.eq_ignore_ascii_case(&bus.name)) {
                Some(i) => {
                    let mut subs: Vec<BusSubPath> = Vec::new();
                    let existing: Vec<BusSubPath> = s.busses.get(i).map(|b| b.sub_paths.clone()).unwrap_or_default();
                    for mut sp in bus.sub_paths {
                        let taken_elsewhere = s.busses.iter().enumerate().any(|(j, b)| {
                            j != i && (b.name.eq_ignore_ascii_case(&sp.name) || b.sub_paths.iter().any(|x| x.name.eq_ignore_ascii_case(&sp.name)))
                        });
                        if taken_elsewhere {
                            continue;
                        }
                        sp.id = existing.iter().find(|x| x.name.eq_ignore_ascii_case(&sp.name)).map_or_else(|| BusSubId(s.alloc()), |x| x.id);
                        subs.push(sp);
                    }
                    if let Some(b) = s.busses.get_mut(i) {
                        b.format = bus.format;
                        b.active = bus.active;
                        b.output = bus.output;
                        b.sub_paths = subs;
                        b.sanitize();
                        replaced += 1;
                    }
                }
                None => {
                    if s.bus_path_by_name(&bus.name).is_some() || s.busses.len() >= MAX_INPUT_PATHS {
                        continue;
                    }
                    let id = s.add_bus(&bus.name, bus.format);
                    let mut subs = Vec::new();
                    for mut sp in bus.sub_paths {
                        if s.bus_path_by_name(&sp.name).is_some() {
                            continue;
                        }
                        sp.id = BusSubId(s.alloc());
                        subs.push(sp);
                    }
                    if let Some(b) = s.busses.iter_mut().find(|b| b.id == id) {
                        b.active = bus.active;
                        b.output = bus.output;
                        b.sub_paths = subs;
                        b.sanitize();
                    }
                    added += 1;
                }
            }
        }
        report["busses"] = json!({"added": added, "replaced": replaced});
    }
    // A mapping or monitor path naming an output that isn't there comes off.
    let names: Vec<String> = s.outputs.iter().map(|o| o.name.clone()).collect();
    let known = |n: &str| names.iter().any(|k| k.eq_ignore_ascii_case(n));
    for b in &mut s.busses {
        if b.output.as_deref().is_some_and(|o| !known(o)) {
            b.output = None;
        }
    }
    for slot in [&mut s.monitor_output, &mut s.meter_output] {
        if slot.as_deref().is_some_and(|n| !known(n)) {
            *slot = None;
        }
    }
    s.sanitize();
    report["path"] = json!(path);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackId};

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-io-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn io(e: &mut Engine, action: &str, rest: Value) -> Result<Value> {
        let mut p = rest;
        p["action"] = json!(action);
        e.execute("setup.io", &p)
    }

    #[test]
    fn a_settings_file_round_trips_all_three_tabs_and_keeps_bus_ids() {
        let d = dir("roundtrip");
        let file = d.join("studio");
        let mut e = Engine::default();
        io(&mut e, "default_outputs", json!({"channels": 4})).unwrap();
        io(&mut e, "create_input", json!({"name": "Vocal", "format": "mono"})).unwrap();
        io(&mut e, "create_bus", json!({"name": "Verb"})).unwrap();
        io(&mut e, "map_bus_output", json!({"name": "Verb", "output": "Out 3-4"})).unwrap();
        io(&mut e, "set_monitor_output", json!({"name": "Out 3-4"})).unwrap();
        let out = e.execute("setup.io_export", &json!({"path": file})).unwrap();
        assert_eq!(out["path"].as_str().map(|p| p.ends_with(".scio")), Some(true));
        assert!(e.undo(), "the last edit is still the monitor path: an export is no edit");
        assert_eq!(e.session().monitor_output, None);
        // Another session, with a bus of the same name a track is routed to.
        let mut f = Engine::default();
        let verb = f.execute("mix.new_bus", &json!({"name": "Verb", "format": "mono"})).unwrap()["bus"].as_u64().unwrap();
        let t = f.execute("track.new", &json!({"name": "A", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        f.execute("track.output", &json!({"tracks": [t], "output": "Verb"})).unwrap();
        let r = f.execute("setup.io_import", &json!({"path": file.with_extension("scio")})).unwrap();
        assert_eq!(r["busses"]["replaced"], 1);
        assert_eq!(r["inputs"]["added"], 1);
        assert_eq!(r["outputs"]["added"], 1, "Out 3-4 came in; Out 1-2 was replaced");
        let s = f.session();
        assert!(s.input_path("Vocal").is_some());
        assert_eq!(s.monitor_output.as_deref(), Some("Out 3-4"));
        let bus = s.bus_by_name("Verb").unwrap();
        assert_eq!(
            (bus.id.0, bus.format, bus.output.as_deref()),
            (verb, ChannelFormat::Stereo, Some("Out 3-4")),
            "the bus kept its id and took the settings"
        );
        assert_eq!(bus.sub_paths.iter().map(|sp| sp.name.as_str()).collect::<Vec<_>>(), ["Verb.L", "Verb.R"]);
        assert_eq!(s.track(TrackId(t)).unwrap().mixer.output, Route::Bus(bus.id), "still routed");
        assert!(f.undo());
        assert!(f.session().input_path("Vocal").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tabs_choose_what_goes_in_and_out_and_replace_drops_the_unused() {
        let d = dir("tabs");
        let file = d.join("busses.scio");
        let mut e = Engine::default();
        io(&mut e, "create_bus", json!({"name": "Keep", "sub_paths": false})).unwrap();
        e.execute("setup.io_export", &json!({"path": file, "tabs": ["bus"]})).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"busses\"") && !text.contains("\"In 1-2\""), "{text}");
        let mut f = Engine::default();
        io(&mut f, "create_bus", json!({"name": "Unused", "sub_paths": false})).unwrap();
        io(&mut f, "create_bus", json!({"name": "Used", "sub_paths": false})).unwrap();
        let t = f.execute("track.new", &json!({"name": "A", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        f.execute("track.output", &json!({"tracks": [t], "output": "Used"})).unwrap();
        f.execute("setup.io_import", &json!({"path": file, "replace": true})).unwrap();
        let names: Vec<String> = f.session().busses.iter().map(|b| b.name.clone()).collect();
        assert_eq!(names, ["Used", "Keep"], "the unused bus went, the routed one stayed, the imported one came");
        assert_eq!(f.session().inputs.len(), 4, "inputs untouched: the file had none");
        assert!(f.execute("setup.io_import", &json!({"path": file, "tabs": ["nope"]})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hostile_files_are_refused_or_tamed() {
        let d = dir("hostile");
        let mut e = Engine::default();
        let junk = d.join("junk.scio");
        std::fs::write(&junk, "{\"hello\": 1}").unwrap();
        assert!(e.execute("setup.io_import", &json!({"path": junk})).is_err(), "not a settings file");
        std::fs::write(&junk, "not json").unwrap();
        assert!(e.execute("setup.io_import", &json!({"path": junk})).is_err());
        assert!(e.execute("setup.io_import", &json!({"path": d.join("missing.scio")})).is_err());
        let wild = d.join("wild.scio");
        let text = format!(
            "{{\"format\": \"{FORMAT}\", \"version\": 1, \"inputs\": [{{\"name\": \"{}\", \"format\": \"Stereo\", \"channels\": [60000, 1, 1], \"sub_paths\": [{{\"name\": \"x\", \"format\": \"Mono\", \"channels\": [7], \"sub_paths\": [{{\"name\": \"deep\", \"format\": \"Mono\", \"channels\": []}}]}}]}}], \"busses\": [{{\"id\": 999, \"name\": \"B\", \"format\": \"Stereo\", \"output\": \"Nowhere\", \"sub_paths\": [{{\"id\": 2, \"name\": \"B.L\", \"format\": \"Mono\", \"channels\": [9]}}]}}], \"monitor_output\": \"Gone\"}}",
            "n".repeat(2000)
        );
        std::fs::write(&wild, text).unwrap();
        e.execute("setup.io_import", &json!({"path": wild})).unwrap();
        let s = e.session();
        let p = s.inputs.last().unwrap();
        assert_eq!(p.name.chars().count(), soundcraft_model::MAX_NAME_CHARS);
        assert_eq!(p.channels, [None, Some(1)]);
        assert!(p.sub_paths[0].sub_paths.is_empty() && p.sub_paths[0].channels == [None]);
        let b = s.bus_by_name("B").unwrap();
        assert_eq!(
            (b.output.clone(), b.sub_paths[0].channels.clone()),
            (None, vec![None]),
            "a mapping to no output and a channel off the bus come off"
        );
        assert_ne!(b.id.0, 999, "ids come from the session, not the file");
        assert_eq!(s.monitor_output, None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
