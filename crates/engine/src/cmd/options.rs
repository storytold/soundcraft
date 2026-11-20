//! Options menu toggles and Setup › Session settings.
use super::{CommandSpec, always, bad, bool_or, position_param, str_param};

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
            let v = position_param(e, "options.pre_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.pre_roll = v;
            Ok(json!({"pre_roll": v}))
        }),
        cmd!(noundo "options.post_roll", "Post-Roll Amount", [], None, "{length}", always, |e, p| {
            let v = position_param(e, "options.post_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.post_roll = v;
            Ok(json!({"post_roll": v}))
        }),
        cmd!(
            "setup.session",
            "Session",
            ["Setup"],
            Some("Cmd+2"),
            "{frame_rate?: '29.97 Drop'…, bit_depth?: 16|24|32f, timecode_start?: position, start_timecode?: HH:MM:SS:FF, external_offset_frames?: integer, freewheel_frames?: 4..40, jam_sync?: bool, name?}",
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
    let mut tc = position_param(e, "setup.session", p, "timecode_start")?;
    let selected_rate = fr.unwrap_or(e.session().frame_rate);
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
    let external = p
        .get("external_offset_frames")
        .map(|v| v.as_i64().filter(|v| v.unsigned_abs() <= 5_184_000).ok_or_else(|| bad("setup.session", "Invalid external timecode offset")))
        .transpose()?;
    let s = e.session_mut();
    if let Some(f) = fr {
        s.frame_rate = f;
    }
    if let Some(b) = bd {
        s.bit_depth = b;
    }
    if let Some(frames) = freewheel {
        s.edit.values.insert("midi.mtc.freewheel_frames".into(), frames as f64);
    }
    if let Some(enabled) = jam {
        s.edit.set_flag("midi.mtc.jam_sync", enabled);
    }
    if let Some(frames) = external {
        s.edit.values.insert("midi.mtc.external_offset_frames".into(), frames as f64);
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
        "external_offset_frames": s.edit.value("midi.mtc.external_offset_frames",0.) as i64,
        "freewheel_frames": e.mtc_freewheel_frames(), "jam_sync": e.mtc_jam_sync(),
    }))
}

#[cfg(test)]
mod sync_session_tests {
    use super::*;
    #[test]
    fn new_session_starts_at_one_hour_and_external_offset_round_trips() {
        let mut e = Engine::default();
        e.execute("session.new", &json!({})).unwrap();
        assert_eq!(e.session().format_position(0, soundcraft_time::TimeFormat::Timecode), "01:00:00:00");
        e.execute("setup.session", &json!({"start_timecode":"02:01:03:04","frame_rate":"25","external_offset_frames":-17})).unwrap();
        assert_eq!(e.session().format_position(0, soundcraft_time::TimeFormat::Timecode), "02:01:03:04");
        let loaded = soundcraft_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(loaded.edit.value("midi.mtc.external_offset_frames", 0.), -17.);
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
