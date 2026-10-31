use super::{Services, SoundApp, automation_step_due};
use serde_json::json;
use soundcraft_model::TrackId;

#[test]
fn new_session_stops_playback_and_the_old_clock() {
    for (command, params) in [
        ("session.new", json!({"name": "New project", "sample_rate": 96_000})),
        ("session.new", json!({"template": "demo"})),
        ("session.close", json!({})),
    ] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({"from": 48_000})).unwrap();
        app.handle_transport(0.25);
        assert!(app.is_playing());
        assert!(app.position() > 48_000);
        app.run(command, params).unwrap();
        let insertion = app.engine.session().edit.selection.start;
        app.handle_transport(0.25);
        assert!(!app.is_playing(), "{command}");
        assert_eq!(app.position(), insertion, "{command}");
        assert_eq!(app.engine.transport.position, insertion, "{command}");
        app.handle_transport(0.25);
        assert_eq!(app.engine.transport.position, insertion, "the old clock must stay stopped");
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(app.is_playing());
        assert_eq!(app.position(), insertion + i64::from(app.engine.session().sample_rate.hz()) / 4);
    }
}

#[test]
fn the_status_names_a_plugin_the_engine_took_out() {
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    s.track_mut(t).unwrap().mixer.inserts[3] = Some(Insert::new("gain"));
    let msg = super::failed_plugin_status(&s, t, 3).unwrap();
    assert!(msg.starts_with("Gain on Vox put out invalid audio"), "{msg}");
    assert_eq!(super::failed_plugin_status(&s, t, 0), None);
    assert_eq!(super::failed_plugin_status(&s, t, soundcraft_mix::INSTRUMENT_SLOT), None);
}

#[test]
fn replacing_the_session_drops_the_midi_note_selection() {
    for command in ["session.new", "session.close"] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.midi.selected = vec![0, 1];
        app.midi.clip = Some(soundcraft_model::ClipId(1));
        app.run(command, json!({})).unwrap();
        assert!(app.midi.selected.is_empty() && app.midi.clip.is_none(), "{command}");
    }
}

#[test]
fn new_session_cancels_play_queued_before_the_next_frame() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({})).unwrap();
    app.run("session.new", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    assert_eq!(app.engine.transport.position, 0);
}

#[test]
fn play_from_starts_at_from_regardless_of_the_selection() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 7_632_000, "end": 7_680_000})).unwrap();
    app.run("transport.play", json!({"from": 4_560_000})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    assert_eq!(app.engine.transport.position, 4_560_000 + 12_000);

    app.run("transport.play", json!({"from": 96_000})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    assert_eq!(app.engine.transport.position, 96_000 + 12_000);
}

#[test]
fn play_from_while_looping_drops_the_selection_loop() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("options.loop_playback", json!({"value": true})).unwrap();
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 96_000, "end": 144_000})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    app.run("transport.play", json!({"from": 480_000})).unwrap();
    app.handle_transport(0.25);
    app.handle_transport(0.25);
    assert!(app.is_playing());
    assert_eq!(app.engine.transport.position, 480_000 + 24_000);
}

#[test]
fn record_with_nothing_armed_says_what_to_do() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing() && !app.engine.transport.recording);
    assert!(app.record_hint().is_some_and(|why| why.contains("Track › New")), "{:?}", app.record_hint());

    app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(app.record_hint().is_some_and(|why| why.contains("R button")), "{:?}", app.record_hint());
    assert_eq!(Some(app.ui.status.as_str()), app.record_hint());
    app.last_frame = Some(5.0);
    assert_eq!(app.record_hint(), None, "the hint shows for a few seconds");

    app.last_frame = None;
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    app.run("mix.record_arm", json!({})).unwrap();
    assert_eq!(app.record_hint(), None, "arming a track answers the hint");
}

#[test]
fn a_hidden_window_still_autosaves() {
    let dir = std::env::temp_dir().join(format!("soundcraft-hidden-autosave-{}", std::process::id()));
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.autosave_dir = Some(dir.clone());
    app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
    app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
    let _ = egui::Context::default().run_logic(&raw, |ctx| app.logic(ctx));
    assert!(app.latest_autosave().is_some());
    let _ = std::fs::remove_dir_all(dir);
}

/// A clean demo session with its autosave folder, and an editor gesture still open on Keys
/// when `gesture` is set (its state hasn't reached the session yet).
fn app_with_editor_gesture(name: &str, gesture: bool) -> (SoundApp, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}-{gesture}", std::process::id()));
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.autosave_dir = Some(dir.clone());
    if gesture {
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        app.editor_states.push((keys, soundcraft_mix::INSTRUMENT_SLOT));
    }
    assert!(!app.engine.is_dirty());
    (app, dir)
}

#[test]
fn an_open_plugin_editor_gesture_is_autosaved() {
    for gesture in [false, true] {
        let (mut app, dir) = app_with_editor_gesture("gesture-autosave", gesture);
        app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
        app.autosave();
        assert_eq!(app.latest_autosave().is_some(), gesture);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn closing_during_a_plugin_editor_gesture_keeps_the_autosave() {
    for gesture in [false, true] {
        let (mut app, dir) = app_with_editor_gesture("gesture-close", gesture);
        app.close_autosave();
        assert_eq!(app.latest_autosave().is_some(), gesture);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn session_reset_does_not_apply_old_stop_edits_to_the_new_session() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({"from": 48_000})).unwrap();
    app.handle_transport(0.25);
    let mut session = soundcraft_model::Session::default();
    session.edit.selection = soundcraft_time::Range::new(12_000, 24_000);
    session.edit.insertion_follows_playback = true;
    session.edit.scrolling = "after_playback".into();
    let selection = session.edit.selection;
    app.engine.replace_session(session);
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    assert_eq!(app.engine.session().edit.selection, selection);
    assert_eq!(app.position(), selection.start);
    assert_eq!(app.engine.session().edit.zoom.scroll, 0);
    assert!(!app.engine.is_dirty());
}

#[test]
fn clear_all_clip_indicators_clears_every_meter() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.meters.entry(kick).or_default().clip = true;
    app.main_meter.clip = true;
    app.run("track.clear_clip_indicators", json!({})).unwrap();
    assert!(!app.meters[&kick].clip && !app.main_meter.clip);
}

#[test]
fn automation_first_pass_after_start_is_due_without_overflow() {
    // `last_write_at` starts (and resets) at `i64::MIN`: the previous
    // `(pos - last).abs()` panicked here in debug builds.
    assert!(automation_step_due(0, i64::MIN, 960));
    assert!(automation_step_due(48_000, i64::MIN, 960));
}

#[test]
fn automation_write_throttles_to_step() {
    assert!(!automation_step_due(1_000, 900, 960));
    assert!(automation_step_due(2_000, 900, 960));
}

#[test]
fn automation_write_fires_after_loop_wrap() {
    assert!(automation_step_due(0, 1_000_000, 960));
}

/// Write-mode passes replace automation only along the stretches they play; skipped jumps keep the old ride, and
/// stop writes the last point.
#[test]
fn a_write_pass_replaces_the_automation_it_plays_over() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let fader = app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
    let q = i64::from(app.engine.session().sample_rate.hz()) / 4;
    // The old ride: a point every 10 ms for 3 s.
    for at in (0..12 * q).step_by(usize::try_from(q / 25).unwrap()) {
        app.run("automation.set_point", json!({"track": "Kick", "at": at, "value": -30.0})).unwrap();
    }
    app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
    app.run("transport.locate", json!({"at": 0})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    // Passes a quarter second apart, sparse like a covered window's.
    let pass = |app: &mut SoundApp| {
        app.handle_transport(0.25);
        app.write_automation();
    };
    pass(&mut app);
    pass(&mut app);
    // Two jumps: the stretches they skip keep the old ride.
    app.run("transport.locate", json!({"at": 4 * q})).unwrap();
    pass(&mut app);
    app.run("edit.select", json!({"tracks": ["Kick"], "start": 7 * q, "end": 11 * q})).unwrap();
    app.run("transport.play_selection", json!({})).unwrap();
    pass(&mut app);
    app.handle_transport(0.25);
    app.run("transport.stop", json!({})).unwrap();
    app.handle_transport(0.0);
    app.write_automation();
    let lane = app.engine.session().track_by_name("Kick").unwrap().lane(&soundcraft_model::AutoParam::Volume).unwrap();
    let values = |from, to| lane.points.iter().filter(|p| (from..=to).contains(&p.at)).map(|p| p.value).collect::<Vec<_>>();
    assert_eq!(values(q, 2 * q), [fader; 2]);
    assert_eq!(lane.value_at(3 * q, 0.0), -30.0);
    assert_eq!(lane.value_at(6 * q, 0.0), -30.0);
    // Stop writes the last point, at 9q.
    assert_eq!(values(8 * q, 9 * q), [fader; 2]);
    assert_eq!(lane.value_at(10 * q, 0.0), -30.0);
}

/// Automation pass writes and the fader ride driving them collapse into one undo step.
#[test]
fn a_pass_and_the_fader_ride_driving_it_are_one_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    let steps = app.engine.undo_history().len();
    for frame in 0..60 {
        app.handle_transport(1.0 / 60.0);
        app.write_automation();
        // What the Mix window's fader sends while it is dragged.
        let db = -10.0 + frame as f32 / 10.0;
        app.engine.execute_merged("mix.volume", &json!({"track": kick.0, "db": db}), &format!("fader:{}", kick.0)).unwrap();
    }
    app.run("transport.stop", json!({})).unwrap();
    app.handle_transport(0.0);
    app.write_automation();
    assert_eq!(app.engine.undo_history().len(), steps + 1);
}

/// #1146: a send's level writes its own automation lane, by the track's mode like its fader: Touch
/// while the send is dragged, Latch from the first drag until stop, Write throughout. The track's
/// volume lane is left alone.
#[test]
fn a_dragged_send_level_writes_send_automation() {
    use soundcraft_model::AutoParam;
    let points = |app: &SoundApp, param: &AutoParam| app.engine.session().track_by_name("Lead").unwrap().lane(param).map_or(0, |l| l.points.len());
    let level = |app: &SoundApp| app.engine.session().track_by_name("Lead").unwrap().lane(&AutoParam::SendLevel(0)).map(|l| l.points.clone());
    let pass = |app: &mut SoundApp| {
        app.handle_transport(0.1);
        app.write_automation();
    };
    for mode in ["touch", "latch", "write"] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let lead = app.engine.session().track_by_name("Lead").unwrap().id;
        assert!(app.engine.session().track(lead).unwrap().mixer.sends[0].is_some(), "the demo's Lead sends to the reverb");
        let volume = points(&app, &AutoParam::Volume);
        app.run("mix.automation_mode", json!({"tracks": ["Lead"], "mode": mode})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        for _ in 0..3 {
            pass(&mut app);
        }
        let before_drag = points(&app, &AutoParam::SendLevel(0));
        if mode == "write" {
            assert!(before_drag > 0, "Write writes the send without a drag");
        } else {
            assert_eq!(before_drag, 0, "{mode}: nothing written before the send is touched");
        }
        // What the send's slot and its floating window do while the level is dragged.
        app.pointer_down = true;
        app.gesture = Some(super::Gesture::SendFader { track: lead, slot: 0 });
        for frame in 0..4 {
            pass(&mut app);
            let db = -12.0 + frame as f32;
            app.engine.execute_merged("mix.send_level", &json!({"track": lead.0, "slot": 0, "db": db}), &format!("send:{}:0", lead.0)).unwrap();
        }
        let dragged = points(&app, &AutoParam::SendLevel(0));
        assert!(dragged >= before_drag + 3, "{mode}: the drag is written ({before_drag} -> {dragged})");
        assert!(level(&app).unwrap().iter().any(|p| (p.value - (-10.0)).abs() < 1e-6), "{mode}: the points follow the send level");
        // Released.
        app.pointer_down = false;
        app.gesture = None;
        for _ in 0..3 {
            pass(&mut app);
        }
        let released = points(&app, &AutoParam::SendLevel(0));
        if mode == "touch" {
            assert_eq!(released, dragged, "Touch stops writing when the send is let go");
        } else {
            assert!(released > dragged, "{mode}: keeps writing after the release");
        }
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.write_automation();
        if mode != "write" {
            assert_eq!(points(&app, &AutoParam::Volume), volume, "{mode}: the track's volume lane is left alone");
        }
        assert!(app.touched_sends.is_empty(), "{mode}: stop ends the pass");
    }
}

#[test]
fn plugin_states_read_during_a_drag_keep_it_one_undo_step() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
    let steps = app.engine.undo_history().len();
    for frame in 0..60u8 {
        // What a plugin editor reports each frame of a drag, and the state the plugin marked dirty.
        let gain = -10.0 + f32::from(frame) / 10.0;
        app.engine.execute_merged("mix.insert_param", &json!({"track": kick.0, "slot": 0, "param": "gain", "value": gain}), "param:drag").unwrap();
        app.store_plugin_states(vec![(kick, 0, vec![frame, 1])]);
    }
    assert_eq!(app.engine.undo_history().len(), steps + 1);
    assert_eq!(app.engine.session().track(kick).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes(), Some(vec![59, 1]));
}

/// One UI frame of plugin editors: the instruments edited in theirs, the plugins that said
/// their state changed, the editors closed, and the state every plugin has now. Returns the
/// plugins whose state was read.
fn editor_frame(app: &mut SoundApp, edited: &[TrackId], changed: &[(TrackId, usize)], closed: &[TrackId], now: u8) -> Vec<(TrackId, usize)> {
    let slot = soundcraft_mix::INSTRUMENT_SLOT;
    let edits: Vec<_> = edited.iter().map(|&t| (t, slot, "cutoff".to_string(), f32::from(now))).collect();
    let due = super::editor_states_due(&mut app.editor_states, &edits, changed.to_vec(), |(t, _)| closed.contains(&t));
    let states = due.iter().map(|&(t, slot)| (t, slot, vec![now, 1])).collect();
    app.store_editor_changes(edits, states);
    due
}

fn instrument_state(app: &SoundApp, t: TrackId) -> Option<Vec<u8>> {
    app.engine.session().track(t).unwrap().instrument.as_ref().unwrap().state_bytes()
}

#[test]
fn an_instrument_editor_drag_reads_and_stores_its_state_once() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let mut reads = Vec::new();
    let mut revisions = Vec::new();
    for now in 0..60 {
        reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
        revisions.push(app.engine.revision);
    }
    reads.extend(editor_frame(&mut app, &[], &[], &[], 59));
    revisions.push(app.engine.revision);
    assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    // The session goes to the audio thread on each new revision: once, not every frame.
    revisions.dedup();
    assert_eq!(revisions.len(), 2, "{revisions:?}");
    assert_eq!(instrument_state(&app, keys), Some(vec![59, 1]));
}

#[test]
fn two_instrument_editor_drags_store_its_state_twice() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let mut reads = Vec::new();
    for now in 0..10 {
        reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
    }
    reads.extend(editor_frame(&mut app, &[], &[], &[], 9));
    assert_eq!(instrument_state(&app, keys), Some(vec![9, 1]));
    for now in 20..30 {
        reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
    }
    reads.extend(editor_frame(&mut app, &[], &[], &[], 29));
    assert_eq!(reads.len(), 2);
    assert_eq!(instrument_state(&app, keys), Some(vec![29, 1]));
}

#[test]
fn closing_an_instrument_editor_mid_gesture_still_stores_its_state() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let lead = app.engine.session().track_by_name("Lead").unwrap().id;
    let lead_before = instrument_state(&app, lead);
    for now in 0..10 {
        editor_frame(&mut app, &[keys, lead], &[], &[], now);
    }
    // Keys' editor closes on a frame it still reports a change in.
    let reads = editor_frame(&mut app, &[keys, lead], &[], &[keys], 10);
    assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    assert_eq!(instrument_state(&app, keys), Some(vec![10, 1]));
    assert_eq!(instrument_state(&app, lead), lead_before);
}

#[test]
fn a_plugin_that_keeps_changing_does_not_hold_back_another_ones_state() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
    // A plugin on Kick marks its state changed on every frame while Keys is dragged, then
    // keeps doing it after the drag.
    for now in 0..10 {
        editor_frame(&mut app, &[keys], &[(kick, 0)], &[], now);
    }
    let reads = editor_frame(&mut app, &[], &[(kick, 0)], &[], 10);
    assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    assert_eq!(instrument_state(&app, keys), Some(vec![10, 1]));
    assert_eq!(app.editor_states, [(kick, 0)]);
}

#[test]
fn a_preset_changed_in_an_effect_editor_is_stored_once() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
    // No parameter edits: the plugin only says its state changed, frame after frame.
    let mut reads = Vec::new();
    for now in 0..5 {
        reads.extend(editor_frame(&mut app, &[], &[(kick, 0)], &[], now));
    }
    reads.extend(editor_frame(&mut app, &[], &[], &[], 4));
    assert_eq!(reads, [(kick, 0)]);
    let state = app.engine.session().track(kick).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes();
    assert_eq!(state, Some(vec![4, 1]));
}

#[test]
fn preferences_saved_before_the_window_was_remembered_keep_full_scale() {
    let ui: super::UiState = serde_json::from_value(json!({"show_transport": true})).unwrap();
    assert_eq!((ui.window_size, ui.window_maximized, ui.ui_scale), ([1600.0, 1000.0], false, 1.0));
}

#[test]
fn a_window_bigger_than_its_screen_opens_maximized() {
    let maximizes = |w: f32, frames: usize| {
        let ctx = egui::Context::default();
        let info = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1000.0))),
            monitor_size: Some(egui::vec2(1728.0, 1117.0)),
            ..Default::default()
        };
        let mut maximized = false;
        for _ in 0..frames {
            let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, info.clone())].into_iter().collect(), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| super::fit_window_to_screen(ui.ctx()));
            out.textures_delta.clear();
            maximized = out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Maximized(true));
        }
        maximized
    };
    assert!(maximizes(2400.0, 1));
    assert!(!maximizes(1600.0, 1));
    assert!(!maximizes(2400.0, 2));
}

#[test]
fn ordinary_play_after_stop_is_full_speed() {
    use serde_json::json;
    let engine = soundcraft_engine::Engine::default();
    let player = soundcraft_playback::Player::new(engine.session_arc());
    let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
    app.engine.execute("edit.select", &json!({"start": 0, "end": 48000})).unwrap();
    app.engine.execute("options.loop_playback", &json!({"value": true})).unwrap();
    app.engine.execute("options.pre_post_roll", &json!({"value": false})).unwrap();
    let speed = |app: &super::SoundApp| app.player.as_ref().map(|p| p.speed()).unwrap_or(0.0);
    assert!((speed(&app) - 1.0).abs() < 1e-3);

    app.engine.execute("transport.half_speed", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    assert!(app.is_playing());
    assert!((speed(&app) - 0.5).abs() < 1e-3, "half-speed playback");

    app.engine.execute("transport.stop", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    assert!(!app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "stop restores full speed");

    app.engine.execute("transport.play", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    assert!(app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "ordinary play stays at full speed");

    app.engine.execute("transport.stop", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    app.engine.execute("options.loop_playback", &json!({"value": false})).unwrap();
    app.engine.execute("transport.half_speed", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    assert!((speed(&app) - 0.5).abs() < 1e-3);
    app.player.as_ref().unwrap().stop();
    app.handle_transport(1.0 / 60.0);
    assert!(!app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "a half-speed pass that ends on its own restores full speed");
}
/// A synthetic batch survives hidden logic-only ticks and is applied exactly once by a real
/// UI pass; hidden ticks never re-inject or discard it.
#[test]
fn one_synthetic_batch_survives_hidden_ticks_and_applies_once() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let raw = |w: f32| {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 900.0));
        egui::RawInput { screen_rect: Some(rect), ..Default::default() }
    };
    for _ in 0..4 {
        let mut r = raw(1400.0);
        app.raw_input_hook(&ctx, &mut r);
        let mut out = ctx.run_ui(r, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    // Synthetic pointer input is delivered one event per host frame, like a real drag.
    let pos = egui::pos2(40.0, 40.0);
    app.synthetic.push(egui::Event::PointerMoved(pos));
    for _ in 0..3 {
        let mut r = raw(1400.0);
        app.raw_input_hook(&ctx, &mut r);
        let _ = ctx.run_logic(&r, |ctx| app.logic(ctx));
        assert_eq!(app.synthetic.len(), 0, "the batch was taken from the queue once");
        assert_eq!(app.synthetic_batch.as_ref().map(Vec::len), Some(1), "the batch stays pending across hidden ticks");
        assert_eq!(app.synthetic_applied_batches, 0, "hidden ticks do not acknowledge the batch");
    }
    let mut r = raw(1400.0);
    app.raw_input_hook(&ctx, &mut r);
    let mut out = ctx.run_ui(r, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.synthetic_applied_batches, 1);
    assert!(app.synthetic_batch.is_none());
}

#[derive(Debug)]
struct TestDrop(std::path::PathBuf);

impl egui::DroppedFile for TestDrop {
    /// The test drop's file path; its bytes are empty because these tests only need a name.
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    /// Test drops carry no bytes; native imports only need the path in these tests.
    #[cfg(not(target_arch = "wasm32"))]
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
    /// Test drops carry no bytes; the browser path only needs a name in these tests.
    #[cfg(target_arch = "wasm32")]
    fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// A drop captured on the font-gated first frame survives to the first laid-out pass and is
/// handed to the shell exactly once.
#[test]
fn a_drop_before_fonts_are_ready_is_imported_once_after_layout() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
    let mut first =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
    first.dropped_files.push(std::sync::Arc::new(TestDrop(std::path::PathBuf::from("/tmp/soundcraft-drop.scraft"))));
    let mut out = ctx.run_ui(first, |ui| app.ui(ui));
    out.textures_delta.clear();
    assert!(app.take_dropped_files().is_empty(), "the drop is queued, not dropped on the font gate");
    for _ in 0..3 {
        let mut r =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
        app.raw_input_hook(&ctx, &mut r);
        let mut out = ctx.run_ui(r, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    let dropped = app.take_dropped_files();
    assert_eq!(dropped.len(), 1, "the session file reached the shell once");
    assert!(dropped.first().is_some_and(|f| f.path().ends_with("soundcraft-drop.scraft")));
    assert!(app.take_dropped_files().is_empty(), "later ticks do not replay the drop");
}

/// eframe keeps the hook-mutated `RawInput` for the next hidden tick and appends new host
/// events to it. Re-injecting the batch must replace that copy, not add another: a restored
/// click or key must run once, not once per hidden tick.
#[test]
fn a_retained_host_input_holds_one_copy_of_a_pending_batch() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let host_input = || egui::RawInput {
        screen_rect: Some(rect),
        viewports: [(egui::ViewportId::ROOT, hidden.clone())].into_iter().collect(),
        ..Default::default()
    };
    // Let fonts install and the palette settle first: the first pass draws nothing.
    for _ in 0..3 {
        let mut r = host_input();
        app.raw_input_hook(&ctx, &mut r);
        let mut out = ctx.run_ui(r, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    // A click is three queued pointer events; each becomes one batch on its own host frame.
    let pos = egui::pos2(40.0, 40.0);
    app.synthetic.push(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    let mut retained = host_input();
    for tick in 0..5 {
        // `prepare_raw_input`: newer host input appended to the retained, hook-mutated input.
        let mut newer = host_input();
        newer.events.push(egui::Event::PointerMoved(egui::pos2(tick as f32, 0.0)));
        retained.append(newer);
        app.raw_input_hook(&ctx, &mut retained);
        let batch = app.synthetic_batch.clone().expect("the first pointer event is pending");
        assert_eq!(batch.len(), 1, "one pointer event per host frame");
        let copies = retained.events.iter().filter(|e| batch.contains(*e)).count();
        assert_eq!(copies, 2, "the pending event and its hover copy appear exactly once, not once per tick");
        let _ = ctx.run_logic(&retained, |ctx| app.logic(ctx));
        assert!(app.synthetic_batch.is_some(), "hidden ticks never apply the batch");
        assert_eq!(app.synthetic_applied_batches, 0);
    }
    // A settled UI pass applies each queued pointer event exactly once.
    for applied in 1..=3 {
        let mut raw = host_input();
        app.raw_input_hook(&ctx, &mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.synthetic_applied_batches, applied);
    }
    assert!(app.synthetic_batch.is_none(), "every queued event was delivered once");
    assert!(app.synthetic.is_empty());
}

/// A batch waiting for a UI pass under a hidden window asks the host for a real paint, so it is
/// delivered instead of spinning forever on logic ticks.
#[test]
fn a_pending_synthetic_batch_asks_a_hidden_window_for_a_paint() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    app.synthetic.push(egui::Event::Text("x".into()));
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
        viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(),
        ..Default::default()
    };
    app.raw_input_hook(&ctx, &mut raw);
    assert!(app.synthetic_batch.is_some());
    let out = ctx.run_logic(&raw, |ctx| app.logic(ctx));
    let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
    assert!(commands.contains(&egui::ViewportCommand::RequestPaintWhileHidden), "{commands:?}");
    assert_eq!(app.synthetic_applied_batches, 0, "logic alone does not apply the gesture");
}

/// A frame that discards and lays out a second pass is still one rendered frame: frame
/// statistics must not sample the extra pass's near-zero delta.
#[test]
fn frame_time_counts_a_multi_pass_frame_once() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    for _ in 0..90 {
        app.advance_test_clock(std::time::Duration::from_secs_f64(1.0 / 60.0));
        let mut discard = true;
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            if discard {
                // A widget that needs another layout pass (Grid, a timeline measurement) asks
                // for the discard during this pass, before `ui` settles.
                discard = false;
                ui.ctx().request_discard("two-pass frame test");
            }
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    let ms = app.frame_ms;
    assert!((ms - 1000.0 / 60.0).abs() < 0.5, "{ms} ms/frame across two-pass frames");
}

/// Scripted input is automation, not a user clipboard action: a synthetic copy/selection in a
/// text widget must not reach the OS clipboard or PRIMARY selection through platform output.
#[test]
fn a_synthetic_pass_does_not_publish_os_clipboard_commands() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    // Control: the same command from a pass without synthetic input still reaches the host.
    let mut out = ctx.run_ui(raw(), |ui| {
        app.logic(ui.ctx());
        ui.ctx().copy_text("real user copy".into());
        app.ui(ui);
    });
    assert!(
        out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text.as_str() == "real user copy")),
        "the host still receives real copy commands"
    );
    out.textures_delta.clear();

    app.synthetic.push(egui::Event::Text("x".into()));
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        // Stand in for a TextEdit/widget that emits clipboard commands during the pass.
        ui.ctx().copy_text("scripted copy".into());
        ui.ctx().output_mut(|o| o.commands.push(egui::OutputCommand::TextSelectionSettled("scripted selection".into())));
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.synthetic_applied_batches, 1, "the pass delivered the synthetic event");
    assert!(
        !out.platform_output.commands.iter().any(|c| matches!(
            c,
            egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_) | egui::OutputCommand::TextSelectionSettled(_)
        )),
        "scripted input must not publish clipboard or PRIMARY commands: {:?}",
        out.platform_output.commands
    );
}

/// egui rewrites a key press's `repeat` flag from the keys held in the previous pass. A scripted
/// press must still run its shortcut even while the same physical key is held, a same-frame host
/// auto-repeat with the same chord must not make it run twice, and the host's held state must
/// survive the scripted press and release.
#[test]
fn a_held_key_does_not_turn_a_scripted_press_into_a_repeat() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    for _ in 0..2 {
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    }
    let depth = app.engine.undo_history().len();
    assert!(depth >= 2, "the test needs two undoable edits");
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    // The user presses and holds Cmd+Z. That real press is one undo and leaves Z physically
    // down; a later same-chord press is rewritten to a repeat.
    let mut held = raw();
    held.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
    let mut out = ctx.run_ui(held, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::Z)), "the host is still holding Z");
    assert_eq!(app.engine.undo_history().len(), depth - 1, "the real host press ran one undo");
    let held_depth = app.engine.undo_history().len();
    // Cmd+Z from the control channel while that chord is held, with the host's same-frame
    // auto-repeat already in the raw input ahead of the appended synthetic batch. Both events
    // have the same key, physical key and modifiers; only the host one is a repeat.
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
    }
    let mut input = raw();
    input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: true, modifiers });
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none(), "the pass that received the rewritten press acknowledged it");
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the scripted release did not report the physically held key as up");
    assert_eq!(app.engine.undo_history().len(), held_depth - 1, "the scripted Cmd+Z ran exactly once despite the identical host repeat");
    // The following host frame must not replay the same key.
    let mut input = raw();
    input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: true, modifiers });
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.engine.undo_history().len(), held_depth - 1, "a later host auto-repeat must not undo a second time");
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the host still holds Z after the repeat");
}

/// A fresh physical Cmd+Z and an identical scripted Cmd+Z in one frame are distinct actions.
/// Matching them by value would swallow the scripted press; provenance keeps both.
#[test]
fn a_fresh_host_press_and_a_scripted_press_both_run() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    for _ in 0..2 {
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    }
    let depth = app.engine.undo_history().len();
    assert!(depth >= 2, "the test needs two undoable edits");
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
    }
    let mut input = raw();
    input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none(), "the pass acknowledged the injected batch");
    assert_eq!(app.engine.undo_history().len(), depth - 2, "the physical press and the injected press are separate actions");
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the physical press still holds Z after the injected release");
}

/// A hidden logic-only tick after the hook runs no egui input pass. Repeated hidden ticks that
/// re-hook the retained host input must leave the physical key state and one pending batch
/// untouched.
#[test]
fn a_hidden_logic_tick_after_the_hook_keeps_the_held_key_and_pending_batch() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let visible = egui::ViewportInfo { inner_rect: Some(rect), ..Default::default() };
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let raw = |viewport: egui::ViewportInfo| egui::RawInput {
        screen_rect: Some(rect),
        viewports: [(egui::ViewportId::ROOT, viewport)].into_iter().collect(),
        ..Default::default()
    };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(visible.clone()), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    let depth = app.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    // Alt+Z keeps Z physically down without firing the plain-Z Commands Focus undo.
    let mut held = raw(visible.clone());
    held.events.push(egui::Event::Key {
        key: egui::Key::Z,
        physical_key: Some(egui::Key::Z),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
    });
    let mut out = ctx.run_ui(held, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
    assert_eq!(app.engine.undo_history().len(), depth, "holding Alt+Z is not a command");
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
    }
    // eframe retains a hook-mutated input across hidden ticks and hands it back to the hook.
    let mut retained = raw(hidden);
    for tick in 0..3 {
        app.raw_input_hook(&ctx, &mut retained);
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "hook tick {tick} mutated the held state");
        assert!(app.synthetic_batch.is_some(), "hook tick {tick} dropped the batch");
        assert_eq!(app.engine.undo_history().len(), depth, "hook tick {tick} ran a shortcut");
        let _ = ctx.run_logic(&retained, |ctx| app.logic(ctx));
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "hidden tick {tick} erased the physically held key");
        assert!(app.synthetic_batch.is_some(), "hidden tick {tick} applied or dropped the batch");
        assert_eq!(app.engine.undo_history().len(), depth, "hidden tick {tick} ran a shortcut");
    }
    // The deferred batch still reaches the next UI pass exactly once.
    let mut out = ctx.run_ui(retained, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none());
    assert_eq!(app.engine.undo_history().len(), depth - 1, "the deferred scripted Cmd+Z ran exactly once");
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the held key survives the deferred pass too");
}

/// A frame that asks for a second layout pass receives the injected key and dispatches it in
/// the first pass. The batch must be acknowledged once, and the physically held key must still
/// be down in the settled pass rather than being cleared by the scripted release.
#[test]
fn a_multi_pass_frame_keeps_the_held_key_and_applies_the_scripted_press_once() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    let depth = app.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    let mut held = raw();
    held.events.push(egui::Event::Key {
        key: egui::Key::Z,
        physical_key: Some(egui::Key::Z),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
    });
    let mut out = ctx.run_ui(held, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
    }
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut first_pass = true;
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        if first_pass {
            first_pass = false;
            // A measurement or grid asks for another layout pass after the widgets ran.
            ui.ctx().request_discard("discard the pass that received the synthetic key");
        }
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(out.platform_output.num_completed_passes >= 2, "the frame really ran more than one pass");
    assert!(app.synthetic_batch.is_none(), "the pass that received the batch consumed it");
    assert_eq!(app.engine.undo_history().len(), depth - 1, "one scripted Cmd+Z undid exactly one edit");
    assert_eq!(app.synthetic_applied_batches, 1);
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the held key survives the settled multi-pass frame");
}

/// Losing focus clears the physical key state even when a scripted key batch is in flight. The
/// injected press still runs once, and restoring the host-only state must not bring the stale
/// held key back.
#[test]
fn a_focus_loss_clears_a_held_key_with_a_pending_scripted_press() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    let depth = app.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    let mut held = raw();
    held.events.push(egui::Event::Key {
        key: egui::Key::Z,
        physical_key: Some(egui::Key::Z),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
    });
    let mut out = ctx.run_ui(held, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
    }
    let mut input = raw();
    input.events.push(egui::Event::WindowFocused(false));
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none(), "the injected batch still ran once after focus loss");
    assert_eq!(app.engine.undo_history().len(), depth - 1);
    assert!(!ctx.input(|i| i.key_down(egui::Key::Z)), "focus loss cleared the held key and the injected release did not resurrect it");
    // The next host frame must not bring the stale key back either.
    let mut out = ctx.run_ui(raw(), |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(!ctx.input(|i| i.key_down(egui::Key::Z)));
    assert_eq!(app.engine.undo_history().len(), depth - 1);
}

/// A host key with the same signature as the pending injection does not acknowledge a batch
/// that was never actually appended. Otherwise a lookalike event could hide a lost scripted
/// action.
#[test]
fn a_host_key_with_the_same_signature_does_not_acknowledge_the_batch() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    for _ in 0..2 {
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    }
    let depth = app.engine.undo_history().len();
    assert!(depth >= 2, "the test needs two undoable edits");
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    // The batch is pending but this pass was not prepared by the hook, so it carries no injected
    // copy. The identical host press is a real action, not the injected one.
    app.synthetic_batch =
        Some(vec![egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers }]);
    let mut input = raw();
    input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_some(), "a lookalike host event must not acknowledge the batch");
    assert_eq!(app.synthetic_applied_batches, 0);
    assert_eq!(app.engine.undo_history().len(), depth - 1, "only the real host press ran");
    // The hook then appends the actual injection; that pass acknowledges and runs it once.
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none());
    assert_eq!(app.synthetic_applied_batches, 1);
    assert_eq!(app.engine.undo_history().len(), depth - 2, "the actual injected press ran in addition to the host press");
}

/// The acknowledgement compares key events without egui's rewritten `repeat` flag. Every other
/// identifying field — the logical key, physical key, press/release state and modifiers — still
/// has to match.
#[test]
fn synthetic_key_acknowledgement_ignores_only_the_rewritten_repeat_flag() {
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    let sent = egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers };
    let mut received = sent.clone();
    if let egui::Event::Key { repeat, .. } = &mut received {
        *repeat = true;
    }
    assert!(super::SoundApp::synthetic_event_received(&sent, &received), "egui's repeat rewrite does not hide the press");
    let mut wrong = received.clone();
    if let egui::Event::Key { key, .. } = &mut wrong {
        *key = egui::Key::X;
    }
    assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a different logical key is not the scripted press");
    let mut wrong = received.clone();
    if let egui::Event::Key { physical_key, .. } = &mut wrong {
        *physical_key = None;
    }
    assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a different physical key is not the scripted press");
    let mut wrong = received.clone();
    if let egui::Event::Key { pressed, .. } = &mut wrong {
        *pressed = false;
    }
    assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a release is not the scripted press");
    let mut wrong = received;
    if let egui::Event::Key { modifiers: got, .. } = &mut wrong {
        *got = egui::Modifiers::NONE;
    }
    assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "different modifiers are not the scripted press");
    assert!(!super::SoundApp::synthetic_event_received(&sent, &egui::Event::Text("z".into())), "other variants match exactly");
}

/// A pass that receives the synthetic events and then asks for another layout pass has already
/// dispatched their widget side effects. It must acknowledge the batch: leaving it pending
/// makes the next host frame inject the same click/key again (a double undo, a toggled command,
/// a dialog action run twice).
#[test]
fn a_discarded_pass_acknowledges_the_synthetic_batch_it_received() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
    let depth = app.engine.undo_history().len();
    assert!(depth > 0, "the test needs an undoable edit");
    // Cmd+Z, queued as egui delivers a real key: press and release together.
    let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed, repeat: false, modifiers });
    }
    let mut first_pass = true;
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        if first_pass {
            first_pass = false;
            // A measurement or grid asks for another layout pass after the widgets ran.
            ui.ctx().request_discard("discard the pass that received the synthetic key");
        }
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_none(), "the pass that received the batch consumed it");
    assert_eq!(app.engine.undo_history().len(), depth - 1, "one Cmd+Z undid exactly one edit");
    assert_eq!(app.synthetic_applied_batches, 1);
    // The following host frame must not replay the same key.
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.engine.undo_history().len(), depth - 1, "a later frame must not undo a second time");
    assert_eq!(app.synthetic_applied_batches, 1);
}

/// The real toolbar tempo `DragValue`/`TextEdit` consumes scripted keys from
/// `InputState.events` (arrow navigation), yet each scripted edit, navigation step and cut is
/// dispatched exactly once. A post-widget acknowledgement would miss the consumed press, keep
/// the batch pending and apply it again on the next frame.
#[test]
fn real_widgets_apply_a_scripted_edit_navigation_and_cut_once_despite_consuming_the_events() {
    struct Rig {
        ctx: egui::Context,
        app: SoundApp,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Rig {
        /// A 1600x1000 rig over a one-track session, warmed for four frames.
        fn new() -> Self {
            let mut engine = soundcraft_engine::Engine::default();
            engine.execute("track.new", &json!({})).unwrap();
            let mut rig = Self { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new() };
            rig.frames(4);
            rig
        }

        /// A blank 1600x1000 host frame.
        fn raw(&self) -> egui::RawInput {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
            egui::RawInput { screen_rect: Some(rect), ..Default::default() }
        }

        /// Run one UI pass with `events`, keeping the emitted shapes for position lookups.
        fn frame(&mut self, events: Vec<egui::Event>) {
            let mut input = self.raw();
            input.events = events;
            self.app.raw_input_hook(&self.ctx, &mut input);
            let mut out = self.ctx.run_ui(input, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            });
            out.textures_delta.clear();
            self.shapes = out.shapes;
        }

        /// Advance `n` blank frames.
        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(Vec::new());
            }
        }

        /// Queue events as scripted input and run one frame.
        fn synthetic(&mut self, events: Vec<egui::Event>) {
            self.app.synthetic.extend(events);
            self.frame(Vec::new());
        }

        /// The session tempo at tick 0.
        fn bpm(&self) -> f64 {
            self.app.engine.session().tempo.tempo_at_tick(0)
        }

        /// Centre of the toolbar tempo, which shows `bpm` with four decimals.
        fn field(&self, bpm: f64) -> egui::Pos2 {
            let text = format!("{bpm:.4}");
            self.shapes
                .iter()
                .find_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                    _ => None,
                })
                .expect("tempo not in the toolbar")
        }

        /// Send a primary press or release at `at` in its own frame.
        fn button(&mut self, at: egui::Pos2, pressed: bool) {
            self.frame(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
            ]);
        }

        /// Press and release at `at`, then let the UI settle for two frames.
        fn click(&mut self, at: egui::Pos2) {
            self.button(at, true);
            self.button(at, false);
            self.frames(2);
        }
    }

    let key = |key: egui::Key, modifiers: egui::Modifiers| {
        [
            egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
            egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
        ]
    };
    let mut rig = Rig::new();
    let mut applied = rig.app.synthetic_applied_batches;

    // Edit: typing replaces the selected tempo, and Enter commits it once.
    let at = rig.field(120.0);
    rig.click(at);
    assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo TextEdit has focus");
    rig.synthetic(vec![egui::Event::Text("98.5".into())]);
    rig.synthetic(key(egui::Key::Enter, egui::Modifiers::NONE).to_vec());
    rig.frames(2);
    assert!((rig.bpm() - 98.5).abs() < 1e-9, "the scripted edit applied once: {}", rig.bpm());
    assert!(rig.app.synthetic_batch.is_none());
    assert_eq!(rig.app.synthetic_applied_batches, applied + 2);
    applied = rig.app.synthetic_applied_batches;

    // Navigation: the focused DragValue consumes ArrowUp out of InputState.events.
    rig.frames(6); // The scripted gesture's pointer grace must end before a real click.
    rig.click(rig.field(98.5));
    assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo DragValue is editing");
    rig.synthetic(key(egui::Key::ArrowUp, egui::Modifiers::NONE).to_vec());
    assert!(
        !rig.ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::ArrowUp, pressed: true, .. }))),
        "the real DragValue consumed the injected ArrowUp press"
    );
    assert!(rig.app.synthetic_batch.is_none(), "the pass that let DragValue consume ArrowUp still acknowledged it");
    let stepped = rig.bpm();
    assert!((stepped - 98.6).abs() < 1e-9, "one scripted navigation step: {stepped}");
    rig.frames(2);
    assert!((rig.bpm() - stepped).abs() < 1e-9, "a later frame must not step again");
    assert_eq!(rig.app.synthetic_applied_batches, applied + 1);
    applied = rig.app.synthetic_applied_batches;

    // Cut: select the tempo text and cut it once. The value only changes on commit, so cancel
    // with Escape and keep the previous tempo.
    rig.frames(6);
    rig.click(rig.field(stepped));
    assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo TextEdit is editing again");
    rig.synthetic(key(egui::Key::A, egui::Modifiers { command: true, ..egui::Modifiers::NONE }).to_vec());
    rig.synthetic(vec![egui::Event::Cut]);
    assert!(rig.app.synthetic_batch.is_none(), "the scripted cut was acknowledged");
    assert!(
        !rig.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "98.6000")),
        "the scripted cut removed the selected tempo text"
    );
    assert_eq!(rig.app.synthetic_applied_batches, applied + 2);
    rig.synthetic(key(egui::Key::Escape, egui::Modifiers::NONE).to_vec());
    rig.frames(2);
    assert!((rig.bpm() - stepped).abs() < 1e-9, "cutting the edit text and cancelling leaves the tempo unchanged");
}

/// A hidden logic-only tick has no fresh input: the last pass's pointer must not write Touch
/// automation or leave the fader gesture stuck.
#[test]
fn a_hidden_tick_does_not_write_automation_from_the_last_pass_pointer() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "touch"})).unwrap();
    app.run("transport.locate", json!({"at": 0})).unwrap();
    app.run("transport.play", json!({})).unwrap();
    app.handle_transport(0.0);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let pos = egui::pos2(700.0, 450.0);
    let press = egui::RawInput {
        screen_rect: Some(rect),
        events: vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
        ],
        ..Default::default()
    };
    let mut out = ctx.run_ui(press, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(ctx.input(|i| i.pointer.any_down()), "the visible pass left the pointer down");
    let start_db = app.engine.session().track(kick).unwrap().mixer.volume_db;
    app.gesture = Some(super::Gesture::Fader { track: kick, start_db });
    app.last_write_at = i64::MIN;
    let points = |app: &super::SoundApp| {
        app.engine.session().track(kick).and_then(|t| t.lane(&soundcraft_model::AutoParam::Volume)).map_or(0, |l| l.points.len())
    };
    let before = points(&app);
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let raw = egui::RawInput { screen_rect: Some(rect), viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
    let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
    assert!(app.gesture.is_none(), "visibility loss drops the transient fader gesture");
    assert_eq!(points(&app), before, "a hidden tick wrote automation from the stale pointer");
    app.run("transport.stop", json!({})).unwrap();
}

/// The font-gated first pass returns before layout, so it cannot have dispatched the batch
/// and must leave it pending for the first laid-out pass.
#[test]
fn a_font_gated_pass_does_not_ack_the_pending_synthetic_batch() {
    let ctx = egui::Context::default();
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    app.synthetic.push(egui::Event::Text("x".into()));
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(!app.fonts_ready, "the first pass installs fonts but does not lay out");
    assert!(app.synthetic_batch.is_some(), "the font gate returns before the acknowledgement");
    assert_eq!(app.synthetic_applied_batches, 0);
    // The next hook offers the same batch to the first laid-out pass, which applies it once.
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.fonts_ready);
    assert!(app.synthetic_batch.is_none());
    assert_eq!(app.synthetic_applied_batches, 1);
    // And the following frame must not replay it.
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.synthetic_applied_batches, 1);
}

/// A settled pass that never received the injected events (for example a forced second pass
/// after a font-gated first pass) must not consume the pending batch.
#[test]
fn a_settled_pass_that_did_not_receive_the_batch_does_not_ack_it() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    app.synthetic_batch = Some(vec![egui::Event::Text("x".into())]);
    let mut out = ctx.run_ui(raw(), |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert!(app.synthetic_batch.is_some(), "a pass without the events keeps the batch pending");
    assert_eq!(app.synthetic_applied_batches, 0);
    let mut injected = raw();
    app.raw_input_hook(&ctx, &mut injected);
    let mut out = ctx.run_ui(injected, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    assert_eq!(app.synthetic_applied_batches, 1);
    assert!(app.synthetic_batch.is_none());
}

/// The transport simulation advances by real elapsed monotonic time, not by a full frame step
/// per call and not by the stale UI input clock.
#[test]
fn transport_simulation_advances_by_monotonic_elapsed_time_only() {
    let ctx = egui::Context::default();
    let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
    app.sim = Some((0, None, None));
    app.engine.transport.playing = true;
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
    let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
    // Two passes of one host frame must not each advance a full predicted frame.
    let before = app.position();
    let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
    let same_frame = app.position() - before;
    assert!(same_frame.abs() < 480, "a repeated pass advanced {same_frame} samples");
    app.advance_test_clock(std::time::Duration::from_millis(200));
    let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
    let elapsed = app.position() - before;
    let expected = (0.2 * app.engine.session().sample_rate.as_f64()) as i64;
    assert!((elapsed - expected).abs() < expected / 20, "200 ms advanced {elapsed} samples, expected about {expected}");
    app.engine.transport.playing = false;
    app.sim = None;
}
/// One host iteration: the raw-input hook, then logic and UI in a single pass.
fn run_host_frame(app: &mut SoundApp, ctx: &egui::Context, mut input: egui::RawInput) {
    app.raw_input_hook(ctx, &mut input);
    let mut output = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    output.textures_delta.clear();
}

/// Run one frame that discards its first layout pass; returns how many passes ran.
fn run_host_frame_discarding_first_pass(app: &mut SoundApp, ctx: &egui::Context, input: egui::RawInput) -> usize {
    let mut passes = 0;
    let mut output = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        passes += 1;
        if passes == 1 {
            ui.ctx().request_discard("plugin gesture must survive a second layout pass");
        }
        app.ui(ui);
    });
    output.textures_delta.clear();
    passes
}

/// The editor-poll fixture: each poll counts itself, takes the queued edits and, for the gestures that
/// ended, captures the instrument state `latest` holds.
fn poll_fixture(
    reports: &std::rc::Rc<std::cell::RefCell<Vec<soundcraft_playback::EditorEdit>>>,
    latest: &std::rc::Rc<std::cell::Cell<u8>>,
    polls: &std::rc::Rc<std::cell::Cell<i32>>,
    captures: &std::rc::Rc<std::cell::RefCell<Vec<(soundcraft_model::TrackId, usize)>>>,
) -> super::EditorPollFixture {
    let (poll_reports, poll_latest, poll_count, poll_captures) = (reports.clone(), latest.clone(), polls.clone(), captures.clone());
    Box::new(move |pending| {
        poll_count.set(poll_count.get() + 1);
        let edits = std::mem::take(&mut *poll_reports.borrow_mut());
        super::editor_changes(
            pending,
            edits,
            Vec::new(),
            |_| false,
            |due| {
                poll_captures.borrow_mut().extend_from_slice(due);
                due.iter().map(|&(track, slot)| (track, slot, vec![poll_latest.get(), 1])).collect()
            },
        )
    })
}

/// Plugin queues are drained once per host iteration, rather than once per discarded pass.
/// The fixture replaces only native polling; the real raw hook, logic/UI, scheduler and
/// engine state/undo commands run normally.
#[test]
fn editor_gesture_polling_survives_discarded_and_hidden_host_iterations() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    let ctx = egui::Context::default();
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
    for _ in 0..4 {
        run_host_frame(&mut app, &ctx, raw());
    }
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let original = instrument_state(&app, keys);
    let revision = app.engine.revision;
    let undo = app.engine.undo_history().len();
    let history = app.engine.undo_history();
    let journal = app.engine.journal.clone();
    let state_command = soundcraft_engine::find_command("mix.insert_state").unwrap();
    assert!(!state_command.undoable && !state_command.journal, "this fixture exercises state-only instrument persistence");
    assert!(!app.engine.is_dirty());
    let reports = Rc::new(RefCell::new(Vec::<soundcraft_playback::EditorEdit>::new()));
    let latest = Rc::new(Cell::new(1u8));
    let polls = Rc::new(Cell::new(0));
    let captures = Rc::new(RefCell::new(Vec::new()));
    app.editor_poll_fixture = Some(poll_fixture(&reports, &latest, &polls, &captures));
    reports.borrow_mut().push((keys, soundcraft_mix::INSTRUMENT_SLOT, "cutoff".into(), 1.0));
    let mut input = raw();
    app.raw_input_hook(&ctx, &mut input);
    let passes = run_host_frame_discarding_first_pass(&mut app, &ctx, input);
    assert!(passes >= 2, "the regression must actually force a discarded pass");
    assert_eq!(polls.get(), 1, "the rerun cannot drain an empty editor queue");
    assert!(captures.borrow().is_empty(), "the ongoing gesture has not ended");
    assert_eq!(app.editor_states, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    assert_eq!(instrument_state(&app, keys), original);
    assert_eq!(app.engine.revision, revision);
    assert_eq!(app.engine.undo_history().len(), undo);
    assert!(app.has_unsaved_changes(), "pending editor work still needs autosave/recovery");

    // The next host hook is hidden: it still polls the continuing gesture despite a stale
    // discarded-pass index and without drawing any UI.
    latest.set(2);
    reports.borrow_mut().push((keys, soundcraft_mix::INSTRUMENT_SLOT, "cutoff".into(), 2.0));
    let mut hidden = raw();
    hidden.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().minimized = Some(true);
    app.raw_input_hook(&ctx, &mut hidden);
    let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
    let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
    assert_eq!(polls.get(), 2, "hidden hook rearms exactly one poll, not one per logic callback");
    assert!(captures.borrow().is_empty());
    assert_eq!(app.editor_states, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    assert_eq!(instrument_state(&app, keys), original);
    assert_eq!(app.engine.revision, revision);

    // A genuine next-iteration empty poll ends the gesture and persists its latest state.
    app.raw_input_hook(&ctx, &mut hidden);
    let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
    assert_eq!(polls.get(), 3);
    assert_eq!(*captures.borrow(), [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
    assert!(app.editor_states.is_empty());
    assert_eq!(instrument_state(&app, keys), Some(vec![2, 1]));
    assert_eq!(app.engine.revision, revision.wrapping_add(1), "the completed gesture persists exactly one document revision");
    assert!(app.engine.is_dirty(), "captured instrument state is unsaved document work");
    assert_eq!(app.engine.undo_history().len(), undo, "state-only instrument persistence does not create an undo step");
    assert_eq!(app.engine.undo_history(), history, "state capture does not replace another history entry");
    assert_eq!(app.engine.journal, journal, "plugin-state blobs are not journaled");
    let settled_revision = app.engine.revision;
    app.raw_input_hook(&ctx, &mut hidden);
    let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
    assert_eq!(polls.get(), 4, "a fresh empty host iteration is still live");
    assert_eq!(captures.borrow().len(), 1, "the completed gesture is captured once");
    assert_history_unchanged(&app, settled_revision, undo, &history, &journal);
}

/// The engine's revision, undo history and journal are as they were.
fn assert_history_unchanged(
    app: &SoundApp,
    revision: u64,
    undo: usize,
    history: &[String],
    journal: &std::collections::VecDeque<(String, serde_json::Value)>,
) {
    assert_eq!(app.engine.revision, revision);
    assert_eq!(app.engine.undo_history().len(), undo);
    assert_eq!(app.engine.undo_history(), history);
    assert_eq!(&app.engine.journal, journal);
}
