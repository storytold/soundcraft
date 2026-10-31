use egui::os::OperatingSystem;
use egui::{Key, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn e_fits_the_selection_to_the_edit_window_and_the_button_goes_back() {
    for os in [OperatingSystem::Mac, OperatingSystem::Windows, OperatingSystem::Nix] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let bass = app.engine.session().track_by_name("Bass").unwrap().id.0;
        app.engine.execute("edit.select", &json!({"tracks": [bass], "start": 96_000, "end": 192_000})).unwrap();
        let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_os(os).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.run_steps(5);
        let view = |a: &SoundApp| (a.engine.session().edit.zoom, a.edit_layout.rows.clone(), a.edit_layout.scroll_y);
        let before = view(h.state());
        h.state_mut().engine.execute("options.keyboard_focus", &json!({"focus": "none"})).unwrap();
        h.key_press(Key::E);
        h.run_steps(1);
        assert_eq!(view(h.state()), before, "{os:?}");

        h.state_mut().engine.execute("options.keyboard_focus", &json!({"focus": "commands"})).unwrap();
        h.key_press(Key::E);
        h.run_steps(1);
        let [x0, y0, x1, y1] = h.state().edit_layout.timeline;
        let zoom = h.state().engine.session().edit.zoom;
        assert_eq!(zoom.scroll, 96_000, "{os:?}");
        assert!((f64::from(x1 - x0) * zoom.samples_per_px - 96_000.0).abs() < 1.0, "{os:?}");
        let row = h.state().edit_layout.rows.iter().find(|(id, _)| *id == bass).unwrap().1;
        assert_eq!(row[1], y0, "{os:?}");
        assert!((row[3] + 1.0 - y1).abs() < 1.0, "{os:?}: {row:?} in {y0}..{y1}");

        h.get_by_label("Zoom Toggle (E)").click();
        h.run_steps(1);
        assert_eq!(view(h.state()), before, "{os:?}");
    }
}
