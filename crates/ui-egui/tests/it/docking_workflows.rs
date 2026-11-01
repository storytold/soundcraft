//! Real Edit/Mix panel workflows, persistence, and command ownership without OS windows.
use egui::{Event, Modifiers, PointerButton, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, UiState, docking::Panel};

pub(crate) fn harness(state: Option<UiState>, width: f32, scale: f32) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name":"Voice"})).unwrap();
    if let Some(state) = state {
        app.ui = state;
    }
    let mut h = Harness::builder().with_size(vec2(width, 900.0)).with_pixels_per_point(scale).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

pub(crate) fn drag(h: &mut Harness<'_, SoundApp>, from: egui::Pos2, to: egui::Pos2) {
    h.event(Event::PointerMoved(from));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    for step in 1..=8 {
        h.event(Event::PointerMoved(from.lerp(to, step as f32 / 8.0)));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
}

/// A real panel drag, float, close and reopen keeps the recorded floating rectangle through a UI-state reload;
/// the Trim tool and record-arm controls still work, and a floating edit panel does not cover the mix strip used
/// after switching windows.
#[test]
fn real_drag_float_close_reopen_reload_retains_transport_and_mixer_commands() {
    let mut h = harness(None, 1400.0, 1.0);
    let source = h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").rect().center();
    let target = h.get_by_role_and_label(egui::accesskit::Role::Tab, "Tracks and Groups").rect().center();
    drag(&mut h, source, target);
    assert_eq!(h.state().ui.docking.layout.location(&Panel::Clips).unwrap().anchor, Some(Panel::Tracks));
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").click_button(PointerButton::Secondary);
    h.run_steps(3);
    h.get_by_label("Float Panel").click();
    h.run_steps(5);
    let rect = h.state().ui.docking.layout.floating.iter().find(|g| g.contains(&Panel::Clips)).unwrap().rect;
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").click_button(PointerButton::Secondary);
    h.run_steps(3);
    h.get_by_label("Close Panel").click();
    h.run_steps(4);
    assert!(!h.state().ui.show_clip_list);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.docking.layout.floating.iter().find(|g| g.contains(&Panel::Clips)).unwrap().rect, rect);
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let expected = serde_json::to_value(&h.state().ui.docking).unwrap();
    let mut reloaded = harness(Some(serde_json::from_str(&saved).unwrap()), 1400.0, 1.0);
    assert_eq!(serde_json::to_value(&reloaded.state().ui.docking).unwrap(), expected);
    reloaded.state_mut().run("ui.dock", json!({"operation":"move","panel":"clips","target":"tracks","zone":"center"})).unwrap();
    reloaded.run_steps(4);
    // A Tracks column too narrow for both tabs lists Tracks under the "Panels" overflow.
    if let Some(overflow) = reloaded.query_by_label("Panels") {
        overflow.click();
        reloaded.run_steps(3);
        reloaded.get_by_label("Tracks and Groups").click();
    } else {
        reloaded.get_by_role_and_label(egui::accesskit::Role::Tab, "Tracks and Groups").click();
    }
    reloaded.run_steps(3);
    assert_eq!(reloaded.state().ui.docking.layout.panel_state(&Panel::Tracks), craft_ui::docking::PanelState::Front);
    reloaded.get_by_label("Trim (F6)").click();
    reloaded.run_steps(3);
    assert_eq!(reloaded.state().engine.session().edit.tool, soundcraft_model::Tool::Trim);
    assert!(!reloaded.state().engine.transport.playing);
    let edit = serde_json::to_value(&reloaded.state().ui.docking).unwrap();
    reloaded.state_mut().ui.window = MainWindow::Mix;
    reloaded.run_steps(5);
    reloaded.state_mut().run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[900,70,220,420]})).unwrap();
    reloaded.run_steps(4);
    assert_eq!(serde_json::to_value(&reloaded.state().ui.docking).unwrap(), edit);
    assert!(!reloaded.state().ui.mix_docking.layout.contains(&Panel::Clips));
    let record = reloaded.get_by_label_contains("Record enable");
    // A real pointer click must remain outside the floating panel's occluding rectangle.
    assert!(!egui::Rect::from_min_size(egui::pos2(900.0, 70.0), vec2(220.0, 420.0)).contains(record.rect().center()));
    record.click();
    reloaded.run_steps(3);
    assert!(reloaded.state().engine.session().tracks[0].mixer.record_arm);
    assert!(!reloaded.state().engine.transport.recording);
}

#[test]
fn activating_a_hidden_panel_shows_it_and_its_window_flag() {
    let mut h = harness(None, 1400.0, 1.0);
    h.state_mut().run("ui.dock", json!({"operation":"close","panel":"tracks"})).unwrap();
    h.run_steps(4);
    assert!(!h.state().ui.show_tracks_list);
    h.state_mut().run("ui.dock", json!({"operation":"activate","panel":"tracks"})).unwrap();
    h.run_steps(4);
    assert!(h.state().ui.show_tracks_list);
    assert_eq!(h.state().ui.docking.layout.panel_state(&Panel::Tracks), craft_ui::docking::PanelState::Front);
}

#[test]
fn invalid_docking_requests_are_atomic_in_edit_and_mix() {
    let mut h = harness(None, 1400.0, 1.0);
    for window in [MainWindow::Edit, MainWindow::Mix] {
        h.state_mut().ui.window = window;
        h.run_steps(4);
        let before = serde_json::to_value(&h.state().ui).unwrap();
        for params in [
            json!({"operation":"close","panel":"canvas"}),
            json!({"operation":"move","panel":"tracks","target":"canvas","zone":"center"}),
            json!({"operation":"float","panel":"tracks","rect":[0,0,-1,100]}),
        ] {
            assert!(h.state_mut().run("ui.dock", params).is_err());
            assert_eq!(serde_json::to_value(&h.state().ui).unwrap(), before);
        }
        if window == MainWindow::Mix {
            assert!(h.state_mut().run("ui.dock", json!({"operation":"open","panel":"midi"})).is_err());
        }
        assert_eq!(h.state().engine.session().tracks.len(), 1);
    }
}

#[test]
fn docking_renders_edit_and_mix_default_floating_and_redocked() {
    for width in [1000.0, 1400.0] {
        for scale in [1.0, 2.0] {
            for window in [MainWindow::Edit, MainWindow::Mix] {
                let mut h = harness(Some(UiState { window, ..Default::default() }), width, scale);
                h.state_mut().engine = soundcraft_engine::demo::demo_engine();
                for state in ["default", "floating", "redocked"] {
                    if state == "floating" {
                        h.state_mut().run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[100,80,260,500]})).unwrap();
                    } else if state == "redocked" {
                        h.state_mut().run("ui.dock", json!({"operation":"move","panel":"tracks","target":"canvas","zone":"left"})).unwrap();
                    }
                    h.run_steps(5);
                    soundcraft_ui_egui::docking::validate(&h.state().ui.docking).unwrap();
                    soundcraft_ui_egui::docking::validate(&h.state().ui.mix_docking).unwrap();
                    if let Ok(directory) = std::env::var("CRAFT_UI_DOCKING_DIR") {
                        std::fs::create_dir_all(&directory).unwrap();
                        h.render().unwrap().save(format!("{directory}/sound-{window:?}-{width}-{scale}-{state}.png")).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn reset_is_enabled_and_resolves_in_native_and_in_window_menu_trees() {
    use soundcraft_ui_egui::menus;
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let path = "Window > Reset Panel Layout";
    let extra: Vec<_> = menus::UI_COMMANDS.iter().map(|(id, _, path, _)| (*path, *id)).collect();
    assert_eq!(menus::command_for_path(path, &extra).as_deref(), Some("ui.dock.reset"));
    let tree = menus::tree();
    let window = tree.iter().find(|node| node.label == "Window").unwrap();
    assert!(window.children.iter().any(|node| node.path == path));
    assert!(menus::item_state(&app, path, Some("ui.dock.reset")).0);
    app.run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[50,60,260,420]})).unwrap();
    menus::invoke_menu(&mut app, "ui.dock.reset", path);
    assert!(!app.ui.docking.layout.floating.iter().any(|group| group.contains(&Panel::Tracks)));
}

#[test]
fn legacy_preferences_restore_the_theme_and_validate_default_docking() {
    let mut value = serde_json::to_value(UiState { theme: soundcraft_ui_egui::theme::ThemeMode::Light, ..Default::default() }).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("docking");
    object.remove("mix_docking");
    let restored: UiState = serde_json::from_value(value).unwrap();
    assert_eq!(restored.theme, soundcraft_ui_egui::theme::ThemeMode::Light);
    soundcraft_ui_egui::docking::validate(&restored.docking).unwrap();
    soundcraft_ui_egui::docking::validate_mix(&restored.mix_docking).unwrap();
}

/// The tab group holding `panel` in the docked frames or a dock column.
fn docked_group(node: &craft_ui::docking::Node<Panel>, panel: Panel) -> Option<Vec<Panel>> {
    use craft_ui::docking::Node;
    match node {
        Node::Split { first, second, .. } => docked_group(first, panel).or_else(|| docked_group(second, panel)),
        Node::Tabs { panels, .. } if panels.contains(&panel) => Some(panels.clone()),
        Node::Dock { columns, .. } => {
            columns.iter().flat_map(|column| &column.groups).find(|group| group.panels.contains(&panel)).map(|group| group.panels.clone())
        }
        _ => None,
    }
}

#[test]
fn a_floating_window_saved_in_the_one_group_shape_keeps_it_when_written_back() {
    use craft_ui::docking::Node;
    // A floating window as one `{panels, active, rect}` group, as releases before the shared
    // docking model wrote it, beside the dock layout.
    let mut settings = serde_json::to_value(UiState { show_midi_editor: true, ..Default::default() }).unwrap();
    settings["docking"]["layout"]["floating"] = json!([{"panels": ["memory", "history"], "active": 1, "rect": [300.0, 120.0, 420.0, 360.0]}]);
    let state: UiState = serde_json::from_value(settings).unwrap();
    soundcraft_ui_egui::docking::validate(&state.docking).unwrap();
    let floating = &state.docking.layout.floating;
    assert_eq!(floating.len(), 1);
    assert_eq!(floating[0].root, Node::Tabs { panels: vec![Panel::Memory, Panel::History], active: 1 });
    let root = state.docking.layout.root.clone().unwrap();

    let mut state = state;
    state.show_memory_locations = true;
    state.show_undo_history = true;
    let h = harness(Some(state), 1400.0, 1.0);
    assert_eq!(h.state().ui.docking.layout.root.as_ref(), Some(&root), "drawing the loaded layout must not rearrange it");
    assert_eq!(h.state().ui.docking.layout.floating[0].rect, [300.0, 120.0, 420.0, 360.0]);
    let saved = serde_json::to_value(&h.state().ui.docking).unwrap();
    let window = saved["layout"]["floating"].as_array().unwrap().iter().find(|w| w["panels"] == json!(["memory", "history"])).unwrap();
    assert_eq!(window["active"], json!(1));
    assert_eq!(window["rect"], json!([300.0, 120.0, 420.0, 360.0]));
}

#[test]
fn dragging_a_group_by_its_drag_bar_joins_it_to_another_group() {
    use craft_ui::docking::{DockArea, Hit};
    let mut h = harness(None, 1400.0, 1.0);
    let root = h.state().ui.docking.layout.root.clone().unwrap();
    assert_eq!(docked_group(&root, Panel::Tracks), Some(vec![Panel::Tracks]));
    let hits = DockArea::<Panel>::hit_rects(&h.ctx, soundcraft_ui_egui::docking::area_id(MainWindow::Edit));
    let bar = hits.iter().find(|(hit, _)| *hit == Hit::DragBar(Panel::Clips)).expect("the Clips group has a drag bar").1;
    let target = h.get_by_role_and_label(egui::accesskit::Role::Tab, "Tracks and Groups").rect().center();
    drag(&mut h, bar.center(), target);
    let layout = &h.state().ui.docking.layout;
    let group = docked_group(layout.root.as_ref().unwrap(), Panel::Tracks).unwrap();
    assert_eq!(group.len(), 2, "the dropped group joins the Tracks tabs: {group:?}");
    assert!(group.contains(&Panel::Clips));
    assert!(h.state().ui.show_clip_list);
    soundcraft_ui_egui::docking::validate(&h.state().ui.docking).unwrap();
}

#[test]
fn malformed_dock_operations_name_the_field() {
    let mut h = harness(None, 1400.0, 1.0);
    for (params, error) in [
        (json!({"operation":"close"}), "panel is required"),
        (json!({"operation":"close","panel":"mixer"}), "Unknown panel: mixer"),
        (
            json!({"operation":"move","panel":"clips","target":"tracks","zone":"middle"}),
            "zone must be tab, center, left, right, top, bottom, above or below",
        ),
    ] {
        assert_eq!(h.state_mut().run("ui.dock", params).unwrap_err(), error);
    }
}

/// Panels hidden one by one reopen into their original floating window, rebuilding the hidden-anchor chain in the
/// process.
#[test]
fn panels_hidden_one_by_one_reopen_into_their_recorded_window() {
    use craft_ui::docking::{Action, Node, Permissions};
    let allow = |_: &Panel| Permissions::default();
    let mut state = UiState::default();
    state.docking.layout.floating.retain(|window| !window.contains(&Panel::History));
    let window = state.docking.layout.floating.iter_mut().find(|window| window.contains(&Panel::Memory)).unwrap();
    let rect = window.rect;
    window.root = Node::Tabs { panels: vec![Panel::Memory, Panel::History], active: 0 };
    state.docking.apply(Action::Close { panel: Panel::Memory }, allow).unwrap();
    state.docking.apply(Action::Close { panel: Panel::History }, allow).unwrap();
    let own = &mut state.docking.hidden.iter_mut().find(|(panel, _)| *panel == Panel::Memory).unwrap().1;
    assert_eq!(own.anchor, Some(Panel::History));
    // Only the outer hidden anchor retains a floating fallback, so the chain walk is required.
    own.floating = None;
    let own = own.clone();
    assert_eq!(state.docking.layout.clone().restore(Panel::Memory, &own), Err(craft_ui::docking::DockError::MissingTarget));
    state.show_memory_locations = false;
    state.show_undo_history = false;
    let mut h = harness(Some(state), 1400.0, 1.0);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"memory"})).unwrap();
    h.run_steps(4);
    let hidden_history = &h.state().ui.docking.hidden.iter().find(|(panel, _)| *panel == Panel::History).unwrap().1;
    assert_eq!(hidden_history.anchor, Some(Panel::Memory), "the restored chain reverses its hidden neighbour's anchor");
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"history"})).unwrap();
    h.run_steps(4);
    let layout = &h.state().ui.docking.layout;
    let window = layout.floating.iter().find(|window| window.contains(&Panel::Memory)).expect("Memory reopens in its window");
    assert!(window.contains(&Panel::History), "the hidden chain reopens History in Memory's window: {window:?}");
    assert_eq!(window.rect, rect);
    assert!(h.state().ui.show_memory_locations && h.state().ui.show_undo_history);
    soundcraft_ui_egui::docking::validate(&h.state().ui.docking).unwrap();
}
