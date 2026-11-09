//! The side lists in dock columns: the default shapes, saved layouts, drops into a column and
//! panels reopening in their dock.
use craft_ui::docking::{Column, Divider, DockArea, Group, Hit, Node, Side};
use craft_ui::layout::{SplitAxis, SplitSize};
use egui::{Rect, accesskit::Role};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::docking::{self, CLIPS_WIDTH, MIDI_HEIGHT, Panel, TRACKS_WIDTH};
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, UiState};

use super::docking_workflows::{drag, harness};

fn tabs(panel: Panel) -> Node<Panel> {
    Node::Tabs { panels: vec![panel], active: 0 }
}

fn dock(side: Side, panel: Panel, width: f32) -> Node<Panel> {
    Node::Dock { side, columns: vec![Column::new(vec![Group::new(vec![panel], 0)], width)] }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

/// The columns of the dock on `side`.
fn columns(node: &Node<Panel>, side: Side) -> Option<&Vec<Column<Panel>>> {
    match node {
        Node::Split { first, second, .. } => columns(first, side).or_else(|| columns(second, side)),
        Node::Dock { side: found, columns } if *found == side => Some(columns),
        _ => None,
    }
}

fn dock_columns(h: &egui_kittest::Harness<'_, SoundApp>, side: Side) -> Vec<Column<Panel>> {
    h.state().ui.docking.layout.root.as_ref().and_then(|root| columns(root, side)).cloned().unwrap_or_default()
}

#[test]
fn default_layouts_put_the_side_lists_in_full_height_dock_columns() {
    let edit = docking::edit_default();
    let session = split(SplitAxis::Vertical, SplitSize::FixedSecond(MIDI_HEIGHT + 26.0), tabs(Panel::Canvas), tabs(Panel::Midi));
    let expected = split(
        SplitAxis::Horizontal,
        SplitSize::FixedFirst(TRACKS_WIDTH),
        dock(Side::Start, Panel::Tracks, TRACKS_WIDTH),
        split(SplitAxis::Horizontal, SplitSize::FixedSecond(CLIPS_WIDTH), session, dock(Side::End, Panel::Clips, CLIPS_WIDTH)),
    );
    assert_eq!(edit.layout.root, Some(expected));
    assert_eq!((TRACKS_WIDTH, CLIPS_WIDTH, MIDI_HEIGHT), (168.0, 230.0, 320.0));
    let mix = docking::mix_default();
    assert_eq!(
        mix.layout.root,
        Some(split(SplitAxis::Horizontal, SplitSize::FixedFirst(TRACKS_WIDTH), dock(Side::Start, Panel::Tracks, TRACKS_WIDTH), tabs(Panel::Canvas)))
    );
    for workspace in [&edit, &mix] {
        let floating: Vec<_> = workspace.layout.floating.iter().map(|window| (window.panels().copied().collect::<Vec<_>>(), window.rect)).collect();
        assert_eq!(floating, [(vec![Panel::Memory], [160.0, 120.0, 480.0, 280.0]), (vec![Panel::History], [680.0, 120.0, 260.0, 300.0])]);
        assert!(workspace.hidden.is_empty());
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
    assert!((midi.height() - MIDI_HEIGHT).abs() < 1.0 && midi.top() > clips.bottom(), "midi {midi:?}");
}

/// The Tracks group's body in the start dock: below its tab strip, down to the column's foot.
fn tracks_body(h: &egui_kittest::Harness<'_, SoundApp>) -> Rect {
    let hits = DockArea::<Panel>::hit_rects(&h.ctx, docking::area_id(MainWindow::Edit));
    let strip = hits
        .iter()
        .filter(|(hit, _)| matches!(hit, Hit::Tab(Panel::Tracks) | Hit::DragBar(Panel::Tracks)))
        .fold(Rect::NOTHING, |all, (_, rect)| all.union(*rect));
    // The column's inner edge runs the column's full height.
    let bottom =
        hits.iter().find(|(hit, _)| *hit == Hit::Divider(Divider::Column(Panel::Tracks))).map_or(f32::NEG_INFINITY, |(_, rect)| rect.bottom());
    assert!(strip.is_positive() && bottom > strip.bottom() + 100.0, "strip {strip:?}, bottom {bottom}, hits {hits:?}");
    Rect::from_min_max(egui::pos2(strip.left(), strip.bottom()), egui::pos2(strip.right(), bottom))
}

#[test]
fn a_tab_dropped_on_a_dock_group_joins_it_in_the_middle_and_stacks_below_in_the_lower_third() {
    let mut h = harness(None, 1400.0, 1.0);
    let body = tracks_body(&h);
    let clips = h.get_by_role_and_label(Role::Tab, "Clips").rect().center();
    drag(&mut h, clips, body.center());
    assert_eq!(
        dock_columns(&h, Side::Start),
        [Column::new(vec![Group { active: 1, ..Group::new(vec![Panel::Tracks, Panel::Clips], 0) }], TRACKS_WIDTH)]
    );
    assert!(dock_columns(&h, Side::End).is_empty(), "the emptied end dock goes");

    let mut h = harness(None, 1400.0, 1.0);
    let body = tracks_body(&h);
    let clips = h.get_by_role_and_label(Role::Tab, "Clips").rect().center();
    drag(&mut h, clips, egui::pos2(body.center().x, body.bottom() - body.height() / 6.0));
    let column = dock_columns(&h, Side::Start);
    let groups: Vec<_> = column.iter().flat_map(|column| column.groups.iter().map(|group| group.panels.clone())).collect();
    assert_eq!(groups, [vec![Panel::Tracks], vec![Panel::Clips]], "Clips stacks below Tracks in its column");
    assert_eq!(column.len(), 1);
    assert!(h.state().ui.show_clip_list);
    docking::validate(&h.state().ui.docking).unwrap();
}

#[test]
fn a_panel_without_a_recorded_place_reopens_at_its_default_place() {
    use craft_ui::docking::{Action, Permissions, Placement, Zone};
    let close = |state: &mut UiState, panel: Panel| {
        state.docking.apply(Action::Close { panel }, |_| Permissions::default()).unwrap();
        state.docking.hidden.clear();
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
    assert_eq!(dock_columns(&h, Side::Start), [Column::new(vec![Group::new(vec![Panel::Tracks], 0)], TRACKS_WIDTH)]);
    assert_eq!(dock_columns(&h, Side::End), [Column::new(vec![Group::new(vec![Panel::Clips], 0)], CLIPS_WIDTH)]);
    let layout = &h.state().ui.docking.layout;
    let midi = layout.location(&Panel::Midi).unwrap();
    assert_eq!((midi.anchor, midi.placement), (Some(Panel::Canvas), Placement::Split(Zone::Bottom)));
    let rect = |panel: Panel| layout.floating.iter().find(|window| window.contains(&panel)).map(|window| window.rect);
    assert_eq!((rect(Panel::Memory), rect(Panel::History)), (Some([160.0, 120.0, 480.0, 280.0]), Some([680.0, 120.0, 260.0, 300.0])));
    let ui = &h.state().ui;
    assert!(ui.show_tracks_list && ui.show_clip_list && ui.show_midi_editor && ui.show_memory_locations && ui.show_undo_history);
    docking::validate(&h.state().ui.docking).unwrap();

    // A dock that is still shown takes the list into its first group.
    let mut state = UiState::default();
    state
        .docking
        .apply(
            Action::Move {
                panel: Panel::History,
                anchor: Panel::Tracks,
                placement: craft_ui::docking::Placement::Group { below: true, height: None },
            },
            |_| Permissions::default(),
        )
        .unwrap();
    close(&mut state, Panel::Tracks);
    state.show_tracks_list = false;
    state.show_undo_history = true;
    let mut h = harness(Some(state), 1400.0, 1.0);
    h.state_mut().run("window.track_list", json!({})).unwrap();
    h.run_steps(4);
    let groups: Vec<_> = dock_columns(&h, Side::Start).iter().flat_map(|column| column.groups.iter().map(|group| group.panels.clone())).collect();
    assert_eq!(groups, [vec![Panel::History, Panel::Tracks]]);
    assert_eq!(h.state().ui.docking.layout.panel_state(&Panel::Tracks), craft_ui::docking::PanelState::Front);
}

/// A hidden list or editor whose recorded anchor has disappeared reopens at its default place
/// instead of where craft-ui opens a panel it cannot restore.
#[test]
fn a_recorded_place_whose_anchor_is_gone_reopens_at_its_default_place() {
    use craft_ui::docking::{Action, Permissions, Placement, Zone};
    let allow = |_: &Panel| Permissions::default();
    let mut state = UiState::default();
    // Stale saved locations: their recorded anchor closed without leaving a place to follow.
    for panel in [Panel::Clips, Panel::Midi, Panel::Tracks] {
        state.docking.apply(Action::Close { panel }, allow).unwrap();
    }
    state.docking.hidden.retain(|(panel, _)| *panel != Panel::Tracks);
    for (_, location) in state.docking.hidden.iter_mut().filter(|(panel, _)| matches!(panel, Panel::Clips | Panel::Midi)) {
        location.anchor = Some(Panel::Tracks);
        location.placement = Placement::Split(Zone::Left);
        location.sibling.clear();
        location.floating = None;
    }
    docking::validate(&state.docking).unwrap();
    state.show_clip_list = false;
    state.show_midi_editor = false;
    state.show_tracks_list = false;
    let mut h = harness(Some(state), 1400.0, 1.0);
    assert!(!h.state().ui.docking.layout.contains(&Panel::Tracks), "app sync keeps the stale anchor absent");
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"midi"})).unwrap();
    h.run_steps(4);
    assert_eq!(dock_columns(&h, Side::End), [Column::new(vec![Group::new(vec![Panel::Clips], 0)], CLIPS_WIDTH)]);
    let midi = h.state().ui.docking.layout.location(&Panel::Midi).unwrap();
    assert_eq!((midi.anchor, midi.placement), (Some(Panel::Canvas), Placement::Split(Zone::Bottom)));
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
    assert!(serde_json::from_value::<UiState>(settings.clone()).unwrap().docking.layout.contains(&Panel::Clips));
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
    let groups: Vec<_> = columns(loaded.docking.layout.root.as_ref().unwrap(), Side::Start)
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
    let rect = h.state().ui.docking.layout.floating.iter().find(|w| w.contains(&Panel::Clips)).unwrap().rect;
    assert!(dock_columns(&h, Side::End).iter().any(|c| c.groups.iter().any(|g| g.panels.contains(&Panel::Tracks))));
    h.state_mut().run("ui.dock", json!({"operation":"close","panel":"clips"})).unwrap();
    h.run_steps(4);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.docking.layout.floating.iter().find(|w| w.contains(&Panel::Clips)).map(|w| w.rect), Some(rect));
}
