//! Application panel identities and persistence around the shared docking renderer.
//! Document contents remain owned by their existing view implementations.

use craft_ui::docking::{
    Action, DockArea, DockCommand, DockError, DockOperation, DockStyle, FloatSpace, Floating, Layout, Node, PanelLimits, Permissions, Placement,
    UiDock, Zone, ZoneName,
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
    fn default_side(self) -> Zone {
        match self {
            Self::Canvas => Zone::Right,
            Self::Tracks => Zone::Left,
            Self::Clips => Zone::Right,
            Self::Midi => Zone::Bottom,
            Self::Memory => Zone::Bottom,
            Self::History => Zone::Right,
        }
    }
}

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
    egui::Id::new(("soundcraft-docking", window == crate::MainWindow::Edit))
}

fn area(edit: bool) -> egui::Id {
    area_id(if edit { crate::MainWindow::Edit } else { crate::MainWindow::Mix })
}

/// The Edit window's built-in arrangement.
pub fn edit_default() -> Workspace {
    Workspace::new(Layout {
        root: Some(split(
            SplitAxis::Vertical,
            SplitSize::FixedSecond(320.0 + MIDI_TAB_HEIGHT),
            split(
                SplitAxis::Horizontal,
                SplitSize::FixedFirst(168.0),
                tabs(Panel::Tracks),
                split(SplitAxis::Horizontal, SplitSize::FixedSecond(230.0), tabs(Panel::Canvas), tabs(Panel::Clips)),
            ),
            tabs(Panel::Midi),
        )),
        // Fixed ids: the default is rebuilt every frame and must stay the same windows.
        floating: vec![
            Floating::single(vec![Panel::Memory], 0, [160.0, 120.0, 480.0, 280.0]).with_id(1),
            Floating::single(vec![Panel::History], 0, [680.0, 120.0, 260.0, 300.0]).with_id(2),
        ],
    })
}

/// Mix retains its own arrangement; the timeline-only Clips and MIDI panels stay in Edit.
pub fn mix_default() -> Workspace {
    let mut workspace = edit_default();
    let _ = set_visible(&mut workspace, Panel::Clips, false);
    let _ = set_visible(&mut workspace, Panel::Midi, false);
    workspace
}

/// The document view where the Edit window's rules keep it: in the docked frames, alone in
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

/// Show a panel at its recorded place, else beside the document view on its default side.
fn set_visible(workspace: &mut Workspace, panel: Panel, visible: bool) -> Result<(), String> {
    if workspace.layout.contains(&panel) == visible {
        return Ok(());
    }
    checked(workspace, |next| {
        if !visible {
            return next.apply(Action::Close { panel }, permissions).map(drop);
        }
        if next.set_visible(panel, true, None, permissions).is_ok() {
            return Ok(());
        }
        next.apply(Action::OpenAt { panel, anchor: Panel::Canvas, placement: Placement::Split(panel.default_side()) }, permissions).map(drop)
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
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("soundcraft-midi-body"), ui.max_rect()));
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
