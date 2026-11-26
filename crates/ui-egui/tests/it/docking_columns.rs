//! The side lists in dock columns: the default shapes, saved layouts and panels reopening in
//! their dock.
use craft_behavior::DockSide;
use craft_behavior::dock::{Column, DockEdge, Group, Node, SplitSize};
use craft_behavior::flow::Axis;
use egui::{Rect, accesskit::Role, pos2, vec2};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::docking::{self, CLIPS_WIDTH, MIDI_HEIGHT, Panel, TRACKS_WIDTH};
use soundcraft_ui_egui::{Services, SoundApp, UiState};

use super::docking_workflows::harness;

fn tabs(panel: Panel) -> Node<Panel> {
    Node::tabs(vec![panel], 0)
}

fn column(panels: Vec<Panel>, active: usize, width: f32) -> Column<Panel> {
    Column::new(vec![Group::new(panels, active)]).with_width(width)
}

fn dock(side: DockEdge, panel: Panel, width: f32) -> Node<Panel> {
    Node::dock(side, vec![column(vec![panel], 0, width)])
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

/// The columns of the dock on `side`.
fn columns(node: &Node<Panel>, side: DockEdge) -> Option<&Vec<Column<Panel>>> {
    match node {
        Node::Split { first, second, .. } => columns(first, side).or_else(|| columns(second, side)),
        Node::Dock { side: found, columns, .. } if *found == side => Some(columns),
        _ => None,
    }
}

fn dock_columns(h: &egui_kittest::Harness<'_, SoundApp>, side: DockEdge) -> Vec<Column<Panel>> {
    h.state().ui.docking.layout().root.as_ref().and_then(|root| columns(root, side)).cloned().unwrap_or_default()
}

#[test]
fn default_layouts_put_the_side_lists_in_full_height_dock_columns() {
    let edit = docking::edit_default();
    let session = Node::split(Axis::Y, SplitSize::FixedSecond(MIDI_HEIGHT + 28.0), tabs(Panel::Canvas), tabs(Panel::Midi));
    let expected = Node::split(
        Axis::X,
        SplitSize::FixedFirst(TRACKS_WIDTH),
        dock(DockEdge::Start, Panel::Tracks, TRACKS_WIDTH),
        Node::split(Axis::X, SplitSize::FixedSecond(CLIPS_WIDTH), session, dock(DockEdge::End, Panel::Clips, CLIPS_WIDTH)),
    );
    assert_eq!(edit.layout().root, Some(expected));
    assert_eq!((TRACKS_WIDTH, CLIPS_WIDTH, MIDI_HEIGHT), (168.0, 230.0, 320.0));
    let mix = docking::mix_default();
    assert_eq!(
        mix.layout().root,
        Some(Node::split(Axis::X, SplitSize::FixedFirst(TRACKS_WIDTH), dock(DockEdge::Start, Panel::Tracks, TRACKS_WIDTH), tabs(Panel::Canvas)))
    );
    for workspace in [&edit, &mix] {
        let floating: Vec<_> = workspace.layout().floating.iter().map(|window| (window.panels().copied().collect::<Vec<_>>(), window.rect)).collect();
        assert_eq!(floating, [(vec![Panel::Memory], rect(160.0, 120.0, 480.0, 280.0)), (vec![Panel::History], rect(680.0, 120.0, 260.0, 300.0))]);
        assert!(workspace.hidden().is_empty());
    }
    docking::validate(&edit).unwrap();
    docking::validate_mix(&mix).unwrap();

    // Drawn: the docks run the window's height at its sides, and the MIDI editor sits under the
    // session view between them.
    let width = 1400.0;
    let mut h = harness(Some(UiState { show_midi_editor: true, ..Default::default() }), width, 1.0);
    h.run_steps(3);
    let tracks = h.get_by_role_and_label(Role::Tab, "Tracks and Groups").rect();
    let clips = h.get_by_role_and_label(Role::Tab, "Clips").rect();
    let midi = h.ctx.data(|data| data.get_temp::<Rect>(egui::Id::unique("soundcraft-midi-body"))).unwrap();
    assert!(tracks.left() < TRACKS_WIDTH && clips.right() > width - CLIPS_WIDTH, "tracks {tracks:?}, clips {clips:?}");
    assert!(midi.left() >= TRACKS_WIDTH && midi.right() <= width - CLIPS_WIDTH, "midi {midi:?}");
    assert!(midi.height() > MIDI_HEIGHT / 2.0 && midi.top() > clips.bottom(), "midi {midi:?}");
}

#[test]
fn a_panel_without_a_recorded_place_reopens_at_its_default_place() {
    use craft_behavior::dock::{Action, DockPlacement, Permissions};
    let close = |state: &mut UiState, panel: Panel| {
        state.docking.apply(Action::close(panel), false, |_| Permissions::default()).unwrap();
        state.docking.forget_hidden(|_| true);
    };
    // Both docks gone: each list makes its dock again, at its default width. The MIDI editor
    // opens under the session view, and Memory Locations and Undo History float where they
    // start.
    let mut state = UiState::default();
    for panel in [Panel::Tracks, Panel::Clips, Panel::Midi, Panel::Memory, Panel::History] {
        close(&mut state, panel);
    }
    state.show_tracks_list = false;
    state.show_clip_list = false;
    let mut h = harness(Some(state), 1400.0, 1.0);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"tracks"})).unwrap();
    h.state_mut().run("window.clip_list", json!({})).unwrap();
    for panel in ["midi", "memory", "history"] {
        h.state_mut().run("ui.dock", json!({"operation":"open","panel":panel})).unwrap();
    }
    h.run_steps(4);
    assert_eq!(dock_columns(&h, DockEdge::Start), [column(vec![Panel::Tracks], 0, TRACKS_WIDTH)]);
    assert_eq!(dock_columns(&h, DockEdge::End), [column(vec![Panel::Clips], 0, CLIPS_WIDTH)]);
    let layout = h.state().ui.docking.layout();
    let midi = layout.location(&Panel::Midi).unwrap();
    assert_eq!((midi.anchor, midi.placement), (Some(Panel::Canvas), DockPlacement::Split(DockSide::Bottom)));
    let floats = |panel: Panel| layout.floating.iter().find(|window| window.contains(&panel)).map(|window| window.rect);
    assert_eq!((floats(Panel::Memory), floats(Panel::History)), (Some(rect(160.0, 120.0, 480.0, 280.0)), Some(rect(680.0, 120.0, 260.0, 300.0))));
    let ui = &h.state().ui;
    assert!(ui.show_tracks_list && ui.show_clip_list && ui.show_midi_editor && ui.show_memory_locations && ui.show_undo_history);
    docking::validate(&h.state().ui.docking).unwrap();

    // A dock that is still shown takes the list into its first group.
    let mut state = UiState::default();
    state
        .docking
        .apply(Action::move_beside(Panel::History, Panel::Tracks, DockPlacement::group(true, None)), false, |_| Permissions::default())
        .unwrap();
    close(&mut state, Panel::Tracks);
    state.show_tracks_list = false;
    state.show_undo_history = true;
    let mut h = harness(Some(state), 1400.0, 1.0);
    h.state_mut().run("window.track_list", json!({})).unwrap();
    h.run_steps(4);
    let groups: Vec<_> = dock_columns(&h, DockEdge::Start).iter().flat_map(|column| column.groups.iter().map(|group| group.panels.clone())).collect();
    assert_eq!(groups, [vec![Panel::History, Panel::Tracks]]);
    assert_eq!(h.state().ui.docking.layout().panel_state(&Panel::Tracks), craft_behavior::dock::PanelState::Front);
}

/// A hidden list or editor whose recorded anchor has disappeared reopens at its default place
/// instead of where craft-ui opens a panel it cannot restore.
#[test]
fn a_recorded_place_whose_anchor_is_gone_reopens_at_its_default_place() {
    use craft_behavior::dock::{Action, DockPlacement, Permissions};
    let allow = |_: &Panel| Permissions::default();
    let mut state = UiState::default();
    // Stale saved locations: their recorded anchor closed without leaving a place to follow.
    for panel in [Panel::Clips, Panel::Midi, Panel::Tracks] {
        state.docking.apply(Action::close(panel), false, allow).unwrap();
    }
    let mut hidden: Vec<_> = state.docking.hidden().iter().filter(|(panel, _)| *panel != Panel::Tracks).cloned().collect();
    for (_, location) in hidden.iter_mut().filter(|(panel, _)| matches!(panel, Panel::Clips | Panel::Midi)) {
        location.anchor = Some(Panel::Tracks);
        location.placement = DockPlacement::Split(DockSide::Start);
        location.sibling.clear();
        location.floating = None;
    }
    state.docking = docking::Workspace::from_parts(state.docking.layout().clone(), hidden, state.docking.floated().to_vec());
    docking::validate(&state.docking).unwrap();
    state.show_clip_list = false;
    state.show_midi_editor = false;
    state.show_tracks_list = false;
    let mut h = harness(Some(state), 1400.0, 1.0);
    assert!(!h.state().ui.docking.layout().contains(&Panel::Tracks), "app sync keeps the stale anchor absent");
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"midi"})).unwrap();
    h.run_steps(4);
    assert_eq!(dock_columns(&h, DockEdge::End), [column(vec![Panel::Clips], 0, CLIPS_WIDTH)]);
    let midi = h.state().ui.docking.layout().location(&Panel::Midi).unwrap();
    assert_eq!((midi.anchor, midi.placement), (Some(Panel::Canvas), DockPlacement::Split(DockSide::Bottom)));
    assert!(h.state().ui.show_clip_list && h.state().ui.show_midi_editor);
    docking::validate(&h.state().ui.docking).unwrap();
}

/// A workspace as releases before dock columns saved it: no layout version, the lists in split
/// frames, a v1 floating window and hidden places.
fn split_frame_settings() -> serde_json::Value {
    let edit = json!({
        "layout": {
            "root": {"Split": {
                "axis": "Horizontal",
                "size": {"FixedFirst": 200.0},
                "first": {"Tabs": {"panels": ["tracks"], "active": 0}},
                "second": {"Tabs": {"panels": ["canvas"], "active": 0}}
            }},
            "floating": [{"panels": ["clips", "midi"], "active": 1, "rect": [300.0, 120.0, 420.0, 360.0]}]
        },
        "hidden": [
            ["memory", {"floating": [160.0, 120.0, 480.0, 280.0], "anchor": null, "placement": {"Tab": {"before": null}}, "size": null, "stack": null}],
            ["history", {"floating": null, "anchor": "canvas", "placement": {"Split": "Right"}, "size": {"FixedSecond": 260.0}, "stack": null, "sibling": ["canvas"]}]
        ]
    });
    let mix = json!({"layout": {"root": {"Split": {
        "axis": "Horizontal",
        "size": {"FixedFirst": 240.0},
        "first": {"Tabs": {"panels": ["tracks"], "active": 0}},
        "second": {"Tabs": {"panels": ["canvas"], "active": 0}}
    }}, "floating": []}});
    let mut settings =
        serde_json::to_value(UiState { theme: soundcraft_ui_egui::theme::ThemeMode::Light, narrow_mix: true, ..Default::default() }).unwrap();
    let fields = settings.as_object_mut().unwrap();
    fields.remove("docking_version");
    fields.insert("docking".into(), edit);
    fields.insert("mix_docking".into(), mix);
    settings
}

#[test]
fn split_frame_settings_load_as_the_dock_defaults_and_dock_layouts_round_trip() {
    let mut settings = split_frame_settings();
    // The old layout still parses: the reset is what replaces it.
    assert!(serde_json::from_value::<UiState>(settings.clone()).unwrap().docking.layout().contains(&Panel::Clips));
    docking::reset_old_layouts(&mut settings);
    let state: UiState = serde_json::from_value(settings).unwrap();
    assert_eq!(state.docking, docking::edit_default());
    assert_eq!(state.mix_docking, docking::mix_default());
    assert_eq!(state.docking_version, docking::LAYOUT_VERSION);
    assert_eq!(state.theme, soundcraft_ui_egui::theme::ThemeMode::Light);
    assert!(state.narrow_mix);

    // A configuration saved before dock columns resets the same way when recalled.
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[80,90,260,400]})).unwrap();
    app.ui.configurations = vec![("Old".into(), split_frame_settings())];
    soundcraft_ui_egui::windows::recall_configuration(&mut app, &split_frame_settings()).unwrap();
    assert_eq!(app.ui.docking, docking::edit_default());
    assert_eq!(app.ui.mix_docking, docking::mix_default());
    assert!(app.ui.narrow_mix && app.ui.configurations.len() == 1);

    // A dock layout comes back as it was saved, and drawing it does not rearrange it.
    let mut h = harness(Some(UiState { show_undo_history: true, ..Default::default() }), 1400.0, 1.0);
    h.state_mut().run("ui.dock", json!({"operation":"move","panel":"clips","target":"tracks","zone":"bottom"})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"float","panel":"history","rect":[500,200,260,300]})).unwrap();
    h.run_steps(4);
    let saved = h.state().ui.clone();
    let mut value = serde_json::to_value(&saved).unwrap();
    assert_eq!(value["docking_version"], json!(docking::LAYOUT_VERSION));
    docking::reset_old_layouts(&mut value);
    let loaded: UiState = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.docking, saved.docking);
    assert_eq!(loaded.mix_docking, saved.mix_docking);
    let groups: Vec<_> = columns(loaded.docking.layout().root.as_ref().unwrap(), DockEdge::Start)
        .unwrap()
        .iter()
        .flat_map(|column| column.groups.iter().map(|group| group.panels.clone()))
        .collect();
    assert_eq!(groups, [vec![Panel::Tracks], vec![Panel::Clips]]);
    let h = harness(Some(loaded), 1400.0, 1.0);
    assert_eq!(h.state().ui.docking, saved.docking);
}

#[test]
fn a_floating_list_closed_beside_its_shown_dock_reopens_floating() {
    let mut h = harness(None, 1400.0, 1.0);
    // Tracks joins Clips in the end dock, so the end dock stays shown after Clips floats.
    h.state_mut().run("ui.dock", json!({"operation":"move","panel":"tracks","target":"clips","zone":"center"})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"float","panel":"clips","rect":[300,200,260,320]})).unwrap();
    h.run_steps(4);
    let floated = h.state().ui.docking.layout().floating.iter().find(|w| w.contains(&Panel::Clips)).unwrap().rect;
    assert!(dock_columns(&h, DockEdge::End).iter().any(|c| c.groups.iter().any(|g| g.panels.contains(&Panel::Tracks))));
    h.state_mut().run("ui.dock", json!({"operation":"close","panel":"clips"})).unwrap();
    h.run_steps(4);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.docking.layout().floating.iter().find(|w| w.contains(&Panel::Clips)).map(|w| w.rect), Some(floated));
}
