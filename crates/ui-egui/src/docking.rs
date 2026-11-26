//! Application panel identities and persistence around craft-ui's `DockArea`. Document contents remain owned by
//! their existing view implementations.
//!
//! `UiState::docking` and `UiState::mix_docking` are the saved workspaces and the source of truth. The dock shows an
//! `Arc` of each ([`LiveDocks`]), kept while the saved workspace is unchanged; every change the dock reports goes
//! through [`command_for`] like a `ui.dock` command.

use std::sync::Arc;

use craft_behavior::DockSide;
use craft_behavior::dock::{
    Action, Column, DockEdge, DockError, DockPlacement, Floating, Group, Layout, Location, Node, Permissions, SplitSize, Target,
};
use craft_behavior::flow::Axis;
use craft_ui::contract::{Bind, BodyCx, PlainData, SizeLimits, Tracked};
use craft_ui::docking::{DockConfig, DockPanels, GroupAt, PanelSpec, PanelSpecs};
use craft_ui::patterns::dock::DockArea;
use egui::{Rect, pos2, vec2};
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
pub type Workspace = craft_behavior::dock::Workspace<Panel>;

/// The height the MIDI editor's tab adds above its body in the default layout.
const MIDI_TAB_HEIGHT: f32 = 28.0;

/// The document view stays docked and open, and panels open in splits beside it.
fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Canvas { Permissions::PROTECTED.accepts_splits(true) } else { Permissions::default() }
}

fn tabs(panel: Panel) -> Node<Panel> {
    Node::tabs(vec![panel], 0)
}

/// The id the Edit or Mix dock area is drawn under.
pub fn area_id(window: crate::MainWindow) -> egui::Id {
    egui::Id::unique(("soundcraft-docking", window == crate::MainWindow::Edit))
}

/// A dock on `side` with one column holding `panel`.
fn dock(side: DockEdge, panel: Panel, width: f32) -> Node<Panel> {
    Node::dock(side, vec![Column::new(vec![Group::new(vec![panel], 0)]).with_width(width)])
}

const MEMORY_RECT: Rect = Rect::from_min_max(pos2(160.0, 120.0), pos2(640.0, 400.0));
const HISTORY_RECT: Rect = Rect::from_min_max(pos2(680.0, 120.0), pos2(940.0, 420.0));

/// The floating windows both windows start with. Fixed ids: the default is rebuilt every frame
/// and must stay the same windows.
fn default_floating() -> Vec<Floating<Panel>> {
    vec![Floating::single(1, vec![Panel::Memory], 0, MEMORY_RECT), Floating::single(2, vec![Panel::History], 0, HISTORY_RECT)]
}

/// Where a panel that floats in the default layouts floats.
fn default_float(panel: Panel) -> Option<Rect> {
    match panel {
        Panel::Memory => Some(MEMORY_RECT),
        Panel::History => Some(HISTORY_RECT),
        _ => None,
    }
}

/// The dock side and column width a panel opens in when it has no recorded place.
fn default_dock(panel: Panel) -> Option<(DockEdge, f32)> {
    match panel {
        Panel::Tracks => Some((DockEdge::Start, TRACKS_WIDTH)),
        Panel::Clips => Some((DockEdge::End, CLIPS_WIDTH)),
        _ => None,
    }
}

/// The Edit window's built-in arrangement: Tracks and Groups in a full-height start dock,
/// Clips in a full-height end dock, and the MIDI editor under the session view between them.
pub fn edit_default() -> Workspace {
    let session = Node::split(Axis::Y, SplitSize::FixedSecond(MIDI_HEIGHT + MIDI_TAB_HEIGHT), tabs(Panel::Canvas), tabs(Panel::Midi));
    let root = Node::split(
        Axis::X,
        SplitSize::FixedFirst(TRACKS_WIDTH),
        dock(DockEdge::Start, Panel::Tracks, TRACKS_WIDTH),
        Node::split(Axis::X, SplitSize::FixedSecond(CLIPS_WIDTH), session, dock(DockEdge::End, Panel::Clips, CLIPS_WIDTH)),
    );
    Workspace::new(Layout::new(Some(root), default_floating()))
}

/// The Mix window's built-in arrangement: Tracks and Groups in a start dock beside the mixer.
/// The timeline-only Clips and MIDI panels stay in Edit.
pub fn mix_default() -> Workspace {
    let root = Node::split(Axis::X, SplitSize::FixedFirst(TRACKS_WIDTH), dock(DockEdge::Start, Panel::Tracks, TRACKS_WIDTH), tabs(Panel::Canvas));
    Workspace::new(Layout::new(Some(root), default_floating()))
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
        Node::Stack { entries, .. } if entries.iter().any(|entry| entry.panel == Panel::Canvas) => Some(Err("The document view cannot be collapsed")),
        Node::Dock { columns, .. } if columns.iter().flat_map(|column| &column.groups).any(|group| group.panels.contains(&Panel::Canvas)) => {
            Some(Err("The document view cannot join a dock column"))
        }
        _ => None,
    }
}

/// Check a workspace against craft-behavior's rules and SoundCraft's own: saved settings cannot
/// float, hide, collapse or share the protected document view.
pub fn validate(workspace: &Workspace) -> Result<(), String> {
    workspace.validate().map_err(|error| error.to_string())?;
    let layout = workspace.layout();
    if !layout.contains(&Panel::Canvas) || workspace.hidden().len() > Panel::ALL.len() {
        return Err("The panel layout must contain its document view and bounded hidden panels".into());
    }
    if layout.floating.iter().any(|window| window.contains(&Panel::Canvas)) {
        return Err("The document view cannot float".into());
    }
    layout.root.as_ref().and_then(canvas_placement).unwrap_or(Err("The document view cannot float"))?;
    for (panel, location) in workspace.hidden() {
        if *panel == Panel::Canvas {
            return Err("A hidden panel has an invalid or duplicate identity".into());
        }
        if location.anchor == Some(Panel::Canvas) && matches!(location.placement, DockPlacement::Tab { .. } | DockPlacement::Split(DockSide::Center))
        {
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
    if workspace.layout().contains(&Panel::Clips) || workspace.layout().contains(&Panel::Midi) {
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
    checked(workspace, |next| next.apply(action, false, permissions).map(drop))
}

/// Whether [`Workspace::set_visible`] can actually put `panel` back at its recorded place.
/// It follows the recorded location and the chain of hidden anchors; when none of that
/// restores, craft-behavior falls back to an unanchored open, which chooses a dock group or
/// right edge instead of reporting the missing anchor. Probe each chain location with the
/// public [`Layout::restore`] on a clone, so a deleted anchor is recognized before calling in;
/// craft-behavior still performs the restore and rewrites the chain itself.
fn restores(workspace: &Workspace, panel: Panel) -> bool {
    let recorded = |p: &Panel| workspace.hidden().iter().find(|(q, _)| q == p).map(|(_, location)| location.clone());
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
    // Revisit this eligibility probe when updating the pinned craft-behavior restore algorithm.
    chain
        .iter()
        .rev()
        .find_map(|location| {
            let mut location = location.clone();
            location.group = group.clone().or(location.group);
            let mut probe = workspace.layout().clone();
            match probe.restore(panel, &location) {
                Err(DockError::MissingTarget) => None,
                result => Some(result.is_ok()),
            }
        })
        .unwrap_or(false)
}

/// The first panel of the dock on `side`, when that dock is shown.
fn dock_panel(node: &Node<Panel>, side: DockEdge) -> Option<Panel> {
    match node {
        Node::Split { first, second, .. } => dock_panel(first, side).or_else(|| dock_panel(second, side)),
        Node::Dock { side: found, columns, .. } if *found == side => {
            columns.iter().flat_map(|column| &column.groups).find_map(|group| group.panels.first().copied())
        }
        _ => None,
    }
}

/// Show a panel at its recorded place, else at its default place: Tracks and Groups and Clips
/// in their docks (joining the dock's first group when the dock is shown, else making the dock
/// again), the MIDI editor under the session view and the others floating.
fn set_visible(workspace: &mut Workspace, panel: Panel, visible: bool) -> Result<(), String> {
    if workspace.layout().contains(&panel) == visible {
        return Ok(());
    }
    checked(workspace, |next| {
        if !visible {
            return next.apply(Action::close(panel), false, permissions).map(drop);
        }
        let dock = default_dock(panel);
        if let Some((side, _)) = dock
            && let Some(anchor) = next.layout().root.as_ref().and_then(|root| dock_panel(root, side))
        {
            return next.set_visible(panel, true, Some(anchor), false, permissions);
        }
        // Without an anchor, craft-behavior would open a panel it has no record of, or whose record no
        // longer restores, in some other panel's group, so the default place is applied here.
        if next.hidden().iter().any(|(hidden, _)| *hidden == panel)
            && restores(next, panel)
            && next.set_visible(panel, true, None, false, permissions).is_ok()
        {
            return Ok(());
        }
        let open = Action::open(panel, None);
        let actions = match (dock, default_float(panel)) {
            (Some((side, width)), _) => vec![open, Action::move_panel(panel, Target::new_dock(side)), Action::resize_column(panel, width)],
            (None, Some(rect)) => vec![open, Action::float(panel, rect)],
            (None, None) => vec![Action::open_at(panel, Panel::Canvas, DockPlacement::Split(DockSide::Bottom))],
        };
        next.apply_all(actions, false, permissions)?;
        next.apply(Action::activate(panel), false, permissions).map(drop)
    })
}

/// A drop zone by name, as `ui.dock` writes it. PascalCase names also parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ZoneName {
    Tab,
    #[serde(alias = "Center")]
    Center,
    #[serde(alias = "Left")]
    Left,
    #[serde(alias = "Right")]
    Right,
    #[serde(alias = "Top")]
    Top,
    #[serde(alias = "Bottom")]
    Bottom,
    Above,
    Below,
}

/// A present field, `null` included: `Some(None)` for `null`, `None` when absent.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// A required field that may be `null`.
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<T, D::Error> {
    T::deserialize(deserializer)
}

/// A `ui.dock` operation, tagged by `operation`. `move` takes `target` (the anchor), `zone`
/// (default `center`) and `before`, which only applies to a center move. Rects are `[x, y, width, height]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
enum DockOperation {
    /// Show a panel at its last location and select it.
    Open {
        panel: Panel,
    },
    Close {
        panel: Panel,
    },
    Activate {
        panel: Panel,
    },
    Float {
        panel: Panel,
        rect: [f32; 4],
    },
    MoveFloating {
        panel: Panel,
        rect: [f32; 4],
    },
    Move {
        panel: Panel,
        target: Panel,
        #[serde(default)]
        zone: Option<ZoneName>,
        #[serde(default, deserialize_with = "present")]
        before: Option<Option<Panel>>,
    },
    SetStackOpen {
        panel: Panel,
        open: bool,
    },
    /// `height` is required; `null` clears it.
    ResizeStack {
        panel: Panel,
        #[serde(deserialize_with = "required")]
        height: Option<f32>,
    },
    ResizeSplit {
        path: Vec<bool>,
        size: SplitSize,
    },
}

/// What `ui.dock` and `ui.dock.reset` ask of the front window's workspace, and the changes the dock reports.
#[derive(Clone, Debug, PartialEq)]
enum DockCommand {
    /// A raw layout action: `ui.dock` with `{action}`, or a change made in the dock.
    Layout(Action<Panel>),
    /// `ui.dock` with `{operation, …}`.
    Operation(DockOperation),
    /// `ui.dock.reset`: the built-in arrangement.
    Reset,
}

/// `[x, y, width, height]` as a rect.
fn rect_of([x, y, width, height]: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

/// Where a `move` puts the panel relative to `anchor`: `zone` (default `center`) and `before`,
/// with side zones mapped onto the anchor's container by [`DockPlacement::for_zone`].
fn placement(
    layout: &Layout<Panel>,
    anchor: &Panel,
    zone: Option<ZoneName>,
    before: Option<Option<Panel>>,
) -> Result<DockPlacement<Panel>, DockError> {
    let side = |side| DockPlacement::for_zone(layout, anchor, side);
    Ok(match (zone.unwrap_or(ZoneName::Center), before) {
        (ZoneName::Tab | ZoneName::Center, Some(before)) => DockPlacement::tab(before),
        (ZoneName::Tab, None) => DockPlacement::tab(None),
        (ZoneName::Center, None) => DockPlacement::Split(DockSide::Center),
        (_, Some(_)) => return Err(DockError::InvalidPlacement),
        (ZoneName::Left, None) => side(DockSide::Start)?,
        (ZoneName::Right, None) => side(DockSide::End)?,
        (ZoneName::Top, None) => side(DockSide::Top)?,
        (ZoneName::Bottom, None) => side(DockSide::Bottom)?,
        (ZoneName::Above, None) => DockPlacement::group(false, None),
        (ZoneName::Below, None) => DockPlacement::group(true, None),
    })
}

impl DockOperation {
    /// The layout action this operation makes on `layout`. A panel that is not shown is
    /// [`DockError::MissingPanel`] for Open and Activate, which show it first ([`run`]).
    fn action(&self, layout: &Layout<Panel>) -> Result<Action<Panel>, DockError> {
        Ok(match self {
            Self::Open { panel } | Self::Activate { panel } => {
                if !layout.contains(panel) {
                    return Err(DockError::MissingPanel);
                }
                Action::activate(*panel)
            }
            Self::Close { panel } => Action::close(*panel),
            Self::Float { panel, rect } => Action::float(*panel, rect_of(*rect)),
            Self::MoveFloating { panel, rect } => Action::move_floating(*panel, rect_of(*rect)),
            Self::Move { panel, target, zone, before } => Action::move_beside(*panel, *target, placement(layout, target, *zone, *before)?),
            Self::SetStackOpen { panel, open } => Action::set_stack_open(*panel, *open),
            Self::ResizeStack { panel, height } => Action::resize_stack(*panel, *height),
            Self::ResizeSplit { path, size } => Action::resize_split(path.clone(), *size),
        })
    }
}

/// Run a docking command. A hidden panel that is opened or activated comes back with
/// [`set_visible`], so it gets SoundCraft's default place when it has no recorded one.
fn run(workspace: &mut Workspace, command: &DockCommand) -> Result<(), String> {
    let action = match command {
        DockCommand::Operation(DockOperation::Open { panel } | DockOperation::Activate { panel }) if !workspace.layout().contains(panel) => {
            return set_visible(workspace, *panel, true);
        }
        DockCommand::Operation(operation) => operation.action(workspace.layout()).map_err(|error| error.to_string())?,
        DockCommand::Layout(action) => action.clone(),
        DockCommand::Reset => return Err("Reset is applied by the window, not the layout".into()),
    };
    apply(workspace, action)
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
    let layout = workspace.layout();
    app.ui.show_tracks_list = layout.contains(&Panel::Tracks);
    app.ui.show_memory_locations = layout.contains(&Panel::Memory);
    app.ui.show_undo_history = layout.contains(&Panel::History);
    if edit {
        app.ui.show_clip_list = layout.contains(&Panel::Clips);
        app.ui.show_midi_editor = layout.contains(&Panel::Midi);
    }
}

fn store(app: &mut crate::SoundApp, workspace: Workspace, edit: bool) {
    if edit {
        app.ui.docking = workspace;
    } else {
        app.ui.mix_docking = workspace;
    }
}

/// A `ui.dock` operation. Panels and zones are checked first so an error names the field.
fn operation(fields: &serde_json::Map<String, Value>) -> Result<DockOperation, String> {
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

/// `ui.dock` and `ui.dock.reset`, applied to the front window's workspace.
pub fn command(app: &mut crate::SoundApp, id: &str, params: &Value) -> Result<Value, String> {
    let edit = app.ui.window == crate::MainWindow::Edit;
    if !edit && ["panel", "target"].iter().any(|field| matches!(params.get(*field).and_then(Value::as_str), Some("clips" | "midi"))) {
        return Err("Clips and MIDI panels belong to the Edit window".into());
    }
    let fields = match params {
        Value::Object(fields) => fields.clone(),
        Value::Null => serde_json::Map::new(),
        _ => return Err("Docking params must be an object".into()),
    };
    let command = match (id, fields.get("action")) {
        ("ui.dock.reset", _) => DockCommand::Reset,
        (_, Some(action)) => DockCommand::Layout(serde_json::from_value(action.clone()).map_err(|error| error.to_string())?),
        (_, None) => DockCommand::Operation(operation(&fields)?),
    };
    let previous = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    let result = command_for(app, &command, edit)?;
    let next = if edit { &app.ui.docking } else { &app.ui.mix_docking };
    cancel_replaced_workspace(app, &previous, next);
    Ok(result)
}

/// Ends a running panel drag when the workspace it started on is replaced or changed by something else
/// than the dock.
fn cancel_replaced_workspace(app: &crate::SoundApp, previous: &Workspace, next: &Workspace) {
    if previous != next
        && let Some(ctx) = &app.docking_context
    {
        craft_ui::runtime::drag::cancel(ctx, craft_behavior::CancelReason::HostChanged);
    }
}

/// Restoring complete preferences/configurations uses the same ownership boundary as commands.
pub(crate) fn cancel_replaced_state(app: &crate::SoundApp, next: &crate::UiState) {
    cancel_replaced_workspace(app, &app.ui.docking, &next.docking);
    cancel_replaced_workspace(app, &app.ui.mix_docking, &next.mix_docking);
}

fn command_for(app: &mut crate::SoundApp, command: &DockCommand, edit: bool) -> Result<Value, String> {
    let mut workspace = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    if *command == DockCommand::Reset {
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
    let floating: Vec<_> =
        workspace.layout().floating.iter().filter_map(|window| window.panels().next().map(|panel| (*panel, window.rect))).collect();
    for (index, (panel, rect)) in floating.into_iter().enumerate() {
        let at = pos2(32.0 + index as f32 * 28.0, 80.0 + index as f32 * 28.0);
        apply(&mut workspace, Action::move_floating(panel, Rect::from_min_size(at, rect.size())))?;
    }
    cancel_replaced_workspace(app, &previous, &workspace);
    store(app, workspace, edit);
    Ok(())
}

/// The workspaces the Edit and Mix docks show, the panels' specs and the session views' keyboard focus.
#[derive(Default)]
pub(crate) struct LiveDocks {
    edit: Option<Arc<Workspace>>,
    mix: Option<Arc<Workspace>>,
    specs: Tracked<DockSpecs>,
    edit_focus: ViewFocus,
    mix_focus: ViewFocus,
}

impl LiveDocks {
    /// The `Arc` the dock shows for `workspace`: the last one while its arrangement is the same, so an idle pass
    /// keeps the workspace's revision and the dock reads no spec.
    fn shown(&mut self, edit: bool, workspace: &Workspace) -> Arc<Workspace> {
        let slot = if edit { &mut self.edit } else { &mut self.mix };
        if let Some(shown) = slot.as_ref().filter(|shown| ***shown == *workspace) {
            return Arc::clone(shown);
        }
        let shown = Arc::new(workspace.clone());
        *slot = Some(Arc::clone(&shown));
        shown
    }
}

/// What the dock reads of the panels: their names in the interface language.
#[derive(Clone, Debug, Default, PartialEq, PlainData)]
struct DockSpecs {
    /// The tab names, in [`Panel::ALL`] order.
    labels: Vec<String>,
}

impl PanelSpecs<Panel> for DockSpecs {
    fn spec(&self, panel: &Panel) -> PanelSpec {
        let label = Panel::ALL.iter().position(|p| p == panel).and_then(|index| self.labels.get(index)).cloned().unwrap_or_default();
        // Every panel is SoundCraft's own egui code: the dock runs it with the window's style.
        let spec = PanelSpec::new(label).limits(panel_limits(panel)).host_body();
        if *panel == Panel::Canvas { spec.canvas() } else { spec }
    }

    fn permissions(&self, panel: &Panel) -> Permissions {
        permissions(panel)
    }
}

/// Reads the panel names in the interface language, moving the specs' revision only when one changed.
fn refresh_specs(app: &mut crate::SoundApp) {
    let labels: Vec<String> = Panel::ALL.iter().map(|panel| crate::localization::text(panel.label()).to_owned()).collect();
    if app.docks.specs.labels != labels {
        app.docks.specs.labels = labels;
    }
}

/// What a session view knows of its keyboard focus from the pass that drew it last.
#[derive(Default)]
struct ViewFocus {
    /// That pass's number and the document the view showed (`SoundApp::doc_epoch`).
    shown: Option<(u64, u64)>,
    /// The view opened or was pressed and has yet to ask for the focus.
    wanted: bool,
    /// The view had the focus, and no other widget has taken it.
    returns: bool,
    /// An overlay ([`crate::framework::overlay_open`]) was up.
    overlay: bool,
    /// The view kept Tab.
    kept_tab: bool,
}

impl LiveDocks {
    fn view_focus(&mut self, edit: bool) -> &mut ViewFocus {
        if edit { &mut self.edit_focus } else { &mut self.mix_focus }
    }
}

/// Gives the session view, drawn in `rect`, the keyboard focus on a press in it, when it opens or shows another
/// document, and when its focus went nowhere (a dialog that closed). It waits for
/// a modal dialog, menu or popup to close. A press leaves the focus with the text field it landed on; a view that opens
/// leaves it with any text field and with a widget of another layer. Called after the view's widgets, so that the field
/// under a press has taken the focus already.
fn focus_session_view(app: &mut crate::SoundApp, ui: &egui::Ui, edit: bool, rect: Rect) {
    let ctx = ui.ctx();
    let id = crate::framework::session_view_id(edit);
    let focused = ctx.memory(|memory| memory.focused());
    let pressed = ui.rect_contains_pointer(rect) && ui.input(|input| input.pointer.any_pressed());
    let overlay = crate::framework::overlay_open(app, ctx);
    let (pass, document) = (ctx.cumulative_pass_nr(), app.doc_epoch);
    let state = app.docks.view_focus(edit);
    let stayed = state.shown.is_some_and(|(last, shown)| shown == document && (last == pass || last + 1 == pass));
    state.shown = Some((pass, document));
    state.wanted |= pressed || !stayed;
    state.returns |= focused == Some(id);
    // egui keeps the focus on a widget of a closed overlay for the pass after the last one that drew it.
    if std::mem::replace(&mut state.overlay, overlay) || overlay {
        if !overlay {
            ctx.request_repaint();
        }
        return;
    }
    let wanted = std::mem::take(&mut state.wanted);
    state.returns &= focused.is_none_or(|focused| focused == id);
    let layer = |id| ctx.read_response(id).map(|widget| widget.layer_id);
    let field_pressed =
        || ctx.text_edit_focused() && focused.and_then(|field| ctx.read_response(field)).is_some_and(|field| field.is_pointer_button_down_on());
    let takes = if pressed {
        focused != Some(id) && !field_pressed()
    } else if wanted {
        focused != Some(id) && !ctx.text_edit_focused() && focused.is_none_or(|focused| layer(focused) == layer(id))
    } else {
        state.returns && focused.is_none()
    };
    if takes {
        crate::framework::focus_session_view(ctx, id);
    }
}

/// The dock's panels over the app in one pass.
struct Panels<'a> {
    app: &'a mut crate::SoundApp,
    edit: bool,
}

impl DockPanels<Panel> for Panels<'_> {
    fn body(&mut self, panel: &Panel, ui: &mut egui::Ui, _: &mut BodyCx<'_>) {
        let app = &mut *self.app;
        match panel {
            // The session view is its group's `group_view`.
            Panel::Canvas => {}
            Panel::Tracks => crate::panels::tracks_and_groups(app, ui),
            Panel::Clips => crate::panels::clip_list(app, ui),
            Panel::Midi => {
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::unique("soundcraft-midi-body"), ui.max_rect()));
                minimum_body(ui, vec2(420.0, 260.0), |ui| crate::midi_editor::show(app, ui));
            }
            Panel::Memory => crate::panels::memory_locations(app, ui),
            Panel::History => crate::panels::undo_history(app, ui),
        }
    }

    /// The session view: the Edit window's tracks and timeline, or the mixer.
    fn group_view(&mut self, panels: &[Panel], _: usize, ui: &mut egui::Ui, at: GroupAt, _: &mut BodyCx<'_>) -> Option<f32> {
        let GroupAt::Show(rect) = at else { return None };
        if !panels.contains(&Panel::Canvas) {
            return None;
        }
        let id = crate::framework::session_view_id(self.edit);
        let view = ui.interact(rect, id, egui::Sense::focusable_noninteractive());
        view.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, crate::localization::text(Panel::Canvas.label())));
        // The shortcuts read the root window's keys only, so a Mix window of its own leaves Tab to the focus. A press of
        // Tab is the view's to its release: the repeats of the one that reached the last stop stay here.
        let shortcuts_run = ui.ctx().viewport_id() == egui::ViewportId::ROOT;
        let held = self.app.docks.view_focus(self.edit).kept_tab && ui.input(|input| input.key_down(egui::Key::Tab));
        let keeps_tab = |app: &crate::SoundApp| shortcuts_run && (held || crate::shortcuts::tab_has_work(app));
        // Declared before the widgets drawn over it, for its place in Tab order, and again after this pass's input has
        // changed what Tab has to do.
        crate::framework::host_canvas(ui.ctx(), id, keeps_tab(self.app));
        ui.painter().rect_filled(rect, 0.0, crate::theme::Tokens::current().window_bg);
        if self.edit {
            crate::edit_window::main_area(self.app, ui);
        } else {
            crate::mix_window::content(self.app, ui);
        }
        focus_session_view(self.app, ui, self.edit, rect);
        let kept_tab = keeps_tab(self.app);
        self.app.docks.view_focus(self.edit).kept_tab = kept_tab;
        crate::framework::host_canvas(ui.ctx(), id, kept_tab);
        Some(rect.height())
    }
}

/// Draw and edit the dock layout for the requested window, falling back to the default layout if validation or panel
/// sync fails, then publish panel visibility, store the workspace and apply the pass's layout actions.
pub fn show(app: &mut crate::SoundApp, ui: &mut egui::Ui, window: crate::MainWindow) {
    app.docking_context = Some(ui.ctx().clone());
    let edit = window == crate::MainWindow::Edit;
    let mut workspace =
        if edit { std::mem::replace(&mut app.ui.docking, edit_default()) } else { std::mem::replace(&mut app.ui.mix_docking, mix_default()) };
    let before_sync = workspace.clone();
    if let Err(error) = (if edit { validate(&workspace) } else { validate_mix(&workspace) }).and_then(|()| sync(&mut workspace, app, edit)) {
        app.ui.status = crate::localized_format!("Panel layout could not be restored: {error}", error = error);
        workspace = if edit { edit_default() } else { mix_default() };
        let _ = sync(&mut workspace, app, edit);
    }
    cancel_replaced_workspace(app, &before_sync, &workspace);
    // The MIDI body's rect stands for a piano roll on screen (the arrow keys go to it), so only a
    // frame that draws the panel records one: open behind another tab, the old rect is gone.
    if edit {
        ui.ctx().data_mut(|data| data.remove::<egui::Rect>(egui::Id::unique("soundcraft-midi-body")));
    }
    refresh_specs(app);
    let shown = app.docks.shown(edit, &workspace);
    // The specs leave the app while its panels borrow it, and come back with their revision.
    let specs = std::mem::take(&mut app.docks.specs);
    // Floating windows stay inside the app window; a drop at an empty window edge makes a dock there, and the top and
    // bottom edges take frames (`DockConfig::default()`).
    let config = DockConfig::default().native_windows(false);
    let output = ui
        .push_id(area_id(window), |ui| {
            let mut panels = Panels { app: &mut *app, edit };
            DockArea::new("Panels", Bind::given(shown), &specs, &mut panels, config).show(ui)
        })
        .inner;
    app.docks.specs = specs;
    let _ = sync(&mut workspace, app, edit);
    publish_visibility(app, &workspace, edit);
    store(app, workspace, edit);
    for action in output.actions.into_iter().chain(output.requested) {
        if let Err(error) = command_for(app, &DockCommand::Layout(action), edit) {
            app.ui.status = crate::localized_format!("Panel move was not applied: {error}", error = error);
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

/// The panels' body limits: a minimum size and, for side panels, a maximum width.
fn panel_limits(panel: &Panel) -> SizeLimits {
    let (min, max_width) = match panel {
        Panel::Canvas => (vec2(300.0, 240.0), SizeLimits::BOUND),
        Panel::Tracks => (vec2(140.0, 120.0), 520.0),
        Panel::Clips => (vec2(180.0, 120.0), 720.0),
        Panel::Midi => (vec2(420.0, 260.0), SizeLimits::BOUND),
        Panel::Memory => (vec2(260.0, 160.0), SizeLimits::BOUND),
        Panel::History => (vec2(200.0, 160.0), 720.0),
    };
    SizeLimits::new(min, vec2(max_width, SizeLimits::BOUND))
}
