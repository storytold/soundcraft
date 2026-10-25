use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{Track, TrackId};
use soundcraft_ui_egui::edit_window::HEADER_W;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

const CMD: Modifiers = Modifiers { mac_cmd: true, command: true, ..Modifiers::NONE };

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
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
    let category = h.get_by_label(&format!("{:?} ⏵", plugin.category)).rect().center();
    h.state_mut().synthetic.push(Event::PointerMoved(category));
    for _ in 0..10 {
        step(&mut h);
    }
    let pick = h.get_by_label(plugin.name).rect().center();
    click(&mut h, pick);
    assert_eq!(insert(&h, 2).as_deref(), Some(plugin.id));
}

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
    assert!(h.query_by_label(&format!("{:?} ⏵", plugin.category)).is_some());
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
