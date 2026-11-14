//! Send to New Track: a send's destination menu makes the track it feeds. As in Pro Tools, the new
//! track (Aux Input, Audio or Instrument) takes the name typed for it, a new internal bus of the
//! same name and width becomes its input, and the send is assigned to that bus, all as one Undo step.
use super::{CommandSpec, bad, has_selection, tracks_required};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{ChannelFormat, MAX_NAME_CHARS, Route, SEND_SLOTS, Session, TrackId, TrackKind, truncate_chars};

const CMD: &str = "mix.send_new_track";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.send_new_track",
        "Send to New Track",
        [],
        None,
        "{track?, slot?: 0..9|a..j (first free), kind?: aux|audio|instrument, format?: mono|stereo|5.1…, timebase?: samples|ticks, name?, next_to_track?: true}",
        has_selection,
        send_new_track
    )]
}

/// Creates the destination track and its bus, then points the send at the bus.
fn send_new_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let source = tracks_required(e, CMD, p)?.first().copied().ok_or_else(|| bad(CMD, "no track"))?;
    let explicit = super::mix::optional_slot(p, CMD, SEND_SLOTS)?;
    // Only tracks with an audio input can be fed by a bus.
    let kind = match text(p, "kind")? {
        None => TrackKind::Aux,
        Some(k) => match TrackKind::from_id(k) {
            Some(kind @ (TrackKind::Aux | TrackKind::Audio | TrackKind::Instrument)) => kind,
            Some(_) => return Err(bad(CMD, format!("a `{k}` track has no input a send can feed; use aux, audio or instrument"))),
            None => return Err(bad(CMD, format!("unknown kind `{k}`"))),
        },
    };
    let mut name = text(p, "name")?.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(kind.default_name()).to_string();
    truncate_chars(&mut name, MAX_NAME_CHARS);
    let mut row = json!({"kind": kind.id(), "name": name});
    if let Some(f) = text(p, "format")? {
        ChannelFormat::from_id(f).ok_or_else(|| bad(CMD, format!("unknown format `{f}`")))?;
        row["format"] = json!(f);
    }
    if let Some(t) = text(p, "timebase")? {
        if !matches!(t, "samples" | "ticks") {
            return Err(bad(CMD, format!("unknown timebase `{t}`")));
        }
        row["timebase"] = json!(t);
    }
    let next_to_track = match p.get("next_to_track") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().ok_or_else(|| bad(CMD, "`next_to_track` must be true or false"))?,
    };
    // Checked before anything is made, so a full or missing source makes nothing.
    let sends = &e.session().track(source).ok_or_else(|| bad(CMD, "no track"))?.mixer.sends;
    let slot = explicit.or_else(|| sends.iter().position(Option::is_none)).ok_or_else(|| bad(CMD, "all send slots are in use"))?;

    let track = super::track::new_track_row(e, &row)?.first().copied().ok_or_else(|| bad(CMD, "the track was not created"))?;
    let s = e.session_mut();
    let (track_name, format) = s.track(track).map(|t| (t.name.clone(), t.format)).ok_or_else(|| bad(CMD, "the track was not created"))?;
    let bus_name = unique_bus_name(s, &track_name);
    let bus = s.add_bus(&bus_name, format);
    if let Some(t) = s.track_mut(track) {
        t.mixer.input = Route::Bus(bus);
    }
    if next_to_track {
        place_after(s, track, source);
    }
    super::mix::send(e, &json!({"track": source.0, "slot": slot, "bus": bus_name}))?;
    Ok(json!({"track": track, "bus": bus, "slot": slot, "name": track_name}))
}

/// The text parameter `key`, if given; any other kind of value is an error rather than the default.
fn text<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad(CMD, format!("`{key}` must be text"))),
    }
}

/// `base`, or `base 2`, `base 3`… when a bus already has that name. Bus names compare without case,
/// as `Session::bus_by_name` does, so the new bus can never resolve to an older one.
fn unique_bus_name(s: &Session, base: &str) -> String {
    let taken = |name: &str| s.busses.iter().any(|b| b.name.eq_ignore_ascii_case(name));
    (1..=s.busses.len().saturating_add(1))
        .map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") })
        .find(|name| !taken(name))
        .unwrap_or_else(|| format!("{base} {}", s.busses.len().saturating_add(2)))
}

/// Moves `track` to just below `source`, and below the tracks inside `source` when it is a folder,
/// into the folder `source` is in. A Master Fader stays at the bottom, so nothing goes below it.
fn place_after(s: &mut Session, track: TrackId, source: TrackId) {
    let Some(from) = s.track_index(track) else { return };
    let mut moved = s.tracks.remove(from);
    let folder = s.track(source).and_then(|t| t.folder);
    let mut at = s.track_index(source).map_or(from, |i| i.saturating_add(1));
    while s.tracks.get(at).is_some_and(|t| inside(s, t.folder, source)) {
        at += 1;
    }
    let first_master = s.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(s.tracks.len());
    moved.folder = folder;
    s.tracks.insert(at.min(first_master), moved);
}

/// Whether a track whose folder is `folder` sits somewhere inside `source`. Each step goes one
/// folder up, at most once per track, so a damaged session with a folder cycle still ends.
fn inside(s: &Session, mut folder: Option<TrackId>, source: TrackId) -> bool {
    for _ in 0..s.tracks.len() {
        match folder {
            Some(f) if f == source => return true,
            Some(f) => folder = s.track(f).and_then(|t| t.folder),
            None => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests;
