//! Pro Tools keys for commands that had none: Grid and Nudge values, Commands Focus nudging and selection
//! keys, playlists, F12, Ctrl+Tab off macOS and the window keys.

use egui::os::OperatingSystem;
use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_engine::TransportRequest;
use soundcraft_model::ClipContent;
use soundcraft_ui_egui::{Services, SoundApp, shortcuts};

const MAC_CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
const PC_CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
const OSES: [(OperatingSystem, Modifiers); 3] =
    [(OperatingSystem::Mac, MAC_CMD), (OperatingSystem::Windows, PC_CTRL), (OperatingSystem::Nix, PC_CTRL)];
const ALT_SHIFT: Modifiers = Modifiers { alt: true, shift: true, ..Modifiers::NONE };

fn keys(key: Key, modifiers: Modifiers) -> Vec<Event> {
    let presses = [true, false].map(|pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
    [Event::ModifiersChanged(modifiers)].into_iter().chain(presses).collect()
}

fn press(ctx: &egui::Context, app: &mut SoundApp, key: Key, modifiers: Modifiers) {
    ctx.run_ui(RawInput { events: keys(key, modifiers), ..Default::default() }, |ui| shortcuts::handle(app, ui.ctx())).textures_delta.clear();
}

fn rig(os: OperatingSystem) -> (egui::Context, SoundApp) {
    let ctx = egui::Context::default();
    ctx.set_os(os);
    (ctx, SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default()))
}

fn start(app: &SoundApp) -> i64 {
    app.engine.session().edit.selection.start
}

#[test]
fn shift_equals_and_minus_step_the_grid_value_and_with_alt_the_nudge_value() {
    for (os, _) in OSES {
        let (ctx, mut app) = rig(os);
        app.run("view.grid", json!({"value": "1/16"})).unwrap();
        app.run("view.nudge", json!({"value": "1/16"})).unwrap();
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 48_000, "end": 96_000})).unwrap();
        press(&ctx, &mut app, Key::Plus, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::Minus, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::Minus, Modifiers::SHIFT);
        let up = if os == OperatingSystem::Mac { Key::Equals } else { Key::Plus };
        press(&ctx, &mut app, up, ALT_SHIFT);
        press(&ctx, &mut app, up, ALT_SHIFT);
        press(&ctx, &mut app, Key::Minus, ALT_SHIFT);
        let edit = &app.engine.session().edit;
        assert_eq!((edit.grid.label(), edit.nudge.label()), ("1/32 note".to_string(), "1/8 note".to_string()), "{os:?}");
        assert_eq!(start(&app), 48_000, "{os:?}");
        press(&ctx, &mut app, Key::Plus, Modifiers::NONE);
        assert!(start(&app) > 48_000, "{os:?}");
    }
}

#[test]
fn commands_focus_nudges_matches_the_selections_plays_and_redoes() {
    for (os, _) in OSES {
        let (ctx, mut app) = rig(os);
        app.run("options.keyboard_focus", json!({"focus": "commands"})).unwrap();
        app.run("view.nudge", json!({"value": {"samples": 100}})).unwrap();
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 48_000, "end": 96_000})).unwrap();
        let mut at = vec![];
        for key in [Key::Period, Key::Slash, Key::Comma, Key::M] {
            press(&ctx, &mut app, key, Modifiers::NONE);
            at.push(start(&app));
        }
        assert_eq!(at, [48_100, 49_100, 49_000, 48_000], "{os:?}");
        press(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(start(&app), 49_000, "{os:?}");
        press(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert_eq!(start(&app), 48_000, "{os:?}");

        app.run("options.link_timeline_edit", json!({"value": false})).unwrap();
        press(&ctx, &mut app, Key::O, Modifiers::NONE);
        let edit_selection = app.engine.session().edit.selection;
        assert_eq!(app.engine.session().edit.timeline_selection, edit_selection, "{os:?}");
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": 10})).unwrap();
        press(&ctx, &mut app, Key::Num0, Modifiers::NONE);
        assert_eq!(app.engine.session().edit.selection, edit_selection, "{os:?}");

        app.engine.transport_requests.clear();
        press(&ctx, &mut app, Key::OpenBracket, Modifiers::NONE);
        press(&ctx, &mut app, Key::CloseBracket, Modifiers::NONE);
        assert_eq!(
            app.engine.transport_requests,
            [TransportRequest::PlaySelection, TransportRequest::Locate(48_000), TransportRequest::Play],
            "{os:?}"
        );
    }
}

#[test]
fn commands_focus_shift_p_and_semicolon_extend_the_selection_onto_shown_tracks() {
    for (os, _) in OSES {
        let (ctx, mut app) = rig(os);
        app.run("track.move_to_new_folder", json!({"tracks": ["Bass"], "name": "Low"})).unwrap();
        app.run("track.folder_toggle", json!({"track": "Low", "open": false})).unwrap();
        app.run("track.hide", json!({"tracks": ["Hats"], "hidden": true})).unwrap();
        app.run("options.keyboard_focus", json!({"focus": "commands"})).unwrap();
        app.run("edit.select", json!({"tracks": ["Snare"], "start": 48_000, "end": 96_000})).unwrap();
        press(&ctx, &mut app, Key::P, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::Colon, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::Semicolon, Modifiers::SHIFT);
        let s = app.engine.session();
        let names: Vec<&str> = s.edit.selected_tracks.iter().map(|t| s.track(*t).unwrap().name.as_str()).collect();
        assert_eq!(names, ["Kick", "Snare", "Low", "Pad"], "{os:?}");
        assert_eq!((s.edit.selection.start, s.edit.selection.end), (48_000, 96_000), "{os:?}");
    }
}

#[test]
fn alt_shift_and_shift_right_keys_work_the_playlists() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        let first = app.engine.session().track(kick).unwrap().clips()[0].range();
        let third = first.len() / 3;
        let select = |app: &mut SoundApp, part: i64| {
            let start = first.start + part * third;
            app.run("edit.select", json!({"tracks": ["Kick"], "start": start, "end": start + third})).unwrap();
        };
        select(&mut app, 0);
        let playlists = |app: &SoundApp| {
            let t = app.engine.session().track(kick).unwrap();
            (t.playlists.iter().map(|p| p.clips.len()).collect::<Vec<_>>(), t.active_playlist)
        };
        let main = playlists(&app).0[0];

        press(&ctx, &mut app, Key::ArrowDown, ALT_SHIFT);
        assert_eq!(playlists(&app), (vec![main, 1], 1), "{os:?}: copied to a new playlist, now shown");
        press(&ctx, &mut app, Key::ArrowRight, Modifiers { shift: true, ..cmd });
        app.run("track.playlist_select", json!({"index": 0})).unwrap();
        select(&mut app, 1);
        press(&ctx, &mut app, Key::ArrowUp, ALT_SHIFT);
        assert_eq!(playlists(&app).0[1], 2, "{os:?}: copied to the target playlist");
        select(&mut app, 2);
        press(&ctx, &mut app, Key::T, ALT_SHIFT);
        assert_eq!(playlists(&app).0[1], 3, "{os:?}: moved to the target playlist");
        select(&mut app, 0);
        press(&ctx, &mut app, Key::N, ALT_SHIFT);
        assert_eq!(playlists(&app).0.len(), 3, "{os:?}: moved to a new playlist");
        press(&ctx, &mut app, Key::ArrowRight, Modifiers::SHIFT);
        assert_eq!(playlists(&app).1, 1, "{os:?}: the target playlist is shown");
    }
}

#[test]
fn f12_records_ctrl_tab_goes_back_off_macos_and_cmd_alt_tab_toggles_tab_to_transient() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        press(&ctx, &mut app, Key::F12, Modifiers::NONE);
        assert_eq!(app.engine.transport_requests.last(), Some(&TransportRequest::Record), "{os:?}");

        let clip = app.engine.session().track_by_name("Kick").unwrap().clips()[0].range();
        let mid = clip.start + clip.len() / 2;
        app.run("edit.select", json!({"tracks": ["Kick"], "start": mid, "end": mid})).unwrap();
        press(&ctx, &mut app, Key::Tab, Modifiers::NONE);
        assert_eq!(start(&app), clip.end, "{os:?}");
        let mac = os == OperatingSystem::Mac;
        press(&ctx, &mut app, Key::Tab, if mac { Modifiers::CTRL } else { cmd });
        assert_eq!(start(&app), if mac { clip.end } else { clip.start }, "{os:?}: Ctrl+Tab");
        app.run("edit.select", json!({"tracks": ["Kick"], "start": mid, "end": mid})).unwrap();
        press(&ctx, &mut app, Key::Tab, Modifiers::ALT);
        assert_eq!(start(&app), clip.start, "{os:?}: Alt+Tab");

        let transient = app.engine.session().edit.tab_to_transient;
        let at = start(&app);
        press(&ctx, &mut app, Key::Tab, Modifiers { alt: true, ..cmd });
        assert_eq!((app.engine.session().edit.tab_to_transient, start(&app)), (!transient, at), "{os:?}");
    }
}

#[test]
fn window_keys_open_their_windows() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        for key in [Key::Num7, Key::Num6, Key::Equals, Key::Quote] {
            press(&ctx, &mut app, key, Modifiers::ALT);
        }
        press(&ctx, &mut app, Key::K, Modifiers::SHIFT);
        press(&ctx, &mut app, Key::J, Modifiers { alt: true, ..cmd });
        let ui = &app.ui;
        assert!(
            ui.show_universe && ui.show_clip_effects && ui.show_event_list && ui.show_task_manager && ui.show_midi_keyboard && ui.show_configurations,
            "{os:?}"
        );
        for key in [Key::I, Key::J] {
            app.run("window.workspace_close", json!({})).unwrap();
            press(&ctx, &mut app, key, Modifiers::ALT);
            assert!(app.ui.show_workspace, "{os:?}: Alt+{key:?}");
        }
    }
}

/// Modified arrow keys stay with the global shortcuts, while an unmodified arrow still moves the selected note.
#[test]
fn the_piano_roll_leaves_modified_arrows_to_the_shortcuts() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let ctx = egui::Context::default();
    let frame = |app: &mut SoundApp, events: Vec<Event>| {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear();
    };
    for _ in 0..6 {
        frame(&mut app, vec![]);
    }
    let body = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("soundcraft-midi-body"))).unwrap();
    frame(&mut app, vec![Event::PointerMoved(body.center())]);
    app.midi.selected = vec![0];
    let pitch = |app: &SoundApp| {
        let clip = soundcraft_ui_egui::midi_editor::target_clip(app).unwrap();
        match &app.engine.session().find_clip(clip).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes[0].pitch,
            _ => panic!("not a MIDI clip"),
        }
    };
    let before = pitch(&app);
    for modifiers in [ALT_SHIFT, Modifiers::ALT, MAC_CMD, Modifiers::CTRL] {
        frame(&mut app, keys(Key::ArrowUp, modifiers));
    }
    frame(&mut app, keys(Key::ArrowRight, Modifiers::SHIFT));
    assert_eq!(pitch(&app), before);
    frame(&mut app, keys(Key::ArrowUp, Modifiers::NONE));
    assert_eq!(pitch(&app), before + 1);
}

/// Shift+Up moves the selection to the same note an octave above, past intervening notes in the chord.
#[test]
fn the_piano_roll_selection_follows_notes_the_arrows_move_past_others() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let ctx = egui::Context::default();
    let frame = |app: &mut SoundApp, events: Vec<Event>| {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
        ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear();
    };
    for _ in 0..6 {
        frame(&mut app, vec![]);
    }
    let body = ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("soundcraft-midi-body"))).unwrap();
    frame(&mut app, vec![Event::PointerMoved(body.center())]);
    let notes = |app: &SoundApp| {
        let clip = soundcraft_ui_egui::midi_editor::target_clip(app).unwrap();
        match &app.engine.session().find_clip(clip).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.clone(),
            _ => panic!("not a MIDI clip"),
        }
    };
    // The first chord's lowest note, an octave up, sorts after the other chord notes.
    let first = notes(&app)[0];
    app.midi.selected = vec![0];
    for octaves in 1..=2u8 {
        frame(&mut app, keys(Key::ArrowUp, Modifiers::SHIFT));
        let &[i] = app.midi.selected.as_slice() else { panic!("selection {:?}", app.midi.selected) };
        assert_ne!(i, 0);
        assert_eq!((notes(&app)[i].start, notes(&app)[i].pitch), (first.start, first.pitch + 12 * octaves));
    }
}

#[test]
fn shift_tab_extends_the_selection_and_tab_to_transient_stops_on_hits() {
    for (os, cmd) in OSES {
        let (ctx, mut app) = rig(os);
        let sel = |app: &SoundApp| {
            let r = app.engine.session().edit.selection;
            (r.start, r.end)
        };
        let clip = app.engine.session().track_by_name("Kick").unwrap().clips()[0].range();
        let mid = clip.start + clip.len() / 2;
        app.run("edit.select", json!({"tracks": ["Kick"], "start": mid, "end": mid})).unwrap();
        press(&ctx, &mut app, Key::Tab, Modifiers::SHIFT);
        assert_eq!(sel(&app), (mid, clip.end), "{os:?}");
        let back_extend = if os == OperatingSystem::Mac { ALT_SHIFT } else { Modifiers { shift: true, ..cmd } };
        press(&ctx, &mut app, Key::Tab, back_extend);
        assert_eq!(sel(&app), (clip.start, clip.end), "{os:?}");

        press(&ctx, &mut app, Key::Tab, Modifiers { alt: true, ..cmd });
        app.run("edit.select", json!({"start": clip.start})).unwrap();
        press(&ctx, &mut app, Key::Tab, Modifiers::NONE);
        let hit = start(&app);
        assert!(hit > clip.start + 480 && hit < clip.end, "{os:?}: {hit}");
        press(&ctx, &mut app, Key::Tab, if os == OperatingSystem::Mac { Modifiers::ALT } else { cmd });
        assert_eq!(start(&app), clip.start, "{os:?}");
    }
}
