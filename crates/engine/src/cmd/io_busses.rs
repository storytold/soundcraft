//! Setup › I/O › Bus: the bus actions of `setup.io` (the command lives in `setup_more.rs`):
//! internal mix busses, their sub-paths (routing targets within a bus), their mapping to an
//! output path, and Pro Tools' defaults. Names are unique across busses and sub-paths (and
//! tracks route to sub-paths by id, so renaming one changes nothing else).

use super::*;
use serde_json::json;
use soundcraft_model::{Bus, BusId, BusSubId, BusSubPath, ChannelFormat, MAX_SUB_PATHS, Route, Session, default_bus_sub_paths};

const ID: &str = "setup.io";

/// Runs a bus action; `None` when `action` isn't one.
pub(crate) fn bus_action(e: &mut Engine, action: &str, p: &Value) -> Option<Result<Value>> {
    Some(match action {
        "create_bus" => create_bus(e, p),
        "rename_bus" => rename_bus(e, p),
        "delete_bus" => delete_bus(e, p),
        "set_bus_format" => set_format(e, p),
        "set_bus_active" => set_active(e, p),
        "map_bus_output" => map_output(e, p),
        "create_bus_subpath" => create_subpath(e, p),
        "map_bus_subpath" => map_subpath(e, p),
        "set_bus_subpath_channel" => set_subpath_channel(e, p),
        "default_busses" => default_busses(e, p),
        "sort_busses" => sort_busses(e, p),
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

/// Bus and sub-path names are one namespace: track menus list both.
fn ensure_free(s: &Session, name: &str) -> Result<()> {
    if s.bus_path_by_name(name).is_some() { Err(bad(ID, format!("a bus or sub-path named `{name}` exists"))) } else { Ok(()) }
}

/// The bus named `name`, or the bus a sub-path named `name` belongs to and that sub-path's id.
fn find(s: &Session, name: &str) -> Result<(BusId, Option<BusSubId>)> {
    s.bus_path_by_name(name).map(|(b, sp)| (b.id, sp.map(|sp| sp.id))).ok_or_else(|| bad(ID, format!("no bus or sub-path `{name}`")))
}

fn bus_mut(s: &mut Session, id: BusId) -> Result<&mut Bus> {
    s.busses.iter_mut().find(|b| b.id == id).ok_or_else(|| bad(ID, format!("no bus {id}")))
}

/// `{name, format?: stereo, count?: 1, sub_paths?: true}`: new busses named `name` (`name 1`,
/// `name 2`… for several), with the format's default sub-paths unless `sub_paths` is false.
fn create_bus(e: &mut Engine, p: &Value) -> Result<Value> {
    let base = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let count = usize::try_from(i64_or(p, "count", 1).clamp(1, 256)).unwrap_or(1);
    let subs = bool_or(p, "sub_paths", true);
    let s = e.session_mut();
    let names: Vec<String> = (1..=count).map(|k| if count == 1 { base.clone() } else { format!("{base} {k}") }).collect();
    for n in &names {
        ensure_free(s, n)?;
    }
    let mut made = Vec::new();
    for n in names {
        let id = s.add_bus(&n, format);
        if subs {
            let paths = s.bus(id).map(|b| default_bus_sub_paths(b, || 0)).unwrap_or_default();
            let mut paths: Vec<BusSubPath> = paths.into_iter().filter(|sp| s.bus_path_by_name(&sp.name).is_none()).collect();
            for sp in &mut paths {
                sp.id = BusSubId(s.alloc());
            }
            bus_mut(s, id)?.sub_paths = paths;
        }
        made.push(id);
    }
    Ok(json!({"created": made, "busses": s.busses}))
}

/// `{name, new_name}`: renames a bus (its `<name>.x` sub-paths follow) or a sub-path. Routes are
/// by id, so nothing else changes.
fn rename_bus(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let to = name_param(p, "new_name")?;
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    if !to.eq_ignore_ascii_case(&name) {
        ensure_free(s, &to)?;
    }
    let b = bus_mut(s, bid)?;
    let mut renamed = Vec::new();
    match sub {
        Some(sid) => {
            let sp = b.sub_paths.iter_mut().find(|sp| sp.id == sid).ok_or_else(|| bad(ID, format!("no sub-path `{name}`")))?;
            renamed.push((std::mem::replace(&mut sp.name, to.clone()), to.clone()));
        }
        None => {
            let old = std::mem::replace(&mut b.name, to.clone());
            let prefix = format!("{old}.");
            for sp in &mut b.sub_paths {
                if sp.name.len() > prefix.len() && sp.name.get(..prefix.len()).is_some_and(|x| x.eq_ignore_ascii_case(&prefix)) {
                    let suffix = sp.name.get(prefix.len()..).unwrap_or_default().to_string();
                    renamed.push((std::mem::replace(&mut sp.name, format!("{to}.{suffix}")), format!("{to}.{suffix}")));
                }
            }
            renamed.push((old, to.clone()));
        }
    }
    Ok(json!({"renamed": renamed, "busses": s.busses}))
}

/// `{name}`: removes a bus (with its sub-paths) or a sub-path; tracks, sends and auxes on them
/// lose the route.
fn delete_bus(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    let (gone_bus, gone_subs): (Option<BusId>, Vec<BusSubId>) = match sub {
        Some(sid) => {
            bus_mut(s, bid)?.sub_paths.retain(|sp| sp.id != sid);
            (None, vec![sid])
        }
        None => {
            let subs: Vec<BusSubId> = s.bus(bid).map(|b| b.sub_paths.iter().map(|sp| sp.id).collect()).unwrap_or_default();
            s.busses.retain(|b| b.id != bid);
            (Some(bid), subs)
        }
    };
    let dead = |r: &Route| match r {
        Route::Bus(b) => Some(*b) == gone_bus,
        Route::BusSub(id) => gone_subs.contains(id),
        _ => false,
    };
    for t in &mut s.tracks {
        if dead(&t.mixer.input) {
            t.mixer.input = Route::None;
        }
        if dead(&t.mixer.output) {
            t.mixer.output = Route::None;
        }
        for slot in &mut t.mixer.sends {
            if slot.as_ref().is_some_and(|x| dead(&x.target)) {
                *slot = None;
            }
        }
    }
    Ok(json!({"deleted": name, "busses": s.busses}))
}

/// `{name, format}`: a bus's width (its sub-paths lose channels that left it) or a sub-path's
/// (within its bus).
fn set_format(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let format = format_param(p, ChannelFormat::Stereo)?;
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    let b = bus_mut(s, bid)?;
    match sub {
        Some(sid) => {
            if format.channels() > b.width() {
                return Err(bad(ID, format!("a {} sub-path does not fit in the {} bus `{}`", format.label(), b.format.label(), b.name)));
            }
            let sp = b.sub_paths.iter_mut().find(|sp| sp.id == sid).ok_or_else(|| bad(ID, format!("no sub-path `{name}`")))?;
            sp.format = format;
            sp.channels.resize(format.channels(), None);
        }
        None => {
            if format.is_ambisonic() {
                return Err(bad(ID, "a bus has speakers, not Ambisonics"));
            }
            b.format = format;
            if let Some(o) = b.output.clone()
                && s.output_path(&o).is_some_and(|o| o.width() < format.channels())
            {
                bus_mut(s, bid)?.output = None;
            }
        }
    }
    bus_mut(s, bid)?.sanitize();
    Ok(json!({"busses": s.busses}))
}

/// `{name, active: bool}`: an inactive bus takes and gives no signal (its sub-paths with it).
fn set_active(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let active = p.get("active").and_then(Value::as_bool).ok_or_else(|| bad(ID, "`active` (bool) required"))?;
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    if sub.is_some() {
        return Err(bad(ID, "a sub-path is active with its bus"));
    }
    bus_mut(s, bid)?.active = active;
    Ok(json!({"busses": s.busses}))
}

/// `{name, output: path|null, all?: false, cascade?: false}`: Mapping to Output: the bus's mix
/// also drives the output path (of its width or wider). `all` maps every bus of the same format
/// to it (Alt-click); `cascade` maps them to successive output paths of that width from it
/// (Cmd+Alt-click).
fn map_output(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let output = match p.get("output") {
        None | Some(Value::Null) => None,
        Some(_) => Some(name_param(p, "output")?),
    };
    let all = bool_or(p, "all", false);
    let cascade = bool_or(p, "cascade", false);
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    if sub.is_some() {
        return Err(bad(ID, "a sub-path goes where its bus goes"));
    }
    let width = s.bus(bid).map_or(0, Bus::width);
    let fits = |s: &Session, o: &str| -> Result<String> {
        let path = s.output_path(o).ok_or_else(|| bad(ID, format!("no output path `{o}`")))?;
        if path.width() < width {
            return Err(bad(ID, format!("`{}` ({}) is narrower than the bus ({})", path.name, path.format.label(), width)));
        }
        Ok(path.name.clone())
    };
    let format = s.bus(bid).map(|b| b.format);
    let targets: Vec<BusId> = if all || cascade { s.busses.iter().filter(|b| Some(b.format) == format).map(|b| b.id).collect() } else { vec![bid] };
    match (output, cascade) {
        (None, _) => {
            for id in targets {
                bus_mut(s, id)?.output = None;
            }
        }
        (Some(o), false) => {
            let o = fits(s, &o)?;
            for id in targets {
                bus_mut(s, id)?.output = Some(o.clone());
            }
        }
        (Some(o), true) => {
            // Successive output paths of the bus's width, from the chosen one on, in I/O Setup order.
            fits(s, &o)?;
            let start = s.find_output_path(&o).unwrap_or(0);
            let paths: Vec<String> = s.outputs.iter().skip(start).filter(|p| p.active && p.width() == width).map(|p| p.name.clone()).collect();
            for (id, path) in targets.into_iter().zip(paths) {
                bus_mut(s, id)?.output = Some(path);
            }
        }
    }
    Ok(json!({"busses": s.busses}))
}

/// `{bus, name?, format?: mono, assign?: true}`: a sub-path of `bus`, reading its first
/// channels (nothing with `assign: false`); named `<bus>.<n>` by default.
fn create_subpath(e: &mut Engine, p: &Value) -> Result<Value> {
    let bus_name = name_param(p, "bus")?;
    let format = format_param(p, ChannelFormat::Mono)?;
    let assign = bool_or(p, "assign", true);
    let given = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let s = e.session_mut();
    let (bid, sub) = find(s, &bus_name)?;
    if sub.is_some() {
        return Err(bad(ID, "a sub-path has no sub-paths"));
    }
    let (bus_width, bus_name) = s.bus(bid).map(|b| (b.width(), b.name.clone())).ok_or_else(|| bad(ID, "no bus"))?;
    if s.bus(bid).is_some_and(|b| b.sub_paths.len() >= MAX_SUB_PATHS) {
        return Err(bad(ID, format!("at most {MAX_SUB_PATHS} sub-paths per bus")));
    }
    if format.channels() > bus_width {
        return Err(bad(ID, format!("a {} sub-path does not fit in the bus `{bus_name}`", format.label())));
    }
    let name = match given {
        Some(mut n) => {
            soundcraft_model::truncate_chars(&mut n, soundcraft_model::MAX_NAME_CHARS);
            n
        }
        None => (1..).map(|k| format!("{bus_name}.{k}")).find(|n| s.bus_path_by_name(n).is_none()).unwrap_or_default(),
    };
    ensure_free(s, &name)?;
    let mut sp = BusSubPath::new(BusSubId(s.alloc()), name, format);
    if assign {
        sp.assign_from(0, bus_width);
    }
    let id = sp.id;
    bus_mut(s, bid)?.sub_paths.push(sp);
    Ok(json!({"created": id, "busses": s.busses}))
}

/// `{name, channel: k|null}`: the grid click: the sub-path reads the bus from channel `k` on
/// (its channels to the right); `null` clears it.
fn map_subpath(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let from = match p.get("channel") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            u8::try_from(v.as_u64().ok_or_else(|| bad(ID, "`channel` must be a bus channel index or null"))?)
                .map_err(|_| bad(ID, "`channel` out of range"))?,
        ),
    };
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    let sid = sub.ok_or_else(|| bad(ID, format!("`{name}` is a bus; the grid maps its sub-paths")))?;
    let b = bus_mut(s, bid)?;
    let w = b.width();
    let sp = b.sub_paths.iter_mut().find(|sp| sp.id == sid).ok_or_else(|| bad(ID, format!("no sub-path `{name}`")))?;
    match from {
        None => sp.channels.iter_mut().for_each(|c| *c = None),
        Some(k) if usize::from(k) < w => sp.assign_from(k, w),
        Some(k) => return Err(bad(ID, format!("the bus has {w} channels, not {}", usize::from(k) + 1))),
    }
    b.sanitize();
    Ok(json!({"busses": s.busses}))
}

/// `{name, channel: k, bus_channel: n|null}`: one sub-path channel reads bus channel `n`
/// (another of its channels that read it is cleared), or nothing.
fn set_subpath_channel(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = name_param(p, "name")?;
    let k = usize::try_from(p.get("channel").and_then(Value::as_u64).ok_or_else(|| bad(ID, "`channel` (index in the sub-path) required"))?)
        .unwrap_or(usize::MAX);
    let n = match p.get("bus_channel") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            u8::try_from(v.as_u64().ok_or_else(|| bad(ID, "`bus_channel` must be a bus channel index or null"))?)
                .map_err(|_| bad(ID, "`bus_channel` out of range"))?,
        ),
    };
    let s = e.session_mut();
    let (bid, sub) = find(s, &name)?;
    let sid = sub.ok_or_else(|| bad(ID, format!("`{name}` is a bus")))?;
    let b = bus_mut(s, bid)?;
    let w = b.width();
    if n.is_some_and(|n| usize::from(n) >= w) {
        return Err(bad(ID, format!("the bus has {w} channels")));
    }
    let sp = b.sub_paths.iter_mut().find(|sp| sp.id == sid).ok_or_else(|| bad(ID, format!("no sub-path `{name}`")))?;
    if k >= sp.width() {
        return Err(bad(ID, format!("`{}` has {} channels", sp.name, sp.width())));
    }
    for (idx, c) in sp.channels.iter_mut().enumerate() {
        if idx == k {
            *c = n;
        } else if n.is_some() && *c == n {
            *c = None;
        }
    }
    b.sanitize();
    Ok(json!({"busses": s.busses}))
}

/// `{which?: all|internal|output, count?: 8}`: the Default button: `internal` adds the stereo
/// busses `Bus 1-2`… up to `count` (with `.L`/`.R` sub-paths); `output` adds a bus per active
/// output path, mapped to it; `all` does both. Busses that exist are kept.
fn default_busses(e: &mut Engine, p: &Value) -> Result<Value> {
    let which = str_param(p, "which").unwrap_or("all");
    let count = usize::try_from(i64_or(p, "count", 8).clamp(0, 128)).unwrap_or(8);
    let s = e.session_mut();
    let mut made = Vec::new();
    match which {
        "all" => {
            made.extend(s.add_output_busses());
            made.extend(s.add_default_busses(count));
        }
        "internal" => made.extend(s.add_default_busses(count)),
        "output" => made.extend(s.add_output_busses()),
        other => return Err(bad(ID, format!("`which` is all, internal or output, not `{other}`"))),
    }
    Ok(json!({"created": made, "busses": s.busses}))
}

/// `{by: name|format|output, descending?: false}`: the column-header sort of the Bus tab; the
/// order is the session's (and the menus').
fn sort_busses(e: &mut Engine, p: &Value) -> Result<Value> {
    let by = str_param(p, "by").unwrap_or("name");
    let desc = bool_or(p, "descending", false);
    let s = e.session_mut();
    match by {
        "name" => s.busses.sort_by_cached_key(|b| b.name.to_lowercase()),
        "format" => s.busses.sort_by_key(|b| (b.format.channels(), b.format.label())),
        "output" => s.busses.sort_by_cached_key(|b| (b.output.is_none(), b.output.clone().unwrap_or_default().to_lowercase())),
        other => return Err(bad(ID, format!("`by` is name, format or output, not `{other}`"))),
    }
    if desc {
        s.busses.reverse();
    }
    Ok(json!({"busses": s.busses}))
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

    fn sub(e: &Engine, name: &str) -> Option<(BusSubId, Vec<Option<u8>>)> {
        e.session().bus_path_by_name(name).and_then(|(_, sp)| sp.map(|sp| (sp.id, sp.channels.clone())))
    }

    #[test]
    fn busses_come_with_sub_paths_and_names_stay_unique() {
        let mut e = Engine::default();
        io(&mut e, "create_bus", json!({"name": "Drums", "count": 2})).unwrap();
        assert!(sub(&e, "Drums 1.L").is_some() && sub(&e, "Drums 2.R").is_some());
        assert_eq!(sub(&e, "Drums 1.R").map(|x| x.1), Some(vec![Some(1)]));
        io(&mut e, "create_bus", json!({"name": "Surr", "format": "5.1"})).unwrap();
        assert_eq!(sub(&e, "Surr.LR").map(|x| x.1), Some(vec![Some(0), Some(1)]));
        assert_eq!(sub(&e, "Surr.LFE").map(|x| x.1), Some(vec![Some(3)]));
        io(&mut e, "create_bus", json!({"name": "Plain", "sub_paths": false})).unwrap();
        assert!(e.session().bus_by_name("Plain").unwrap().sub_paths.is_empty());
        assert!(io(&mut e, "create_bus", json!({"name": "Surr.LR"})).is_err(), "a sub-path's name is taken");
        assert!(io(&mut e, "create_bus", json!({"name": "drums 1"})).is_err());
        let ids: std::collections::BTreeSet<u64> = e.session().busses.iter().flat_map(|b| b.sub_paths.iter().map(|p| p.id.0)).collect();
        assert_eq!(ids.len(), 2 + 2 + 7, "every sub-path has its own id");
        assert!(e.undo());
    }

    #[test]
    fn routing_to_sub_paths_by_name_and_following_deletes_and_renames() {
        let mut e = Engine::default();
        io(&mut e, "create_bus", json!({"name": "Verb"})).unwrap();
        let (left, _) = sub(&e, "Verb.L").unwrap();
        let t = e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.output", &json!({"tracks": [t], "output": "Verb.L"})).unwrap();
        let output = |e: &Engine| e.session().track(TrackId(t)).unwrap().mixer.output.clone();
        assert_eq!(output(&e), Route::BusSub(left));
        e.execute("mix.send", &json!({"track": t, "slot": 0, "bus": "Verb.R"})).unwrap();
        let (right, _) = sub(&e, "Verb.R").unwrap();
        assert_eq!(e.session().track(TrackId(t)).unwrap().mixer.sends[0].as_ref().map(|s| s.target.clone()), Some(Route::BusSub(right)));
        let aux = e.execute("track.new", &json!({"name": "Ret", "kind": "aux", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [aux], "input": "verb.r"})).unwrap();
        assert_eq!(e.session().track(TrackId(aux)).unwrap().mixer.input, Route::BusSub(right));
        // Renaming changes names only.
        io(&mut e, "rename_bus", json!({"name": "Verb", "new_name": "Hall"})).unwrap();
        assert!(
            sub(&e, "Hall.L").is_some() && sub(&e, "Hall.R").is_some(),
            "{:?}",
            e.session().busses[0].sub_paths.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert_eq!(output(&e), Route::BusSub(left));
        io(&mut e, "rename_bus", json!({"name": "Hall.L", "new_name": "Left in"})).unwrap();
        assert!(io(&mut e, "rename_bus", json!({"name": "Hall", "new_name": "left in"})).is_err());
        // Deleting a sub-path drops its routes; deleting the bus drops the rest.
        io(&mut e, "delete_bus", json!({"name": "Left in"})).unwrap();
        assert_eq!(output(&e), Route::None);
        assert_eq!(e.session().track(TrackId(aux)).unwrap().mixer.input, Route::BusSub(right));
        io(&mut e, "delete_bus", json!({"name": "Hall"})).unwrap();
        assert_eq!(e.session().track(TrackId(aux)).unwrap().mixer.input, Route::None);
        assert!(e.session().track(TrackId(t)).unwrap().mixer.sends[0].is_none());
        assert!(e.session().busses.is_empty());
    }

    #[test]
    fn the_bus_grid_and_formats() {
        let mut e = Engine::default();
        io(&mut e, "create_bus", json!({"name": "Q", "format": "quad", "sub_paths": false})).unwrap();
        io(&mut e, "create_bus_subpath", json!({"bus": "Q"})).unwrap();
        assert_eq!(sub(&e, "Q.1").map(|x| x.1), Some(vec![Some(0)]));
        io(&mut e, "create_bus_subpath", json!({"bus": "Q", "name": "Rear", "format": "stereo", "assign": false})).unwrap();
        assert_eq!(sub(&e, "Rear").map(|x| x.1), Some(vec![None, None]));
        io(&mut e, "map_bus_subpath", json!({"name": "Rear", "channel": 2})).unwrap();
        assert_eq!(sub(&e, "Rear").map(|x| x.1), Some(vec![Some(2), Some(3)]));
        io(&mut e, "map_bus_subpath", json!({"name": "Rear", "channel": 3})).unwrap();
        assert_eq!(sub(&e, "Rear").map(|x| x.1), Some(vec![Some(3), None]), "the second channel falls off the bus");
        assert!(io(&mut e, "map_bus_subpath", json!({"name": "Rear", "channel": 4})).is_err());
        assert!(io(&mut e, "map_bus_subpath", json!({"name": "Q", "channel": 0})).is_err(), "a bus itself isn't mapped");
        io(&mut e, "set_bus_subpath_channel", json!({"name": "Rear", "channel": 1, "bus_channel": 3})).unwrap();
        assert_eq!(sub(&e, "Rear").map(|x| x.1), Some(vec![None, Some(3)]), "the channel that read 3 is cleared");
        assert!(io(&mut e, "set_bus_subpath_channel", json!({"name": "Rear", "channel": 1, "bus_channel": 9})).is_err());
        assert!(io(&mut e, "create_bus_subpath", json!({"bus": "Q", "format": "5.1"})).is_err());
        assert!(io(&mut e, "create_bus_subpath", json!({"bus": "Rear"})).is_err());
        io(&mut e, "set_bus_format", json!({"name": "Q", "format": "stereo"})).unwrap();
        assert_eq!(sub(&e, "Rear").map(|x| x.1), Some(vec![None, None]), "channel 3 left the bus");
        assert!(io(&mut e, "set_bus_format", json!({"name": "Rear", "format": "LCR"})).is_err());
        io(&mut e, "set_bus_active", json!({"name": "Q", "active": false})).unwrap();
        assert!(!e.session().bus_by_name("Q").unwrap().active);
        assert!(io(&mut e, "set_bus_active", json!({"name": "Rear", "active": false})).is_err());
    }

    #[test]
    fn mapping_to_output_all_and_cascading_and_the_defaults() {
        let mut e = Engine::default();
        io(&mut e, "default_outputs", json!({"channels": 8})).unwrap();
        io(&mut e, "create_bus", json!({"name": "Stem", "count": 3, "sub_paths": false})).unwrap();
        io(&mut e, "create_bus", json!({"name": "Mono", "format": "mono", "sub_paths": false})).unwrap();
        io(&mut e, "map_bus_output", json!({"name": "Stem 1", "output": "Out 3-4"})).unwrap();
        let mapped = |e: &Engine, n: &str| e.session().bus_by_name(n).and_then(|b| b.output.clone());
        assert_eq!(mapped(&e, "Stem 1").as_deref(), Some("Out 3-4"));
        assert!(io(&mut e, "map_bus_output", json!({"name": "Stem 1", "output": "Nope"})).is_err());
        io(&mut e, "create_output", json!({"name": "Solo", "format": "mono"})).unwrap();
        assert!(io(&mut e, "map_bus_output", json!({"name": "Stem 1", "output": "Solo"})).is_err(), "narrower than the bus");
        io(&mut e, "map_bus_output", json!({"name": "Mono", "output": "Out 1-2"})).unwrap();
        io(&mut e, "map_bus_output", json!({"name": "Stem 1", "output": "Out 5-6", "all": true})).unwrap();
        assert!(["Stem 1", "Stem 2", "Stem 3"].iter().all(|n| mapped(&e, n).as_deref() == Some("Out 5-6")));
        assert_eq!(mapped(&e, "Mono").as_deref(), Some("Out 1-2"), "another format is left alone");
        io(&mut e, "map_bus_output", json!({"name": "Stem 1", "output": "Out 3-4", "cascade": true})).unwrap();
        assert_eq!(
            (mapped(&e, "Stem 1"), mapped(&e, "Stem 2"), mapped(&e, "Stem 3")),
            (Some("Out 3-4".into()), Some("Out 5-6".into()), Some("Out 7-8".into()))
        );
        io(&mut e, "map_bus_output", json!({"name": "Stem 2", "output": null})).unwrap();
        assert_eq!(mapped(&e, "Stem 2"), None);
        io(&mut e, "set_bus_format", json!({"name": "Stem 1", "format": "5.1"})).unwrap();
        assert_eq!(mapped(&e, "Stem 1"), None, "a bus wider than its output comes off it");
        io(&mut e, "default_busses", json!({"which": "internal", "count": 2})).unwrap();
        assert!(e.session().bus_by_name("Bus 1-2").is_some() && e.session().bus_by_name("Bus 3-4").is_some());
        io(&mut e, "default_busses", json!({"which": "output"})).unwrap();
        assert_eq!(mapped(&e, "Out 7-8").as_deref(), Some("Out 7-8"));
        assert!(io(&mut e, "default_busses", json!({"which": "nope"})).is_err());
        io(&mut e, "sort_busses", json!({"by": "name"})).unwrap();
        let names: Vec<String> = e.session().busses.iter().map(|b| b.name.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names, sorted);
        io(&mut e, "sort_busses", json!({"by": "format", "descending": true})).unwrap();
        assert_eq!(e.session().busses[0].name, "Stem 1", "the widest first");
    }
}
