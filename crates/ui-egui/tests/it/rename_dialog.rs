use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

#[test]
fn typing_in_the_rename_clip_dialog_replaces_the_old_name() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let clip = app.engine.session().track_by_name("Kick").unwrap().clips()[0].clone();
    app.run("edit.select", json!({"clips": [clip.id.0]})).unwrap();
    menus::invoke_menu(&mut app, "clip.rename", "Clip > Rename...");
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..3 {
        h.step();
    }
    h.input_mut().events.push(Event::Text("Intro".into()));
    h.step();
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    let session = h.state().engine.session();
    let renamed = session.tracks.iter().flat_map(|t| t.clips()).find(|c| c.id == clip.id).map(|c| c.name.clone());
    assert_eq!(renamed.as_deref(), Some("Intro"));
}
