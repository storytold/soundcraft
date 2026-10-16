//! Master insert-order controls use the command transaction and remain visible in both strip widths.

use egui::{Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, MasterInsertOrder, Session, TrackId, TrackKind};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};
use std::sync::Arc;

fn fixture(narrow: bool) -> (SoundApp, TrackId, [TrackId; 2]) {
    let mut session = Session::default();
    let audio = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Audio"));
    let first = session.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("First master"));
    let second = session.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Second master"));
    session.edit.selected_tracks = vec![second];
    let mut app = SoundApp::new(Engine::new(session), None, Services::default());
    app.ui.window = MainWindow::Mix;
    app.ui.narrow_mix = narrow;
    (app, audio, [first, second])
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn choose_first(h: &mut Harness<SoundApp>, label: &str) {
    assert_eq!(h.query_all_by_label_contains("Master insert order").count(), 2);
    h.query_all_by_label_contains("Master insert order").next().unwrap().click();
    h.run_steps(3);
    assert!(h.query_by_label("Pre-fader").is_some());
    assert!(h.query_by_label("Post-fader").is_some());
    h.get_by_label(label).click();
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
}

fn menu_action(h: &mut Harness<SoundApp>, label: &str) {
    h.get_by_label("Edit").click();
    h.run_steps(3);
    assert_eq!(h.query_all_by_label_contains(label).count(), 1);
    h.get_by_label_contains(label).click();
    h.run_steps(3);
}

fn order(h: &Harness<SoundApp>, id: TrackId) -> MasterInsertOrder {
    h.state().engine.session().track(id).unwrap().mixer.master_insert_order
}

fn visible_labels_fit(h: &Harness<SoundApp>, narrow: bool, expected: &[&str]) {
    let selectors: Vec<Rect> = h.query_all_by_label_contains("Master insert order").map(|node| node.rect()).collect();
    assert_eq!(selectors.len(), 2);
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0));
    for rect in &selectors {
        assert!(viewport.contains_rect(*rect), "selector is outside the ordinary Mix viewport: {rect:?}");
        assert_eq!(rect.width(), soundcraft_ui_egui::mix_window::strip_width(narrow) - 8.0);
        assert_eq!(rect.height(), 18.0);
    }
    let labels: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if expected.contains(&text.galley.job.text.as_str()) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(labels.len(), expected.len());
    for (label, expected) in labels.iter().zip(expected) {
        assert_eq!(label.galley.job.text, *expected);
        assert!(!label.galley.elided, "{expected} was elided");
        assert!(selectors.iter().any(|rect| rect.contains_rect(label.visual_bounding_rect())), "{expected} is clipped");
    }
}

#[test]
fn master_popup_targets_its_strip_and_supports_pointer_undo_redo_in_both_widths() {
    for narrow in [false, true] {
        let (app, audio, masters) = fixture(narrow);
        let mut h = harness(app);
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PreFader);
        assert_eq!(order(&h, masters[1]), MasterInsertOrder::PreFader);
        assert!(h.state().engine.undo_history().is_empty());
        visible_labels_fit(&h, narrow, if narrow { &["pre", "pre"] } else { &["ins pre", "ins pre"] });

        choose_first(&mut h, "Post-fader");
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PostFader);
        assert_eq!(order(&h, masters[1]), MasterInsertOrder::PreFader);
        assert_eq!(order(&h, audio), MasterInsertOrder::PreFader);
        assert_eq!(h.state().engine.session().edit.selected_tracks, vec![masters[1]]);
        assert_eq!(h.state().engine.undo_history(), vec!["Set Master Insert Order"]);
        visible_labels_fit(&h, narrow, if narrow { &["post", "pre"] } else { &["ins post", "ins pre"] });

        menu_action(&mut h, "Undo Set Master Insert Order");
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PreFader);
        assert!(h.state().engine.undo_history().is_empty());
        assert!(h.state().engine.can_redo());
        menu_action(&mut h, "Redo Set Master Insert Order");
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PostFader);
        assert_eq!(h.state().engine.undo_history(), vec!["Set Master Insert Order"]);
        assert!(!h.state().engine.can_redo());

        choose_first(&mut h, "Pre-fader");
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PreFader);
        assert_eq!(order(&h, masters[1]), MasterInsertOrder::PreFader);
        assert_eq!(h.state().engine.undo_history().len(), 2);
        menu_action(&mut h, "Undo Set Master Insert Order");
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PostFader);
        assert_eq!(h.state().engine.session().edit.selected_tracks, vec![masters[1]]);
    }
}

#[test]
fn choosing_the_current_master_order_preserves_document_and_pending_redo() {
    for narrow in [false, true] {
        let (mut app, _, masters) = fixture(narrow);
        app.engine.execute("mix.volume", &json!({"track": masters[0], "db": -3})).unwrap();
        assert!(app.engine.undo());
        app.engine.mark_clean();
        let mut h = harness(app);
        let before = h.state().engine.session_arc();
        let state = (
            h.state().engine.revision,
            h.state().engine.is_dirty(),
            h.state().engine.undo_history(),
            h.state().engine.redo_label().map(str::to_owned),
        );
        let journal = h.state().engine.journal.len();
        assert!(h.state().engine.can_redo());
        choose_first(&mut h, "Pre-fader");
        assert!(Arc::ptr_eq(&before, &h.state().engine.session_arc()));
        assert_eq!(
            (
                h.state().engine.revision,
                h.state().engine.is_dirty(),
                h.state().engine.undo_history(),
                h.state().engine.redo_label().map(str::to_owned)
            ),
            state
        );
        assert!(h.state().engine.can_redo());
        assert_eq!(h.state().engine.journal.len(), journal + 1);
        assert_eq!(
            h.state().engine.journal.last().unwrap(),
            &("mix.master_insert_order".into(), json!({"track": masters[0].0, "order": "pre_fader"}))
        );
        assert_eq!(h.state().engine.session().edit.selected_tracks, vec![masters[1]]);
        menu_action(&mut h, "Redo Set Volume");
        assert_eq!(h.state().engine.session().track(masters[0]).unwrap().mixer.volume_db, -3.0);
        assert_eq!(order(&h, masters[0]), MasterInsertOrder::PreFader);
    }
}

#[test]
fn ordinary_strips_keep_solo_mute_without_a_master_order_selector() {
    for narrow in [false, true] {
        let mut session = Session::default();
        for kind in [TrackKind::Audio, TrackKind::Aux, TrackKind::Instrument] {
            session.add_track(kind, ChannelFormat::Stereo, None);
        }
        let mut app = SoundApp::new(Engine::new(session), None, Services::default());
        app.ui.window = MainWindow::Mix;
        app.ui.narrow_mix = narrow;
        let h = harness(app);
        assert_eq!(h.query_all_by_label_contains("Master insert order").count(), 0);
        for label in ["S", "M"] {
            assert_eq!(
                h.output().shapes.iter().filter(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == label)).count(),
                3,
                "ordinary strips retain their {label} keys"
            );
        }
        assert!(h.state().engine.undo_history().is_empty());
    }
}
