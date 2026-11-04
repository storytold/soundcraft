//! More Track menu commands: Convert Aux to Routing Folder, Extract MIDI, track presets, insert /
//! send bypass subsets, MIDI real-time properties, trim/VCA coalescing and target playlists.
use super::{CommandSpec, bad, has_selection, has_tracks, range_param, str_param, tracks_param, tracks_required};

use super::more_util::{activate_playlist, target_playlist, track_key};
use crate::{Engine, EngineError, Result, cmd};
use serde_json::{Value, json};
use soundcraft_dsp::Category;
use soundcraft_model::{AutoParam, AutomationLane, ChannelFormat, Clip, ClipContent, Insert, Route, Session, Track, TrackId, TrackKind};
use soundcraft_time::Samples;

/// Trim automation lives in a lane addressed past the last insert slot, so the mixer ignores it
/// until it is coalesced into the volume lane.
pub fn trim_param() -> AutoParam {
    AutoParam::Plugin { slot: u8::MAX, param: "trim".into() }
}

const PRESET_FORMAT: &str = "soundcraft-track-preset";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.object",
            "Object / Bed",
            [],
            None,
            "{tracks?, object?: bool} — routes the tracks as immersive objects (true) or to the bed; toggles when `object` is omitted",
            has_selection,
            |e, p| {
                let tracks = tracks_required(e, "track.object", p)?;
                let want = p.get("object").and_then(Value::as_bool);
                let s = e.session_mut();
                let mut out = Vec::new();
                for t in tracks {
                    let key = format!("object.{}", t.0);
                    let on = want.unwrap_or(!s.edit.flag(&key));
                    s.edit.set_flag(&key, on);
                    out.push(json!({"track": t.0, "object": on}));
                }
                Ok(json!({"tracks": out}))
            }
        ),
        cmd!(
            "track.convert_aux_to_folder",
            "Convert Aux to Routing Folder",
            ["Track"],
            None,
            "{tracks?} — aux inputs become routing folders; tracks feeding their input bus become members",
            has_selection,
            convert_aux
        ),
        cmd!(
            "track.extract_midi",
            "Extract MIDI to New Track",
            ["Track"],
            None,
            "{tracks?} — copies the MIDI clips of each MIDI/instrument track to a new MIDI track",
            has_selection,
            extract_midi
        ),
        cmd!(noundo "track.save_preset", "Save Track Preset...", ["Track"], None, "{path, track?} — writes the track's type, width, mixer, inserts, sends and instrument as JSON", has_selection, save_preset),
        cmd!(
            "track.load_preset",
            "Load Track Preset",
            [],
            None,
            "{path, tracks?} — applies a saved track preset's mixer, inserts, sends and instrument",
            has_selection,
            load_preset
        ),
        cmd!("track.bypass_inserts_ae", "Inserts A-E", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| {
            bypass_where(e, p, |i, _| i < 5)
        }),
        cmd!("track.bypass_inserts_fj", "Inserts F-J", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| {
            bypass_where(e, p, |i, _| (5..10).contains(&i))
        }),
        cmd!("track.bypass_eq", "EQ", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool} — EQ plug-ins only", has_selection, |e, p| {
            bypass_where(e, p, |_, c| c == Some(Category::Eq))
        }),
        cmd!("track.bypass_dynamics", "Dynamics", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| bypass_where(
            e,
            p,
            |_, c| c == Some(Category::Dynamics)
        )),
        cmd!("track.bypass_reverb", "Reverb", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| bypass_where(
            e,
            p,
            |_, c| c == Some(Category::Reverb)
        )),
        cmd!("track.bypass_delay", "Delay", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| bypass_where(
            e,
            p,
            |_, c| c == Some(Category::Delay)
        )),
        cmd!("track.bypass_modulation", "Modulation", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, |e, p| {
            bypass_where(e, p, |_, c| c == Some(Category::Modulation))
        }),
        cmd!(
            "track.mute_sends_ae",
            "Sends A-E",
            ["Track", "Mute Sends"],
            None,
            "{tracks?, mute?: bool} (toggles when omitted)",
            has_selection,
            |e, p| mute_sends_where(e, p, 0, 5)
        ),
        cmd!(
            "track.mute_sends_fj",
            "Sends F-J",
            ["Track", "Mute Sends"],
            None,
            "{tracks?, mute?: bool} (toggles when omitted)",
            has_selection,
            |e, p| mute_sends_where(e, p, 5, 10)
        ),
        cmd!(
            "track.write_midi_rtp",
            "Write MIDI Real-Time Properties",
            ["Track"],
            None,
            "{tracks?} — writes the track's real-time properties (velocity, transpose, duration, delay) into its notes and resets them",
            has_selection,
            write_rtp
        ),
        cmd!(
            "track.trim_automation_write",
            "Write Trim Automation",
            [],
            None,
            "{tracks?, start?, end?, db} — writes a relative volume trim (dB) over a range",
            has_selection,
            trim_write
        ),
        cmd!(
            "track.coalesce_trim",
            "Coalesce Trim Automation",
            ["Track"],
            None,
            "{tracks?} — adds trim automation into volume automation and removes the trim",
            has_selection,
            coalesce_trim
        ),
        cmd!(
            "track.coalesce_vca",
            "Coalesce VCA Controller Automation",
            ["Track"],
            None,
            "{tracks?} — for selected VCA masters: folds VCA level/automation into every member's volume automation, then resets the VCA",
            has_tracks,
            coalesce_vca
        ),
        cmd!(
            "track.designate_target_playlist",
            "Designate as Target Playlist",
            ["Track"],
            Some("Cmd+Shift+Right"),
            "{tracks?, index?} — the active playlist (or `index`) becomes the target for Copy/Move Selection to",
            has_selection,
            designate_target
        ),
        cmd!(
            "track.show_target_playlist",
            "Show Target Playlist",
            ["Track"],
            Some("Shift+Right"),
            "{tracks?} — makes the target playlist the main playlist",
            has_selection,
            show_target
        ),
        cmd!(
            "track.toggle_recent_playlist",
            "Toggle Recent Playlist",
            ["Track"],
            Some("Ctrl+Alt+Shift+Left"),
            "{tracks?} — swaps to the previously shown playlist",
            has_selection,
            toggle_recent
        ),
    ]
}

fn convert_aux(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.convert_aux_to_folder", p)?;
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in &tracks {
        let Some(tr) = s.track(*t) else { continue };
        if tr.kind != TrackKind::Aux {
            continue;
        }
        let input = tr.mixer.input.clone();
        if let Some(tr) = s.track_mut(*t) {
            tr.kind = TrackKind::Folder;
            tr.folder_open = true;
        }
        let mut members = 0;
        if let Route::Bus(_) = input {
            for m in s.tracks.iter_mut().filter(|m| m.id != *t && m.mixer.output == input) {
                m.folder = Some(*t);
                members += 1;
            }
        }
        out.push(json!({"track": t, "members": members}));
    }
    if out.is_empty() {
        return Err(bad("track.convert_aux_to_folder", "select one or more aux input tracks"));
    }
    Ok(json!({"converted": out}))
}

fn extract_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.extract_midi", p)?;
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in &tracks {
        let Some(src) = s.track(*t).cloned() else { continue };
        if !src.kind.is_midi() {
            continue;
        }
        let midi: Vec<Clip> = src.clips().iter().filter(|c| matches!(c.content, ClipContent::Midi { .. })).cloned().collect();
        let nt = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some(&format!("{} MIDI", src.name)));
        if let (Some(i), Some(ni)) = (s.track_index(*t), s.track_index(nt)) {
            let tr = s.tracks.remove(ni);
            s.tracks.insert((i + 1).min(s.tracks.len()), tr);
        }
        let mut clips = Vec::new();
        for mut c in midi {
            c.id = s.new_clip_id();
            clips.push(c);
        }
        let n = clips.len();
        if let Some(pl) = s.track_mut(nt).and_then(Track::playlist_mut) {
            pl.clips = clips;
            pl.sort();
        }
        if let Some(tr) = s.track_mut(nt) {
            tr.color = src.color;
            tr.ticks_timebase = src.ticks_timebase;
        }
        out.push(json!({"from": t, "track": nt, "clips": n}));
    }
    if out.is_empty() {
        return Err(bad("track.extract_midi", "select a MIDI or instrument track"));
    }
    Ok(json!({"tracks": out}))
}

fn save_preset(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("track.save_preset", "`path` required"))?.to_string();
    let t = tracks_required(e, "track.save_preset", p)?;
    let first = *t.first().ok_or_else(|| bad("track.save_preset", "no track"))?;
    let tr = e.session().track(first).ok_or_else(|| bad("track.save_preset", "no track"))?;
    let preset = json!({
        "format": PRESET_FORMAT,
        "version": 1,
        "name": tr.name,
        "kind": tr.kind.id(),
        "width": tr.format.label(),
        "color": tr.color,
        "mixer": tr.mixer,
        "instrument": tr.instrument,
        "elastic": tr.elastic,
        "comments": tr.comments,
    });
    let text = serde_json::to_string_pretty(&preset).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_file(&path, text.as_bytes())?;
    Ok(json!({"path": path, "track": first}))
}

fn load_preset(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.load_preset";
    let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let tracks = tracks_required(e, id, p)?;
    let text = crate::io::read_text(&path)?;
    let v: Value = serde_json::from_str(&text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    if v.get("format").and_then(Value::as_str) != Some(PRESET_FORMAT) {
        return Err(EngineError::Io(format!("{path}: not a SoundCraft track preset")));
    }
    let mut mixer: soundcraft_model::Mixer =
        serde_json::from_value(v.get("mixer").cloned().unwrap_or(Value::Null)).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    mixer.sanitize();
    let instrument: Option<Insert> = v.get("instrument").cloned().and_then(|x| serde_json::from_value(x).ok());
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            // Routing is session-specific: keep the track's own input/output, VCA and arm state.
            let keep = (tr.mixer.input.clone(), tr.mixer.output.clone(), tr.mixer.vca, tr.mixer.record_arm, tr.mixer.pan.clone());
            tr.mixer = mixer.clone();
            tr.mixer.input = keep.0;
            tr.mixer.output = keep.1;
            tr.mixer.vca = keep.2;
            tr.mixer.record_arm = keep.3;
            if tr.mixer.pan.len() != keep.4.len() {
                tr.mixer.pan = keep.4;
            }
            if tr.kind == TrackKind::Instrument && instrument.is_some() {
                tr.instrument = instrument.clone();
            }
        }
    }
    Ok(json!({"tracks": tracks}))
}

fn bypass_where(e: &mut Engine, p: &Value, pick: fn(usize, Option<Category>) -> bool) -> Result<Value> {
    let tracks = tracks_required(e, "track.bypass_inserts", p)?;
    let s = e.session_mut();
    let hits = |tr: &Track| -> Vec<usize> {
        tr.mixer
            .inserts
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|ins| (i, ins)))
            .filter(|(i, ins)| pick(*i, soundcraft_dsp::plugin_info(&ins.plugin).map(|info| info.category)))
            .map(|(i, _)| i)
            .collect()
    };
    let all_bypassed = tracks
        .iter()
        .filter_map(|t| s.track(*t))
        .all(|tr| hits(tr).iter().all(|i| tr.mixer.inserts.get(*i).and_then(Option::as_ref).is_some_and(|x| x.bypass)));
    let target = p.get("bypass").and_then(Value::as_bool).unwrap_or(!all_bypassed);
    let mut n = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        for i in hits(tr) {
            if let Some(Some(ins)) = tr.mixer.inserts.get_mut(i) {
                ins.bypass = target;
                n += 1;
            }
        }
    }
    Ok(json!({"bypass": target, "inserts": n}))
}

fn mute_sends_where(e: &mut Engine, p: &Value, from: usize, to: usize) -> Result<Value> {
    let tracks = tracks_required(e, "track.mute_sends", p)?;
    let s = e.session_mut();
    let all_muted = tracks.iter().filter_map(|t| s.track(*t)).all(|tr| tr.mixer.sends.iter().skip(from).take(to - from).flatten().all(|x| x.mute));
    let target = p.get("mute").and_then(Value::as_bool).unwrap_or(!all_muted);
    let mut n = 0;
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for snd in tr.mixer.sends.iter_mut().skip(from).take(to - from).flatten() {
                snd.mute = target;
                n += 1;
            }
        }
    }
    Ok(json!({"mute": target, "sends": n}))
}

/// Real-time properties for a track (stored by Event › MIDI Real-Time Properties).
pub struct Rtp {
    pub velocity: i64,
    pub transpose: i64,
    pub duration_pct: f64,
    pub delay_ticks: i64,
}

pub fn rtp_of(s: &Session, t: TrackId) -> Rtp {
    let v = |name: &str, d: f64| s.edit.value(&track_key("rtp", t, name), d);
    Rtp {
        velocity: v("velocity", 0.0).clamp(-127.0, 127.0) as i64,
        transpose: v("transpose", 0.0).clamp(-127.0, 127.0) as i64,
        duration_pct: v("duration", 100.0).clamp(1.0, 1000.0),
        delay_ticks: v("delay", 0.0).clamp(-1e7, 1e7) as i64,
    }
}

fn write_rtp(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.write_midi_rtp", p)?;
    let s = e.session_mut();
    let mut notes = 0;
    for t in &tracks {
        let r = rtp_of(s, *t);
        let Some(pl) = s.track_mut(*t).and_then(Track::playlist_mut) else { continue };
        for c in &mut pl.clips {
            let ClipContent::Midi { sequence } = &mut c.content else { continue };
            for n in &mut sequence.notes {
                n.velocity = (i64::from(n.velocity) + r.velocity).clamp(1, 127) as u8;
                n.pitch = (i64::from(n.pitch) + r.transpose).clamp(0, 127) as u8;
                n.length = ((n.length as f64 * r.duration_pct / 100.0).round() as i64).max(1);
                n.start = n.start.saturating_add(r.delay_ticks).max(0);
                notes += 1;
            }
            // Playback delays controller events too, but ignores those before tick 0 (hidden by a start trim).
            for c in sequence.ctrls.iter_mut().filter(|c| c.tick >= 0) {
                c.tick = c.tick.saturating_add(r.delay_ticks).max(0);
            }
            sequence.sort();
        }
        let pre = format!("rtp.{}.", t.0);
        s.edit.values.retain(|k, _| !k.starts_with(&pre));
    }
    Ok(json!({"notes": notes}))
}

fn trim_write(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.trim_automation_write";
    let tracks = tracks_required(e, id, p)?;
    let r = range_param(e, id, p)?;
    if r.is_empty() {
        return Err(bad(id, "the selection is empty"));
    }
    let db = p.get("db").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`db` required"))?.clamp(-144.0, 24.0) as f32;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.lane_mut(&trim_param()).write_range(r.start, r.end, db, 0.0);
        }
    }
    Ok(json!({"db": db}))
}

/// Add an offset curve (dB) to a track's volume automation (creating it from the fader level).
fn add_to_volume(tr: &mut Track, offset: &AutomationLane, offset_default: f32) {
    let fader = tr.mixer.volume_db;
    let vol = tr.lane(&AutoParam::Volume).filter(|l| !l.points.is_empty()).cloned();
    let mut times: Vec<Samples> = offset.points.iter().map(|p| p.at).collect();
    if let Some(v) = &vol {
        times.extend(v.points.iter().map(|p| p.at));
    }
    if times.is_empty() {
        tr.mixer.volume_db = (fader + offset_default).clamp(-144.0, 12.0);
        return;
    }
    times.sort_unstable();
    times.dedup();
    let base = |at: Samples| vol.as_ref().map_or(fader, |l| l.value_at(at, fader));
    let pts: Vec<(Samples, f32)> = times.iter().map(|&at| (at, base(at) + offset.value_at(at, offset_default))).collect();
    let lane = tr.lane_mut(&AutoParam::Volume);
    lane.points.clear();
    for (at, v) in pts {
        lane.set_point(at, v);
    }
}

fn coalesce_trim(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.coalesce_trim", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let Some(trim) = tr.lane(&trim_param()).cloned() else { continue };
        if !trim.points.is_empty() {
            add_to_volume(tr, &trim, 0.0);
            n += 1;
        }
        tr.automation.retain(|l| l.param != trim_param());
    }
    Ok(json!({"tracks": n}))
}

pub(super) fn clear_trim(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.clear_trim_automation", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        let before = tr.automation.len();
        tr.automation.retain(|l| l.param != trim_param());
        if tr.automation.len() < before {
            n += 1;
        }
    }
    Ok(json!({"tracks": n}))
}

fn coalesce_vca(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.coalesce_vca";
    let picked = tracks_param(e, id, p)?;
    let s = e.session_mut();
    let vcas: Vec<TrackId> = if picked.is_empty() {
        s.tracks.iter().filter(|t| t.kind == TrackKind::Vca).map(|t| t.id).collect()
    } else {
        picked.into_iter().filter(|t| s.track(*t).is_some_and(|x| x.kind == TrackKind::Vca)).collect()
    };
    if vcas.is_empty() {
        return Err(bad(id, "select a VCA master"));
    }
    let mut out = Vec::new();
    for v in vcas {
        let Some(vca) = s.track(v).cloned() else { continue };
        let lane = vca.lane(&AutoParam::Volume).cloned().unwrap_or_else(|| AutomationLane::new(AutoParam::Volume));
        let level = vca.mixer.volume_db;
        let members: Vec<TrackId> = s.tracks.iter().filter(|t| t.mixer.vca == Some(v.0)).map(|t| t.id).collect();
        for m in &members {
            if let Some(tr) = s.track_mut(*m) {
                add_to_volume(tr, &lane, level);
            }
        }
        if let Some(tr) = s.track_mut(v) {
            tr.automation.retain(|l| l.param != AutoParam::Volume);
            tr.mixer.volume_db = 0.0;
        }
        out.push(json!({"vca": v, "members": members}));
    }
    Ok(json!({"coalesced": out}))
}

fn designate_target(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.designate_target_playlist", p)?;
    let idx = p.get("index").and_then(Value::as_u64);
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in &tracks {
        let Some(tr) = s.track(*t) else { continue };
        let i = idx.map_or(tr.active_playlist, |x| usize::try_from(x).unwrap_or(usize::MAX));
        if i >= tr.playlists.len() {
            return Err(bad("track.designate_target_playlist", "no such playlist"));
        }
        s.edit.values.insert(format!("playlist.target.{}", t.0), i as f64);
        out.push(json!({"track": t, "target": i}));
    }
    Ok(json!({"tracks": out}))
}

fn show_target(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.show_target_playlist", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        if let Some(i) = target_playlist(s, *t)
            && activate_playlist(s, *t, i)
        {
            n += 1;
        }
    }
    Ok(json!({"shown": n}))
}

fn toggle_recent(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.toggle_recent_playlist", p)?;
    let s = e.session_mut();
    let mut n = 0;
    for t in &tracks {
        let recent = s.edit.values.get(&format!("playlist.recent.{}", t.0)).copied().filter(|v| v.is_finite() && *v >= 0.0);
        if let Some(i) = recent
            && activate_playlist(s, *t, i as usize)
        {
            n += 1;
        }
    }
    Ok(json!({"toggled": n}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bypass_by_category_and_range() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        e.execute("edit.select", &json!({"tracks": [kick.0]})).unwrap();
        e.execute("track.bypass_dynamics", &json!({})).unwrap();
        let tr = e.session().track(kick).unwrap();
        assert!(tr.mixer.inserts[1].as_ref().unwrap().bypass, "compressor bypassed");
        assert!(!tr.mixer.inserts[0].as_ref().unwrap().bypass, "EQ untouched");
        e.execute("track.bypass_eq", &json!({})).unwrap();
        assert!(e.session().track(kick).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
        e.execute("track.bypass_inserts_ae", &json!({})).unwrap();
        let tr = e.session().track(kick).unwrap();
        assert!(tr.mixer.inserts.iter().flatten().all(|i| !i.bypass), "A-E toggles back off when all are bypassed");
    }

    #[test]
    fn mute_sends_subset() {
        let mut e = crate::demo::demo_engine();
        e.execute("edit.select", &json!({"tracks": ["Snare"]})).unwrap();
        e.execute("track.mute_sends_fj", &json!({})).unwrap();
        assert!(!e.session().track_by_name("Snare").unwrap().mixer.sends[0].as_ref().unwrap().mute);
        e.execute("track.mute_sends_ae", &json!({})).unwrap();
        assert!(e.session().track_by_name("Snare").unwrap().mixer.sends[0].as_ref().unwrap().mute);
    }

    #[test]
    fn extract_midi_copies_clips() {
        let mut e = crate::demo::demo_engine();
        e.execute("edit.select", &json!({"tracks": ["Keys"]})).unwrap();
        let r = e.execute("track.extract_midi", &json!({})).unwrap();
        let nt = TrackId(r["tracks"][0]["track"].as_u64().unwrap());
        let tr = e.session().track(nt).unwrap();
        assert_eq!(tr.kind, TrackKind::Midi);
        assert_eq!(tr.clips().len(), e.session().track_by_name("Keys").unwrap().clips().len());
        assert_ne!(tr.clips()[0].id, e.session().track_by_name("Keys").unwrap().clips()[0].id);
    }

    #[test]
    fn convert_aux_adopts_members() {
        let mut e = crate::demo::demo_engine();
        let s = e.session_mut();
        let verb = s.track_by_name("Verb").unwrap().id;
        let bus = match s.track(verb).unwrap().mixer.input.clone() {
            Route::Bus(b) => b,
            _ => panic!("demo verb is fed by a bus"),
        };
        let pad = s.track_by_name("Pad").unwrap().id;
        s.track_mut(pad).unwrap().mixer.output = Route::Bus(bus);
        e.execute("edit.select", &json!({"tracks": [verb.0]})).unwrap();
        e.execute("track.convert_aux_to_folder", &json!({})).unwrap();
        assert_eq!(e.session().track(verb).unwrap().kind, TrackKind::Folder);
        assert_eq!(e.session().track(pad).unwrap().folder, Some(verb));
    }

    #[test]
    fn preset_round_trip() {
        let dir = std::env::temp_dir().join(format!("sc-preset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bass.json").to_string_lossy().into_owned();
        let mut e = crate::demo::demo_engine();
        e.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
        e.execute("track.save_preset", &json!({"path": path})).unwrap();
        e.execute("edit.select", &json!({"tracks": ["Hats"]})).unwrap();
        e.execute("track.load_preset", &json!({"path": path})).unwrap();
        let hats = e.session().track_by_name("Hats").unwrap();
        assert_eq!(hats.mixer.inserts[0].as_ref().map(|i| i.plugin.as_str()), Some("compressor"));
        assert_eq!(hats.mixer.volume_db, -5.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rtp_written_into_notes() {
        let mut e = crate::demo::demo_engine();
        let keys = e.session().track_by_name("Keys").unwrap().id;
        let first = |e: &Engine| match &e.session().track(keys).unwrap().clips()[0].content {
            ClipContent::Midi { sequence } => sequence.notes[0],
            _ => panic!(),
        };
        let before = first(&e);
        let s = e.session_mut();
        s.edit.values.insert(track_key("rtp", keys, "transpose"), 12.0);
        s.edit.values.insert(track_key("rtp", keys, "velocity"), -10.0);
        e.execute("edit.select", &json!({"tracks": [keys.0]})).unwrap();
        e.execute("track.write_midi_rtp", &json!({})).unwrap();
        let after = first(&e);
        assert_eq!(after.pitch, before.pitch + 12);
        assert_eq!(after.velocity, before.velocity - 10);
        assert!(!e.session().edit.values.keys().any(|k| k.starts_with(&format!("rtp.{}.", keys.0))));
    }

    #[test]
    fn rtp_delay_shifts_controller_events() {
        use soundcraft_midi::{CtrlEvent, CtrlKind};
        let mut e = crate::demo::demo_engine();
        let keys = e.session().track_by_name("Keys").unwrap().id;
        let s = e.session_mut();
        if let Some(pl) = s.track_mut(keys).and_then(Track::playlist_mut)
            && let ClipContent::Midi { sequence } = &mut pl.clips[0].content
        {
            sequence.ctrls.clear();
            sequence.ctrls.push(CtrlEvent { tick: 100, channel: 0, kind: CtrlKind::Cc(7), value: 90 });
            sequence.ctrls.push(CtrlEvent { tick: 200, channel: 0, kind: CtrlKind::PitchBend, value: 10 });
            sequence.ctrls.push(CtrlEvent { tick: -30, channel: 0, kind: CtrlKind::Cc(1), value: 5 });
        }
        s.edit.values.insert(track_key("rtp", keys, "delay"), 50.0);
        e.execute("edit.select", &json!({"tracks": [keys.0]})).unwrap();
        e.execute("track.write_midi_rtp", &json!({})).unwrap();
        let ClipContent::Midi { sequence } = &e.session().track(keys).unwrap().clips()[0].content else { panic!() };
        let tick_of = |k: CtrlKind| sequence.ctrls.iter().find(|c| c.kind == k).map(|c| c.tick);
        assert_eq!(tick_of(CtrlKind::Cc(7)), Some(150), "playback delayed the CC by 50 ticks; baked result must match");
        assert_eq!(tick_of(CtrlKind::PitchBend), Some(250));
        // Hidden by a start trim: playback ignores it, so baking leaves it alone.
        assert_eq!(tick_of(CtrlKind::Cc(1)), Some(-30));
    }

    #[test]
    fn trim_and_vca_coalesce() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2})).unwrap();
        e.execute("track.new", &json!({"kind": "vca"})).unwrap();
        let ids: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        let (a, b, v) = (ids[0], ids[1], ids[2]);
        let s = e.session_mut();
        s.track_mut(a).unwrap().mixer.vca = Some(v.0);
        s.track_mut(b).unwrap().mixer.vca = Some(v.0);
        s.track_mut(b).unwrap().mixer.volume_db = -3.0;
        let l = s.track_mut(v).unwrap().lane_mut(&AutoParam::Volume);
        l.set_point(0, 0.0);
        l.set_point(1000, -6.0);
        e.execute("edit.select", &json!({"tracks": [v.0]})).unwrap();
        e.execute("track.coalesce_vca", &json!({})).unwrap();
        let s = e.session();
        assert!((s.track(a).unwrap().lane(&AutoParam::Volume).unwrap().value_at(1000, 0.0) + 6.0).abs() < 1e-4);
        assert!((s.track(b).unwrap().lane(&AutoParam::Volume).unwrap().value_at(500, 0.0) + 6.0).abs() < 1e-4);
        assert!(s.track(v).unwrap().lane(&AutoParam::Volume).is_none());
        // Trim.
        e.execute("edit.select", &json!({"tracks": [a.0]})).unwrap();
        e.execute("track.trim_automation_write", &json!({"start": 2000, "end": 3000, "db": -4.0})).unwrap();
        e.execute("track.coalesce_trim", &json!({})).unwrap();
        let tr = e.session().track(a).unwrap();
        assert!((tr.lane(&AutoParam::Volume).unwrap().value_at(2500, 0.0) + 10.0).abs() < 1e-4);
        assert!(tr.lane(&trim_param()).is_none());
    }

    #[test]
    fn clear_trim_automation_removes_only_trim() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2})).unwrap();
        let ids: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        let (a, b) = (ids[0], ids[1]);
        e.execute("automation.set_point", &json!({"track": a.0, "param": "volume", "at": 0, "value": -3.0})).unwrap();
        e.execute("track.trim_automation_write", &json!({"tracks": [a.0, b.0], "start": 0, "end": 4800, "db": -12.0})).unwrap();
        let has_trim = |e: &Engine, t: TrackId| e.session().track(t).unwrap().lane(&trim_param()).is_some_and(|l| !l.points.is_empty());
        e.execute("edit.select", &json!({"tracks": [a.0]})).unwrap();
        e.execute("track.clear_trim_automation", &json!({})).unwrap();
        assert!(!has_trim(&e, a), "the selected track loses its trim");
        assert!(has_trim(&e, b), "other tracks keep theirs");
        assert_eq!(e.session().track(a).unwrap().lane(&AutoParam::Volume).unwrap().points.len(), 1);
        e.execute("track.clear_trim_automation", &json!({"tracks": [b.0]})).unwrap();
        assert!(!has_trim(&e, b), "an explicit track is cleared too");
        assert!(e.undo());
        assert!(has_trim(&e, b), "undo brings the trim back");
    }

    #[test]
    fn target_and_recent_playlists() {
        let mut e = crate::demo::demo_engine();
        e.execute("edit.select", &json!({"tracks": ["Kick"]})).unwrap();
        e.execute("track.playlist_new", &json!({})).unwrap();
        e.execute("track.designate_target_playlist", &json!({})).unwrap();
        e.execute("track.playlist_select", &json!({"index": 0})).unwrap();
        e.execute("track.show_target_playlist", &json!({})).unwrap();
        assert_eq!(e.session().track_by_name("Kick").unwrap().active_playlist, 1);
        e.execute("track.toggle_recent_playlist", &json!({})).unwrap();
        assert_eq!(e.session().track_by_name("Kick").unwrap().active_playlist, 0);
        e.execute("track.toggle_recent_playlist", &json!({})).unwrap();
        assert_eq!(e.session().track_by_name("Kick").unwrap().active_playlist, 1);
    }
}
