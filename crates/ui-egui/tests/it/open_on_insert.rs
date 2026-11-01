//! #486: picking a plugin for an insert slot opens it straight away, as DAWs do, from the Mix
//! window's slots and the Edit window's Inserts column alike.

use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};

fn harness(engine: soundcraft_engine::Engine, window: MainWindow) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(engine, None, Services::default());
    app.ui.window = window;
    app.run("view.edit_inserts_ae", json!({})).unwrap();
    app.run("track.height", json!({"tracks": ["Vox"], "height": "medium"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1400.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn vox() -> soundcraft_engine::Engine {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    engine
}

fn vox_id(h: &Harness<SoundApp>) -> TrackId {
    h.state().engine.session().tracks[0].id
}

/// Two built-in effects from one category, and the category's menu label.
fn two_effects() -> (String, &'static soundcraft_dsp::PluginInfo, &'static soundcraft_dsp::PluginInfo) {
    let first = soundcraft_dsp::plugins().iter().find(|p| !p.is_instrument).unwrap();
    let other = soundcraft_dsp::plugins().iter().find(|p| p.category == first.category && p.id != first.id).unwrap();
    (format!("{:?}", first.category), first, other)
}

/// Opens `slot`'s menu (a click on an empty slot, a right-click on a filled one) and picks
/// `plugin` from its category.
fn pick(h: &mut Harness<SoundApp>, slot: &str, filled: bool, category: &str, plugin: &str) {
    let node = h.get_by_label(slot);
    if filled {
        node.click_secondary();
    } else {
        node.click();
    }
    h.run_steps(3);
    h.get_by_label(category).hover();
    h.run_steps(3);
    h.get_by_label(plugin).click();
    h.run_steps(3);
}

fn inserted(h: &Harness<SoundApp>) -> Option<String> {
    h.state().engine.session().tracks[0].mixer.inserts[0].as_ref().map(|i| i.plugin.clone())
}

/// Choosing a plugin for an empty insert inserts it and opens its window.
#[test]
fn picking_a_plugin_for_an_empty_insert_opens_it() {
    let (category, plugin, _) = two_effects();
    let mut h = harness(vox(), MainWindow::Mix);
    pick(&mut h, "Insert A for Vox", false, &category, plugin.name);
    assert_eq!(inserted(&h).as_deref(), Some(plugin.id));
    let id = vox_id(&h);
    assert_eq!(h.state().ui.plugin_windows, vec![(id, 0)], "the new insert's window opened");
}

/// A plugin chosen by typing a search and pressing Enter also opens its window.
#[test]
fn picking_from_the_search_opens_it_too() {
    let mut h = harness(vox(), MainWindow::Mix);
    h.get_by_label("Insert A for Vox").click();
    h.run_steps(3);
    h.event(egui::Event::Text("reverb".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(inserted(&h).is_some(), "Enter inserted a reverb");
    let id = vox_id(&h);
    assert_eq!(h.state().ui.plugin_windows, vec![(id, 0)]);
}

/// Replacing a filled insert inserts the new plugin and opens that plugin's window.
#[test]
fn replacing_a_filled_insert_opens_the_new_plugin() {
    let (category, first, second) = two_effects();
    let mut engine = vox();
    let track = engine.session().tracks[0].id;
    engine.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": first.id})).unwrap();
    let mut h = harness(engine, MainWindow::Mix);
    assert!(h.state().ui.plugin_windows.is_empty(), "nothing opens before a pick");
    pick(&mut h, "Insert A for Vox", true, &category, second.name);
    assert_eq!(inserted(&h).as_deref(), Some(second.id));
    assert_eq!(h.state().ui.plugin_windows, vec![(track, 0)]);
}

/// Choosing a plugin from the Edit window's insert column inserts it and opens its window.
#[test]
fn the_edit_windows_inserts_column_opens_it_too() {
    let (category, plugin, _) = two_effects();
    let mut h = harness(vox(), MainWindow::Edit);
    pick(&mut h, "Insert A for Vox", false, &category, plugin.name);
    assert_eq!(inserted(&h).as_deref(), Some(plugin.id));
    let id = vox_id(&h);
    assert_eq!(h.state().ui.plugin_windows, vec![(id, 0)]);
}
