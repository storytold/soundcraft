//! Setup › I/O: the I/O Setup window, laid out like Pro Tools': Input, Output and Bus tabs. Each
//! lists its paths (active, name, format) beside a grid: the Input and Output tabs' grids are the
//! device's channels, where a click under a channel maps the path from that channel on; the Bus
//! tab's grid is the bus's channels, where its sub-paths are mapped, with a Mapping to Output
//! column beside it. The buttons below make, remove and reset paths. Every change is a `setup.io`
//! command, so it is one undo step and the CLI, control channel and MCP can do the same.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Align2, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use serde_json::{Value, json};
use soundcraft_model::{Bus, ChannelFormat, InputPath, MAX_INPUT_CHANNEL, OutputPath};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Input,
    Output,
    Bus,
}

/// Which kind of path a grid row is, and so which commands its controls send.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RowKind {
    Input,
    Output,
    Bus,
    BusSub,
}

/// One row of a tab's grid: a path, main or sub, and what it maps onto.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub name: String,
    pub format: ChannelFormat,
    /// Per path channel: the hardware channel (Input, Output) or the bus channel (Bus) it is on.
    pub channels: Vec<Option<u16>>,
    pub active: bool,
    pub is_sub: bool,
    /// For a sub-path, the channels of its main path it may use.
    pub within: Option<Vec<Option<u16>>>,
    pub has_subs: bool,
    /// Bus rows: the output path the bus is mapped to.
    pub mapping: Option<String>,
    /// Output rows: the monitor path, whose format is the main mix's.
    pub monitor: bool,
    pub kind: RowKind,
}

/// What the window keeps between frames (none of it is session state).
#[derive(Clone, Default)]
struct State {
    tab: Tab,
    /// The selected path's name (New Sub-Path and Delete Path act on it).
    selected: Option<String>,
    /// Main paths whose sub-paths are folded away.
    collapsed: BTreeSet<String>,
    /// The name being edited: (path, text).
    renaming: Option<(String, String)>,
    new_path: Option<NewPath>,
    default_format: Option<ChannelFormat>,
    /// The Bus tab's Default menu: all, internal or output busses.
    bus_default: BusDefault,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum BusDefault {
    #[default]
    All,
    Internal,
    Output,
}

impl BusDefault {
    fn label(self) -> &'static str {
        match self {
            BusDefault::All => "All Busses",
            BusDefault::Internal => "Internal Busses",
            BusDefault::Output => "Output Busses",
        }
    }
    fn id(self) -> &'static str {
        match self {
            BusDefault::All => "all",
            BusDefault::Internal => "internal",
            BusDefault::Output => "output",
        }
    }
}

/// The New Paths dialog's row and options.
#[derive(Clone)]
struct NewPath {
    tab: Tab,
    count: u32,
    format: ChannelFormat,
    name: String,
    assign: bool,
    sub_paths: bool,
}

/// Cell size in the grid.
const CELL: f32 = 22.0;
const ROW: f32 = 18.0;

pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_io_setup;
    if !open {
        return;
    }
    let id = egui::Id::new("io_setup_state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if let Some(tab) = app.ui.io_setup_tab.take() {
        st.tab = match tab.as_str() {
            "output" => Tab::Output,
            "bus" => Tab::Bus,
            _ => Tab::Input,
        };
        st.selected = None;
    }
    egui::Window::new("I/O Setup").open(&mut open).default_size(vec2(1000.0, 580.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::Input, "Input"), (Tab::Output, "Output"), (Tab::Bus, "Bus")] {
                if ui.selectable_label(st.tab == tab, label).clicked() && st.tab != tab {
                    st.tab = tab;
                    st.selected = None;
                    st.renaming = None;
                }
            }
        });
        ui.separator();
        match st.tab {
            Tab::Input => input_tab(app, ui, &mut st),
            Tab::Output => output_tab(app, ui, &mut st),
            Tab::Bus => bus_tab(app, ui, &mut st),
        }
        ui.separator();
        // The settings file holds all three tabs, as a session does; an import merges by name.
        ui.horizontal(|ui| {
            if ui.button("Export Settings...").on_hover_text("Write the I/O Setup (all three tabs) to a .scio settings file").clicked() {
                app.dialogs.open_for_command(&app.engine, "setup.io_export");
            }
            if ui
                .button("Import Settings...")
                .on_hover_text("Merge a .scio settings file into the I/O Setup by name (busses keep their routing)")
                .clicked()
            {
                app.dialogs.open_for_command(&app.engine, "setup.io_import");
            }
            ui.label(RichText::new("The routing is saved with the session; settings files carry it to other sessions and systems.").small().weak());
        });
    });
    if st.new_path.is_some() {
        new_paths_dialog(app, ctx, &mut st);
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
    app.ui.show_io_setup = open;
}

/// The input device the grid shows and how many inputs it has: the open one, else the one the
/// Playback Engine chooses (enumerated once per window), else nothing known. The count is every
/// channel the device offers, not just those the open stream carries.
fn input_device(app: &SoundApp, ctx: &egui::Context) -> (String, usize) {
    let systems = crate::windows::audio_systems(ctx, false);
    let sys = crate::windows::chosen_system(&systems, &app.ui.audio);
    let open = app.recorder.as_ref().map(|r| (r.device_name.clone(), r.channels));
    let known = sys.and_then(|s| match &open {
        Some((name, _)) => {
            s.inputs.iter().find(|d| d.name == *name).or_else(|| crate::windows::chosen_device(&s.inputs, app.ui.audio.input.as_deref()))
        }
        None => crate::windows::chosen_device(&s.inputs, app.ui.audio.input.as_deref()),
    });
    match (open, known) {
        (Some((name, ch)), Some(d)) => (name, ch.max(usize::from(d.max_channels))),
        (Some((name, ch)), None) => (name, ch),
        (None, Some(d)) => (d.name.clone(), usize::from(d.max_channels)),
        (None, None) => ("Input device".into(), 0),
    }
}

/// The output device and how many outputs it has, likewise.
fn output_device(app: &SoundApp, ctx: &egui::Context) -> (String, usize) {
    let systems = crate::windows::audio_systems(ctx, false);
    let sys = crate::windows::chosen_system(&systems, &app.ui.audio);
    let open = app.player.as_ref().filter(|p| !p.silent).map(|p| (p.device_name.clone(), usize::from(p.device_channels)));
    let known = sys.and_then(|s| match &open {
        Some((name, _)) => {
            s.outputs.iter().find(|d| d.name == *name).or_else(|| crate::windows::chosen_device(&s.outputs, app.ui.audio.output.as_deref()))
        }
        None => crate::windows::chosen_device(&s.outputs, app.ui.audio.output.as_deref()),
    });
    match (open, known) {
        (Some((name, ch)), Some(d)) => (name, ch.max(usize::from(d.max_channels))),
        (Some((name, ch)), None) => (name, ch),
        (None, Some(d)) => (d.name.clone(), usize::from(d.max_channels)),
        (None, None) => ("Output device".into(), 0),
    }
}

/// Columns a device grid shows: the device's channels, at least eight, and every channel a row uses.
pub(crate) fn grid_columns(device_channels: usize, rows: &[Row]) -> usize {
    let used = rows.iter().flat_map(|r| r.channels.iter().flatten()).map(|c| usize::from(*c).saturating_add(1)).max().unwrap_or(0);
    device_channels.max(used).max(8).min(usize::from(MAX_INPUT_CHANNEL).saturating_add(1))
}

/// What a click in a row does: on a cell the path is on, that channel is cleared; on any other
/// cell the path maps from that channel on, its channels filling to the right ("clicking under
/// channel 1 fills both channel 1 and 2").
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CellClick {
    Clear(usize),
    MapFrom(u16),
}

pub(crate) fn cell_click(channels: &[Option<u16>], hw: u16) -> CellClick {
    match channels.iter().position(|c| *c == Some(hw)) {
        Some(k) => CellClick::Clear(k),
        None => CellClick::MapFrom(hw),
    }
}

/// The label a row shows under channel `hw`: the path channel's speaker (L, R, C, LFE…) when the
/// path is on it, and M for a mono path, as Pro Tools' grid shows them.
pub(crate) fn cell_label(format: ChannelFormat, channels: &[Option<u16>], hw: u16) -> Option<String> {
    channels.iter().position(|c| *c == Some(hw)).map(|k| if format == ChannelFormat::Mono { "M".to_string() } else { format.channel_label(k) })
}

/// The rows of the Input tab: main paths each followed by its sub-paths.
pub(crate) fn input_rows(paths: &[InputPath]) -> Vec<Row> {
    let mut rows = Vec::new();
    for m in paths {
        rows.push(Row {
            name: m.name.clone(),
            format: m.format,
            channels: m.channels.clone(),
            active: m.active,
            is_sub: false,
            within: None,
            has_subs: !m.sub_paths.is_empty(),
            mapping: None,
            monitor: false,
            kind: RowKind::Input,
        });
        for sp in &m.sub_paths {
            rows.push(Row {
                name: sp.name.clone(),
                format: sp.format,
                channels: sp.channels.clone(),
                active: m.active && sp.active,
                is_sub: true,
                within: Some(m.channels.clone()),
                has_subs: false,
                mapping: None,
                monitor: false,
                kind: RowKind::Input,
            });
        }
    }
    rows
}

/// The rows of the Output tab: the paths, none with sub-paths; `monitor` names the monitor path.
pub(crate) fn output_rows(paths: &[OutputPath], monitor: Option<&str>) -> Vec<Row> {
    paths
        .iter()
        .map(|o| Row {
            name: o.name.clone(),
            format: o.format,
            channels: o.channels.clone(),
            active: o.active,
            is_sub: false,
            within: None,
            has_subs: false,
            mapping: None,
            monitor: monitor.is_some_and(|m| m.eq_ignore_ascii_case(&o.name)),
            kind: RowKind::Output,
        })
        .collect()
}

/// The rows of the Bus tab: each bus (its channels in order on the bus grid) followed by its
/// sub-paths (on the bus channels they read).
pub(crate) fn bus_rows(busses: &[Bus]) -> Vec<Row> {
    let mut rows = Vec::new();
    for b in busses {
        let own: Vec<Option<u16>> = (0..b.width()).map(|k| u16::try_from(k).ok()).collect();
        rows.push(Row {
            name: b.name.clone(),
            format: b.format,
            channels: own.clone(),
            active: b.active,
            is_sub: false,
            within: None,
            has_subs: !b.sub_paths.is_empty(),
            mapping: b.output.clone(),
            monitor: false,
            kind: RowKind::Bus,
        });
        for sp in &b.sub_paths {
            rows.push(Row {
                name: sp.name.clone(),
                format: sp.format,
                channels: sp.channels.iter().map(|c| c.map(u16::from)).collect(),
                active: b.active,
                is_sub: true,
                within: Some(own.clone()),
                has_subs: false,
                mapping: None,
                monitor: false,
                kind: RowKind::BusSub,
            });
        }
    }
    rows
}

/// Columns the Bus tab's grid shows: the widest bus, at least two.
pub(crate) fn bus_columns(busses: &[Bus]) -> usize {
    busses.iter().map(Bus::width).max().unwrap_or(0).clamp(2, 16)
}

/// The `setup.io` command for a row's control.
fn action(kind: RowKind, what: &str) -> &'static str {
    match (kind, what) {
        (RowKind::Input, "active") => "set_input_active",
        (RowKind::Input, "rename") => "rename_input",
        (RowKind::Input, "format") => "set_input_format",
        (RowKind::Input, "map") => "map_input",
        (RowKind::Input, "channel") => "set_input_channel",
        (RowKind::Output, "active") => "set_output_active",
        (RowKind::Output, "rename") => "rename_output",
        (RowKind::Output, "format") => "set_output_format",
        (RowKind::Output, "map") => "map_output",
        (RowKind::Output, "channel") => "set_output_channel",
        (RowKind::Bus | RowKind::BusSub, "active") => "set_bus_active",
        (RowKind::Bus | RowKind::BusSub, "rename") => "rename_bus",
        (RowKind::Bus | RowKind::BusSub, "format") => "set_bus_format",
        (RowKind::BusSub, "map") => "map_bus_subpath",
        (RowKind::BusSub, "channel") => "set_bus_subpath_channel",
        _ => "list",
    }
}

/// Everything a tab needs to draw its grid.
struct GridSpec<'a> {
    device: &'a str,
    columns: usize,
    /// The Bus tab's extra column.
    mapping: bool,
    outputs: &'a [(String, usize)],
}

fn input_tab(app: &mut SoundApp, ui: &mut Ui, st: &mut State) {
    let t = Tokens::current();
    let (device, device_channels) = input_device(app, ui.ctx());
    let rows = input_rows(&app.engine.session().inputs);
    let columns = grid_columns(device_channels, &rows);
    let mut pending: Vec<(&'static str, Value)> = Vec::new();
    let mut select: Option<Option<String>> = None;
    let spec = GridSpec { device: &device, columns, mapping: false, outputs: &[] };
    grid(ui, st, &rows, &spec, &mut pending, &mut select, "io-in");
    if let Some(sel) = select {
        st.selected = sel;
    }
    ui.add_space(6.0);
    let selected_main = st.selected.as_deref().and_then(|n| app.engine.session().find_input_path(n)).is_some_and(|(_, sub)| sub.is_none());
    let selected_any = st.selected.as_deref().and_then(|n| app.engine.session().find_input_path(n)).is_some();
    ui.horizontal(|ui| {
        if ui.button("New Path...").clicked() {
            st.new_path = Some(NewPath {
                tab: Tab::Input,
                count: 1,
                format: st.default_format.unwrap_or(ChannelFormat::Stereo),
                name: String::new(),
                assign: true,
                sub_paths: true,
            });
        }
        if ui.add_enabled(selected_main, egui::Button::new("New Sub-Path")).on_hover_text("A path within the selected main path").clicked()
            && let Some(name) = &st.selected
        {
            pending.push(("setup.io", json!({"action": "create_input_subpath", "path": name, "format": "Mono"})));
        }
        if ui.add_enabled(selected_any, egui::Button::new("Delete Path")).clicked()
            && let Some(name) = st.selected.take()
        {
            pending.push(("setup.io", json!({"action": "delete_input", "name": name})));
        }
        // Every input the device reports, and never fewer than a new session has (some drivers
        // report one stereo pair of a four-input interface).
        let every = device_channels.max(8);
        if ui
            .button("Default")
            .on_hover_text(format!("Every input of the device ({every}): a stereo path per channel pair, with mono sub-paths"))
            .clicked()
        {
            pending.push(("setup.io", json!({"action": "default_inputs", "channels": every})));
            st.selected = None;
        }
        default_format_combo(ui, st);
    });
    ui.label(
        RichText::new("Click a cell to feed the path from that channel on; click a lit cell to clear it. Double-click a name to rename it. A sub-path reads its main path's channels only. Inactive paths are shown in italics and feed nothing.")
            .small()
            .color(t.text_dim),
    );
    run(app, pending);
}

fn output_tab(app: &mut SoundApp, ui: &mut Ui, st: &mut State) {
    let t = Tokens::current();
    let (device, device_channels) = output_device(app, ui.ctx());
    let rows = {
        let s = app.engine.session();
        output_rows(&s.outputs, s.monitor_output_path().map(|o| o.name.as_str()))
    };
    let columns = grid_columns(device_channels, &rows);
    let mut pending: Vec<(&'static str, Value)> = Vec::new();
    let mut select: Option<Option<String>> = None;
    let spec = GridSpec { device: &device, columns, mapping: false, outputs: &[] };
    grid(ui, st, &rows, &spec, &mut pending, &mut select, "io-out");
    if let Some(sel) = select {
        st.selected = sel;
    }
    ui.add_space(6.0);
    let selected_any = st.selected.as_deref().and_then(|n| app.engine.session().find_output_path(n)).is_some();
    let (monitor, meter) = {
        let s = app.engine.session();
        (s.monitor_output_path().map(|o| o.name.clone()), s.meter_output_path().map(|o| o.name.clone()))
    };
    let paths: Vec<String> = app.engine.session().outputs.iter().filter(|o| o.active).map(|o| o.name.clone()).collect();
    ui.horizontal(|ui| {
        if ui.button("New Path...").clicked() {
            st.new_path = Some(NewPath {
                tab: Tab::Output,
                count: 1,
                format: st.default_format.unwrap_or(ChannelFormat::Stereo),
                name: String::new(),
                assign: true,
                sub_paths: false,
            });
        }
        ui.add_enabled(false, egui::Button::new("New Sub-Path")).on_disabled_hover_text("Output paths have no sub-paths; they may overlap instead");
        if ui.add_enabled(selected_any, egui::Button::new("Delete Path")).clicked()
            && let Some(name) = st.selected.take()
        {
            pending.push(("setup.io", json!({"action": "delete_output", "name": name})));
        }
        let every = device_channels.max(2);
        if ui
            .button("Default")
            .on_hover_text(format!("Every output of the device ({every}): a stereo path per channel pair, the first the monitor path"))
            .clicked()
        {
            pending.push(("setup.io", json!({"action": "default_outputs", "channels": every})));
            st.selected = None;
        }
        default_format_combo(ui, st);
        ui.add_space(16.0);
        ui.label("Monitor Path:");
        path_combo(ui, "io-monitor", monitor.as_deref(), &paths, |name| {
            pending.push(("setup.io", json!({"action": "set_monitor_output", "name": name})))
        });
        ui.label("Output Meter Path:");
        path_combo(ui, "io-meter", meter.as_deref(), &paths, |name| pending.push(("setup.io", json!({"action": "set_meter_output", "name": name}))));
    });
    ui.label(
        RichText::new("The monitor path carries the main mix (its format is the mix's); the meter path is what the main meter shows. Click a cell to drive the device from that channel on; click a lit cell to clear it. Paths may overlap. Inactive paths drive nothing.")
            .small()
            .color(t.text_dim),
    );
    run(app, pending);
}

fn bus_tab(app: &mut SoundApp, ui: &mut Ui, st: &mut State) {
    let t = Tokens::current();
    let busses = app.engine.session().busses.clone();
    let rows = bus_rows(&busses);
    let columns = bus_columns(&busses);
    let outputs: Vec<(String, usize)> = app.engine.session().outputs.iter().filter(|o| o.active).map(|o| (o.name.clone(), o.width())).collect();
    let mut pending: Vec<(&'static str, Value)> = Vec::new();
    let mut select: Option<Option<String>> = None;
    let spec = GridSpec { device: "Bus channels", columns, mapping: true, outputs: &outputs };
    grid(ui, st, &rows, &spec, &mut pending, &mut select, "io-bus");
    if let Some(sel) = select {
        st.selected = sel;
    }
    ui.add_space(6.0);
    let selected = st.selected.as_deref().and_then(|n| app.engine.session().bus_path_by_name(n).map(|(_, sp)| sp.is_none()));
    ui.horizontal(|ui| {
        if ui.button("New Path...").clicked() {
            st.new_path = Some(NewPath {
                tab: Tab::Bus,
                count: 1,
                format: st.default_format.unwrap_or(ChannelFormat::Stereo),
                name: String::new(),
                assign: false,
                sub_paths: true,
            });
        }
        if ui.add_enabled(selected == Some(true), egui::Button::new("New Sub-Path")).on_hover_text("A path within the selected bus").clicked()
            && let Some(name) = &st.selected
        {
            pending.push(("setup.io", json!({"action": "create_bus_subpath", "bus": name, "format": "Mono"})));
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new("Delete Path")).clicked()
            && let Some(name) = st.selected.take()
        {
            pending.push(("setup.io", json!({"action": "delete_bus", "name": name})));
        }
        if ui
            .button("Default")
            .on_hover_text("Internal busses Bus 1-2, Bus 3-4… with mono sub-paths, and/or a bus per output path mapped to it")
            .clicked()
        {
            pending.push(("setup.io", json!({"action": "default_busses", "which": st.bus_default.id(), "count": 8})));
        }
        egui::ComboBox::from_id_salt("io-bus-default").selected_text(st.bus_default.label()).show_ui(ui, |ui| {
            for d in [BusDefault::All, BusDefault::Output, BusDefault::Internal] {
                ui.selectable_value(&mut st.bus_default, d, d.label());
            }
        });
        default_format_combo(ui, st);
    });
    ui.label(
        RichText::new("Mapping to Output: the bus's mix also drives that output path (Alt-click the selector: every bus of the format; Alt+Ctrl: cascading). Sub-paths are mapped on the bus's channels. Click a column head to sort. Inactive busses carry nothing.")
            .small()
            .color(t.text_dim),
    );
    run(app, pending);
}

fn run(app: &mut SoundApp, pending: Vec<(&'static str, Value)>) {
    for (cmd, p) in pending {
        if let Err(e) = app.run(cmd, p) {
            app.ui.status = e;
        }
    }
}

fn default_format_combo(ui: &mut Ui, st: &mut State) {
    ui.add_space(12.0);
    ui.label("Default Format:");
    let shown = st.default_format.unwrap_or(ChannelFormat::Stereo);
    egui::ComboBox::from_id_salt("io-default-format").selected_text(shown.label()).show_ui(ui, |ui| {
        for f in formats_up_to(16) {
            if ui.selectable_label(f == shown, f.label()).clicked() {
                st.default_format = Some(f);
            }
        }
    });
}

/// A selector of output paths; `choose` gets the name picked, `null` for the default.
fn path_combo(ui: &mut Ui, id: &str, current: Option<&str>, paths: &[String], mut choose: impl FnMut(Value)) {
    egui::ComboBox::from_id_salt(id).width(120.0).selected_text(current.unwrap_or("(none)")).show_ui(ui, |ui| {
        if ui.selectable_label(false, "Default (first path)").clicked() {
            choose(Value::Null);
        }
        for p in paths {
            if ui.selectable_label(current == Some(p.as_str()), p).clicked() {
                choose(json!(p));
            }
        }
    });
}

/// A tab's grid: the column heads (clickable on the Bus tab to sort), then a row per path.
fn grid(
    ui: &mut Ui,
    st: &mut State,
    rows: &[Row],
    spec: &GridSpec<'_>,
    pending: &mut Vec<(&'static str, Value)>,
    select: &mut Option<Option<String>>,
    id: &str,
) {
    let t = Tokens::current();
    let button_h = 120.0;
    let extra = usize::from(spec.mapping);
    egui::ScrollArea::both().id_salt(id).max_height((ui.available_height() - button_h).max(120.0)).show(ui, |ui| {
        egui::Grid::new(id).num_columns(4 + extra + spec.columns).spacing(vec2(2.0, 1.0)).min_col_width(0.0).show(ui, |ui| {
            // The device's name over its channels, then the column heads.
            for _ in 0..4 + extra {
                ui.label("");
            }
            for c in 0..spec.columns {
                let (r, _) = ui.allocate_exact_size(vec2(CELL, ROW), Sense::hover());
                if c == 0 {
                    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, spec.device, egui::FontId::proportional(11.0), t.text);
                }
            }
            ui.end_row();
            ui.label("");
            ui.label("");
            let head = |ui: &mut Ui, text: &str, sort: Option<&str>| {
                let label = RichText::new(text).strong().size(11.0);
                match sort {
                    Some(by) => ui.add(egui::Button::new(label).frame(false).small()).on_hover_text("Sort").clicked().then(|| by.to_string()),
                    None => {
                        ui.label(label);
                        None
                    }
                }
            };
            let sorts = [head(ui, "Name", spec.mapping.then_some("name")), head(ui, "Format", spec.mapping.then_some("format"))];
            let mapping_sort = if spec.mapping { head(ui, "Mapping to Output", Some("output")) } else { None };
            for by in sorts.into_iter().chain(std::iter::once(mapping_sort)).flatten() {
                pending.push(("setup.io", json!({"action": "sort_busses", "by": by})));
            }
            for c in 0..spec.columns {
                let (r, _) = ui.allocate_exact_size(vec2(CELL, ROW), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", c + 1), egui::FontId::proportional(10.0), t.text_dim);
            }
            ui.end_row();
            let mut folded_main: Option<String> = None;
            for row in rows {
                if row.is_sub && folded_main.as_deref().is_some_and(|_| true) {
                    continue;
                }
                if !row.is_sub {
                    folded_main = st.collapsed.contains(&row.name).then(|| row.name.clone());
                }
                path_row(ui, st, row, spec, pending, select);
            }
        });
    });
}

/// One row of the grid: the path's controls, then a cell per channel.
fn path_row(
    ui: &mut Ui,
    st: &mut State,
    row: &Row,
    spec: &GridSpec<'_>,
    pending: &mut Vec<(&'static str, Value)>,
    select: &mut Option<Option<String>>,
) {
    let t = Tokens::current();
    let selected = st.selected.as_deref() == Some(row.name.as_str());
    // Active.
    let mut on = row.active;
    let toggles = !(row.is_sub && row.kind == RowKind::BusSub);
    if crate::widgets::checkbox(ui, &mut on, "").on_hover_text(if row.is_sub { "Active (with its main path)" } else { "Active" }).changed() && toggles
    {
        pending.push(("setup.io", json!({"action": action(row.kind, "active"), "name": row.name, "active": on})));
    }
    // Fold.
    if row.is_sub || !row.has_subs {
        ui.label("");
    } else {
        let folded = st.collapsed.contains(&row.name);
        if ui.add(egui::Button::new(if folded { "▶" } else { "▼" }).frame(false).small()).clicked() {
            if folded {
                st.collapsed.remove(&row.name);
            } else {
                st.collapsed.insert(row.name.clone());
            }
        }
    }
    // Name: select on click, rename on double-click.
    match &mut st.renaming {
        Some((name, text)) if name == &row.name => {
            let r = ui.add(egui::TextEdit::singleline(text).desired_width(if row.is_sub { 160.0 } else { 180.0 }));
            r.request_focus();
            let done = r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if cancel {
                st.renaming = None;
            } else if done {
                let to = text.trim().to_string();
                if !to.is_empty() && to != row.name {
                    pending.push(("setup.io", json!({"action": action(row.kind, "rename"), "name": row.name, "new_name": to})));
                    *select = Some(Some(to));
                }
                st.renaming = None;
            }
        }
        _ => {
            let mut label = RichText::new(if row.is_sub { format!("    {}", row.name) } else { row.name.clone() }).size(12.0);
            if !row.active {
                label = label.italics().color(t.text_dim);
            }
            let r = ui.selectable_label(selected, label).on_hover_text("Click to select; double-click to rename");
            if r.double_clicked() {
                st.renaming = Some((row.name.clone(), row.name.clone()));
            } else if r.clicked() {
                *select = Some(Some(row.name.clone()));
            }
        }
    }
    // Format: a sub-path fits within its main path; the monitor path's format is the main mix's
    // (Setup › Main Format), two to sixteen channels.
    let max = row.within.as_ref().map_or(16, Vec::len);
    egui::ComboBox::from_id_salt(("io-fmt", &row.name)).width(70.0).selected_text(RichText::new(row.format.label()).size(11.0)).show_ui(ui, |ui| {
        for f in formats_up_to(max).filter(|f| !row.monitor || f.channels() >= 2) {
            if ui.selectable_label(f == row.format, f.label()).clicked() && f != row.format {
                if row.monitor {
                    pending.push(("setup.main_format", json!({"format": f.label()})));
                } else {
                    pending.push(("setup.io", json!({"action": action(row.kind, "format"), "name": row.name, "format": f.label()})));
                }
            }
        }
    });
    // Mapping to Output (Bus tab).
    if spec.mapping {
        if row.kind == RowKind::Bus {
            let width = row.format.channels();
            let fitting: Vec<&(String, usize)> = spec.outputs.iter().filter(|(_, w)| *w >= width).collect();
            ui.horizontal(|ui| {
                let mut mapped = row.mapping.is_some();
                if crate::widgets::checkbox(ui, &mut mapped, "").on_hover_text("Mapping to Output").changed() {
                    let output = if mapped { fitting.first().map(|(n, _)| json!(n)).unwrap_or(Value::Null) } else { Value::Null };
                    pending.push(("setup.io", json!({"action": "map_bus_output", "name": row.name, "output": output})));
                }
                let shown = row.mapping.clone().unwrap_or_else(|| "(none)".into());
                egui::ComboBox::from_id_salt(("io-bus-map", &row.name)).width(110.0).selected_text(RichText::new(shown).size(11.0)).show_ui(
                    ui,
                    |ui| {
                        let mods = ui.input(|i| i.modifiers);
                        if ui.selectable_label(row.mapping.is_none(), "(none)").clicked() {
                            pending.push(("setup.io", json!({"action": "map_bus_output", "name": row.name, "output": null, "all": mods.alt})));
                        }
                        for (name, _) in fitting {
                            if ui.selectable_label(row.mapping.as_deref() == Some(name.as_str()), name).clicked() {
                                let (all, cascade) = bus_mapping_modifiers(mods);
                                pending.push((
                                    "setup.io",
                                    json!({"action": "map_bus_output", "name": row.name, "output": name, "all": all, "cascade": cascade}),
                                ));
                            }
                        }
                    },
                );
            });
        } else {
            ui.label("");
        }
    }
    // The cells.
    let clickable = matches!(row.kind, RowKind::Input | RowKind::Output | RowKind::BusSub);
    for c in 0..spec.columns {
        let hw = u16::try_from(c).unwrap_or(u16::MAX);
        let enabled = clickable && row.within.as_ref().is_none_or(|w| w.contains(&Some(hw)));
        let label = cell_label(row.format, &row.channels, hw);
        let (r, resp) = ui.allocate_exact_size(vec2(CELL, ROW), if enabled { Sense::click() } else { Sense::hover() });
        let p = ui.painter();
        let fill = if !enabled && label.is_none() {
            t.panel_bg
        } else if label.is_some() {
            if row.active { t.button_hi } else { t.button }
        } else if resp.hovered() {
            t.panel_bg2
        } else {
            t.slot_bg
        };
        p.rect(r.shrink(0.5), CornerRadius::same(2), fill, Stroke::new(1.0, if selected { t.accent } else { t.border }), StrokeKind::Inside);
        if let Some(l) = &label {
            p.text(r.center(), Align2::CENTER_CENTER, l, egui::FontId::proportional(10.0), if row.active { t.text } else { t.text_dim });
        }
        if resp.clicked() {
            *select = Some(Some(row.name.clone()));
            let (map, channel) = (action(row.kind, "map"), action(row.kind, "channel"));
            match cell_click(&row.channels, hw) {
                CellClick::Clear(k) if row.kind == RowKind::BusSub => {
                    pending.push(("setup.io", json!({"action": channel, "name": row.name, "channel": k, "bus_channel": null})))
                }
                CellClick::Clear(k) => pending.push(("setup.io", json!({"action": channel, "name": row.name, "channel": k, "hardware": null}))),
                CellClick::MapFrom(hw) if row.kind == RowKind::BusSub => {
                    pending.push(("setup.io", json!({"action": map, "name": row.name, "channel": hw})))
                }
                CellClick::MapFrom(hw) => pending.push(("setup.io", json!({"action": map, "name": row.name, "hardware": hw}))),
            }
        }
    }
    ui.end_row();
}

/// Pro Tools' modifiers on a bus's Mapping to Output selector: Alt applies the choice to every
/// bus of the format, Alt+Cmd (Ctrl) cascades them over the output paths from it.
pub(crate) fn bus_mapping_modifiers(mods: egui::Modifiers) -> (bool, bool) {
    (mods.alt && !mods.command, mods.alt && mods.command)
}

/// The formats a path may have, up to `max` channels (Ambisonics has no speakers to map).
fn formats_up_to(max: usize) -> impl Iterator<Item = ChannelFormat> {
    ChannelFormat::ALL.into_iter().filter(move |f| !f.is_ambisonic() && f.channels() <= max)
}

/// New Paths: how many, their format and name, and the two options Pro Tools' dialog has.
fn new_paths_dialog(app: &mut SoundApp, ctx: &egui::Context, st: &mut State) {
    let Some(draft) = &mut st.new_path else { return };
    let mut close = false;
    let mut create = false;
    egui::Window::new("New Paths").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
        egui::Grid::new("io-new").num_columns(3).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            ui.label(RichText::new("Number of new paths").small());
            ui.label(RichText::new("Path format").small());
            ui.label(RichText::new("Path name").small());
            ui.end_row();
            ui.add(egui::DragValue::new(&mut draft.count).range(1..=64));
            egui::ComboBox::from_id_salt("io-new-format").selected_text(draft.format.label()).show_ui(ui, |ui| {
                for f in formats_up_to(16) {
                    ui.selectable_value(&mut draft.format, f, f.label());
                }
            });
            let r = ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(180.0).hint_text("Name"));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            ui.end_row();
        });
        if draft.tab != Tab::Bus {
            crate::widgets::checkbox(ui, &mut draft.assign, "Add default channel assignments");
        }
        if draft.tab != Tab::Output {
            crate::widgets::checkbox(ui, &mut draft.sub_paths, "Auto-create sub-paths");
        }
        ui.label(RichText::new("Several paths are numbered: Name 1, Name 2, …").small().weak());
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.add_enabled(!draft.name.trim().is_empty(), egui::Button::new("Create")).clicked() {
                create = true;
            }
        });
    });
    if create && !draft.name.trim().is_empty() {
        let mut p = json!({"name": draft.name.trim(), "format": draft.format.label(), "count": draft.count});
        match draft.tab {
            Tab::Input => {
                p["action"] = json!("create_input");
                p["assign"] = json!(draft.assign);
                p["sub_paths"] = json!(draft.sub_paths);
                let n = input_device(app, ctx).1;
                if n > 0 {
                    p["device_channels"] = json!(n);
                }
            }
            Tab::Output => {
                p["action"] = json!("create_output");
                p["assign"] = json!(draft.assign);
                let n = output_device(app, ctx).1;
                if n > 0 {
                    p["device_channels"] = json!(n);
                }
            }
            Tab::Bus => {
                p["action"] = json!("create_bus");
                p["sub_paths"] = json!(draft.sub_paths);
            }
        }
        // One bus with no sub-paths is the plain New Bus command.
        let cmd = if draft.tab == Tab::Bus && draft.count == 1 && !draft.sub_paths { "mix.new_bus" } else { "setup.io" };
        match app.run(cmd, p) {
            Ok(_) => close = true,
            Err(e) => app.ui.status = e,
        }
    }
    if close {
        st.new_path = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::Session;

    #[test]
    fn the_grid_is_as_wide_as_the_device_or_the_paths_but_at_least_eight() {
        let paths = Session::default_input_paths(4);
        let rows = input_rows(&paths);
        assert_eq!(rows.len(), 6);
        assert_eq!(grid_columns(2, &rows), 8);
        assert_eq!(grid_columns(16, &rows), 16);
        let mut wide = paths.clone();
        wide[0].channels[1] = Some(31);
        assert_eq!(grid_columns(2, &input_rows(&wide)), 32, "a path on channel 32 needs the column");
        wide[1].sub_paths[0].channels[0] = Some(40);
        assert_eq!(grid_columns(2, &input_rows(&wide)), 41, "sub-paths count too");
        wide[0].channels[0] = Some(u16::MAX);
        assert_eq!(grid_columns(2, &input_rows(&wide)), usize::from(MAX_INPUT_CHANNEL) + 1, "capped");
        let rows = output_rows(&Session::default_output_paths(12), Some("out 3-4"));
        assert_eq!(grid_columns(2, &rows), 12);
        assert_eq!(rows.iter().filter(|r| r.monitor).map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Out 3-4"]);
    }

    #[test]
    fn a_click_clears_a_lit_cell_or_maps_the_path_from_the_clicked_channel() {
        let p = InputPath::assigned("In 3-4", ChannelFormat::Stereo, 2);
        assert_eq!(cell_click(&p.channels, 2), CellClick::Clear(0));
        assert_eq!(cell_click(&p.channels, 3), CellClick::Clear(1));
        assert_eq!(cell_click(&p.channels, 5), CellClick::MapFrom(5));
        assert_eq!(cell_label(p.format, &p.channels, 2).as_deref(), Some("L"));
        assert_eq!(cell_label(p.format, &p.channels, 3).as_deref(), Some("R"));
        assert_eq!(cell_label(p.format, &p.channels, 4), None);
        let five = OutputPath::assigned("Surr", ChannelFormat::Surround51, 0);
        assert_eq!(cell_label(five.format, &five.channels, 3).as_deref(), Some("LFE"));
        assert_eq!(cell_label(five.format, &five.channels, 5).as_deref(), Some("Rs"));
        let mono = InputPath::assigned("In 1", ChannelFormat::Mono, 0);
        assert_eq!(cell_label(mono.format, &mono.channels, 0).as_deref(), Some("M"));
    }

    #[test]
    fn bus_rows_put_the_bus_on_its_own_channels_and_sub_paths_within() {
        let mut s = Session::new("x", Default::default());
        s.outputs = Session::default_output_paths(4);
        s.add_default_busses(1);
        s.add_bus("Surr", ChannelFormat::Surround51);
        let rows = bus_rows(&s.busses);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Bus 1-2", "Bus 1-2.L", "Bus 1-2.R", "Surr"]);
        assert_eq!(rows[0].channels, [Some(0), Some(1)]);
        assert_eq!((rows[0].kind, rows[0].has_subs, rows[0].mapping.clone()), (RowKind::Bus, true, None));
        assert_eq!((rows[2].kind, rows[2].channels.clone(), rows[2].within.clone()), (RowKind::BusSub, vec![Some(1)], Some(vec![Some(0), Some(1)])));
        assert_eq!(bus_columns(&s.busses), 6);
        assert_eq!(bus_columns(&[]), 2);
        assert_eq!(cell_label(rows[0].format, &rows[0].channels, 1).as_deref(), Some("R"));
        assert_eq!(action(RowKind::BusSub, "map"), "map_bus_subpath");
        assert_eq!(action(RowKind::Output, "channel"), "set_output_channel");
        assert_eq!(action(RowKind::Bus, "map"), "list", "a bus row's cells are its own channels, not mapped");
    }

    #[test]
    fn the_mapping_selectors_modifiers_and_the_formats_offered() {
        use egui::Modifiers;
        assert_eq!(bus_mapping_modifiers(Modifiers::NONE), (false, false));
        assert_eq!(bus_mapping_modifiers(Modifiers::ALT), (true, false));
        assert_eq!(bus_mapping_modifiers(Modifiers::ALT | Modifiers::COMMAND), (false, true));
        assert_eq!(bus_mapping_modifiers(Modifiers::COMMAND), (false, false));
        let mono: Vec<ChannelFormat> = formats_up_to(1).collect();
        assert_eq!(mono, [ChannelFormat::Mono]);
        assert!(formats_up_to(2).any(|f| f == ChannelFormat::Stereo));
        assert!(formats_up_to(16).all(|f| !f.is_ambisonic() && f.channels() <= 16));
    }
}
