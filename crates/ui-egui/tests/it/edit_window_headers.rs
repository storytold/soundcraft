use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{AutoParam, Track, TrackId};
use soundcraft_ui_egui::edit_window::HEADER_W;
use soundcraft_ui_egui::{Gesture, MainWindow, Services, SoundApp};

const CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };
const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };

/// Feed one harness frame through the raw-input hook before stepping, so scripted input reaches the app.
fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    let ctx = h.ctx.clone();
    h.state_mut().raw_input_hook(&ctx, &mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn press(h: &mut Harness<'_, SoundApp>, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
    h.state_mut().synthetic.extend([
        Event::ModifiersChanged(modifiers),
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button, pressed: true, modifiers },
        Event::PointerButton { pos, button, pressed: false, modifiers },
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    for _ in 0..10 {
        step(h);
    }
}

fn click(h: &mut Harness<'_, SoundApp>, pos: Pos2) {
    press(h, pos, PointerButton::Primary, Modifiers::NONE);
}

fn kick(h: &Harness<'_, SoundApp>) -> Track {
    h.state().engine.session().track_by_name("Kick").cloned().unwrap()
}

fn insert(h: &Harness<'_, SoundApp>, slot: usize) -> Option<String> {
    kick(h).mixer.inserts.get(slot).cloned().flatten().map(|i| i.plugin)
}

fn bypassed(h: &Harness<'_, SoundApp>, slot: usize) -> bool {
    kick(h).mixer.inserts.get(slot).cloned().flatten().is_some_and(|i| i.bypass)
}

fn harness(track: &str, height: &str, view: &str) -> (Harness<'static, SoundApp>, TrackId, [f32; 4]) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run(view, json!({})).unwrap();
    app.run("track.height", json!({"tracks": [track], "height": height})).unwrap();
    let id = app.engine.session().track_by_name(track).unwrap().id;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    let row = h.state().edit_layout.rows.iter().find(|(t, _)| *t == id.0).map(|(_, r)| *r).unwrap();
    (h, id, row)
}

#[test]
fn header_keys_freeze_silence_and_show_automation() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    let at = |x: f32, y: f32| pos2(row[0] + x, row[1] + y);

    click(&mut h, at(23.5, 65.5));
    assert!(kick(&h).frozen);

    click(&mut h, at(46.5, 65.5));
    let off = h.get_by_label("off").rect().center();
    click(&mut h, off);
    assert!(kick(&h).inactive);

    click(&mut h, at(7.5, row[3] - row[1] - 7.5));
    assert_eq!(kick(&h).view, "volume");

    click(&mut h, at(HEADER_W + 81.5, 29.5));
    assert_eq!(h.state().ui.window, MainWindow::Mix);
}

/// From the Edit window's insert column, clicking a plugin opens it, Cmd-click bypasses it, right-click changes or
/// removes it through the menu and an empty slot adds a plugin.
#[test]
fn edit_window_insert_slots_open_change_and_add_plugins() {
    let (mut h, kick, row) = harness("Kick", "medium", "view.edit_inserts_ae");
    let slot = |i: usize| pos2(row[0] + HEADER_W + 37.5, row[1] + 8.5 + i as f32 * 18.0);

    click(&mut h, slot(0));
    assert!(h.state().ui.plugin_windows.contains(&(kick, 0)));
    h.state_mut().ui.plugin_windows.clear();

    press(&mut h, slot(0), PointerButton::Primary, CMD);
    assert!(bypassed(&h, 0));
    assert!(h.state().ui.plugin_windows.is_empty());

    press(&mut h, slot(1), PointerButton::Secondary, Modifiers::NONE);
    let remove = h.get_by_label("no insert").rect().center();
    click(&mut h, remove);
    assert_eq!(insert(&h, 1), None);

    let plugin = soundcraft_dsp::plugins().iter().find(|p| !p.is_instrument).unwrap();
    click(&mut h, slot(2));
    let category = h.get_by_label(&format!("{:?}", plugin.category)).rect().center();
    h.state_mut().synthetic.push(Event::PointerMoved(category));
    for _ in 0..10 {
        step(&mut h);
    }
    let pick = h.get_by_label(plugin.name).rect().center();
    click(&mut h, pick);
    assert_eq!(insert(&h, 2).as_deref(), Some(plugin.id));
}

/// Short tracks draw insert slots as squares that still open, bypass and open the add-plugin menu.
#[test]
fn short_tracks_show_insert_slots_as_squares() {
    let (mut h, kick, row) = harness("Kick", "small", "view.edit_inserts_ae");
    let square = |i: usize| pos2(row[0] + HEADER_W + 9.0 + i as f32 * 14.0, row[1] + 8.5);

    click(&mut h, square(1));
    assert!(h.state().ui.plugin_windows.contains(&(kick, 1)));

    press(&mut h, square(0), PointerButton::Primary, CMD);
    assert!(bypassed(&h, 0));

    click(&mut h, square(4));
    let plugin = soundcraft_dsp::plugins().iter().find(|p| !p.is_instrument).unwrap();
    assert!(h.query_by_label(&format!("{:?}", plugin.category)).is_some());
}

#[test]
fn the_views_menu_stays_open_until_a_click_elsewhere() {
    let (mut h, _, _) = harness("Kick", "medium", "view.edit_inserts_ae");
    let views = h.get_by_label("Views").rect().center();
    click(&mut h, views);
    for column in ["Sends A-E", "I/O"] {
        let item = h.get_by_label(column).rect().center();
        click(&mut h, item);
    }
    let edit = &h.state().engine.session().edit;
    assert!(edit.flag("edit_view.sends_ae") && edit.flag("edit_view.io"));
    click(&mut h, pos2(800.0, 990.0));
    assert!(h.query_by_label("Sends A-E").is_none());
}

#[test]
fn the_io_columns_route_menus_stay_open_while_searching_buses() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    click(&mut h, pos2(row[0] + HEADER_W + 40.0, row[1] + 29.5));
    let bus = h.get_by_label("Bus").rect();
    let search = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .map(|node| node.rect())
        .filter(|r| r.min.y >= bus.max.y)
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap()
        .center();
    click(&mut h, search);
    assert!(h.query_by_label("no output").is_some(), "a click in the bus search closed the Output menu");
}

#[test]
fn only_a_sends_fader_icon_opens_its_level() {
    let (mut h, snare, row) = harness("Snare", "medium", "view.edit_sends_ae");
    let send_a = |x: f32| pos2(row[0] + HEADER_W + x, row[1] + 8.5);

    // The right of the name, where the level used to start, picks the destination.
    click(&mut h, send_a(50.0));
    assert!(h.query_by_label("DESTINATION").is_some());
    assert_eq!(h.state().ui.send_window, None);
    click(&mut h, pos2(800.0, 990.0));

    click(&mut h, send_a(65.5));
    assert_eq!(h.state().ui.send_window, Some((snare, 0)));
    assert!(h.query_by_label("DESTINATION").is_none());
}

/// The I/O column's "vol" and "pan" rows, for a track whose row is `row`.
fn vol_at(row: [f32; 4]) -> Pos2 {
    pos2(row[0] + HEADER_W + 40.0, row[1] + 52.5)
}

fn pan_at(row: [f32; 4]) -> Pos2 {
    pos2(row[0] + HEADER_W + 40.0, row[1] + 66.5)
}

fn undo_steps(h: &Harness<'_, SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn track(h: &Harness<'_, SoundApp>, name: &str) -> Track {
    h.state().engine.session().track_by_name(name).cloned().unwrap()
}

/// The open popup (the only foreground layer while one is open).
fn popup_rect(h: &Harness<'_, SoundApp>) -> Rect {
    let rects: Vec<Rect> = h.ctx.memory(|m| {
        m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Foreground).filter_map(|l| m.area_rect(l.id)).collect()
    });
    assert_eq!(rects.len(), 1, "expected exactly one popup, found {}", rects.len());
    rects[0]
}

/// Press at `from`, drag to `to` in four moves and release. `during` runs after each move, button down.
fn drag(h: &mut Harness<'_, SoundApp>, from: Pos2, to: Pos2, mut during: impl FnMut(&mut Harness<'_, SoundApp>)) {
    let modifiers = Modifiers::NONE;
    h.state_mut()
        .synthetic
        .extend([Event::PointerMoved(from), Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers }]);
    for _ in 0..6 {
        step(h);
    }
    for i in 1..=4 {
        h.state_mut().synthetic.push(Event::PointerMoved(from + (to - from) * (i as f32 / 4.0)));
        for _ in 0..3 {
            step(h);
        }
        during(h);
    }
    h.state_mut().synthetic.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    for _ in 0..6 {
        step(h);
    }
}

#[test]
fn alt_click_on_the_io_readouts_resets_volume_and_pan() {
    let (mut h, id, row) = harness("Kick", "medium", "view.edit_io");
    h.state_mut().run("mix.pan", json!({"track": id.0, "pan": 0.5})).unwrap();
    assert!(kick(&h).mixer.volume_db != 0.0 && kick(&h).mixer.pan == [0.5]);
    let before = undo_steps(&h);

    press(&mut h, vol_at(row), PointerButton::Primary, ALT);
    assert_eq!(kick(&h).mixer.volume_db, 0.0);
    press(&mut h, pan_at(row), PointerButton::Primary, ALT);
    assert_eq!(kick(&h).mixer.pan, [0.0]);

    assert_eq!(undo_steps(&h), before + 2, "each reset is one undo step");
    assert!(!egui::Popup::is_any_open(&h.ctx), "an Alt-click opened a popup instead of resetting");
}

#[test]
fn alt_click_on_a_stereo_tracks_pan_resets_both_panners_in_one_step() {
    let (mut h, id, row) = harness("Pad", "medium", "view.edit_io");
    for (index, pan) in [(0, -0.25), (1, 0.75)] {
        h.state_mut().run("mix.pan", json!({"track": id.0, "pan": pan, "index": index})).unwrap();
    }
    assert_eq!(track(&h, "Pad").mixer.pan, [-0.25, 0.75]);
    let before = undo_steps(&h);

    press(&mut h, pan_at(row), PointerButton::Primary, ALT);
    assert_eq!(track(&h, "Pad").mixer.pan, [-1.0, 1.0], "a stereo track goes back to hard left and right, like a new one");
    assert_eq!(undo_steps(&h), before + 1);
    assert!(!egui::Popup::is_any_open(&h.ctx));
}

#[test]
fn clicking_the_volume_readout_opens_a_fader_whose_drag_is_one_undo_step() {
    let (mut h, id, _) = harness("Kick", "medium", "view.edit_io");
    let db = kick(&h).mixer.volume_db;
    let before = undo_steps(&h);
    assert!(!egui::Popup::is_any_open(&h.ctx));

    // The readout is a button named after its track and its value.
    let readout = h.get_by_label("Kick volume -3.0").rect().center();
    click(&mut h, readout);
    assert!(egui::Popup::is_any_open(&h.ctx), "the click didn't open the fader");
    assert_eq!(kick(&h).mixer.volume_db, db, "opening the fader changed the volume");

    // The popup floats over the timeline: a drag in it must not also select or move anything there.
    let edit = h.state().engine.session().edit.clone();
    // The fader fills the bottom of the popup and moves by drag distance, so any point on it will do.
    let popup = popup_rect(&h);
    let from = pos2(popup.center().x, popup.max.y - 50.0);
    let mut gestures = 0;
    drag(&mut h, from, from - vec2(0.0, 60.0), |h| {
        assert!(matches!(h.state().gesture, Some(Gesture::Fader { track, .. }) if track == id), "the drag isn't a Fader gesture");
        gestures += 1;
    });
    assert_eq!(gestures, 4);
    assert!(kick(&h).mixer.volume_db > db, "dragging up didn't raise the volume");
    assert_eq!(undo_steps(&h), before + 1, "a whole drag is one undo step");
    let after = h.state().engine.session().edit.clone();
    assert!(after.selection == edit.selection && after.selected_tracks == edit.selected_tracks, "the drag leaked through to the timeline");
    assert!(h.state().gesture.is_none(), "the gesture outlived the drag");
    assert!(egui::Popup::is_any_open(&h.ctx), "releasing the fader closed it");

    h.state_mut().engine.undo();
    assert_eq!(kick(&h).mixer.volume_db, db, "one undo should restore the volume from before the drag");

    click(&mut h, pos2(800.0, 990.0));
    assert!(!egui::Popup::is_any_open(&h.ctx), "a click outside left the fader open");
}

#[test]
fn the_popup_fader_writes_touch_automation() {
    let (mut h, id, row) = harness("Kick", "medium", "view.edit_io");
    h.state_mut().run("mix.automation_mode", json!({"track": id.0, "mode": "touch"})).unwrap();
    click(&mut h, vol_at(row));
    let popup = popup_rect(&h);
    let from = pos2(popup.center().x, popup.max.y - 50.0);

    h.state_mut().engine.transport.playing = true;
    drag(&mut h, from, from - vec2(0.0, 60.0), |h| {
        // Touch writes at the playhead, which has to keep moving for a new point to be due.
        h.state_mut().engine.transport.position += 4800;
    });
    h.state_mut().engine.transport.playing = false;
    step(&mut h);

    let points = kick(&h).lane(&AutoParam::Volume).map_or(0, |lane| lane.points.len());
    assert!(points > 0, "dragging the popup fader in Touch mode wrote no automation");
}

#[test]
fn clicking_the_pan_readout_opens_one_slider_on_mono_and_two_on_stereo() {
    for (name, panners) in [("Kick", 1), ("Pad", 2)] {
        let (mut h, _, row) = harness(name, "medium", "view.edit_io");
        let before = undo_steps(&h);
        click(&mut h, pan_at(row));
        assert!(egui::Popup::is_any_open(&h.ctx), "{name}: the click didn't open the sliders");
        let sliders: Vec<_> = h.query_all_by_role(egui::accesskit::Role::Slider).collect();
        assert_eq!(sliders.len(), panners, "{name}: wrong number of pan sliders");
        assert_eq!(undo_steps(&h), before, "{name}: opening the sliders changed the session");

        // Drag the last panner's slider (R on stereo) by 30 points: a slider follows the pointer.
        let slider = sliders.last().unwrap().rect();
        let (from, to) = (slider.center(), slider.center() + vec2(30.0, 0.0));
        let index = panners - 1;
        let pan = track(&h, name).mixer.pan.clone();
        drag(&mut h, from, to, |_| {});
        let moved = track(&h, name).mixer.pan;
        assert_eq!(moved.len(), panners);
        assert!(moved[index] != pan[index], "{name}: the slider didn't move the panner: {pan:?} -> {moved:?}");
        assert_eq!(moved[..index], pan[..index], "{name}: moving R touched L");
        assert_eq!(undo_steps(&h), before + 1, "{name}: a slider drag is one undo step");
    }
}

#[test]
fn a_value_typed_in_the_fader_popup_sets_the_volume() {
    let (mut h, _, row) = harness("Kick", "medium", "view.edit_io");
    click(&mut h, vol_at(row));
    let popup = popup_rect(&h);
    click(&mut h, pos2(popup.center().x, popup.min.y + 14.0));
    let key = |key, modifiers| {
        [
            Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
            Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
        ]
    };
    h.state_mut().synthetic.extend(key(egui::Key::A, CMD));
    h.state_mut().synthetic.push(Event::Text("-12".into()));
    h.state_mut().synthetic.extend(key(egui::Key::Enter, Modifiers::NONE));
    for _ in 0..10 {
        step(&mut h);
    }
    assert_eq!(kick(&h).mixer.volume_db, -12.0);
}

#[test]
fn opening_the_pan_popup_leaves_a_pan_off_the_percent_grid_alone() {
    let (mut h, id, row) = harness("Kick", "medium", "view.edit_io");
    h.state_mut().run("mix.pan", json!({"track": id.0, "pan": 0.3337})).unwrap();
    let before = undo_steps(&h);
    click(&mut h, pan_at(row));
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert_eq!(kick(&h).mixer.pan, [0.3337], "opening the sliders rewrote the pan");
    assert_eq!(undo_steps(&h), before, "opening the sliders left an undo step");
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Slider, "Pan").is_some(), "the lone pan slider has no name");
}

#[test]
fn a_track_on_a_surround_output_keeps_a_text_only_pan() {
    let (mut h, id, row) = harness("Kick", "medium", "view.edit_io");
    h.state_mut().run("setup.main_format", json!({"format": "5.1"})).unwrap();
    h.state_mut().run("mix.pan", json!({"track": id.0, "pan": 0.5})).unwrap();
    for _ in 0..4 {
        step(&mut h);
    }
    let before = undo_steps(&h);
    click(&mut h, pan_at(row));
    assert!(!egui::Popup::is_any_open(&h.ctx), "the Mix window pans this track with a surround panner, not these sliders");
    press(&mut h, pan_at(row), PointerButton::Primary, ALT);
    assert_eq!(kick(&h).mixer.pan, [0.5]);
    assert_eq!(undo_steps(&h), before);
}
