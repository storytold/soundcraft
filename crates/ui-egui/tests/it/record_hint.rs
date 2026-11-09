//! Record with nothing to record on explains itself at the Record buttons.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn record_in_an_empty_session_shows_why_at_both_record_buttons() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_transport = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    assert_eq!(h.query_all_by_label_contains("Nothing to record on").count(), 0);
    h.get_by_label("transport.record").click();
    h.run_steps(3);
    assert!(!h.state().engine.transport.recording);
    assert_eq!(h.query_all_by_label_contains("Nothing to record on").count(), 2, "the toolbar's and the Transport window's");
}
