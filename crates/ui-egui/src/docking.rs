//! Application panel identities and persistence around the shared docking renderer.
//! Document contents remain owned by their existing view implementations.

use craft_ui::docking::{
    Action, Column, DockArea, DockCommand, DockError, DockOperation, DockStyle, FloatSpace, Floating, Group, Layout, Location, Node, PanelLimits,
    Permissions, Placement, Side, Target, UiDock, Zone, ZoneName,
};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Canvas,
    Tracks,
    Clips,
    Midi,
    Memory,
    History,
}

impl Panel {
    pub const ALL: &[Self] = &[Self::Canvas, Self::Tracks, Self::Clips, Self::Midi, Self::Memory, Self::History];
    fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Session",
            Self::Tracks => "Tracks and Groups",
            Self::Clips => "Clips",
            Self::Midi => "MIDI Editor",
            Self::Memory => "Memory Locations",
            Self::History => "Undo History",
        }
    }
}

/// Width of the Tracks and Groups column in the start dock.
pub const TRACKS_WIDTH: f32 = 168.0;
/// Width of the Clips column in the end dock.
pub const CLIPS_WIDTH: f32 = 230.0;
/// Height of the MIDI editor's body under the session view.
pub const MIDI_HEIGHT: f32 = 320.0;

/// An Edit or Mix panel arrangement, saved with the UI preferences.
pub type Workspace = craft_ui::docking::Workspace<Panel>;

const MIDI_TAB_HEIGHT: f32 = 26.0;

/// The document view stays docked and open, and panels open in splits beside it.
fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Canvas { Permissions { accepts_splits: true, ..Permissions::PROTECTED } } else { Permissions::default() }
}

fn tabs(panel: Panel) -> Node<Panel> {
    Node::Tabs { panels: vec![panel], active: 0 }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

/// The dock area that draws the Edit or Mix workspace.
pub fn area_id(window: crate::MainWindow) -> egui::Id {
    egui::Id::unique(("soundcraft-docking", window == crate::MainWindow::Edit))
}

fn area(edit: bool) -> egui::Id {
    area_id(if edit { crate::MainWindow::Edit } else { crate::MainWindow::Mix })
}

/// A dock on `side` with one column holding `panel`.
fn dock(side: Side, panel: Panel, width: f32) -> Node<Panel> {
    Node::Dock { side, columns: vec![Column::new(vec![Group::new(vec![panel], 0)], width)] }
}

const MEMORY_RECT: [f32; 4] = [160.0, 120.0, 480.0, 280.0];
const HISTORY_RECT: [f32; 4] = [680.0, 120.0, 260.0, 300.0];

/// The floating windows both windows start with. Fixed ids: the default is rebuilt every frame
/// and must stay the same windows.
fn default_floating() -> Vec<Floating<Panel>> {
    vec![Floating::single(vec![Panel::Memory], 0, MEMORY_RECT).with_id(1), Floating::single(vec![Panel::History], 0, HISTORY_RECT).with_id(2)]
}

/// Where a panel that floats in the default layouts floats.
fn default_float(panel: Panel) -> Option<[f32; 4]> {
    match panel {
        Panel::Memory => Some(MEMORY_RECT),
        Panel::History => Some(HISTORY_RECT),
        _ => None,
    }
}

/// The dock side and column width a panel opens in when it has no recorded place.
fn default_dock(panel: Panel) -> Option<(Side, f32)> {
    match panel {
        Panel::Tracks => Some((Side::Start, TRACKS_WIDTH)),
        Panel::Clips => Some((Side::End, CLIPS_WIDTH)),
        _ => None,
    }
}

/// The Edit window's built-in arrangement: Tracks and Groups in a full-height start dock,
/// Clips in a full-height end dock, and the MIDI editor under the session view between them.
pub fn edit_default() -> Workspace {
    let session = split(SplitAxis::Vertical, SplitSize::FixedSecond(MIDI_HEIGHT + MIDI_TAB_HEIGHT), tabs(Panel::Canvas), tabs(Panel::Midi));
    Workspace::new(Layout {
        root: Some(split(
            SplitAxis::Horizontal,
            SplitSize::FixedFirst(TRACKS_WIDTH),
            dock(Side::Start, Panel::Tracks, TRACKS_WIDTH),
            split(SplitAxis::Horizontal, SplitSize::FixedSecond(CLIPS_WIDTH), session, dock(Side::End, Panel::Clips, CLIPS_WIDTH)),
        )),
        floating: default_floating(),
    })
}

/// The Mix window's built-in arrangement: Tracks and Groups in a start dock beside the mixer.
/// The timeline-only Clips and MIDI panels stay in Edit.
pub fn mix_default() -> Workspace {
    Workspace::new(Layout {
        root: Some(split(
            SplitAxis::Horizontal,
            SplitSize::FixedFirst(TRACKS_WIDTH),
            dock(Side::Start, Panel::Tracks, TRACKS_WIDTH),
            tabs(Panel::Canvas),
        )),
        floating: default_floating(),
    })
}

/// The version of the saved panel layouts, kept in [`crate::UiState::docking_version`]. Version 1
/// has Tracks and Groups and Clips in dock columns; settings without a version have split frames.
pub const LAYOUT_VERSION: u32 = 1;

/// Drop the panel layouts (with their hidden panels) from saved [`crate::UiState`] settings older
/// than [`LAYOUT_VERSION`], before they are read, so they load as the default layouts. Every other
/// setting stays. Preferences and window configurations go through this.
pub fn reset_old_layouts(saved: &mut Value) {
    let Some(fields) = saved.as_object_mut() else { return };
    if fields.get("docking_version").and_then(Value::as_u64).is_some_and(|version| version >= u64::from(LAYOUT_VERSION)) {
        return;
    }
    fields.remove("docking");
    fields.remove("mix_docking");
    fields.insert("docking_version".into(), json!(LAYOUT_VERSION));
}

/// The document view where the Edit window's rules keep it: docked outside the dock columns, alone in
/// its tab group. `None` when `node` does not hold it.
fn canvas_placement(node: &Node<Panel>) -> Option<Result<(), &'static str>> {
    match node {
        Node::Split { first, second, .. } => canvas_placement(first).or_else(|| canvas_placement(second)),
        Node::Tabs { panels, .. } if panels.contains(&Panel::Canvas) => {
            Some(if panels.len() == 1 { Ok(()) } else { Err("The document view cannot share a tab group") })
        }
        Node::Stack { entries } if entries.iter().any(|entry| entry.panel == Panel::Canvas) => Some(Err("The document view cannot be collapsed")),
        Node::Dock { columns, .. } if columns.iter().flat_map(|column| &column.groups).any(|group| group.panels.contains(&Panel::Canvas)) => {
            Some(Err("The document view cannot join a dock column"))
        }
        _ => None,
    }
}

/// Check a workspace against craft-ui's rules and SoundCraft's own: saved settings cannot
/// float, hide, collapse or share the protected document view.
pub fn validate(workspace: &Workspace) -> Result<(), String> {
    workspace.validate().map_err(|error| error.to_string())?;
    let layout = &workspace.layout;
    if !layout.contains(&Panel::Canvas) || workspace.hidden.len() > Panel::ALL.len() {
        return Err("The panel layout must contain its document view and bounded hidden panels".into());
    }
    if layout.floating.iter().any(|window| window.contains(&Panel::Canvas)) {
        return Err("The document view cannot float".into());
    }
    layout.root.as_ref().and_then(canvas_placement).unwrap_or(Err("The document view cannot float"))?;
    for (panel, location) in &workspace.hidden {
        if *panel == Panel::Canvas {
            return Err("A hidden panel has an invalid or duplicate identity".into());
        }
        if location.anchor == Some(Panel::Canvas) && matches!(location.placement, Placement::Tab { .. } | Placement::Split(Zone::Center)) {
            return Err("A hidden panel cannot restore over the document view".into());
        }
        let mut probe = layout.clone();
        if let Err(error) = probe.restore(*panel, location)
            && error != DockError::MissingTarget
        {
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// [`validate`] plus the Mix window's rule: Clips and MIDI stay in Edit.
pub fn validate_mix(workspace: &Workspace) -> Result<(), String> {
    validate(workspace)?;
    if workspace.layout.contains(&Panel::Clips) || workspace.layout.contains(&Panel::Midi) {
        return Err("Clips and MIDI panels belong to the Edit window".into());
    }
    Ok(())
}

/// Change a copy of `workspace` and keep it only when it passes [`validate`].
fn checked(workspace: &mut Workspace, change: impl FnOnce(&mut Workspace) -> Result<(), DockError>) -> Result<(), String> {
    validate(workspace)?;
    let mut next = workspace.clone();
    change(&mut next).map_err(|error| error.to_string())?;
    validate(&next)?;
    *workspace = next;
    Ok(())
}

fn apply(workspace: &mut Workspace, action: Action<Panel>) -> Result<(), String> {
    checked(workspace, |next| next.apply(action, permissions).map(drop))
}

/// Whether [`Workspace::set_visible`] can actually put `panel` back at its recorded place.
/// It follows the recorded location and the chain of hidden anchors; when none of that
/// restores, craft-ui falls back to an unanchored open, which since craft-ui 650b1c0b chooses
/// a dock group or right edge instead of reporting the missing anchor. Probe each chain
/// location with the public [`Layout::restore`] on a clone, so a deleted anchor is recognized
/// before calling in; craft-ui still performs the restore and rewrites the chain itself.
fn restores(workspace: &Workspace, panel: Panel) -> bool {
    let recorded = |p: &Panel| workspace.hidden.iter().find(|(q, _)| q == p).map(|(_, location)| location.clone());
    let mut chain: Vec<Location<Panel>> = Vec::new();
    let mut seen = vec![panel];
    let mut current = panel;
    while let Some(location) = recorded(&current) {
        let anchor = location.anchor;
        chain.push(location);
        let Some(anchor) = anchor.filter(|anchor| !seen.contains(anchor)) else { break };
        seen.push(anchor);
        current = anchor;
    }
    // Like the chain itself, the panel's own group/column state applies to every attempt.
    let group = chain.first().and_then(|location| location.group.clone());
    // Revisit this eligibility probe when updating the pinned craft-ui restore algorithm.
    chain
        .iter()
        .rev()
        .find_map(|location| {
            let location = Location { group: group.clone().or_else(|| location.group.clone()), ..location.clone() };
            let mut probe = workspace.layout.clone();
            match probe.restore(panel, &location) {
                Err(DockError::MissingTarget) => None,
                result => Some(result.is_ok()),
            }
        })
        .unwrap_or(false)
}

/// The first panel of the dock on `side`, when that dock is shown.
fn dock_panel(node: &Node<Panel>, side: Side) -> Option<Panel> {
    match node {
        Node::Split { first, second, .. } => dock_panel(first, side).or_else(|| dock_panel(second, side)),
        Node::Dock { side: found, columns } if *found == side => {
            columns.iter().flat_map(|column| &column.groups).find_map(|group| group.panels.first().copied())
        }
        _ => None,
    }
}

/// Show a panel at its recorded place, else at its default place: Tracks and Groups and Clips
/// in their docks (joining the dock's first group when the dock is shown, else making the dock
/// again), the MIDI editor under the session view and the others floating.
fn set_visible(workspace: &mut Workspace, panel: Panel, visible: bool) -> Result<(), String> {
    if workspace.layout.contains(&panel) == visible {
        return Ok(());
    }
    checked(workspace, |next| {
        if !visible {
            return next.apply(Action::Close { panel }, permissions).map(drop);
        }
        let dock = default_dock(panel);
        if let Some((side, _)) = dock
            && let Some(anchor) = next.layout.root.as_ref().and_then(|root| dock_panel(root, side))
        {
            return next.set_visible(panel, true, Some(anchor), permissions);
        }
        // Without an anchor, craft-ui would open a panel it has no record of, or whose record no
        // longer restores, in some other panel's group, so the default place is applied here.
        if next.hidden.iter().any(|(hidden, _)| *hidden == panel) && restores(next, panel) && next.set_visible(panel, true, None, permissions).is_ok()
        {
            return Ok(());
        }
        let open = Action::Open { panel, anchor: None };
        let actions = match (dock, default_float(panel)) {
            (Some((side, width)), _) => vec![open, Action::MovePanel { panel, to: Target::NewDock { side } }, Action::ResizeColumn { panel, width }],
            (None, Some(rect)) => vec![open, Action::Float { panel, rect }],
            (None, None) => vec![Action::OpenAt { panel, anchor: Panel::Canvas, placement: Placement::Split(Zone::Bottom) }],
        };
        next.apply_all(actions, permissions)?;
        next.apply(Action::Activate { panel }, permissions).map(drop)
    })
}

/// Run a shared docking command. A hidden panel that is opened or activated comes back with
/// [`set_visible`], so it gets SoundCraft's default place when it has no recorded one.
fn run(workspace: &mut Workspace, command: &DockCommand<Panel>) -> Result<(), String> {
    if let DockCommand::Activate { panel } | DockCommand::UiDock(UiDock::Operation(DockOperation::Open { panel } | DockOperation::Activate { panel })) =
        command
        && !workspace.layout.contains(panel)
    {
        return set_visible(workspace, *panel, true);
    }
    // Only `window.panel.float` reads the float space, and SoundCraft does not register it.
    checked(workspace, |next| command.apply(next, false, &FloatSpace::new(egui::Rect::NOTHING), permissions).map(drop))
}

fn sync(workspace: &mut Workspace, app: &crate::SoundApp, edit: bool) -> Result<(), String> {
    set_visible(workspace, Panel::Tracks, app.ui.show_tracks_list)?;
    set_visible(workspace, Panel::Memory, app.ui.show_memory_locations)?;
    set_visible(workspace, Panel::History, app.ui.show_undo_history)?;
    if edit {
        set_visible(workspace, Panel::Clips, app.ui.show_clip_list)?;
        set_visible(workspace, Panel::Midi, app.ui.show_midi_editor)?;
    }
    Ok(())
}

fn publish_visibility(app: &mut crate::SoundApp, workspace: &Workspace, edit: bool) {
    app.ui.show_tracks_list = workspace.layout.contains(&Panel::Tracks);
    app.ui.show_memory_locations = workspace.layout.contains(&Panel::Memory);
    app.ui.show_undo_history = workspace.layout.contains(&Panel::History);
    if edit {
        app.ui.show_clip_list = workspace.layout.contains(&Panel::Clips);
        app.ui.show_midi_editor = workspace.layout.contains(&Panel::Midi);
    }
}

fn store(app: &mut crate::SoundApp, workspace: Workspace, edit: bool) {
    if edit {
        app.ui.docking = workspace;
    } else {
        app.ui.mix_docking = workspace;
    }
}

/// A `ui.dock` operation. Panels and zones are checked first so an error names the field;
/// parsed through the untagged [`UiDock`], every mistake reads "did not match any variant".
fn operation(fields: &serde_json::Map<String, Value>) -> Result<DockOperation<Panel>, String> {
    let name = fields.get("operation").ok_or("operation is required")?.as_str().ok_or("operation must be a string")?;
    if name != "resizeSplit" && !fields.contains_key("panel") {
        return Err("panel is required".into());
    }
    for field in ["panel", "target"] {
        if let Some(value) = fields.get(field) {
            let id = value.as_str().ok_or_else(|| format!("{field} must be a panel ID"))?;
            serde_json::from_value::<Panel>(json!(id)).map_err(|_| format!("Unknown panel: {id}"))?;
        }
    }
    if let Some(zone) = fields.get("zone")
        && serde_json::from_value::<ZoneName>(zone.clone()).is_err()
    {
        return Err("zone must be tab, center, left, right, top, bottom, above or below".into());
    }
    serde_json::from_value(Value::Object(fields.clone())).map_err(|error| error.to_string())
}

/// `ui.dock` and `ui.dock.reset`: the shared [`DockCommand`] vocabulary, applied to the
/// front window's workspace.
pub fn command(app: &mut crate::SoundApp, id: &str, params: &Value) -> Result<Value, String> {
    let edit = app.ui.window == crate::MainWindow::Edit;
    if !edit && ["panel", "target"].iter().any(|field| matches!(params.get(*field).and_then(Value::as_str), Some("clips" | "midi"))) {
        return Err("Clips and MIDI panels belong to the Edit window".into());
    }
    let mut fields = match params {
        Value::Object(fields) => fields.clone(),
        Value::Null => serde_json::Map::new(),
        _ => return Err("Docking params must be an object".into()),
    };
    let command = if id == "ui.dock" && !fields.contains_key("action") {
        DockCommand::UiDock(UiDock::Operation(operation(&fields)?))
    } else {
        fields.insert("command".into(), json!(id));
        serde_json::from_value(Value::Object(fields)).map_err(|error| error.to_string())?
    };
    let previous = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    let result = command_for(app, &command, edit)?;
    let next = if edit { &app.ui.docking } else { &app.ui.mix_docking };
    cancel_replaced_workspace(app, &previous, next, edit);
    Ok(result)
}

fn cancel_replaced_workspace(app: &crate::SoundApp, previous: &Workspace, next: &Workspace, edit: bool) {
    if previous != next
        && let Some(ctx) = &app.docking_context
    {
        craft_ui::docking::cancel_drag::<Panel>(ctx, area(edit));
    }
}

/// Restoring complete preferences/configurations uses the same ownership boundary as commands.
pub(crate) fn cancel_replaced_state(app: &crate::SoundApp, next: &crate::UiState) {
    cancel_replaced_workspace(app, &app.ui.docking, &next.docking, true);
    cancel_replaced_workspace(app, &app.ui.mix_docking, &next.mix_docking, false);
}

fn command_for(app: &mut crate::SoundApp, command: &DockCommand<Panel>, edit: bool) -> Result<Value, String> {
    let mut workspace = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    if command.is_app_owned() {
        // Reset is the only application-owned command SoundCraft registers.
        if !matches!(command, DockCommand::ResetWorkspace {}) {
            return Err("Named panel workspaces are not supported".into());
        }
        workspace = if edit { edit_default() } else { mix_default() };
        sync(&mut workspace, app, edit)?;
    } else {
        if !edit {
            validate_mix(&workspace)?;
        }
        sync(&mut workspace, app, edit)?;
        run(&mut workspace, command)?;
        if !edit {
            validate_mix(&workspace)?;
        }
    }
    publish_visibility(app, &workspace, edit);
    let result = json!({"docking": workspace});
    store(app, workspace, edit);
    Ok(result)
}

/// Existing Arrange commands reposition shared floating panels along with the legacy windows.
pub(crate) fn arrange(app: &mut crate::SoundApp) -> Result<(), String> {
    let edit = app.ui.window == crate::MainWindow::Edit;
    let mut workspace = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    if edit {
        validate(&workspace)?;
    } else {
        validate_mix(&workspace)?;
    }
    let previous = workspace.clone();
    let floating: Vec<_> = workspace.layout.floating.iter().filter_map(|window| window.panels().next().map(|panel| (*panel, window.rect))).collect();
    for (index, (panel, mut rect)) in floating.into_iter().enumerate() {
        rect[0] = 32.0 + index as f32 * 28.0;
        rect[1] = 80.0 + index as f32 * 28.0;
        apply(&mut workspace, Action::MoveFloating { panel, rect })?;
    }
    cancel_replaced_workspace(app, &previous, &workspace, edit);
    store(app, workspace, edit);
    Ok(())
}

/// Draw and edit the dock layout for the requested window, falling back to the default layout if validation or panel
/// sync fails, then publish panel visibility, store the workspace and apply the frame's layout actions.
pub fn show(app: &mut crate::SoundApp, ui: &mut egui::Ui, window: crate::MainWindow) {
    app.docking_context = Some(ui.ctx().clone());
    let t = crate::theme::Tokens::current();
    let edit = window == crate::MainWindow::Edit;
    let mut workspace =
        if edit { std::mem::replace(&mut app.ui.docking, edit_default()) } else { std::mem::replace(&mut app.ui.mix_docking, mix_default()) };
    let before_sync = workspace.clone();
    if let Err(error) = (if edit { validate(&workspace) } else { validate_mix(&workspace) }).and_then(|()| sync(&mut workspace, app, edit)) {
        app.ui.status = format!("Panel layout could not be restored: {error}");
        workspace = if edit { edit_default() } else { mix_default() };
        let _ = sync(&mut workspace, app, edit);
    }
    cancel_replaced_workspace(app, &before_sync, &workspace, edit);
    let mut style = DockStyle::from_ui(ui);
    style.min_pane = 96.0;
    style.tab_height = MIDI_TAB_HEIGHT;
    style.background = t.panel_bg;
    style.tab_background = t.panel_bg2;
    style.active_background = t.row_selected;
    style.text = t.text;
    style.inactive_text = t.text_dim;
    style.border = egui::Stroke::new(1.0, t.border);
    style.accent = t.accent;
    style.font = crate::theme::regular(11.5);
    // Free docking: a panel dropped at an empty window edge makes a dock there, and the top
    // and bottom edges take frames (`drop_edges` keeps its default).
    style.new_dock_zones = true;
    style.side_edge_zones = false;
    let output = DockArea::new(area(edit)).show_with_limits(
        ui,
        &workspace.layout,
        &style,
        |panel| panel.label().into(),
        permissions,
        panel_limits,
        |ui, panel| match panel {
            Panel::Canvas => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.window_bg);
                if edit {
                    crate::edit_window::main_area(app, ui);
                } else {
                    crate::mix_window::content(app, ui);
                }
            }
            Panel::Tracks => crate::panels::tracks_and_groups(app, ui),
            Panel::Clips => crate::panels::clip_list(app, ui),
            Panel::Midi => {
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::unique("soundcraft-midi-body"), ui.max_rect()));
                minimum_body(ui, egui::vec2(420.0, 260.0), |ui| crate::midi_editor::show(app, ui));
            }
            Panel::Memory => crate::panels::memory_locations(app, ui),
            Panel::History => crate::panels::undo_history(app, ui),
        },
    );
    let _ = sync(&mut workspace, app, edit);
    publish_visibility(app, &workspace, edit);
    store(app, workspace, edit);
    for action in output.actions {
        if let Err(error) = command_for(app, &DockCommand::Layout { action }, edit) {
            app.ui.status = format!("Panel move was not applied: {error}");
        }
    }
}

/// Docking may make a specialist view smaller than its fixed tool/ruler geometry. Keep that
/// geometry scrollable inside its clipped panel instead of producing inverted rectangles.
pub(crate) fn minimum_body(ui: &mut egui::Ui, minimum: egui::Vec2, body: impl FnOnce(&mut egui::Ui)) {
    let available = ui.available_size();
    if available.x >= minimum.x && available.y >= minimum.y {
        body(ui);
        return;
    }
    let extent = available.max(minimum);
    egui::ScrollArea::both().id_salt("minimum-body").auto_shrink([false, false]).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(extent, egui::Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        body(&mut child);
    });
}

fn panel_limits(panel: &Panel) -> PanelLimits {
    let (min, max_width) = match panel {
        Panel::Canvas => (egui::vec2(300.0, 240.0), 1_000_000.0),
        Panel::Tracks => (egui::vec2(140.0, 120.0), 520.0),
        Panel::Clips => (egui::vec2(180.0, 120.0), 720.0),
        Panel::Midi => (egui::vec2(420.0, 260.0), 1_000_000.0),
        Panel::Memory => (egui::vec2(260.0, 160.0), 1_000_000.0),
        Panel::History => (egui::vec2(200.0, 160.0), 720.0),
    };
    PanelLimits { min, max: egui::vec2(max_width, 1_000_000.0) }
}
