//! Options menu toggles and Setup › Session settings.
use super::{CommandSpec, always, bad, bool_or, duration_param, str_param, zero_based_param};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{BitDepthSetting, EditState};
use soundcraft_time::FrameRate;

macro_rules! toggle {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $field:ident) => {
        cmd!(noundo $id, $label, [$($m),*], $sc, "{value?: bool}", always, |e, p| {
            let s = e.session_mut();
            s.edit.$field = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.$field);
            Ok(json!({"value": s.edit.$field}))
        })
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        toggle!("options.loop_record", "Loop Record", ["Options"], Some("Alt+L"), loop_record),
        toggle!("options.quickpunch", "Punch Recording", ["Options"], Some("Cmd+Shift+P"), quickpunch),
        toggle!("options.pre_post_roll", "Pre/Post-Roll", ["Options"], Some("Cmd+K"), pre_post_roll),
        toggle!("options.loop_playback", "Loop Playback", ["Options"], Some("Cmd+Shift+L"), loop_playback),
        toggle!("options.link_timeline_edit", "Link Timeline and Edit Selection", ["Options"], Some("Shift+/"), link_timeline_edit),
        toggle!("options.link_track_edit", "Link Track and Edit Selection", ["Options"], None, link_track_edit),
        toggle!("options.insertion_follows_playback", "Insertion Follows Playback", ["Options"], None, insertion_follows_playback),
        toggle!("options.tab_to_transient", "Tab to Transient", ["Options"], Some("Cmd+Alt+Tab"), tab_to_transient),
        toggle!("options.mirrored_midi", "Mirrored MIDI Editing", ["Options"], None, mirrored_midi),
        toggle!("options.automation_follows_edit", "Automation Follows Edit", ["Options"], None, automation_follows_edit),
        toggle!("options.markers_follow_edit", "Markers Follow Edit", ["Options"], None, markers_follow_edit),
        toggle!("options.layered_editing", "Layered Editing", ["Options"], None, layered_editing),
        toggle!("options.click", "Click", ["Options"], None, click),
        toggle!("options.countoff", "Count Off", [], None, countoff),
        toggle!("options.midi_merge", "MIDI Merge", [], None, midi_merge),
        toggle!("options.pre_fader_metering", "Pre-Fader Metering", ["Options"], None, pre_fader_metering),
        toggle!("options.delay_compensation", "Delay Compensation", ["Options"], None, delay_compensation),
        cmd!(noundo "options.destructive_record", "Destructive Record", ["Options"], None, "{value?: bool}", always, |e, p| record_mode(e, p, "destructive")),
        cmd!(noundo "options.trackpunch", "TrackPunch", ["Options"], Some("Cmd+Shift+T"), "{value?: bool}", always, |e, p| record_mode(e, p, "trackpunch")),
        cmd!(noundo "options.scrolling", "Edit Window Scrolling", ["Options", "Edit Window Scrolling"], None, "{mode: none|after_playback|page|continuous|center}", always, |e, p| {
            let m = str_param(p, "mode").filter(|m| ["none", "after_playback", "page", "continuous", "center"].contains(m)).ok_or_else(|| bad("options.scrolling", "unknown mode"))?.to_string();
            e.session_mut().edit.scrolling = m.clone();
            Ok(json!({"mode": m}))
        }),
        cmd!(noundo "options.solo_mode", "Solo Mode", ["Options", "Solo Mode"], None, "{mode: sip|afl|pfl, latch?: bool, xor?: bool}", always, |e, p| {
            let m = str_param(p, "mode").unwrap_or("sip").to_string();
            let mut v = m.clone();
            if bool_or(p, "xor", false) { v.push_str("+xor") }
            e.session_mut().edit.solo_mode = v.clone();
            Ok(json!({"mode": v}))
        }),
        cmd!(noundo "options.keyboard_focus", "Keyboard Focus", [], None, "{focus?: commands|clips|groups|none} — toggles Commands Keyboard Focus when omitted", always, |e, p| {
            let s = e.session_mut();
            let f = match str_param(p, "focus") {
                Some(f @ ("commands" | "clips" | "groups" | "none")) => f.to_string(),
                Some(other) => return Err(bad("options.keyboard_focus", format!("unknown focus `{other}`"))),
                None => if s.edit.keyboard_focus == "commands" { "none".into() } else { "commands".into() },
            };
            s.edit.keyboard_focus = f.clone();
            Ok(json!({"focus": f}))
        }),
        cmd!(noundo "options.pre_roll", "Pre-Roll Amount", [], None, "{length: samples|{seconds}}", always, |e, p| {
            let v = duration_param(e, "options.pre_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.pre_roll = v;
            Ok(json!({"pre_roll": v}))
        }),
        cmd!(noundo "options.post_roll", "Post-Roll Amount", [], None, "{length}", always, |e, p| {
            let v = duration_param(e, "options.post_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.post_roll = v;
            Ok(json!({"post_roll": v}))
        }),
        cmd!(
            "setup.session",
            "Session",
            ["Setup"],
            Some("Cmd+2"),
            "{frame_rate?: '29.97 Drop'…, bit_depth?: 16|24|32f, timecode_start?: position, start_timecode?: HH:MM:SS:FF, keep?: relative|timecode, external_offset_timecode?: signed HH:MM:SS:FF, external_offset_frames?: integer, freewheel?: none|frames|jam_sync, freewheel_frames?: 4..40, jam_sync?: bool, name?}",
            always,
            session_setup
        ),
        cmd!(noundo "setup.preferences", "Preferences...", ["Setup"], None, "{}", always, |e, _| Ok(json!({"edit": e.session().edit.clone()}))),
        cmd!(noundo "setup.reset_edit_state", "Reset Edit Settings", [], None, "{}", always, |e, _| {
            let s = e.session_mut();
            let keep = (s.edit.selection, s.edit.selected_tracks.clone());
            s.edit = EditState::default();
            s.edit.selection = keep.0;
            s.edit.selected_tracks = keep.1;
            Ok(json!({}))
        }),
    ]
}

fn record_mode(e: &mut Engine, p: &Value, mode: &str) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(s.edit.record_mode != mode);
    s.edit.record_mode = if on { mode.to_string() } else { "normal".into() };
    Ok(json!({"record_mode": s.edit.record_mode}))
}

fn session_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    let fr = match str_param(p, "frame_rate") {
        Some(f) => {
            Some(FrameRate::ALL.into_iter().find(|r| r.label().eq_ignore_ascii_case(f)).ok_or_else(|| bad("setup.session", "unknown frame rate"))?)
        }
        None => None,
    };
    let bd = match p.get("bit_depth").map(|v| v.to_string().trim_matches('"').to_ascii_lowercase()) {
        Some(b) if b == "16" => Some(BitDepthSetting::Int16),
        Some(b) if b == "24" => Some(BitDepthSetting::Int24),
        Some(b) if b.starts_with("32") => Some(BitDepthSetting::Float32),
        Some(_) => return Err(bad("setup.session", "bit_depth must be 16, 24 or 32f")),
        None => None,
    };
    let selected_rate = fr.unwrap_or(e.session().frame_rate);
    let mut tc = zero_based_param(e, "setup.session", p, "timecode_start", selected_rate)?;
    if let Some(value) = p.get("start_timecode") {
        if tc.is_some() {
            return Err(bad("setup.session", "Choose one session start"));
        }
        let label = value.as_str().ok_or_else(|| bad("setup.session", "Expected a timecode label"))?;
        let parsed = soundcraft_time::Timecode::parse(label).map_err(|e| bad("setup.session", e.to_string()))?;
        if parsed.hours >= 24 || soundcraft_time::Timecode::from_frames(parsed.to_frames(selected_rate), selected_rate) != parsed {
            return Err(bad("setup.session", "Invalid timecode label at this frame rate"));
        }
        tc = Some(parsed.to_samples(selected_rate, e.session().sample_rate));
    } else if tc.is_none() && selected_rate != e.session().frame_rate {
        let s = e.session();
        let label = soundcraft_time::Timecode::from_samples(s.timecode_start, s.frame_rate, s.sample_rate);
        if soundcraft_time::Timecode::from_frames(label.to_frames(selected_rate), selected_rate) != label {
            return Err(bad("setup.session", "Invalid timecode label at this frame rate"));
        }
        tc = Some(label.to_samples(selected_rate, s.sample_rate));
    }
    let freewheel = p
        .get("freewheel_frames")
        .map(|v| v.as_u64().filter(|n| (4..=40).contains(n)).ok_or_else(|| bad("setup.session", "Freewheel must be 4 to 40 frames")))
        .transpose()?;
    let jam = p.get("jam_sync").map(|v| v.as_bool().ok_or_else(|| bad("setup.session", "Jam Sync must be a boolean"))).transpose()?;
    let mode = p
        .get("freewheel")
        .map(|v| match v.as_str() {
            Some(m @ ("none" | "frames" | "jam_sync")) => Ok(m),
            _ => Err(bad("setup.session", "Unknown MTC freewheel mode")),
        })
        .transpose()?
        .or_else(|| {
            jam.map(|b| {
                if b {
                    "jam_sync"
                } else if e.mtc_jam_sync() {
                    "frames"
                } else {
                    e.mtc_freewheel_mode()
                }
            })
        });
    let external = p
        .get("external_offset_frames")
        .map(|v| v.as_i64().filter(|v| v.unsigned_abs() <= 5_184_000).ok_or_else(|| bad("setup.session", "Invalid external timecode offset")))
        .transpose()?;
    let sr = e.session().sample_rate;
    let external = if let Some(v) = p.get("external_offset_timecode") {
        Some(
            super::setup_more::parse_offset(v.as_str().ok_or_else(|| bad("setup.session", "Expected a timecode label"))?, selected_rate, sr)
                .map_err(|m| bad("setup.session", m))?,
        )
    } else {
        external.map(|f| selected_rate.samples_at_frame(f, sr))
    };
    if external.is_some_and(|n| n.unsigned_abs() > sr.samples(3600.) as u64) {
        return Err(bad("setup.session", "External timecode offset exceeds one hour of samples"));
    }
    let keep = match p.get("keep").map(Value::as_str) {
        None | Some(Some("relative")) => false,
        Some(Some("timecode")) => true,
        _ => return Err(bad("setup.session", "Choose timecode or relative clip positions")),
    };
    let shift = if keep { e.session().timecode_start.saturating_sub(tc.unwrap_or(e.session().timecode_start)) } else { 0 };
    if shift != 0
        && e.session()
            .tracks
            .iter()
            .flat_map(|t| &t.playlists)
            .flat_map(|p| &p.clips)
            .any(|clip| clip.start.checked_add(shift).is_none_or(|start| start < 0 || start.checked_add(clip.length).is_none()))
    {
        return Err(bad("setup.session", "A clip would fall outside the session timeline"));
    }
    if shift != 0
        && e.session().edit.automation_follows_edit
        && e.session()
            .tracks
            .iter()
            .flat_map(|t| &t.automation)
            .flat_map(|lane| &lane.points)
            .any(|p| p.at.checked_add(shift).is_none_or(|at| at < 0))
    {
        return Err(bad("setup.session", "An automation point would fall outside the session timeline"));
    }
    let s = e.session_mut();
    if shift != 0 {
        if s.edit.automation_follows_edit {
            for point in s.tracks.iter_mut().flat_map(|t| &mut t.automation).flat_map(|lane| &mut lane.points) {
                point.at += shift;
            }
        }
        for clip in s.tracks.iter_mut().flat_map(|t| &mut t.playlists).flat_map(|p| &mut p.clips) {
            clip.start += shift;
        }
    }
    if let Some(f) = fr {
        s.frame_rate = f;
    }
    if let Some(b) = bd {
        s.bit_depth = b;
    }
    if let Some(frames) = freewheel {
        s.edit.values.insert("midi.mtc.freewheel_frames".into(), frames as f64);
    }
    if let Some(mode) = mode {
        super::more_util::set_text(&mut s.edit, "midi.mtc.freewheel_mode", Some(mode));
    }
    if let Some(samples) = external {
        s.edit.values.remove("midi.mtc.external_offset_frames");
        s.edit.values.insert("sync.external_offset".into(), samples as f64);
    }
    if let Some(t) = tc {
        s.timecode_start = t;
    }
    if let Some(n) = p.get("name").and_then(Value::as_str) {
        s.name = n.to_string();
    }
    Ok(json!({
        "frame_rate": s.frame_rate.label(), "bit_depth": s.bit_depth, "sample_rate": s.sample_rate.hz(),
        "start_timecode": s.format_position(0,soundcraft_time::TimeFormat::Timecode),
        "external_offset_frames": s.frame_rate.frames_at(s.edit.value("sync.external_offset",0.) as i64,s.sample_rate),
        "external_offset_timecode":super::setup_more::signed_offset(s.edit.value("sync.external_offset",0.) as i64,s.frame_rate,s.sample_rate),
        "freewheel": e.mtc_freewheel_mode(),
        "freewheel_frames": e.mtc_freewheel_frames(), "jam_sync": e.mtc_jam_sync(),
    }))
}

#[cfg(test)]
mod sync_session_tests {
    use super::*;
    #[test]
    fn timecode_labels_for_session_start_and_durations_have_no_session_offset() {
        let mut e = Engine::default();
        for cmd in ["options.pre_roll", "options.post_roll"] {
            let result = e.execute(cmd, &json!({"length":"timecode:00:00:05:00"})).unwrap();
            assert_eq!(result[cmd.split('.').nth(1).unwrap()], e.session().sample_rate.samples(5.));
        }
        for label in ["02:00:00:00", "00:30:00:00"] {
            let start = e.execute("setup.session", &json!({"timecode_start":format!("timecode:{label}")})).unwrap();
            assert_eq!(start["start_timecode"], label);
        }
    }

    #[test]
    fn timecode_retention_shifts_following_automation_and_refuses_underflow_atomically() {
        let mut e = crate::demo::demo_engine();
        e.session_mut().edit.automation_follows_edit = true;
        e.session_mut().timecode_start = e.session().sample_rate.samples(3600.);
        let points = |e: &Engine| e.session().tracks.iter().flat_map(|t| &t.automation).flat_map(|l| &l.points).map(|p| p.at).collect::<Vec<_>>();
        let before = points(&e);
        assert!(!before.is_empty());
        let delta = e.session().sample_rate.samples(1800.);
        e.execute("setup.session", &json!({"start_timecode":"00:30:00:00","keep":"timecode"})).unwrap();
        assert_eq!(points(&e), before.iter().map(|at| at + delta).collect::<Vec<_>>());
        for t in &mut e.session_mut().tracks {
            for pl in &mut t.playlists {
                pl.clips.clear();
            }
        }
        let original = e.session().clone();
        assert!(e.execute("setup.session", &json!({"start_timecode":"23:59:59:00","keep":"timecode"})).is_err());
        assert_eq!(e.session().to_json().unwrap(), original.to_json().unwrap());
        e.session_mut().edit.automation_follows_edit = false;
        e.execute("setup.session", &json!({"start_timecode":"23:59:59:00","keep":"timecode"})).unwrap();
        assert_eq!(points(&e), before.iter().map(|at| at + delta).collect::<Vec<_>>());
    }

    #[test]
    fn timecode_retention_respects_automation_switch_and_keeps_both_rulers_on_undo() {
        for follows in [false, true] {
            let mut e = crate::demo::demo_engine();
            e.session_mut().edit.automation_follows_edit = follows;
            e.session_mut().timecode_start = e.session().sample_rate.samples(3600.);
            let before = e.session().clone();
            assert!(!before.markers.is_empty());
            let delta = before.sample_rate.samples(1800.);
            e.execute("setup.session", &json!({"start_timecode":"00:30:00:00","keep":"timecode"})).unwrap();
            let after = e.session();
            assert_eq!(json!(after.markers), json!(before.markers));
            assert_eq!(json!(after.tempo), json!(before.tempo));
            for (old, new) in before.tracks.iter().zip(&after.tracks) {
                for (a, b) in old.automation.iter().flat_map(|l| &l.points).zip(new.automation.iter().flat_map(|l| &l.points)) {
                    assert_eq!(b.at, a.at + if follows { delta } else { 0 });
                }
                for (a, b) in old.playlists.iter().flat_map(|p| &p.clips).zip(new.playlists.iter().flat_map(|p| &p.clips)) {
                    assert_eq!(b.start, a.start + delta);
                }
            }
            assert!(e.undo());
            assert_eq!(e.session().to_json().unwrap(), before.to_json().unwrap());
        }
    }

    #[test]
    fn external_timecode_offset_is_one_value_for_menu_and_session_setup() {
        let mut e = Engine::default();
        let set = e.execute("setup.external_timecode_offset", &json!({"timecode":"+01:00:00:00"})).unwrap();
        assert_eq!(set["samples"], 172_800_000.);
        let state = e.execute("setup.session", &json!({})).unwrap();
        assert_eq!(state["external_offset_frames"], 108000);
        e.execute("setup.session", &json!({"external_offset_frames":-17})).unwrap();
        let menu = e.execute("setup.external_timecode_offset", &json!({})).unwrap();
        assert_eq!(menu["samples"], -27200.);
        assert_eq!(menu["timecode"], "-00:00:00:17");
    }

    #[test]
    fn signed_one_hour_uses_the_existing_sample_limit_at_each_mtc_rate() {
        let mut e = Engine::default();
        for (rate, allowed) in [
            (FrameRate::Fps24, true),
            (FrameRate::Fps25, true),
            (FrameRate::Fps30, true),
            (FrameRate::Fps29_97Drop, true),
            (FrameRate::Fps23_976, false),
            (FrameRate::Fps29_97, false),
        ] {
            e.session_mut().frame_rate = rate;
            for sign in ["+", "-"] {
                let label = format!("{sign}01:00:00{}00", if rate.is_drop() { ';' } else { ':' });
                let result = e.execute("setup.external_timecode_offset", &json!({"timecode":label}));
                assert_eq!(result.is_ok(), allowed, "{rate:?}, {label}");
                if let Ok(v) = result {
                    assert_eq!(v["timecode"], label);
                }
            }
        }
    }

    #[test]
    fn freewheel_mode_accepts_legacy_keys_and_remembers_frame_count() {
        let mut e = Engine::default();
        let state = e.execute("setup.session", &json!({"freewheel":"none","freewheel_frames":19})).unwrap();
        assert_eq!(state["freewheel"], "none");
        assert_eq!(e.execute("setup.session", &json!({"jam_sync":false})).unwrap()["freewheel"], "none");
        assert_eq!(e.execute("setup.session", &json!({"jam_sync":true})).unwrap()["freewheel"], "jam_sync");
        let changed = e.execute("setup.session", &json!({"jam_sync":false})).unwrap();
        assert_eq!(changed["freewheel"], "frames");
        assert_eq!(changed["freewheel_frames"], 19);
        assert!(e.execute("setup.session", &json!({"freewheel":"other"})).is_err());
    }

    #[test]
    fn old_freewheel_sessions_restore_default_frames_saved_frames_or_jam() {
        for (jam, frames, expected_mode, expected_frames) in
            [(false, None, "frames", 8), (false, Some(17), "frames", 17), (true, Some(21), "jam_sync", 21)]
        {
            let mut old = Engine::default().session().clone();
            if jam {
                old.edit.flags.insert("midi.mtc.jam_sync".into());
            }
            if let Some(n) = frames {
                old.edit.values.insert("midi.mtc.freewheel_frames".into(), n as f64);
            }
            let engine = Engine::new(old);
            assert_eq!(engine.mtc_freewheel_mode(), expected_mode);
            assert_eq!(engine.mtc_freewheel_frames(), expected_frames);
            assert!(!engine.session().edit.flags.contains("midi.mtc.jam_sync"));
        }
    }

    #[test]
    fn mtc_bad_project_fields_load_with_individual_resets_with_a_message() {
        let mut source = Engine::default();
        source.execute("track.new", &json!({"count": 2})).unwrap();
        source.execute("setup.session", &json!({"freewheel":"jam_sync","freewheel_frames":17,"external_offset_frames":-3})).unwrap();
        let base = source.session();
        let mode_key = "midi.mtc.freewheel_mode";
        let frames_key = "midi.mtc.freewheel_frames";
        let offset_key = "sync.external_offset";
        for (field, expected_value, expected_mode, expected_frames) in [
            (frames_key, base.edit.value(offset_key, 0.), "jam_sync", 8),
            (mode_key, base.edit.value(offset_key, 0.), "frames", 17),
            (offset_key, 0., "jam_sync", 17),
        ] {
            let mut session = base.clone();
            if field == mode_key {
                session.edit.flags.remove("midi.mtc.freewheel_mode=jam_sync");
                session.edit.flags.insert("midi.mtc.freewheel_mode=unknown".into());
            } else {
                let bad = if field == offset_key { session.sample_rate.samples(3600.) as f64 + 1. } else { 512. };
                session.edit.values.insert(field.into(), bad);
            }
            let loaded = soundcraft_model::Session::from_json(&session.to_json().unwrap()).unwrap();
            for engine in [Engine::new(loaded.clone()), {
                let mut engine = Engine::default();
                engine.replace_session(loaded.clone());
                engine
            }] {
                assert_eq!(engine.mtc_freewheel_frames(), expected_frames, "{field}");
                assert_eq!(engine.mtc_freewheel_mode(), expected_mode, "{field}");
                assert_eq!(engine.session().edit.value(offset_key, 0.), expected_value, "{field}");
                assert_eq!(serde_json::to_value(&engine.session().tracks).unwrap(), serde_json::to_value(&base.tracks).unwrap());
                assert_eq!(engine.messages.iter().filter(|m| m.contains(field)).count(), 1, "{field}: {:?}", engine.messages);
                assert!(Engine::new(engine.session().clone()).messages.is_empty(), "resolved setting should survive reopen");
            }
        }
    }

    #[test]
    fn mtc_bad_legacy_offset_does_not_replace_valid_sample_offset_or_change_freewheel() {
        let mut old = Engine::default().session().clone();
        old.edit.values.insert("sync.external_offset".into(), -24.);
        old.edit.values.insert("midi.mtc.external_offset_frames".into(), f64::MAX);
        old.edit.values.insert("midi.mtc.freewheel_frames".into(), 20.);
        old.edit.flags.insert("midi.mtc.jam_sync".into());
        let loaded = Engine::new(soundcraft_model::Session::from_json(&old.to_json().unwrap()).unwrap());
        assert_eq!(loaded.mtc_freewheel_mode(), "jam_sync");
        assert_eq!(loaded.mtc_freewheel_frames(), 20);
        assert_eq!(loaded.session().edit.value("sync.external_offset", 0.), -24.);
        assert!(!loaded.session().edit.values.contains_key("midi.mtc.external_offset_frames"));
        assert!(loaded.messages.iter().any(|m| m.contains("midi.mtc.external_offset_frames")));
    }

    #[test]
    fn existing_offset_and_freewheel_preferences_migrate_once_and_conflicts_survive() {
        let base = Engine::default().session().clone();
        for (samples, frames, expected, conflict) in [(160000, None, 160000, false), (0, Some(100), 160000, false), (42, Some(100), 42, true)] {
            let mut session = base.clone();
            session.edit.values.insert("sync.external_offset".into(), samples as f64);
            session.edit.values.insert("midi.mtc.freewheel_frames".into(), 23.);
            session.edit.flags.insert("midi.mtc.jam_sync".into());
            if let Some(f) = frames {
                session.edit.values.insert("midi.mtc.external_offset_frames".into(), f as f64);
            }
            let e = Engine::new(session);
            assert_eq!(e.mtc_freewheel_mode(), "jam_sync");
            assert_eq!(e.mtc_freewheel_frames(), 23);
            assert_eq!(e.session().edit.value("sync.external_offset", 0.), expected as f64);
            assert_eq!(e.session().edit.values.contains_key("midi.mtc.external_offset_frames"), conflict);
            assert_eq!(e.messages.is_empty(), frames.is_none());
            let reopened = Engine::new(serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap());
            assert_eq!(reopened.messages.is_empty(), !conflict);
        }
    }

    #[test]
    fn session_start_retains_either_absolute_timecode_or_relative_clip_position() {
        let mut e = crate::demo::demo_engine();
        e.session_mut().timecode_start = e.session().sample_rate.samples(3600.);
        let before = e.session().tracks.iter().flat_map(|t| &t.playlists).flat_map(|p| &p.clips).map(|c| (c.id, c.start)).collect::<Vec<_>>();
        assert!(!before.is_empty());
        let delta = e.session().sample_rate.samples(1800.);
        e.execute("setup.session", &json!({"start_timecode":"00:30:00:00","keep":"timecode"})).unwrap();
        for (id, old) in &before {
            assert_eq!(e.session().find_clip(*id).unwrap().1.start, old + delta);
        }
        assert!(e.execute("setup.session", &json!({"start_timecode":"23:59:59:00","keep":"timecode"})).is_err());
        e.execute("setup.session", &json!({"start_timecode":"01:00:00:00"})).unwrap();
        for (id, old) in before {
            assert_eq!(e.session().find_clip(id).unwrap().1.start, old + delta);
        }
    }

    #[test]
    fn explicitly_resetting_a_conflicting_external_offset_survives_reopen() {
        for (cmd, args) in [
            ("setup.session", json!({"external_offset_frames":0})),
            ("setup.external_timecode_offset", json!({"samples":0})),
            ("setup.external_timecode_offset", json!({"frames":0})),
            ("setup.external_timecode_offset", json!({"timecode":"+00:00:00:00"})),
        ] {
            let mut s = Engine::default().session().clone();
            s.edit.values.insert("sync.external_offset".into(), 42.);
            s.edit.values.insert("midi.mtc.external_offset_frames".into(), 100.);
            let mut e = Engine::new(s);
            e.execute(cmd, &args).unwrap();
            let reopened = Engine::new(e.session().clone());
            assert_eq!(reopened.session().edit.value("sync.external_offset", 0.), 0., "{cmd} {args}");
        }
    }

    #[test]
    fn new_session_starts_at_one_hour_and_external_offset_round_trips() {
        let mut e = Engine::default();
        e.execute("session.new", &json!({})).unwrap();
        assert_eq!(e.session().format_position(0, soundcraft_time::TimeFormat::Timecode), "01:00:00:00");
        e.execute("setup.session", &json!({"start_timecode":"02:01:03:04","frame_rate":"25","external_offset_frames":-17})).unwrap();
        assert_eq!(e.session().format_position(0, soundcraft_time::TimeFormat::Timecode), "02:01:03:04");
        let loaded = soundcraft_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(loaded.edit.value("sync.external_offset", 0.), -17. * 1920.);
        let old = soundcraft_model::Session::default();
        assert_eq!(soundcraft_model::Session::from_json(&old.to_json().unwrap()).unwrap().timecode_start, 0);
    }

    #[test]
    fn session_sync_preferences_round_trip_and_validate_before_changes() {
        let mut e = Engine::default();
        for n in [4, 40, 12] {
            let state = e.execute("setup.session", &json!({"freewheel_frames":n,"jam_sync":true})).unwrap();
            assert_eq!(state["freewheel_frames"], n);
            assert_eq!(state["jam_sync"], true);
        }
        let copy = soundcraft_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
        let mut reopened = Engine::new(copy);
        for n in [3, 41] {
            assert!(reopened.execute("setup.session", &json!({"freewheel_frames":n,"jam_sync":false})).is_err());
        }
        let state = reopened.execute("setup.session", &json!({})).unwrap();
        assert_eq!(state["freewheel_frames"], 12);
        assert_eq!(state["jam_sync"], true);
        let legacy = Engine::new(soundcraft_model::Session::default()).execute("setup.session", &json!({})).unwrap();
        assert_eq!(legacy["freewheel_frames"], 8);
        assert_eq!(legacy["jam_sync"], false);
    }

    #[test]
    fn new_session_keeps_the_one_hour_label_when_only_the_rate_changes() {
        for label in ["23.976", "24", "25", "29.97", "29.97 Drop", "30"] {
            let mut e = Engine::default();
            let state = e.execute("setup.session", &json!({"frame_rate":label})).unwrap();
            assert_eq!(state["start_timecode"], if label == "29.97 Drop" { "01:00:00;00" } else { "01:00:00:00" }, "{label}");
        }
    }

    #[test]
    fn closing_a_session_creates_a_fresh_one_hour_start() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        e.execute("session.close", &json!({})).unwrap();
        assert_eq!(e.session().format_position(0, soundcraft_time::TimeFormat::Timecode), "01:00:00:00");
    }

    #[test]
    fn absolute_position_before_start_leaves_selection_and_transport_unchanged() {
        let mut e = Engine::default();
        e.execute("edit.select", &json!({"start":3200,"end":9600})).unwrap();
        let selection = e.session().edit.selection;
        for (cmd, args) in [("edit.select", json!({"start":"00:00:20:00"})), ("transport.locate", json!({"at":"timecode:00:00:20:00"}))] {
            let err = e.execute(cmd, &args).unwrap_err().to_string();
            assert!(err.contains("before the session start") && err.contains("01:00:00:00"), "{err}");
            assert_eq!(e.session().edit.selection, selection);
        }
        let legacy = Engine::new(soundcraft_model::Session::default());
        assert_eq!(crate::cmd::position_param(&legacy, "test", &json!({"at":"00:00:20:00"}), "at").unwrap(), Some(960_000));
        assert_eq!(crate::cmd::position_param(&e, "test", &json!({"at":"0:20.000"}), "at").unwrap(), Some(960_000));
    }
}
