//! SoundCraft's egui user interface.
//!
//! The UI is a thin shell over [`soundcraft_engine::Engine`]: it draws engine state and acts by
//! executing commands. It can be replaced without touching anything below it.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod appearance;
pub mod bounce;
pub mod control;
pub mod credits;
pub mod dialogs;
pub mod docking;
pub mod edit_window;
pub mod extra_windows;
pub mod fonts;
pub mod icons;
pub mod import;
pub mod menus;
pub mod midi_editor;
pub mod mix_window;
pub mod ops_windows;
pub mod palette;
pub mod panels;
mod recent;
pub mod recovery;
pub mod sampler_window;
pub mod score_editor;
pub mod shortcuts;
pub mod spot;
pub mod theme;
pub mod toolbar;
pub mod updates_ui;
pub mod video_track;
pub mod video_window;
pub mod waveform;
pub mod widgets;
pub mod windows;

use serde_json::{Value, json};
use soundcraft_engine::{Engine, TransportRequest};
use soundcraft_model::TrackId;
use soundcraft_playback::record::Trouble;
use soundcraft_playback::{OutputTrouble, Player};
use soundcraft_time::{Range, Samples};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;

pub use control::ControlRequest;

/// Which main window is in front (Window › Mix / Edit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MainWindow {
    Edit,
    Mix,
}

/// UI state (serde, so agents can read and set it).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    /// File › Open Recent, newest first (see `recent`).
    pub recent_sessions: Vec<String>,
    pub docking: docking::Workspace,
    #[serde(default = "docking::mix_default")]
    pub mix_docking: docking::Workspace,
    /// [`docking::LAYOUT_VERSION`] of `docking` and `mix_docking`; 0 in settings saved before it.
    #[serde(default)]
    pub docking_version: u32,
    pub window: MainWindow,
    pub show_tracks_list: bool,
    pub show_clip_list: bool,
    pub show_transport: bool,
    pub show_memory_locations: bool,
    pub show_big_counter: bool,
    pub show_undo_history: bool,
    pub show_session_info: bool,
    pub show_audio_health: bool,
    pub show_about: bool,
    pub show_midi_editor: bool,
    pub show_universe: bool,
    pub narrow_mix: bool,
    /// Mix window sections: inserts_ae, inserts_fj, sends_ae, sends_fj, io, comments, eq_curve, color.
    pub mix_views: Vec<String>,
    /// Edit window track-header columns: io, inserts_ae, sends_ae, comments, color.
    pub edit_views: Vec<String>,
    /// How strongly Edit window tracks take on their colour, 0..1 (0: only the colour strip).
    pub track_tint: f32,
    /// Open plugin windows (track, slot).
    pub plugin_windows: Vec<(TrackId, usize)>,
    /// The send in the Send window (track, slot).
    #[serde(skip)]
    pub send_window: Option<(TrackId, usize)>,
    pub audiosuite: Option<String>,
    pub status: String,
    pub show_automation: bool,
    pub show_color_palette: bool,
    pub show_disk_usage: bool,
    pub show_system_usage: bool,
    pub show_task_manager: bool,
    pub show_metadata: bool,
    pub show_event_list: bool,
    pub show_midi_keyboard: bool,
    pub show_workspace: bool,
    pub show_configurations: bool,
    pub show_playback_engine: bool,
    /// The audio device choice (Setup › Playback Engine), kept with the preferences: per machine.
    pub audio: soundcraft_playback::AudioConfig,
    /// The Mix window lives in its own native window (Window › Mix in Separate Window); the main
    /// window then always shows Edit.
    pub mix_separate: bool,
    pub show_io_setup: bool,
    pub show_shortcuts: bool,
    pub show_clip_effects: bool,
    pub show_beat_detective: bool,
    pub show_tempo_ops: bool,
    pub show_time_ops: bool,
    pub show_midi_ops: bool,
    pub show_rtp: bool,
    pub show_score: bool,
    pub show_search: bool,
    /// Window › Video, Window › Video Universe, timecode burn-in in the Video window.
    pub show_video: bool,
    pub show_video_universe: bool,
    pub video_burn_in: bool,
    pub workspace_dir: String,
    pub configurations: Vec<(String, Value)>,
    /// Appearance (`ui.theme`): dark by default, light or follow the system on request.
    pub theme: theme::ThemeMode,
    /// The main window's size (logical points) and maximized state, and the interface scale, as
    /// the desktop app last saw them; it restores them at launch.
    pub window_size: [f32; 2],
    pub window_maximized: bool,
    pub ui_scale: f32,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            recent_sessions: Vec::new(),
            docking: docking::edit_default(),
            mix_docking: docking::mix_default(),
            docking_version: docking::LAYOUT_VERSION,
            window: MainWindow::Edit,
            show_tracks_list: true,
            show_clip_list: true,
            show_transport: false,
            show_memory_locations: false,
            show_big_counter: false,
            show_undo_history: false,
            show_session_info: false,
            show_audio_health: false,
            show_about: false,
            show_midi_editor: false,
            show_universe: false,
            narrow_mix: false,
            mix_views: vec!["inserts_ae".into(), "sends_ae".into(), "io".into(), "color".into()],
            edit_views: vec!["color".into()],
            track_tint: 0.5,
            plugin_windows: Vec::new(),
            send_window: None,
            audiosuite: None,
            status: String::new(),
            show_automation: false,
            show_color_palette: false,
            show_disk_usage: false,
            show_system_usage: false,
            show_task_manager: false,
            show_metadata: false,
            show_event_list: false,
            show_midi_keyboard: false,
            show_workspace: false,
            show_configurations: false,
            show_playback_engine: false,
            audio: soundcraft_playback::AudioConfig::default(),
            mix_separate: false,
            show_io_setup: false,
            show_shortcuts: false,
            show_clip_effects: false,
            show_beat_detective: false,
            show_tempo_ops: false,
            show_time_ops: false,
            show_midi_ops: false,
            show_rtp: false,
            show_score: false,
            show_search: false,
            show_video: false,
            show_video_universe: false,
            video_burn_in: false,
            workspace_dir: String::new(),
            configurations: Vec::new(),
            theme: theme::ThemeMode::Dark,
            window_size: [1600.0, 1000.0],
            window_maximized: false,
            ui_scale: 1.0,
        }
    }
}

/// Host services (file dialogs) injected by the app; absent in tests and on the web.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str, &[&str]) -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str, &str) -> Option<String>>>,
    pub pick_folder: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

/// File-system-safe folder name for a plugin id (`clap:com.x.y` → `clap_com.x.y`).
pub fn preset_folder_name(plugin: &str) -> String {
    plugin.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// On the first frame, maximizes a window too big for the screen it opened on (sized on a larger
/// monitor) instead of letting it run off the edge; its saved size is kept for that monitor.
pub fn fit_window_to_screen(ctx: &egui::Context) {
    let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
    if ctx.cumulative_pass_nr() == 0 && inner.zip(monitor).is_some_and(|(r, m)| (r.size() - m).max_elem() > 0.0) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Drag-and-drop payload: an audio file (source id) dragged from the Clip List.
#[derive(Debug, Clone, Copy)]
pub struct DragSource(pub u64);

/// Ballistic meter display state per strip.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterDisplay {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    pub hold_age: f32,
    pub clip: bool,
    pub gr: f32,
    /// [`soundcraft_mix::PluginFailures::count`] as last seen.
    pub plugin_failures: u32,
}

/// An in-progress mouse gesture in the Edit window.
#[derive(Debug, Clone)]
pub enum Gesture {
    Select { track: TrackId, anchor: Samples, tracks: Vec<TrackId> },
    MoveClips { clips: Vec<soundcraft_model::ClipId>, grab_at: Samples, delta: Samples, from_track: TrackId, to_track: TrackId },
    TrimStart { clip: soundcraft_model::ClipId, track: TrackId, to: Samples },
    TrimEnd { clip: soundcraft_model::ClipId, track: TrackId, to: Samples },
    Fade { clip: soundcraft_model::ClipId, track: TrackId, fade_in: bool, to: Samples },
    FadeCurve { clip: soundcraft_model::ClipId, fade_in: bool, curvature: f32, anchor: (f32, f32), height: f32 },
    Scrub { last: Samples },
    Fader { track: TrackId, start_db: f32 },
    ClipGain { clip: soundcraft_model::ClipId, track: TrackId, pos: f32, anchor: (f32, f32), fine: bool, fader_bottom: egui::Pos2 },
    Pencil { track: TrackId, points: Vec<(Samples, f32)> },
    Breakpoint { track: TrackId, param: soundcraft_model::AutoParam, at: Samples },
    MidiClip { track: TrackId, from: Samples, to: Samples },
    ZoomBox { start: f32 },
}

pub struct SoundApp {
    pub engine: Engine,
    /// The session path as of the last frame, and whether the recent list changed since the host
    /// last saved it (see `recent`).
    recent_seen: Option<String>,
    recent_changed: bool,
    /// When the recent sessions were last checked for existence, which list, and the result.
    recent_checked: Option<(f64, Vec<String>, Vec<bool>)>,
    pub player: Option<Player>,
    /// Input capture, opened on first record.
    pub recorder: Option<soundcraft_playback::record::Recorder>,
    record_start: Samples,
    /// Punch range to trim the take to (pre/post-roll recordings).
    punch: Option<Range>,
    recorder_failed: bool,
    /// Input this take lost to a slow disk, as last reported (ms).
    lost_shown: u64,
    /// Where the takes being recorded go, autosaved with the session so a crash can't lose them.
    take_plan: Option<soundcraft_engine::io::Recording>,
    pub ui: UiState,
    pub audio_health_report: Option<Result<Value, String>>,
    pub services: Services,
    /// Set only by a desktop host after installing an OS menu bar.
    pub native_menu_bar: bool,
    /// Folder for crash-recovery autosaves (native apps set it).
    pub autosave_dir: Option<std::path::PathBuf>,
    /// Writes the open session's autosaves in the background, once there is something to write.
    autosaver: Option<soundcraft_engine::autosave::Autosaver>,
    /// The document revision last handed to the autosaver (0 once that autosave is stale).
    autosaved_rev: u64,
    /// Unsaved work that earlier runs left in the autosave folder, offered for recovery.
    pub leftovers: Vec<soundcraft_engine::autosave::Leftover>,
    /// The document revision last autosaved after a caught panic.
    panic_rev: u64,
    /// Folder for user plugin presets (native apps set it).
    pub preset_dir: Option<std::path::PathBuf>,
    last_autosave: Option<std::time::Instant>,
    pub dialogs: dialogs::Dialogs,
    pub(crate) updates_rt: updates_ui::Runtime,
    pub meters: HashMap<TrackId, MeterDisplay>,
    pub main_meter: MeterDisplay,
    /// Reused meter snapshot, taken from the player at an adaptive rate by `logic`: the highest
    /// peaks since the previous take. The only reader of the player's meters; windows read this.
    pub(crate) meter_snapshot: soundcraft_playback::MeterSnapshot,
    pub gesture: Option<Gesture>,
    pub edit_layout: edit_window::EditLayout,
    pub midi: midi_editor::MidiEditorState,
    pub ops: ops_windows::OpsState,
    pub extra: extra_windows::ExtraState,
    pub palette: palette::PaletteState,
    /// Bring the separate Mix window to the front on the next frame.
    pub(crate) focus_mix: bool,
    /// Scroll Into View: the track each window brings to its top (Edit) or left (Mix) when it
    /// next draws, so a window not on screen catches up when it is shown.
    pub(crate) edit_scroll_into_view: Option<TrackId>,
    pub(crate) mix_scroll_into_view: Option<TrackId>,
    pub synthetic: Vec<egui::Event>,
    /// The one synthetic batch handed to egui and not yet applied by a settled UI pass. Hidden
    /// logic-only ticks neither apply nor drop it.
    synthetic_batch: Option<Vec<egui::Event>>,
    /// A scripted press was injected and its release has not been applied by a UI pass yet.
    synthetic_pointer_active: bool,
    /// Cancel that press before the next egui input pass, without a release click/drop.
    cancel_synthetic_pointer: bool,
    /// A key/text batch crossed its deadline after [`Self::raw_input_hook`] had already appended it
    /// to the pass. Do not dispatch that stale copy through shortcuts or widgets.
    cancel_synthetic_keys: bool,
    /// A scroll/zoom batch crossed its deadline after [`Self::raw_input_hook`] appended it to the
    /// pass. Removing the event is not enough: egui has already folded it into
    /// `InputState.smooth_scroll_delta`/`zoom_delta` and its frame-to-frame wheel smoothing.
    /// [`Self::ui`] rebuilds the pass input from [`Self::input_before_scroll`].
    cancel_synthetic_scroll: bool,
    /// The input state and the host's own raw input from before the hook appended a scroll/zoom
    /// batch. Replaying that raw from here drops the batch's wheel/zoom contribution without
    /// losing real input.
    input_before_scroll: Option<(egui::InputState, egui::RawInput)>,
    /// The exact events (the batch plus the hover `PointerMoved`s added for it) [`Self::raw_input_hook`]
    /// appended to the retained raw input. A host such as eframe hands the hook-mutated input back on
    /// the next tick, so that copy is removed before the batch is appended again: one pending batch
    /// must never grow into two copies per hidden tick.
    injected_events: Vec<egui::Event>,
    /// Where [`Self::injected_events`] starts in that retained input, when that is known. Shortcut
    /// handling and batch acknowledgement use this position to identify the injected events by
    /// provenance instead of by their (host-collidable) key values.
    injected_at: usize,
    /// The host's physical `keys_down` after the key events in this frame's raw input, before the
    /// synthetic batch was appended. `ui` restores this after egui processed the scripted release,
    /// so a scripted press while the user holds the same key does not report the key as up.
    host_keys_down: Option<std::collections::HashSet<egui::Key>>,
    /// The pointer/keyboard state of the last known UI phase. A hidden logic-only tick must not act
    /// on the previous pass's stale input.
    pointer_down: bool,
    wants_keyboard: bool,
    /// Where a dropped audio file loads into a track's sampler (last frame's rects).
    pub sample_drop_targets: Vec<(egui::Rect, TrackId)>,
    /// Files dropped on the window, captured at the start of the UI pass before the font gate.
    pending_drops: Vec<(egui::DroppedFileHandle, Option<egui::Pos2>)>,
    /// Dropped files the side panels did not claim, for the native/browser shell to open or import.
    host_drops: Vec<egui::DroppedFileHandle>,
    sample_reads: Vec<sampler_window::SampleRead>,
    synthetic_grace: u32,
    control_rx: Option<Receiver<ControlRequest>>,
    /// Gestures whose reply waits for their events to be applied by a UI pass, in request order.
    gesture_acks: VecDeque<control::GestureAck>,
    pending_shots: Vec<control::PendingShot>,
    /// Session identity for screenshot/request binding; changes when the open session is replaced.
    doc_epoch: u64,
    /// Fresh viewport geometry from the host's raw input, including hidden logic-only ticks.
    host_geometry: Option<control::CaptureGeometry>,
    /// Monotonic host clock for transport simulation and frame pacing (never the UI input time).
    last_tick: Option<web_time::Instant>,
    /// Test-only offset added to the monotonic clock.
    clock_offset: std::time::Duration,
    /// Test-only frozen clock. Once a test advances time, `clock_now` returns this instead of the
    /// wall clock, so frame pacing and transport assertions do not depend on how long the test
    /// host took to lay the frame out.
    test_clock: Option<web_time::Instant>,
    /// Monotonic seconds since the app started, accumulated from [`Self::last_tick`].
    uptime: f64,
    /// When the last settled UI pass (a rendered frame) ran. Frame statistics describe rendered
    /// frames, not layout passes or hidden logic ticks.
    last_rendered_tick: Option<web_time::Instant>,
    /// An insert just picked from its menu, opened once the audio engine has its instance.
    pub(crate) pending_open: Option<(TrackId, usize)>,
    last_rev: u64,
    fonts_ready: bool,
    fonts_installed: bool,
    /// The palette last applied to the context (light?), so visuals are set only on a change.
    theme_applied: Option<bool>,
    /// Last panel host context, used to cancel renderer-owned gestures on explicit replacement.
    pub(crate) docking_context: Option<egui::Context>,
    /// Transport simulation when there is no player (tests, offscreen renders).
    sim: Option<(Samples, Option<Samples>, Option<Range>)>,
    /// When Record last found nothing to record on, and why: the Record buttons show it for a moment.
    record_hint: Option<(f64, &'static str)>,
    pub quit_requested: bool,
    /// Reset floating-window positions next frame (Window › Arrange).
    pub arrange_request: bool,
    /// Tracks whose automation was touched during this pass (Touch/Latch writing).
    touched: std::collections::HashSet<TrackId>,
    /// The pass's last point; `i64::MIN` after a stop or a jump, so the next point erases nothing it skipped.
    last_write_at: Samples,
    /// Plugins changed in their editors whose state the session hasn't taken yet (see `editor_states_due`).
    editor_states: Vec<(TrackId, usize)>,
    /// One editor poll per host raw-input iteration, including a hidden logic-only iteration.
    /// A discarded layout pass must not mistake its already-drained queue for a gesture end.
    editor_poll_due: bool,
    /// Tests supply drained editor reports at the native polling boundary without an audio device.
    #[cfg(test)]
    editor_poll_fixture: Option<EditorPollFixture>,
    pub frame_ms: f32,
    last_frame: Option<f64>,
    last_meter_poll: f64,
    /// Movies for the Video window and Video track (decoded in the background).
    pub video: video_track::VideoPool,
    /// The Bounce Mix in progress, rendered a slice per frame.
    pub bounce: Option<soundcraft_engine::io::Bounce>,
    /// Audio files importing in the background (issue #597).
    pub imports: import::Imports,
    /// Test-only: owns the process-wide palette lock while this app exists, so frames painted
    /// and colour assertions in one test can never straddle another test's palette change.
    #[cfg(test)]
    _palette_lock: theme::TestPaletteGuard,
    /// Test-only count of synthetic batches acknowledged by a pass that received their events.
    #[cfg(test)]
    synthetic_applied_batches: u32,
}

/// The plugins whose state the session takes this frame, out of those changed in their editors
/// (`pending`, kept across frames). A plugin editor doesn't say when a gesture ends: a plugin's
/// gesture lasts while its editor reports a change every frame, and its state is read on the first
/// frame its editor reports none, or as soon as the editor is closed. A drag reads and stores the
/// state once, not every frame, and a plugin that keeps changing doesn't hold back the others.
fn editor_states_due(
    pending: &mut Vec<(TrackId, usize)>,
    edits: &[soundcraft_playback::EditorEdit],
    changed: Vec<(TrackId, usize)>,
    closed: impl Fn((TrackId, usize)) -> bool,
) -> Vec<(TrackId, usize)> {
    let instruments = edits.iter().filter(|e| e.1 == soundcraft_mix::INSTRUMENT_SLOT).map(|e| (e.0, e.1));
    let reported: Vec<(TrackId, usize)> = instruments.chain(changed).collect();
    for k in &reported {
        if !pending.contains(k) {
            pending.push(*k);
        }
    }
    let (due, rest) = std::mem::take(pending).into_iter().partition(|&k| !reported.contains(&k) || closed(k));
    *pending = rest;
    due
}

/// Collect one admitted editor poll. Both native hosting and the lifecycle test fixture use
/// this same gesture scheduler/state-capture route.
fn editor_changes(
    pending: &mut Vec<(TrackId, usize)>,
    edits: Vec<soundcraft_playback::EditorEdit>,
    changed: Vec<(TrackId, usize)>,
    closed: impl Fn((TrackId, usize)) -> bool,
    capture: impl FnOnce(&[(TrackId, usize)]) -> soundcraft_playback::PluginStates,
) -> (Vec<soundcraft_playback::EditorEdit>, soundcraft_playback::PluginStates) {
    let due = editor_states_due(pending, &edits, changed, closed);
    let states = if due.is_empty() { Vec::new() } else { capture(&due) };
    (edits, states)
}

#[cfg(test)]
type EditorPollFixture = Box<dyn FnMut(&mut Vec<(TrackId, usize)>) -> (Vec<soundcraft_playback::EditorEdit>, soundcraft_playback::PluginStates)>;

/// Whether another automation-write pass is due: the transport moved at least
/// `step` since `last` in either direction. `last == i64::MIN` marks "never
/// written", so the first pass after (re)start is always due. Saturating
/// arithmetic keeps restarts and loop wraps overflow-free: the previous
/// `(pos - last).abs()` panicked in debug builds on the first playing frame,
/// when `last` is still `i64::MIN`.
fn automation_step_due(pos: Samples, last: Samples, step: Samples) -> bool {
    last == i64::MIN || pos.saturating_sub(last) >= step || last.saturating_sub(pos) >= step
}

impl SoundApp {
    /// Build an app around an engine and optional player, initialising UI, recording, autosave, control and
    /// synthetic-input state.
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        SoundApp {
            recent_seen: engine.path.clone(),
            recent_changed: false,
            recent_checked: None,
            engine,
            player,
            recorder: None,
            record_start: 0,
            punch: None,
            recorder_failed: false,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            audio_health_report: None,
            services,
            native_menu_bar: false,
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            preset_dir: None,
            last_autosave: None,
            dialogs: dialogs::Dialogs::default(),
            updates_rt: updates_ui::Runtime::default(),
            meters: HashMap::new(),
            main_meter: MeterDisplay::default(),
            meter_snapshot: soundcraft_playback::MeterSnapshot::default(),
            gesture: None,
            edit_layout: edit_window::EditLayout::default(),
            midi: midi_editor::MidiEditorState::default(),
            ops: ops_windows::OpsState::default(),
            extra: extra_windows::ExtraState::default(),
            palette: palette::PaletteState::default(),
            focus_mix: false,
            edit_scroll_into_view: None,
            mix_scroll_into_view: None,
            synthetic: Vec::new(),
            synthetic_batch: None,
            synthetic_pointer_active: false,
            cancel_synthetic_pointer: false,
            cancel_synthetic_keys: false,
            cancel_synthetic_scroll: false,
            input_before_scroll: None,
            injected_events: Vec::new(),
            injected_at: 0,
            host_keys_down: None,
            pointer_down: false,
            wants_keyboard: false,
            sample_drop_targets: Vec::new(),
            pending_drops: Vec::new(),
            host_drops: Vec::new(),
            sample_reads: Vec::new(),
            synthetic_grace: 0,
            control_rx: None,
            gesture_acks: VecDeque::new(),
            pending_shots: Vec::new(),
            doc_epoch: 0,
            host_geometry: None,
            last_tick: None,
            clock_offset: std::time::Duration::ZERO,
            test_clock: None,
            uptime: 0.0,
            last_rendered_tick: None,
            pending_open: None,
            last_rev: 0,
            fonts_ready: false,
            fonts_installed: false,
            theme_applied: None,
            docking_context: None,
            sim: None,
            record_hint: None,
            quit_requested: false,
            arrange_request: false,
            touched: std::collections::HashSet::new(),
            last_write_at: i64::MIN,
            editor_states: Vec::new(),
            editor_poll_due: true,
            #[cfg(test)]
            editor_poll_fixture: None,
            frame_ms: 0.0,
            last_frame: None,
            last_meter_poll: f64::NEG_INFINITY,
            video: video_track::VideoPool::default(),
            bounce: None,
            imports: import::Imports::default(),
            #[cfg(test)]
            _palette_lock: theme::lock_test_palette(),
            #[cfg(test)]
            synthetic_applied_batches: 0,
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command; errors go to the status line. Returns the result.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if matches!(id, "session.new" | "session.open" | "session.close" | "session.revert") && bounce::refuses_session_switch(self) {
            return Err(self.ui.status.clone());
        }
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        if id.starts_with("session.save") || id.starts_with("file.bounce") || id == "track.save_preset" {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states();
        }
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert");
        match self.engine.execute(id, &params) {
            Ok(v) => {
                if replacing {
                    // Note indices and gestures belong to the old session's clips (#443).
                    self.document_replaced();
                }
                if replacing || (id.starts_with("session.save") && !self.engine.is_dirty()) {
                    self.release_autosave(false);
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                if id == "options.scrolling" {
                    // Re-selecting a scrolling mode resumes the playhead follow.
                    self.edit_layout.follow_hold = false;
                }
                if id == "track.clear_clip_indicators" {
                    self.meters.values_mut().chain([&mut self.main_meter]).for_each(|m| m.clip = false);
                }
                Ok(v)
            }
            Err(e) => {
                self.ui.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// The open document was replaced outside [`Self::run`] (Recover, a session the browser read
    /// as bytes). Screenshot requests admitted for the old document must never receive the new
    /// one's pixels, so every replacement path bumps the epoch.
    pub(crate) fn document_replaced(&mut self) {
        self.doc_epoch = self.doc_epoch.wrapping_add(1);
        // Note indices and gestures belong to the old session's clips (#443); every document
        // replacement (run, Recover, a session the browser read) goes through here.
        self.midi = midi_editor::MidiEditorState::default();
    }

    pub fn is_playing(&self) -> bool {
        self.engine.transport.playing
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub(crate) fn record_hint(&self) -> Option<&'static str> {
        let (at, why) = self.record_hint?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// Current transport position (playhead while playing, else the edit insertion).
    pub fn position(&self) -> Samples {
        if self.engine.transport.playing { self.engine.transport.position } else { self.engine.session().edit.selection.start }
    }

    fn start_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        if let Some(p) = &self.player {
            p.update_session(self.engine.session_arc());
            p.play(from, end, looped);
        } else {
            self.sim = Some((from, end, looped));
        }
        self.engine.transport.playing = true;
        self.engine.transport.position = from;
        self.last_write_at = i64::MIN;
        // A fresh start resumes the playhead follow: clear a manual hold and
        // re-sync the follow tracker so the current view is not mistaken for
        // an outside move on the first frame.
        self.edit_layout.follow_hold = false;
        self.edit_layout.last_scroll = self.engine.session().edit.zoom.scroll;
        self.edit_layout.last_follow_to = None;
    }

    fn stop_play(&mut self) {
        if let Some(p) = &self.player {
            // Half-speed is a separate command. Stop must not leave that rate for the next Play.
            p.set_speed(1.0);
            p.stop();
        }
        if self.engine.transport.recording {
            self.finish_recording();
        }
        self.sim = None;
        self.engine.transport.playing = false;
        self.engine.transport.recording = false;
        let stop_at = self.engine.transport.position;
        // Snapshot the session flags first: `session_mut` below needs a
        // mutable borrow, so no session borrow may span it.
        let (follow_insertion, after_playback) = {
            let s = self.engine.session();
            (s.edit.insertion_follows_playback, s.edit.scrolling == "after_playback")
        };
        if follow_insertion {
            let pos = self.engine.transport.position;
            self.engine.session_mut().edit.selection = Range::point(pos);
        }
        if after_playback {
            // After Playback never scrolls while playing, so bring the stopped
            // position into view now (only if it is off-screen); every other
            // mode keeps its view.
            let at = stop_at.max(0);
            let (scroll, spp) = {
                let z = &self.engine.session().edit.zoom;
                (z.scroll, z.samples_per_px.max(0.01))
            };
            let width = f64::from(self.edit_layout.timeline[2] - self.edit_layout.timeline[0]).max(0.0);
            let end = scroll.saturating_add((width * spp) as Samples);
            if at < scroll || at >= end {
                let _ = self.engine.execute("view.scroll", &json!({ "to": at }));
            }
        }
        self.edit_layout.follow_hold = false;
    }

    fn start_recording(&mut self) {
        let armed = self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
        if !armed {
            let why = if self.engine.session().tracks.is_empty() {
                "Nothing to record on yet: add a track with Track › New…, then click its R button."
            } else {
                "No track is record-enabled: click the R button on the track to record."
            };
            self.ui.status = why.into();
            self.record_hint = Some((self.last_frame.unwrap_or_default(), why));
            return;
        }
        self.ensure_input();
        let takes = match self.arm_recorder() {
            Ok(takes) => takes,
            Err(err) => {
                self.ui.status = format!("Cannot record: {err}");
                return;
            }
        };
        self.lost_shown = 0;
        if let Some(p) = &self.player {
            p.set_recording(true);
        }
        let already_playing = self.is_playing();
        self.engine.transport.recording = true;
        if already_playing {
            // Punch in on the fly at the current playhead.
            self.record_start = self.position();
            self.punch = None;
            self.plan_recording(takes, None);
            return;
        }
        let e = self.engine.session().edit.clone();
        let sel = e.selection;
        if e.loop_record && !sel.is_empty() {
            self.record_start = sel.start;
            self.punch = None;
            self.start_play(sel.start, None, Some(sel));
            self.plan_recording(takes, Some(sel.len()));
        } else {
            // Record into the selection when there is one (punch), with pre/post-roll if enabled;
            // the take is trimmed back to the punch range afterwards.
            let (pre, post) = if e.pre_post_roll { (e.pre_roll, e.post_roll) } else { (0, 0) };
            let from = (sel.start - pre).max(0);
            self.record_start = from;
            self.punch = (!sel.is_empty()).then_some(sel);
            let end = (!sel.is_empty()).then_some(sel.end + post);
            self.start_play(from, end, None);
            self.plan_recording(takes, None);
        }
    }

    /// Remember where the armed `takes` go (split into passes of `pass` when loop recording) and
    /// autosave that plan with the session.
    fn plan_recording(&mut self, takes: Vec<soundcraft_engine::io::Take>, pass: Option<Samples>) {
        let rate = self.recorder.as_ref().map_or(0, |r| r.sample_rate);
        self.take_plan = Some(soundcraft_engine::io::Recording { start: self.record_start, pass, punch: self.punch, rate, takes });
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Arm the input for the record-enabled tracks; a session never saved records into its
    /// autosave folder. Returns the takes it records.
    fn arm_recorder(&mut self) -> Result<Vec<soundcraft_engine::io::Take>, String> {
        let name = soundcraft_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved_dir = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(name));
        let Some(r) = &mut self.recorder else { return Err("no audio input device".into()) };
        let plan = soundcraft_engine::io::plan_takes(&self.engine, &unsaved_dir, r.channels);
        if plan.is_empty() {
            return Err("no audio track is record-enabled".into());
        }
        // The takes are unsaved work: the folder they go into under the autosave root is
        // owner-only, like the autosaves, before the recorder makes anything in it.
        if self.engine.path.is_none() && self.autosave_dir.is_some() {
            for dir in plan.iter().filter_map(|(_, _, path)| path.parent()) {
                soundcraft_engine::autosave::create_folder(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
        let takes = plan
            .iter()
            .map(|(track, inputs, path)| soundcraft_engine::io::Take { track: *track, path: path.clone(), channels: inputs.len() })
            .collect();
        r.arm(plan)?;
        Ok(takes)
    }

    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves and bounces include it.
    pub fn capture_plugin_states(&mut self) {
        let Some(p) = &self.player else { return };
        let states = p.capture_states();
        self.store_plugin_states(states);
    }

    fn store_plugin_states(&mut self, states: soundcraft_playback::PluginStates) {
        for (t, slot, data) in states {
            let slot = if slot == soundcraft_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
            let params = json!({"track": t.0, "slot": slot, "state": soundcraft_model::b64::encode(&data)});
            // A state read while a knob is dragged (or a pass recorded) joins that gesture, so it
            // stays one undo step.
            let stored = match self.engine.merge_key().map(str::to_string) {
                Some(key) => self.engine.execute_merged("mix.insert_state", &params, &key),
                None => self.engine.execute("mix.insert_state", &params),
            };
            if let Err(e) = stored {
                log::warn!("storing plugin state: {e}");
            }
        }
    }

    /// Opens the insert just picked from its menu: a hosted plugin's own editor, else (a built-in
    /// plugin, no audio engine, or an editor that can't open here) its plugin window.
    fn open_inserted(&mut self) {
        let Some((track, slot)) = self.pending_open.take() else { return };
        let Some(plugin) = self.engine.session().track(track).and_then(|t| t.mixer.inserts.get(slot).cloned().flatten()).map(|i| i.plugin) else {
            return;
        };
        if soundcraft_mix::is_third_party(&plugin)
            && let Some(p) = &self.player
            && p.has_editor(track, slot)
        {
            match p.open_editor(track, slot) {
                Ok(()) => return,
                Err(e) => self.ui.status = format!("{}: {e}", mix_window::plugin_info(&plugin).map_or(plugin.as_str(), |i| i.name)),
            }
        }
        if !self.ui.plugin_windows.contains(&(track, slot)) {
            self.ui.plugin_windows.push((track, slot));
        }
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    /// Hosted instruments keep their parameters in their own state, which the session takes
    /// instead, as it does for plugins that said their state changed: either way the session is
    /// modified, so it is autosaved and asks to be saved. The state is read once per gesture (see
    /// `editor_states_due`).
    fn apply_editor_edits(&mut self) {
        if !std::mem::take(&mut self.editor_poll_due) {
            return;
        }
        #[cfg(test)]
        if let Some(poll) = self.editor_poll_fixture.as_mut() {
            let (edits, states) = poll(&mut self.editor_states);
            self.store_editor_changes(edits, states);
            return;
        }
        let Some(p) = &self.player else {
            self.editor_states.clear();
            return;
        };
        let (edits, states) = editor_changes(
            &mut self.editor_states,
            p.idle(),
            p.changed_states(),
            |(t, slot)| !p.editor_open(t, slot),
            |due| p.capture_states_of(due),
        );
        self.store_editor_changes(edits, states);
    }

    fn store_editor_changes(&mut self, edits: Vec<soundcraft_playback::EditorEdit>, states: soundcraft_playback::PluginStates) {
        for (t, slot, param, value) in edits {
            if slot == soundcraft_mix::INSTRUMENT_SLOT {
                continue;
            }
            let key = format!("param:{}:{slot}:{param}", t.0);
            let _ = self.engine.execute_merged("mix.insert_param", &json!({"track": t.0, "slot": slot, "param": param, "value": value}), &key);
        }
        self.store_plugin_states(states);
    }

    /// The session is modified, or a plugin editor gesture still holds a state the session hasn't
    /// taken (see `editor_states_due`): both read the plugins' states before autosaving.
    fn has_unsaved_changes(&self) -> bool {
        self.engine.is_dirty() || !self.editor_states.is_empty()
    }

    /// Every two minutes, copy a modified session to the autosave folder (not while recording).
    /// Timed by the wall clock: egui's stands still while the window is hidden.
    fn autosave(&mut self) {
        let Some(dir) = self.autosave_dir.clone() else { return };
        let last = *self.last_autosave.get_or_insert_with(std::time::Instant::now);
        if last.elapsed().as_secs() < 120 || !self.has_unsaved_changes() || self.engine.transport.recording {
            return;
        }
        self.last_autosave = Some(std::time::Instant::now());
        self.capture_plugin_states();
        self.autosave_to(&dir);
    }

    /// Autosave a modified session within a second of every change, also while recording. The
    /// plugins' own state reaches the session every two minutes (`autosave`).
    fn autosave_changes(&mut self) {
        if let Some(e) = self.autosaver.as_ref().and_then(soundcraft_engine::autosave::Autosaver::failure) {
            self.ui.status = format!("Autosave failed: {e}");
        }
        if self.autosave_dir.is_none() || self.engine.revision == self.autosaved_rev {
            return;
        }
        self.autosaved_rev = self.engine.revision;
        if !self.engine.is_dirty() {
            self.release_autosave(false);
        } else if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Hand the session to the autosaver for its folder under `root`.
    fn queue_autosave(&mut self, root: std::path::PathBuf) {
        self.autosaved_rev = self.engine.revision;
        let w = self.autosaver.get_or_insert_with(|| soundcraft_engine::autosave::Autosaver::new(root));
        w.write(self.engine.session_arc(), self.engine.path.clone(), self.take_plan.clone());
    }

    /// Write the session to its own folder under `root` and wait for it; false when that failed.
    fn autosave_to(&mut self, root: &std::path::Path) -> bool {
        self.queue_autosave(root.to_path_buf());
        let Some(w) = &self.autosaver else { return false };
        let written = w.flush();
        if let Err(e) = &written {
            self.ui.status = format!("Autosave failed: {e}");
        }
        written.is_ok()
    }

    /// Autosave now, if the app has an autosave folder.
    fn write_autosave(&mut self) -> bool {
        self.autosave_dir.clone().is_some_and(|root| self.autosave_to(&root))
    }

    /// Done with this session's autosave folder: keep it for the next launch, or delete it.
    fn release_autosave(&mut self, keep: bool) {
        if let Some(w) = &self.autosaver {
            w.close(keep);
        }
    }

    /// Done with the open session. Unsaved changes stay in its autosave folder for the next
    /// launch, unless the user quit with SoundCraft's Quit; a take still recording is finished
    /// and always stays.
    pub fn close_autosave(&mut self) {
        let recording = self.engine.transport.recording;
        if recording {
            self.stop_play();
        }
        let keep = self.has_unsaved_changes() && (recording || !self.quit_requested);
        if keep {
            self.capture_plugin_states();
            self.write_autosave();
        }
        self.release_autosave(keep);
        if let Some(w) = &self.autosaver {
            let _ = w.flush();
        }
    }

    /// Run part of a frame. If it panics, the session is autosaved (once per change) and the app
    /// carries on instead of quitting.
    pub fn guarded(&mut self, f: impl FnOnce(&mut SoundApp)) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))).is_ok() || self.panic_rev == self.engine.revision {
            return;
        }
        self.panic_rev = self.engine.revision;
        let saved = self.engine.is_dirty() && self.write_autosave();
        self.ui.status = if saved {
            "SoundCraft hit an internal error (see the log). Your changes were autosaved: save them, then restart SoundCraft.".into()
        } else {
            "SoundCraft hit an internal error (see the log). Restart SoundCraft to be safe.".into()
        };
    }

    /// Replace the open session with unsaved work an earlier run left behind.
    pub fn recover(&mut self, left: &soundcraft_engine::autosave::Leftover) {
        if self.engine.transport.recording {
            self.ui.status = "Stop recording before recovering other work.".into();
            return;
        }
        self.close_autosave();
        match soundcraft_engine::autosave::recover(&mut self.engine, left) {
            Ok(missing) => {
                self.document_replaced();
                if self.write_autosave() {
                    soundcraft_engine::autosave::discard(left);
                }
                self.ui.status = match missing.len() {
                    0 => format!("Recovered “{}”. Save it to keep the changes.", left.name),
                    n => format!("Recovered “{}”, but {n} audio file(s) are missing. Save it to keep the changes.", left.name),
                };
            }
            Err(e) => self.ui.status = format!("Couldn't recover “{}”: {e}", left.name),
        }
    }

    /// The most recent autosave, if any.
    pub fn latest_autosave(&self) -> Option<std::path::PathBuf> {
        let dir = self.autosave_dir.as_ref()?;
        let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
        for d in std::fs::read_dir(dir).ok()?.flatten() {
            for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
                let p = f.path();
                if p.extension().and_then(std::ffi::OsStr::to_str) == Some("scraft")
                    && let Ok(m) = f.metadata().and_then(|m| m.modified())
                    && best.as_ref().is_none_or(|(t, _)| m > *t)
                {
                    best = Some((m, p));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Whether the meters are live: while playing, and while armed or input-monitored tracks are heard.
    fn meters_live(&self) -> bool {
        self.engine.transport.playing
            || (self.recorder.is_some() && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm))
    }

    /// Reopens the audio devices with `cfg` (Setup › Playback Engine › Apply), keeping the
    /// transport position, and keeps it as `ui.audio`. Plugin instances and their editors are
    /// recreated with the new player. Refused while recording, which would lose the take; `ui.audio`
    /// then stays the configuration of the open device.
    pub fn apply_audio_config(&mut self, cfg: soundcraft_playback::AudioConfig) -> Result<(), String> {
        if self.engine.transport.recording {
            return Err("stop recording before changing the audio device".into());
        }
        self.ui.audio = cfg;
        let at = self.position();
        if let Some(p) = &self.player {
            p.stop();
            // The old player's plugin instances go away with it; their editor windows must be
            // closed here, on the main thread, or they would stay on screen orphaned.
            p.close_editors();
        }
        // The input reopens on the next record or monitor with the new choice.
        self.recorder = None;
        self.recorder_failed = false;
        let player = soundcraft_playback::Player::with_config(self.engine.session_arc(), &self.ui.audio);
        player.locate(at);
        self.player = Some(player);
        Ok(())
    }

    /// Open the input device once (for recording and input monitoring) and feed the player.
    fn ensure_input(&mut self) {
        if self.recorder.is_some() || self.recorder_failed || self.player.is_none() {
            return;
        }
        match soundcraft_playback::record::Recorder::open_with(&self.ui.audio) {
            Ok(r) => {
                if let Some(p) = &self.player {
                    p.set_input(std::sync::Arc::clone(&r.monitor));
                }
                self.recorder = Some(r);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = format!("No audio input: {e}");
            }
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails.
    fn watch_output(&mut self) {
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                "the audio engine hit an internal error (see the log)".to_string(),
                "Use Reconnect audio device in Setup › Playback Engine, or restart SoundCraft.",
            ),
            OutputTrouble::Gone => {
                (format!("“{name}” is unavailable"), "Check the audio device, then use Reconnect audio device in Setup › Playback Engine.")
            }
            OutputTrouble::Stalled => (format!("“{name}” stopped asking for audio"), "Use Reconnect audio device in Setup › Playback Engine."),
        };
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        self.ui.status = if recording {
            format!("Recording stopped: {cause}. The take is kept up to there. {then}")
        } else if playing {
            format!("Playback stopped: {cause}. {then}")
        } else {
            format!("Audio output: {cause}. {then}")
        };
    }

    /// Stop recording, keeping the take, when the input device or the disk fails, and say as soon
    /// as a slow disk loses input.
    fn watch_input(&mut self) {
        let recording = self.engine.transport.recording;
        let Some(r) = &mut self.recorder else { return };
        let Some(trouble) = r.trouble() else {
            let lost = r.lost_ms();
            if recording && lost > self.lost_shown {
                self.lost_shown = lost;
                self.ui.status = format!("The disk is too slow: this take has lost {lost} ms of input so far.");
            }
            return;
        };
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.recorder = None;
                self.recorder_failed = true;
                (what, "Reconnect it, then use Reconnect audio device in Setup › Playback Engine.")
            }
            Trouble::Disk(what) => (what, "Check the disk, then record again."),
        };
        self.ui.status = if recording {
            format!("Recording stopped: {what}. The take is kept up to there. {then}")
        } else {
            format!("Audio input: {what}. {then}")
        };
    }

    /// Reopen the audio devices, keeping a take in progress.
    pub fn reconnect_audio(&mut self) {
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        if let Some(p) = &self.player {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            p.close_editors();
        }
        self.recorder = None;
        self.recorder_failed = false;
        self.player = Some(Player::with_config(self.engine.session_arc(), &self.ui.audio));
    }

    fn finish_recording(&mut self) {
        self.engine.transport.recording = false;
        if let Some(p) = &self.player {
            p.set_recording(false);
        }
        let plan = self.take_plan.take();
        let Some(r) = &mut self.recorder else { return };
        let (takes, problem) = r.finish();
        let tracks = takes.len().max(1);
        let plan = plan.unwrap_or_else(|| soundcraft_engine::io::Recording {
            start: self.record_start,
            pass: None,
            punch: self.punch,
            rate: 0,
            takes: Vec::new(),
        });
        self.punch = None;
        self.autosaved_rev = 0;
        let ids = soundcraft_engine::io::place_recording(&mut self.engine, &plan, takes);
        self.ui.status =
            if plan.pass.is_some() { format!("Loop-recorded {} take(s)", ids.len() / tracks) } else { format!("Recorded {} clip(s)", ids.len()) };
        if let Some(p) = problem {
            log::warn!("recording: {p}");
            self.ui.status = format!("{} ({p})", self.ui.status);
        }
    }

    /// While playing, record volume automation from fader positions for tracks in Write mode, and
    /// for Touch/Latch tracks whose fader is (or, for Latch, was) being moved. Each point replaces the
    /// automation since the previous one, and Stop writes a last point. A pass and whatever is
    /// dragged during it, its fader included, are one undo step.
    fn write_automation(&mut self) {
        use soundcraft_model::AutomationMode as M;
        const PASS: &str = "automation_pass";
        let playing = self.is_playing();
        let may_write =
            playing && self.engine.session().tracks.iter().any(|t| matches!(t.mixer.automation_mode, M::Write | M::Touch | M::Latch | M::TouchLatch));
        self.engine.hold_merge(may_write.then_some(PASS));
        if !playing && self.touched.is_empty() {
            self.last_write_at = i64::MIN;
            return;
        }
        let pos = self.engine.transport.position;
        let step = self.engine.session().sample_rate.samples(0.02);
        if playing && !automation_step_due(pos, self.last_write_at, step) {
            return;
        }
        let from = std::mem::replace(&mut self.last_write_at, if playing { pos } else { i64::MIN });
        // The gesture state cached from the known UI phase, never hidden-tick stale input.
        let dragging = self.pointer_down;
        let fader_track = match &self.gesture {
            Some(Gesture::Fader { track, .. }) => Some(*track),
            _ => None,
        };
        let writes: Vec<(TrackId, f32)> = self
            .engine
            .session()
            .tracks
            .iter()
            .filter_map(|t| {
                let m = t.mixer.automation_mode;
                let touched_now = dragging && fader_track == Some(t.id);
                let write = match m {
                    M::Write => true,
                    M::Touch => touched_now,
                    M::Latch | M::TouchLatch => touched_now || self.touched.contains(&t.id),
                    _ => false,
                };
                write.then_some((t.id, t.mixer.volume_db))
            })
            .collect();
        for (t, v) in writes {
            self.touched.insert(t);
            let mut point = serde_json::json!({"track": t.0, "param": "volume", "at": pos, "value": v});
            if from != i64::MIN {
                point["from"] = from.into();
            }
            let _ = self.engine.execute_merged("automation.set_point", &point, PASS);
        }
        if !playing {
            self.touched.clear();
            self.engine.end_merge();
        }
    }

    /// Play from the selection, honouring loop playback and selection end.
    pub fn play_from_selection(&mut self) {
        let s = self.engine.session();
        let sel = s.edit.selection;
        let preroll = if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 };
        let postroll = if s.edit.pre_post_roll { s.edit.post_roll } else { 0 };
        if s.edit.loop_playback && !sel.is_empty() {
            self.start_play(sel.start, None, Some(sel));
        } else if !sel.is_empty() {
            self.start_play((sel.start - preroll).max(0), Some(sel.end + postroll), None);
        } else {
            self.start_play((sel.start - preroll).max(0), None, None);
        }
    }

    /// Apply engine transport requests to the player and recorder, read back the position and refresh meters. A reset
    /// request also clears per-document capture, edit-gesture and meter state.
    fn handle_transport(&mut self, dt: f32) {
        let reqs: Vec<TransportRequest> = std::mem::take(&mut self.engine.transport_requests);
        for r in reqs {
            match r {
                TransportRequest::ResetSession => {
                    if let Some(p) = &self.player {
                        p.stop();
                        p.set_recording(false);
                        p.set_speed(1.0);
                        p.locate(self.engine.transport.position);
                    }
                    self.sim = None;
                    // The old capture belongs to the replaced document, never to the new one.
                    self.recorder = None;
                    self.recorder_failed = false;
                    self.punch = None;
                    self.gesture = None;
                    self.touched.clear();
                    self.last_write_at = i64::MIN;
                    self.edit_layout.follow_hold = false;
                    self.meters.clear();
                    self.main_meter = MeterDisplay::default();
                }
                TransportRequest::Play => {
                    if !self.is_playing() {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Stop | TransportRequest::Pause => self.stop_play(),
                TransportRequest::TogglePlay => {
                    if self.is_playing() {
                        self.stop_play();
                    } else {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Record => {
                    if self.engine.transport.recording {
                        self.stop_play();
                    } else {
                        self.start_recording();
                    }
                }
                TransportRequest::Locate(at) => {
                    if self.is_playing() {
                        if let Some(p) = &self.player {
                            p.locate(at);
                        }
                        if let Some(sim) = &mut self.sim {
                            sim.0 = at;
                        }
                        self.last_write_at = i64::MIN;
                    }
                    self.engine.transport.position = at;
                }
                TransportRequest::PlaySelection => {
                    let sel = self.engine.session().edit.selection;
                    self.start_play(sel.start, Some(sel.end.max(sel.start + 1)), None);
                }
                TransportRequest::HalfSpeed => {
                    if let Some(p) = &self.player {
                        p.set_speed(0.5);
                    }
                    self.play_from_selection();
                }
                TransportRequest::Scrub(at) => self.engine.transport.position = at,
                TransportRequest::AllNotesOff => {}
            }
        }
        // Read back the position.
        if let Some(p) = &self.player {
            let playing = p.is_playing();
            let ended = self.engine.transport.playing && !playing;
            if ended {
                p.set_speed(1.0);
            }
            if self.engine.transport.playing {
                self.engine.transport.position = p.position();
            }
            let now = self.uptime;
            let meter_interval = if self.ui.window == MainWindow::Mix { 1.0 / 60.0 } else { 1.0 / 20.0 };
            if now - self.last_meter_poll >= meter_interval {
                p.take_meters(&mut self.meter_snapshot);
                self.last_meter_poll = now;
            }
            let snap = &self.meter_snapshot;
            for (id, m) in &snap.tracks {
                let d = self.meters.entry(*id).or_default();
                feed_meter(d, m.peak, m.gain_reduction_db, dt);
                if m.plugin_failures.count != d.plugin_failures {
                    d.plugin_failures = m.plugin_failures.count;
                    if let Some(msg) = failed_plugin_status(self.engine.session(), *id, m.plugin_failures.last) {
                        self.ui.status = msg;
                    }
                }
            }
            let main = snap.main.peak;
            feed_meter(&mut self.main_meter, main, 0.0, dt);
            if ended {
                self.stop_play();
            }
        } else if let Some((pos, end, looped)) = &mut self.sim {
            let sr = self.engine.session().sample_rate.as_f64();
            *pos += (f64::from(dt) * sr) as i64;
            if let Some(l) = looped
                && *pos >= l.end
            {
                *pos = l.start;
            }
            let stop = end.is_some_and(|e| *pos >= e);
            self.engine.transport.position = *pos;
            if stop {
                self.stop_play();
            }
        }
        if !self.meters_live() {
            for d in self.meters.values_mut() {
                feed_meter(d, [0.0, 0.0], 0.0, dt);
            }
            feed_meter(&mut self.main_meter, [0.0, 0.0], 0.0, dt);
        }
    }

    /// Per-frame logic: control channel, transport, document sync.
    ///
    /// eframe calls this once per layout pass and also on logic-only ticks while hidden. Consumed
    /// queues are one-shot, autosave is revision/instant based, and the monotonic clock decides
    /// timed work. Bounce and import polling run only on pass zero, including hidden logic ticks.
    /// Each host iteration starts with `raw_input_hook`; its editor-poll admission prevents an
    /// empty queue on a discarded pass from ending a still-active plugin gesture.
    pub fn logic(&mut self, ctx: &egui::Context) {
        // Fonts set now take effect next frame, so draw only from the frame after.
        if !self.fonts_ready {
            if self.fonts_installed {
                self.fonts_ready = true;
            } else {
                fonts::install(ctx);
                self.fonts_installed = true;
                ctx.request_repaint();
            }
        }
        let light = theme::wants_light(ctx, self.ui.theme);
        if self.theme_applied != Some(light) || theme::is_light() != light {
            theme::apply(ctx, light);
            self.theme_applied = Some(light);
        }
        // A logic-only tick while hidden gets no fresh events: `ctx.input` still says what the last
        // completed pass saw. Do not let that stale pointer/keyboard state keep writing automation
        // or hold a transient fader gesture (loss of visibility cancels a Touch drag by design).
        // `ui` (the known UI phase) caches this pass's real state for the next `logic` call.
        let hidden = !ctx.input(|i| i.viewport().visible()).unwrap_or(true);
        if hidden {
            self.pointer_down = false;
            self.wants_keyboard = false;
            if matches!(self.gesture, Some(Gesture::Fader { .. })) {
                self.gesture = None;
            }
        }
        // UI input time is stale on logic-only ticks and identical across passes of one frame.
        // Frame pacing and the transport simulation use a monotonic host clock instead.
        let dt = self.tick_dt();
        self.last_frame = Some(self.uptime);
        control::expire_gestures(self);
        control::drain(self, ctx);
        self.note_recent_session();
        self.apply_editor_edits();
        // A multi-pass frame runs this logic once per pass; bounce slices and background-import
        // polling are per host frame.
        if ctx.current_pass_index() == 0 {
            bounce::step(self, ctx);
            import::step(self, ctx);
            #[cfg(not(target_arch = "wasm32"))]
            updates_ui::tick(self, ctx);
        }
        if !self.pointer_down {
            if matches!(self.gesture, Some(Gesture::Fader { .. })) {
                self.gesture = None;
            }
            // Keep a running automation pass merged until playback stops.
            if !self.wants_keyboard && self.touched.is_empty() {
                self.engine.end_merge();
            }
        }
        self.watch_output();
        self.watch_input();
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        if self.engine.revision != self.last_rev {
            self.last_rev = self.engine.revision;
            if let Some(p) = &self.player {
                p.update_session(self.engine.session_arc());
            }
        }
        self.open_inserted();
        self.autosave_changes();
        self.write_automation();
        self.autosave();
        if self.recorder.is_none()
            && !self.recorder_failed
            && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm)
        {
            self.ensure_input();
        }
        // Autosave and the macOS menu bar only run on frames, and plugin editors are native windows egui never hears from.
        // egui 0.37 schedules the delay as given (it no longer subtracts the predicted frame time),
        // so ask for the real interval instead of adding that time back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open);
        ctx.request_repaint_after_secs(if editors { 0.03 } else { 1.0 });
        control::drain_screenshots(self, ctx);
        if control::screenshots_need_paint(self) {
            // An admitted capture that has not reached a UI pass needs an explicit hidden paint;
            // the screenshot command itself only paints once it has been issued.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
        if self.synthetic_batch.is_some() {
            // A batch waiting for a settled pass (or the UI pass that would apply it) needs a real
            // UI paint, not just logic ticks: a hidden window runs no layout otherwise.
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
        if self.synthetic_batch.is_some() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Monotonic seconds since the previous tick, clamped to a sane frame step.
    fn tick_dt(&mut self) -> f32 {
        let now = self.clock_now();
        let elapsed = self.last_tick.replace(now).map_or(1.0 / 60.0, |prev| now.duration_since(prev).as_secs_f64());
        self.uptime += elapsed;
        (elapsed as f32).clamp(0.0, 0.25)
    }

    /// The app clock: a frozen test clock when set, otherwise host time shifted by the test offset.
    pub(crate) fn clock_now(&self) -> web_time::Instant {
        self.test_clock.unwrap_or_else(|| web_time::Instant::now().checked_add(self.clock_offset).unwrap_or_else(web_time::Instant::now))
    }

    /// Test hook: advance the monotonic clock without sleeping.
    ///
    /// The first call freezes the clock at the current host time; later calls move it by exactly
    /// `by`. Tests that assert frame or transport timing therefore see only the time they asked
    /// for, not how long the test machine spent rendering.
    #[doc(hidden)]
    pub fn advance_test_clock(&mut self, by: std::time::Duration) {
        let base = self.test_clock.unwrap_or_else(|| self.clock_now());
        self.test_clock = Some(base.checked_add(by).unwrap_or(base));
    }

    /// The viewport geometry new captures are bound to. Raw-input geometry wins because it is
    /// refreshed on hidden logic-only ticks; the UI input is a fallback for harnesses without a
    /// raw-input hook.
    pub(crate) fn capture_geometry(&self, ctx: &egui::Context) -> control::CaptureGeometry {
        self.host_geometry.unwrap_or_else(|| control::CaptureGeometry::from_context(ctx))
    }

    /// Remember the latest valid host viewport geometry for screenshot admission.
    fn note_host_geometry(&mut self, ctx: &egui::Context, raw: &egui::RawInput) {
        if let Some(g) = control::CaptureGeometry::from_raw(ctx, raw) {
            self.host_geometry = Some(g);
        }
    }

    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.editor_poll_due = true;
        self.note_host_geometry(ctx, raw);
        // Expire before injecting: a gesture that can no longer reach a UI pass must not put one
        // last copy of its events into this pass after its request was already canceled.
        control::expire_gestures(self);
        // A host that could not run a UI pass retains the input this hook mutated (eframe's
        // `pending_raw_input`, the web runner's `input.raw`) and hands it back next tick. Strip the
        // copy appended last time before appending the still-pending batch again: otherwise a
        // single queued key press would arrive once per hidden tick and run N times on restore.
        self.remove_injected(raw);
        self.host_keys_down = None;
        // An expired key/scroll batch was already stripped from the retained input above. A UI
        // pass that received it before the deadline crossed has consumed these flags; a hidden
        // tick cannot.
        self.cancel_synthetic_keys = false;
        self.cancel_synthetic_scroll = false;
        self.input_before_scroll = None;
        if std::mem::take(&mut self.cancel_synthetic_pointer) {
            // Do this before egui begins its next input/interaction pass. stop_dragging marks a
            // drag_stopped in the current snapshot; the next pass replaces it without a release.
            // Calling it from the UI instead could turn cancellation into a track reorder/drop.
            ctx.input_mut(|i| i.pointer = Default::default());
            ctx.stop_dragging();
            egui::DragAndDrop::clear_payload(ctx);
            self.pointer_down = false;
        }
        // While a synthetic gesture plays (and briefly after), the real pointer is ignored so it
        // cannot interleave with the scripted one. A batch still waiting for a UI pass counts as
        // "playing": the grace must not run out while that gesture is pending.
        if !self.synthetic.is_empty() || self.synthetic_batch.is_some() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        // Exactly one batch is in flight. It is only taken from the queue when no earlier batch is
        // still waiting for a settled UI pass, so hidden logic ticks neither apply nor discard it.
        if self.synthetic_batch.is_none()
            && let Some(first) = self.synthetic.first().cloned()
        {
            let take = match first {
                egui::Event::Key { pressed: true, .. } => {
                    // Press and the matching release together.
                    if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) { 2 } else { 1 }
                }
                _ => 1,
            };
            let n = take.min(self.synthetic.len());
            self.synthetic_batch = Some(self.synthetic.drain(..n).collect());
        }
        let Some(batch) = self.synthetic_batch.clone() else { return };
        if batch.iter().any(|e| matches!(e, egui::Event::Key { .. })) {
            // egui derives a press's `repeat` flag and the final `keys_down` from every event in the
            // batch. A scripted press+release while the host physically holds the same key would
            // therefore be marked as a repeat and would leave `keys_down` reporting the key as up.
            // Remember the host-only state; `ui` puts it back after egui processed the batch. The
            // hook itself never mutates egui's input, so a hidden logic-only tick cannot corrupt it.
            let previous = ctx.input(|i| i.keys_down.clone());
            self.host_keys_down = Some(Self::host_keys_after(previous, &raw.events));
        }
        if batch.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. } | egui::Event::Zoom(_))) {
            // Remember the pass input and the host's own raw input from before egui folds this
            // batch into its scroll/zoom aggregates, so an expiry in `logic` can replay the host
            // input without it.
            self.input_before_scroll = Some((ctx.input(|i| i.clone()), raw.clone()));
        }
        let mut injected = Vec::with_capacity(batch.len() * 2);
        for e in &batch {
            if matches!(e, egui::Event::PointerButton { pressed: true, .. }) {
                self.synthetic_pointer_active = true;
            }
            if let egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) = e {
                // Keep the hover position current for widgets that check it.
                injected.push(egui::Event::PointerMoved(*pos));
            }
        }
        injected.extend(batch);
        self.injected_at = raw.events.len();
        raw.events.extend(injected.iter().cloned());
        self.injected_events = injected;
    }

    /// The physical key state after the host's own events, ignoring any synthetic batch appended
    /// later. egui starts from the previous pass's `keys_down`, then applies key events in order.
    fn host_keys_after(mut down: std::collections::HashSet<egui::Key>, events: &[egui::Event]) -> std::collections::HashSet<egui::Key> {
        for event in events {
            match event {
                egui::Event::Key { key, pressed: true, .. } => {
                    down.insert(*key);
                }
                egui::Event::Key { key, pressed: false, .. } => {
                    down.remove(key);
                }
                // egui clears all held keys when the window loses focus.
                egui::Event::WindowFocused(false) => down.clear(),
                _ => {}
            }
        }
        down
    }

    /// Remove the events [`Self::raw_input_hook`] appended to an input the host retained, so the
    /// pending batch appears exactly once no matter how many logic-only ticks reuse that input.
    fn remove_injected(&mut self, raw: &mut egui::RawInput) {
        let injected = std::mem::take(&mut self.injected_events);
        let n = injected.len();
        if n == 0 || raw.events.len() < n {
            return;
        }
        let here = raw.events.get(self.injected_at..self.injected_at.saturating_add(n)).is_some_and(|run| run == injected.as_slice());
        // The host appends newer events after the retained copy, so it is usually still at its
        // recorded index; fall back to the last identical run for a host that rewraps its input.
        let found = if here {
            Some(self.injected_at)
        } else {
            (0..=raw.events.len() - n).rev().find(|&i| raw.events.get(i..i + n).is_some_and(|run| run == injected.as_slice()))
        };
        if let Some(i) = found {
            raw.events.drain(i..i + n);
        }
    }

    /// Drop the injected copy from this pass's `InputState.events` after its gesture expired in
    /// `logic`. Returns whether the copy is known to be gone: an empty batch, or an exact run that
    /// was drained. On a mismatch the caller must not draw, because the stale events cannot be
    /// identified safely.
    fn remove_received_injected(&self, ctx: &egui::Context) -> bool {
        let n = self.injected_events.len();
        if n == 0 {
            return true;
        }
        ctx.input_mut(|i| {
            let Some(run) = i.events.get(self.injected_at..self.injected_at.saturating_add(n)) else { return false };
            // egui rewrites a press's `repeat` flag before the app sees it; compare like the ack.
            if run.iter().zip(&self.injected_events).all(|(received, sent)| Self::synthetic_event_received(sent, received)) {
                i.events.drain(self.injected_at..self.injected_at.saturating_add(n));
                true
            } else {
                false
            }
        })
    }

    /// Rebuild this pass's input without the expired scroll/zoom batch. egui has already folded
    /// the batch into `InputState.smooth_scroll_delta`, `zoom_delta` and the persistent wheel
    /// smoothing state, none of which can be undone by draining `events`. Replaying the host's own
    /// raw input from the pre-injection state removes that contribution while keeping simultaneous
    /// host events, held keys and pointer/touch state.
    fn rebuild_input_without_expired_scroll(&mut self, ctx: &egui::Context) {
        let Some((before, mut host_raw)) = self.input_before_scroll.take() else { return };
        // `begin_pass` derives `stable_dt` from its own `requested_immediate_repaint_prev_frame`
        // argument. Keep the real pass's value; layout reads it in this pass.
        let stable_dt = ctx.input(|i| i.stable_dt);
        // `ui` already moved any dropped files from this pass's raw input into `pending_drops`.
        // Do not replay them into the raw as well, or the next pass would route them twice.
        host_raw.dropped_files.clear();
        let options = ctx.options(|o| o.input_options);
        let mut fresh = before.begin_pass(host_raw, false, ctx.pixels_per_point(), options);
        fresh.stable_dt = stable_dt;
        ctx.input_mut(|i| *i = fresh);
        // This real pass consumed the hook-mutated raw, so there is no retained copy left to strip.
        // Keeping the old run would let the next hook's last-identical-run fallback eat a genuine
        // host event that happens to match it, such as the next identical physical wheel notch.
        self.injected_events.clear();
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // egui just processed the scripted key events. A scripted release while the host still
        // physically holds the key would report it up; put the host-only state back before any
        // shortcut or widget reads it.
        if let Some(host_down) = self.host_keys_down.take() {
            ctx.input_mut(|i| i.keys_down = host_down);
        }
        // Dropped files arrive in a real UI pass. Capture and clear them before the font gate, so a
        // first frame that returns before layout keeps them in a one-shot queue for the next
        // laid-out pass, and a forced hidden paint handles them like any other.
        let dropped: Vec<(egui::DroppedFileHandle, Option<egui::Pos2>)> = ctx.input_mut(|i| {
            let pos = i.pointer.latest_pos();
            i.raw.dropped_files.drain(..).map(|f| (f, pos)).collect()
        });
        self.pending_drops.extend(dropped);
        if self.cancel_synthetic_pointer {
            // The deadline may have crossed in logic after this frame's raw-input hook. Do not
            // dispatch its stale press/release to widgets; the next hook cancels it safely before
            // egui computes interactions. Captured drops remain queued for that drawn pass.
            ctx.request_repaint();
            return;
        }
        // A discard rerun gets a fresh `RawInput`, so it never contains the injected run. Only
        // replay/remove when this pass really received it; otherwise a later pass would dispatch
        // the earlier pass's host key/text events twice and roll its time back.
        let received_injected = self.pass_received_injected(&ctx);
        if std::mem::take(&mut self.cancel_synthetic_keys) {
            // A key/text batch crossed its deadline in logic, after the hook had already handed it
            // to this pass. Drop that copy before widgets, shortcuts and egui's end-of-pass
            // quit/zoom shortcuts can execute it. A discard rerun did not receive the batch, so it
            // has nothing to remove and must keep its own input.
            if received_injected && !self.remove_received_injected(&ctx) {
                // The stale copy could not be identified. Do not lay out a pass that might execute
                // it; the next hook drops the retained raw copy.
                ctx.request_repaint();
                return;
            }
            // This real pass consumed the hook-mutated raw. Keep drawing when the stale copy was
            // removed so unrelated host key/text events in the same pass still reach shortcuts and
            // widgets; clearing the recorded run also stops the next hook stripping a genuine
            // identical host event.
            self.injected_events.clear();
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.cancel_synthetic_scroll) {
            if received_injected {
                // A scroll/zoom batch expired after the hook injected it. Rebuild this pass's input
                // without it before widgets read scroll aggregates, then keep drawing so a
                // simultaneous real host key, wheel or pointer event is still dispatched.
                self.rebuild_input_without_expired_scroll(&ctx);
            } else {
                // The batch was not injected into this pass (for example a discarded rerun after a
                // font-gated pass). Replaying the earlier pass's host raw would dispatch its key
                // and text events twice and roll back this pass's time.
                self.input_before_scroll = None;
                self.injected_events.clear();
                ctx.request_repaint();
            }
        }
        // Input preparation: remember this pass's real pointer/keyboard state, which `logic` (run
        // before `ui` in each pass and alone on hidden ticks) uses for gestures and automation.
        self.pointer_down = ctx.input(|i| i.pointer.any_down());
        self.wants_keyboard = ctx.egui_wants_keyboard_input();
        // The integration creates this Ui before logic resolves the current frame's palette.
        ui.set_style(ctx.global_style());
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        if self.arrange_request {
            self.arrange_request = false;
            ctx.memory_mut(|m| m.reset_areas());
        }
        // Capture whether this pass received the batch *before* menus and widgets run. egui widgets
        // legitimately consume matching events from `InputState.events` (`consume_key` in docking,
        // `DragValue` and text completion), so checking afterwards would see them missing, leave
        // the batch pending, and inject the same edit/navigation/cut again next frame.
        let received_synthetic = self.pass_received_synthetic(&ctx);
        shortcuts::handle(self, &ctx);
        self.sample_drop_targets.clear();
        if !self.native_menu_bar {
            menus::menu_bar(self, ui);
        }
        // With the Mix window in its own native window, the main window keeps Edit.
        match if self.ui.mix_separate { MainWindow::Edit } else { self.ui.window } {
            MainWindow::Edit => edit_window::show(self, ui),
            MainWindow::Mix => mix_window::show(self, ui),
        }
        if self.ui.mix_separate {
            mix_window::show_separate(self, &ctx);
        }
        panels::floating(self, &ctx);
        windows::show(self, &ctx);
        video_window::show(self, &ctx);
        ops_windows::show(self, &ctx);
        extra_windows::show(self, &ctx);
        recovery::show(self, &ctx);
        score_editor::show(self, &ctx);
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        #[cfg(not(target_arch = "wasm32"))]
        updates_ui::show(self, &ctx);
        // The sampler's drop rects are current now, so route any captured drop in this pass.
        self.route_dropped_files();
        // The pass that actually received the injected events has already dispatched their widget
        // side effects, even when it then asks for another layout pass. `received_synthetic` was
        // captured before menus and widgets, so a widget that consumed the event from InputState
        // still counts as received. Acknowledge the batch here, before the discard check: leaving
        // it pending would make the next host frame inject the same click/key again and run the
        // command twice. A font-gated pass returns above, and a pass without the events never
        // acknowledges, so those batches stay pending.
        if self.synthetic_batch.is_some() && received_synthetic {
            let delivered = self.synthetic_batch.as_ref().map_or(0, Vec::len);
            if self.synthetic_batch.as_ref().is_some_and(|batch| batch.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })))
            {
                self.synthetic_pointer_active = false;
            }
            self.synthetic_batch = None;
            // The pass consumed this copy; nothing is left for a later host tick to strip.
            self.injected_events.clear();
            // A scripted gesture is not a user clipboard action. Do not let a synthetic copy or
            // PRIMARY selection reach the OS clipboard through the host's platform output.
            ctx.output_mut(|o| {
                o.commands.retain(|c| {
                    !matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_) | egui::OutputCommand::TextSelectionSettled(_))
                });
            });
            control::synthetic_delivered(self, delivered);
            // The next control request queued behind this gesture (inspect, engine command or
            // screenshot) needs a logic tick to be drained now that the barrier is gone.
            ctx.request_repaint();
            #[cfg(test)]
            {
                self.synthetic_applied_batches += 1;
            }
        }
        if !ctx.will_discard() {
            // Frame statistics describe rendered frames: one sample per settled host frame, not one
            // per layout pass and never on a hidden logic-only tick.
            let now = self.clock_now();
            let dt = self.last_rendered_tick.replace(now).map_or(1.0 / 60.0, |prev| now.duration_since(prev).as_secs_f64()) as f32;
            self.frame_ms = self.frame_ms.mul_add(0.9, dt.clamp(0.0, 0.25) * 1000.0 * 0.1);
            control::issue_screenshots(self, &ctx);
        }
        // Here, not in logic(): a covered or minimized window runs only logic(), with no vsync to pace it.
        if self.meters_live() {
            let hz = if self.ui.window == MainWindow::Mix { 60 } else { 30 };
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / hz));
        } else if self.meters.values().any(|m| m.level[0] > 0.0001) {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Whether `received` is the event `wanted` after egui's input pass. egui rewrites the `repeat`
    /// flag of `egui::Event::Key` from the keys held in the previous pass, so a scripted press can
    /// arrive marked as a repeat; the other fields still identify the event. All other events must
    /// match exactly.
    pub(crate) fn synthetic_event_received(wanted: &egui::Event, received: &egui::Event) -> bool {
        match (wanted, received) {
            (
                egui::Event::Key { key, physical_key, pressed, modifiers, .. },
                egui::Event::Key { key: got_key, physical_key: got_physical, pressed: got_pressed, modifiers: got_modifiers, .. },
            ) => key == got_key && physical_key == got_physical && pressed == got_pressed && modifiers == got_modifiers,
            _ => wanted == received,
        }
    }

    /// The exact positions in this pass's events of the pending batch's key presses. Shortcut
    /// handling uses provenance rather than matching key/modifier values: a fresh host press with
    /// the same chord is a separate action, and egui's rewritten `repeat` flag only describes the
    /// physical key state, not which event was injected.
    pub(crate) fn scripted_key_press_indices(&self) -> Vec<usize> {
        if self.synthetic_batch.is_none() {
            return Vec::new();
        }
        self.injected_events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, egui::Event::Key { pressed: true, .. }))
            .map(|(offset, _)| self.injected_at.saturating_add(offset))
            .collect()
    }

    /// Whether this pass's `InputState.events` still starts with the exact injected run. This is
    /// also valid after `expire_gestures` cleared `synthetic_batch`, when a cancel needs to know
    /// whether replaying would address this pass or an earlier one.
    fn pass_received_injected(&self, ctx: &egui::Context) -> bool {
        if self.injected_events.is_empty() {
            return false;
        }
        let Some(end) = self.injected_at.checked_add(self.injected_events.len()) else { return false };
        ctx.input(|i| {
            i.events.len() >= end
                && self.injected_events.iter().enumerate().all(|(offset, sent)| {
                    i.events.get(self.injected_at + offset).is_some_and(|received| Self::synthetic_event_received(sent, received))
                })
        })
    }

    /// Whether this UI pass actually received the pending synthetic batch's events. Call this
    /// before menus and widgets: egui's `consume_key`/`consume_shortcut` remove matching events
    /// from `InputState.events`, so a late check would miss a batch that really was dispatched.
    fn pass_received_synthetic(&self, ctx: &egui::Context) -> bool {
        self.synthetic_batch.is_some() && self.pass_received_injected(ctx)
    }

    /// Load dropped audio into the sampler track under the drop point; leave everything else for
    /// the native/browser shell to open or import.
    fn route_dropped_files(&mut self) {
        if self.pending_drops.is_empty() {
            return;
        }
        let mut claimed = false;
        for (file, pos) in std::mem::take(&mut self.pending_drops) {
            let path = file.path().to_string_lossy().into_owned();
            // As before, one dropped audio file goes to the sampler under the pointer; the rest
            // are left to the shell.
            let claim = !claimed && sampler_window::is_audio_name(&path);
            let Some(track) = self.drop_track(pos).filter(|_| claim) else {
                self.host_drops.push(file);
                continue;
            };
            claimed = true;
            if file.path().is_absolute() {
                let r = self.run("mix.sampler_load", json!({"track": track.0, "path": path}));
                self.sample_loaded(track, &path, r);
            } else {
                self.sample_reads.push(sampler_window::SampleRead { track, file });
            }
        }
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "position": self.position(),
            "dialog": self.dialogs.open_name(),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "status": self.ui.status,
        })
    }
}

fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], gr: f32, dt: f32) {
    // Instant attack, ~26 dB/s release, 2 s peak hold.
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    for c in 0..2 {
        let v = peak.get(c).copied().unwrap_or(0.0);
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        if v >= d.hold.get(c).copied().unwrap_or(0.0) {
            if let Some(h) = d.hold.get_mut(c) {
                *h = v;
            }
            d.hold_age = 0.0;
        }
        if v >= 1.0 {
            d.clip = true;
        }
    }
    d.hold_age += dt;
    if d.hold_age > 2.0 {
        d.hold = d.level;
    }
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// The status line naming the plugin in `slot` (an insert index or
/// [`soundcraft_mix::INSTRUMENT_SLOT`]), which the engine took out of the signal path.
fn failed_plugin_status(s: &soundcraft_model::Session, track: TrackId, slot: usize) -> Option<String> {
    let t = s.track(track)?;
    let ins = if slot == soundcraft_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(slot)?.as_ref() }?;
    let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
    Some(format!("{name} on {} put out invalid audio, so it is bypassed until playback stops or starts again.", t.name))
}

#[cfg(test)]
mod tests {
    use super::{Services, SoundApp, automation_step_due};
    use serde_json::json;
    use soundcraft_model::TrackId;

    #[test]
    fn new_session_stops_playback_and_the_old_clock() {
        for (command, params) in [
            ("session.new", json!({"name": "New project", "sample_rate": 96_000})),
            ("session.new", json!({"template": "demo"})),
            ("session.close", json!({})),
        ] {
            let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
            app.run("transport.play", json!({"from": 48_000})).unwrap();
            app.handle_transport(0.25);
            assert!(app.is_playing());
            assert!(app.position() > 48_000);
            app.run(command, params).unwrap();
            let insertion = app.engine.session().edit.selection.start;
            app.handle_transport(0.25);
            assert!(!app.is_playing(), "{command}");
            assert_eq!(app.position(), insertion, "{command}");
            assert_eq!(app.engine.transport.position, insertion, "{command}");
            app.handle_transport(0.25);
            assert_eq!(app.engine.transport.position, insertion, "the old clock must stay stopped");
            app.run("transport.play", json!({})).unwrap();
            app.handle_transport(0.25);
            assert!(app.is_playing());
            assert_eq!(app.position(), insertion + i64::from(app.engine.session().sample_rate.hz()) / 4);
        }
    }

    #[test]
    fn the_status_names_a_plugin_the_engine_took_out() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        s.track_mut(t).unwrap().mixer.inserts[3] = Some(Insert::new("gain"));
        let msg = super::failed_plugin_status(&s, t, 3).unwrap();
        assert!(msg.starts_with("Gain on Vox put out invalid audio"), "{msg}");
        assert_eq!(super::failed_plugin_status(&s, t, 0), None);
        assert_eq!(super::failed_plugin_status(&s, t, soundcraft_mix::INSTRUMENT_SLOT), None);
    }

    #[test]
    fn replacing_the_session_drops_the_midi_note_selection() {
        for command in ["session.new", "session.close"] {
            let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
            app.midi.selected = vec![0, 1];
            app.midi.clip = Some(soundcraft_model::ClipId(1));
            app.run(command, json!({})).unwrap();
            assert!(app.midi.selected.is_empty() && app.midi.clip.is_none(), "{command}");
        }
    }

    #[test]
    fn new_session_cancels_play_queued_before_the_next_frame() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({})).unwrap();
        app.run("session.new", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(!app.is_playing());
        assert_eq!(app.engine.transport.position, 0);
    }

    #[test]
    fn record_with_nothing_armed_says_what_to_do() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(!app.is_playing() && !app.engine.transport.recording);
        assert!(app.record_hint().is_some_and(|why| why.contains("Track › New")), "{:?}", app.record_hint());

        app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(app.record_hint().is_some_and(|why| why.contains("R button")), "{:?}", app.record_hint());
        assert_eq!(Some(app.ui.status.as_str()), app.record_hint());
        app.last_frame = Some(5.0);
        assert_eq!(app.record_hint(), None, "the hint shows for a few seconds");

        app.last_frame = None;
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        app.run("mix.record_arm", json!({})).unwrap();
        assert_eq!(app.record_hint(), None, "arming a track answers the hint");
    }

    #[test]
    fn a_hidden_window_still_autosaves() {
        let dir = std::env::temp_dir().join(format!("soundcraft-hidden-autosave-{}", std::process::id()));
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.autosave_dir = Some(dir.clone());
        app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
        app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
        let _ = egui::Context::default().run_logic(&raw, |ctx| app.logic(ctx));
        assert!(app.latest_autosave().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A clean demo session with its autosave folder, and an editor gesture still open on Keys
    /// when `gesture` is set (its state hasn't reached the session yet).
    fn app_with_editor_gesture(name: &str, gesture: bool) -> (SoundApp, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}-{gesture}", std::process::id()));
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.autosave_dir = Some(dir.clone());
        if gesture {
            let keys = app.engine.session().track_by_name("Keys").unwrap().id;
            app.editor_states.push((keys, soundcraft_mix::INSTRUMENT_SLOT));
        }
        assert!(!app.engine.is_dirty());
        (app, dir)
    }

    #[test]
    fn an_open_plugin_editor_gesture_is_autosaved() {
        for gesture in [false, true] {
            let (mut app, dir) = app_with_editor_gesture("gesture-autosave", gesture);
            app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
            app.autosave();
            assert_eq!(app.latest_autosave().is_some(), gesture);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn closing_during_a_plugin_editor_gesture_keeps_the_autosave() {
        for gesture in [false, true] {
            let (mut app, dir) = app_with_editor_gesture("gesture-close", gesture);
            app.close_autosave();
            assert_eq!(app.latest_autosave().is_some(), gesture);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn session_reset_does_not_apply_old_stop_edits_to_the_new_session() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({"from": 48_000})).unwrap();
        app.handle_transport(0.25);
        let mut session = soundcraft_model::Session::default();
        session.edit.selection = soundcraft_time::Range::new(12_000, 24_000);
        session.edit.insertion_follows_playback = true;
        session.edit.scrolling = "after_playback".into();
        let selection = session.edit.selection;
        app.engine.replace_session(session);
        app.handle_transport(0.25);
        assert!(!app.is_playing());
        assert_eq!(app.engine.session().edit.selection, selection);
        assert_eq!(app.position(), selection.start);
        assert_eq!(app.engine.session().edit.zoom.scroll, 0);
        assert!(!app.engine.is_dirty());
    }

    #[test]
    fn clear_all_clip_indicators_clears_every_meter() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.meters.entry(kick).or_default().clip = true;
        app.main_meter.clip = true;
        app.run("track.clear_clip_indicators", json!({})).unwrap();
        assert!(!app.meters[&kick].clip && !app.main_meter.clip);
    }

    #[test]
    fn automation_first_pass_after_start_is_due_without_overflow() {
        // `last_write_at` starts (and resets) at `i64::MIN`: the previous
        // `(pos - last).abs()` panicked here in debug builds.
        assert!(automation_step_due(0, i64::MIN, 960));
        assert!(automation_step_due(48_000, i64::MIN, 960));
    }

    #[test]
    fn automation_write_throttles_to_step() {
        assert!(!automation_step_due(1_000, 900, 960));
        assert!(automation_step_due(2_000, 900, 960));
    }

    #[test]
    fn automation_write_fires_after_loop_wrap() {
        assert!(automation_step_due(0, 1_000_000, 960));
    }

    /// Write-mode passes replace automation only along the stretches they play; skipped jumps keep the old ride, and
    /// stop writes the last point.
    #[test]
    fn a_write_pass_replaces_the_automation_it_plays_over() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let fader = app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
        let q = i64::from(app.engine.session().sample_rate.hz()) / 4;
        // The old ride: a point every 10 ms for 3 s.
        for at in (0..12 * q).step_by(usize::try_from(q / 25).unwrap()) {
            app.run("automation.set_point", json!({"track": "Kick", "at": at, "value": -30.0})).unwrap();
        }
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
        app.run("transport.locate", json!({"at": 0})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        // Passes a quarter second apart, sparse like a covered window's.
        let pass = |app: &mut SoundApp| {
            app.handle_transport(0.25);
            app.write_automation();
        };
        pass(&mut app);
        pass(&mut app);
        // Two jumps: the stretches they skip keep the old ride.
        app.run("transport.locate", json!({"at": 4 * q})).unwrap();
        pass(&mut app);
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 7 * q, "end": 11 * q})).unwrap();
        app.run("transport.play_selection", json!({})).unwrap();
        pass(&mut app);
        app.handle_transport(0.25);
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.write_automation();
        let lane = app.engine.session().track_by_name("Kick").unwrap().lane(&soundcraft_model::AutoParam::Volume).unwrap();
        let values = |from, to| lane.points.iter().filter(|p| (from..=to).contains(&p.at)).map(|p| p.value).collect::<Vec<_>>();
        assert_eq!(values(q, 2 * q), [fader; 2]);
        assert_eq!(lane.value_at(3 * q, 0.0), -30.0);
        assert_eq!(lane.value_at(6 * q, 0.0), -30.0);
        // Stop writes the last point, at 9q.
        assert_eq!(values(8 * q, 9 * q), [fader; 2]);
        assert_eq!(lane.value_at(10 * q, 0.0), -30.0);
    }

    /// Automation pass writes and the fader ride driving them collapse into one undo step.
    #[test]
    fn a_pass_and_the_fader_ride_driving_it_are_one_undo_step() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        let steps = app.engine.undo_history().len();
        for frame in 0..60 {
            app.handle_transport(1.0 / 60.0);
            app.write_automation();
            // What the Mix window's fader sends while it is dragged.
            let db = -10.0 + frame as f32 / 10.0;
            app.engine.execute_merged("mix.volume", &json!({"track": kick.0, "db": db}), &format!("fader:{}", kick.0)).unwrap();
        }
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.write_automation();
        assert_eq!(app.engine.undo_history().len(), steps + 1);
    }

    #[test]
    fn plugin_states_read_during_a_drag_keep_it_one_undo_step() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
        let steps = app.engine.undo_history().len();
        for frame in 0..60u8 {
            // What a plugin editor reports each frame of a drag, and the state the plugin marked dirty.
            let gain = -10.0 + f32::from(frame) / 10.0;
            app.engine
                .execute_merged("mix.insert_param", &json!({"track": kick.0, "slot": 0, "param": "gain", "value": gain}), "param:drag")
                .unwrap();
            app.store_plugin_states(vec![(kick, 0, vec![frame, 1])]);
        }
        assert_eq!(app.engine.undo_history().len(), steps + 1);
        assert_eq!(app.engine.session().track(kick).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes(), Some(vec![59, 1]));
    }

    /// One UI frame of plugin editors: the instruments edited in theirs, the plugins that said
    /// their state changed, the editors closed, and the state every plugin has now. Returns the
    /// plugins whose state was read.
    fn editor_frame(app: &mut SoundApp, edited: &[TrackId], changed: &[(TrackId, usize)], closed: &[TrackId], now: u8) -> Vec<(TrackId, usize)> {
        let slot = soundcraft_mix::INSTRUMENT_SLOT;
        let edits: Vec<_> = edited.iter().map(|&t| (t, slot, "cutoff".to_string(), f32::from(now))).collect();
        let due = super::editor_states_due(&mut app.editor_states, &edits, changed.to_vec(), |(t, _)| closed.contains(&t));
        let states = due.iter().map(|&(t, slot)| (t, slot, vec![now, 1])).collect();
        app.store_editor_changes(edits, states);
        due
    }

    fn instrument_state(app: &SoundApp, t: TrackId) -> Option<Vec<u8>> {
        app.engine.session().track(t).unwrap().instrument.as_ref().unwrap().state_bytes()
    }

    #[test]
    fn an_instrument_editor_drag_reads_and_stores_its_state_once() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        let mut reads = Vec::new();
        let mut revisions = Vec::new();
        for now in 0..60 {
            reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
            revisions.push(app.engine.revision);
        }
        reads.extend(editor_frame(&mut app, &[], &[], &[], 59));
        revisions.push(app.engine.revision);
        assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        // The session goes to the audio thread on each new revision: once, not every frame.
        revisions.dedup();
        assert_eq!(revisions.len(), 2, "{revisions:?}");
        assert_eq!(instrument_state(&app, keys), Some(vec![59, 1]));
    }

    #[test]
    fn two_instrument_editor_drags_store_its_state_twice() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        let mut reads = Vec::new();
        for now in 0..10 {
            reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
        }
        reads.extend(editor_frame(&mut app, &[], &[], &[], 9));
        assert_eq!(instrument_state(&app, keys), Some(vec![9, 1]));
        for now in 20..30 {
            reads.extend(editor_frame(&mut app, &[keys], &[], &[], now));
        }
        reads.extend(editor_frame(&mut app, &[], &[], &[], 29));
        assert_eq!(reads.len(), 2);
        assert_eq!(instrument_state(&app, keys), Some(vec![29, 1]));
    }

    #[test]
    fn closing_an_instrument_editor_mid_gesture_still_stores_its_state() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        let lead = app.engine.session().track_by_name("Lead").unwrap().id;
        let lead_before = instrument_state(&app, lead);
        for now in 0..10 {
            editor_frame(&mut app, &[keys, lead], &[], &[], now);
        }
        // Keys' editor closes on a frame it still reports a change in.
        let reads = editor_frame(&mut app, &[keys, lead], &[], &[keys], 10);
        assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        assert_eq!(instrument_state(&app, keys), Some(vec![10, 1]));
        assert_eq!(instrument_state(&app, lead), lead_before);
    }

    #[test]
    fn a_plugin_that_keeps_changing_does_not_hold_back_another_ones_state() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
        // A plugin on Kick marks its state changed on every frame while Keys is dragged, then
        // keeps doing it after the drag.
        for now in 0..10 {
            editor_frame(&mut app, &[keys], &[(kick, 0)], &[], now);
        }
        let reads = editor_frame(&mut app, &[], &[(kick, 0)], &[], 10);
        assert_eq!(reads, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        assert_eq!(instrument_state(&app, keys), Some(vec![10, 1]));
        assert_eq!(app.editor_states, [(kick, 0)]);
    }

    #[test]
    fn a_preset_changed_in_an_effect_editor_is_stored_once() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.insert", json!({"track": kick.0, "plugin": "gain", "slot": 0})).unwrap();
        // No parameter edits: the plugin only says its state changed, frame after frame.
        let mut reads = Vec::new();
        for now in 0..5 {
            reads.extend(editor_frame(&mut app, &[], &[(kick, 0)], &[], now));
        }
        reads.extend(editor_frame(&mut app, &[], &[], &[], 4));
        assert_eq!(reads, [(kick, 0)]);
        let state = app.engine.session().track(kick).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes();
        assert_eq!(state, Some(vec![4, 1]));
    }

    #[test]
    fn preferences_saved_before_the_window_was_remembered_keep_full_scale() {
        let ui: super::UiState = serde_json::from_value(json!({"show_transport": true})).unwrap();
        assert_eq!((ui.window_size, ui.window_maximized, ui.ui_scale), ([1600.0, 1000.0], false, 1.0));
    }

    #[test]
    fn a_window_bigger_than_its_screen_opens_maximized() {
        let maximizes = |w: f32, frames: usize| {
            let ctx = egui::Context::default();
            let info = egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1000.0))),
                monitor_size: Some(egui::vec2(1728.0, 1117.0)),
                ..Default::default()
            };
            let mut maximized = false;
            for _ in 0..frames {
                let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, info.clone())].into_iter().collect(), ..Default::default() };
                let mut out = ctx.run_ui(raw, |ui| super::fit_window_to_screen(ui.ctx()));
                out.textures_delta.clear();
                maximized = out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Maximized(true));
            }
            maximized
        };
        assert!(maximizes(2400.0, 1));
        assert!(!maximizes(1600.0, 1));
        assert!(!maximizes(2400.0, 2));
    }

    #[test]
    fn ordinary_play_after_stop_is_full_speed() {
        use serde_json::json;
        let engine = soundcraft_engine::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
        app.engine.execute("edit.select", &json!({"start": 0, "end": 48000})).unwrap();
        app.engine.execute("options.loop_playback", &json!({"value": true})).unwrap();
        app.engine.execute("options.pre_post_roll", &json!({"value": false})).unwrap();
        let speed = |app: &super::SoundApp| app.player.as_ref().map(|p| p.speed()).unwrap_or(0.0);
        assert!((speed(&app) - 1.0).abs() < 1e-3);

        app.engine.execute("transport.half_speed", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing());
        assert!((speed(&app) - 0.5).abs() < 1e-3, "half-speed playback");

        app.engine.execute("transport.stop", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(!app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "stop restores full speed");

        app.engine.execute("transport.play", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "ordinary play stays at full speed");

        app.engine.execute("transport.stop", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        app.engine.execute("options.loop_playback", &json!({"value": false})).unwrap();
        app.engine.execute("transport.half_speed", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!((speed(&app) - 0.5).abs() < 1e-3);
        app.player.as_ref().unwrap().stop();
        app.handle_transport(1.0 / 60.0);
        assert!(!app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "a half-speed pass that ends on its own restores full speed");
    }
    /// A synthetic batch survives hidden logic-only ticks and is applied exactly once by a real
    /// UI pass; hidden ticks never re-inject or discard it.
    #[test]
    fn one_synthetic_batch_survives_hidden_ticks_and_applies_once() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let raw = |w: f32| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 900.0));
            egui::RawInput { screen_rect: Some(rect), ..Default::default() }
        };
        for _ in 0..4 {
            let mut r = raw(1400.0);
            app.raw_input_hook(&ctx, &mut r);
            let mut out = ctx.run_ui(r, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        // Synthetic pointer input is delivered one event per host frame, like a real drag.
        let pos = egui::pos2(40.0, 40.0);
        app.synthetic.push(egui::Event::PointerMoved(pos));
        for _ in 0..3 {
            let mut r = raw(1400.0);
            app.raw_input_hook(&ctx, &mut r);
            let _ = ctx.run_logic(&r, |ctx| app.logic(ctx));
            assert_eq!(app.synthetic.len(), 0, "the batch was taken from the queue once");
            assert_eq!(app.synthetic_batch.as_ref().map(Vec::len), Some(1), "the batch stays pending across hidden ticks");
            assert_eq!(app.synthetic_applied_batches, 0, "hidden ticks do not acknowledge the batch");
        }
        let mut r = raw(1400.0);
        app.raw_input_hook(&ctx, &mut r);
        let mut out = ctx.run_ui(r, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.synthetic_applied_batches, 1);
        assert!(app.synthetic_batch.is_none());
    }

    #[derive(Debug)]
    struct TestDrop(std::path::PathBuf);

    impl egui::DroppedFile for TestDrop {
        /// The test drop's file path; its bytes are empty because these tests only need a name.
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        /// Test drops carry no bytes; native imports only need the path in these tests.
        #[cfg(not(target_arch = "wasm32"))]
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }
        /// Test drops carry no bytes; the browser path only needs a name in these tests.
        #[cfg(target_arch = "wasm32")]
        fn bytes_async(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, String>> + '_>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    /// A drop captured on the font-gated first frame survives to the first laid-out pass and is
    /// handed to the shell exactly once.
    #[test]
    fn a_drop_before_fonts_are_ready_is_imported_once_after_layout() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
        let mut first =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
        first.dropped_files.push(std::sync::Arc::new(TestDrop(std::path::PathBuf::from("/tmp/soundcraft-drop.scraft"))));
        let mut out = ctx.run_ui(first, |ui| app.ui(ui));
        out.textures_delta.clear();
        assert!(app.take_dropped_files().is_empty(), "the drop is queued, not dropped on the font gate");
        for _ in 0..3 {
            let mut r =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
            app.raw_input_hook(&ctx, &mut r);
            let mut out = ctx.run_ui(r, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        let dropped = app.take_dropped_files();
        assert_eq!(dropped.len(), 1, "the session file reached the shell once");
        assert!(dropped.first().is_some_and(|f| f.path().ends_with("soundcraft-drop.scraft")));
        assert!(app.take_dropped_files().is_empty(), "later ticks do not replay the drop");
    }

    /// eframe keeps the hook-mutated `RawInput` for the next hidden tick and appends new host
    /// events to it. Re-injecting the batch must replace that copy, not add another: a restored
    /// click or key must run once, not once per hidden tick.
    #[test]
    fn a_retained_host_input_holds_one_copy_of_a_pending_batch() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let host_input = || egui::RawInput {
            screen_rect: Some(rect),
            viewports: [(egui::ViewportId::ROOT, hidden.clone())].into_iter().collect(),
            ..Default::default()
        };
        // Let fonts install and the palette settle first: the first pass draws nothing.
        for _ in 0..3 {
            let mut r = host_input();
            app.raw_input_hook(&ctx, &mut r);
            let mut out = ctx.run_ui(r, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        // A click is three queued pointer events; each becomes one batch on its own host frame.
        let pos = egui::pos2(40.0, 40.0);
        app.synthetic.push(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        }
        let mut retained = host_input();
        for tick in 0..5 {
            // `prepare_raw_input`: newer host input appended to the retained, hook-mutated input.
            let mut newer = host_input();
            newer.events.push(egui::Event::PointerMoved(egui::pos2(tick as f32, 0.0)));
            retained.append(newer);
            app.raw_input_hook(&ctx, &mut retained);
            let batch = app.synthetic_batch.clone().expect("the first pointer event is pending");
            assert_eq!(batch.len(), 1, "one pointer event per host frame");
            let copies = retained.events.iter().filter(|e| batch.contains(*e)).count();
            assert_eq!(copies, 2, "the pending event and its hover copy appear exactly once, not once per tick");
            let _ = ctx.run_logic(&retained, |ctx| app.logic(ctx));
            assert!(app.synthetic_batch.is_some(), "hidden ticks never apply the batch");
            assert_eq!(app.synthetic_applied_batches, 0);
        }
        // A settled UI pass applies each queued pointer event exactly once.
        for applied in 1..=3 {
            let mut raw = host_input();
            app.raw_input_hook(&ctx, &mut raw);
            let mut out = ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
            assert_eq!(app.synthetic_applied_batches, applied);
        }
        assert!(app.synthetic_batch.is_none(), "every queued event was delivered once");
        assert!(app.synthetic.is_empty());
    }

    /// A batch waiting for a UI pass under a hidden window asks the host for a real paint, so it is
    /// delivered instead of spinning forever on logic ticks.
    #[test]
    fn a_pending_synthetic_batch_asks_a_hidden_window_for_a_paint() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        app.synthetic.push(egui::Event::Text("x".into()));
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
            viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(),
            ..Default::default()
        };
        app.raw_input_hook(&ctx, &mut raw);
        assert!(app.synthetic_batch.is_some());
        let out = ctx.run_logic(&raw, |ctx| app.logic(ctx));
        let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
        assert!(commands.contains(&egui::ViewportCommand::RequestPaintWhileHidden), "{commands:?}");
        assert_eq!(app.synthetic_applied_batches, 0, "logic alone does not apply the gesture");
    }

    /// A frame that discards and lays out a second pass is still one rendered frame: frame
    /// statistics must not sample the extra pass's near-zero delta.
    #[test]
    fn frame_time_counts_a_multi_pass_frame_once() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        for _ in 0..90 {
            app.advance_test_clock(std::time::Duration::from_secs_f64(1.0 / 60.0));
            let mut discard = true;
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                if discard {
                    // A widget that needs another layout pass (Grid, a timeline measurement) asks
                    // for the discard during this pass, before `ui` settles.
                    discard = false;
                    ui.ctx().request_discard("two-pass frame test");
                }
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        let ms = app.frame_ms;
        assert!((ms - 1000.0 / 60.0).abs() < 0.5, "{ms} ms/frame across two-pass frames");
    }

    /// Scripted input is automation, not a user clipboard action: a synthetic copy/selection in a
    /// text widget must not reach the OS clipboard or PRIMARY selection through platform output.
    #[test]
    fn a_synthetic_pass_does_not_publish_os_clipboard_commands() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        // Control: the same command from a pass without synthetic input still reaches the host.
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            ui.ctx().copy_text("real user copy".into());
            app.ui(ui);
        });
        assert!(
            out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text.as_str() == "real user copy")),
            "the host still receives real copy commands"
        );
        out.textures_delta.clear();

        app.synthetic.push(egui::Event::Text("x".into()));
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            // Stand in for a TextEdit/widget that emits clipboard commands during the pass.
            ui.ctx().copy_text("scripted copy".into());
            ui.ctx().output_mut(|o| o.commands.push(egui::OutputCommand::TextSelectionSettled("scripted selection".into())));
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.synthetic_applied_batches, 1, "the pass delivered the synthetic event");
        assert!(
            !out.platform_output.commands.iter().any(|c| matches!(
                c,
                egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_) | egui::OutputCommand::TextSelectionSettled(_)
            )),
            "scripted input must not publish clipboard or PRIMARY commands: {:?}",
            out.platform_output.commands
        );
    }

    /// egui rewrites a key press's `repeat` flag from the keys held in the previous pass. A scripted
    /// press must still run its shortcut even while the same physical key is held, a same-frame host
    /// auto-repeat with the same chord must not make it run twice, and the host's held state must
    /// survive the scripted press and release.
    #[test]
    fn a_held_key_does_not_turn_a_scripted_press_into_a_repeat() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        for _ in 0..2 {
            app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        }
        let depth = app.engine.undo_history().len();
        assert!(depth >= 2, "the test needs two undoable edits");
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        // The user presses and holds Cmd+Z. That real press is one undo and leaves Z physically
        // down; a later same-chord press is rewritten to a repeat.
        let mut held = raw();
        held.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
        let mut out = ctx.run_ui(held, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(ctx.input(|i| i.keys_down.contains(&egui::Key::Z)), "the host is still holding Z");
        assert_eq!(app.engine.undo_history().len(), depth - 1, "the real host press ran one undo");
        let held_depth = app.engine.undo_history().len();
        // Cmd+Z from the control channel while that chord is held, with the host's same-frame
        // auto-repeat already in the raw input ahead of the appended synthetic batch. Both events
        // have the same key, physical key and modifiers; only the host one is a repeat.
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
        }
        let mut input = raw();
        input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: true, modifiers });
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none(), "the pass that received the rewritten press acknowledged it");
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the scripted release did not report the physically held key as up");
        assert_eq!(app.engine.undo_history().len(), held_depth - 1, "the scripted Cmd+Z ran exactly once despite the identical host repeat");
        // The following host frame must not replay the same key.
        let mut input = raw();
        input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: true, modifiers });
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.engine.undo_history().len(), held_depth - 1, "a later host auto-repeat must not undo a second time");
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the host still holds Z after the repeat");
    }

    /// A fresh physical Cmd+Z and an identical scripted Cmd+Z in one frame are distinct actions.
    /// Matching them by value would swallow the scripted press; provenance keeps both.
    #[test]
    fn a_fresh_host_press_and_a_scripted_press_both_run() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        for _ in 0..2 {
            app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        }
        let depth = app.engine.undo_history().len();
        assert!(depth >= 2, "the test needs two undoable edits");
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
        }
        let mut input = raw();
        input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none(), "the pass acknowledged the injected batch");
        assert_eq!(app.engine.undo_history().len(), depth - 2, "the physical press and the injected press are separate actions");
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the physical press still holds Z after the injected release");
    }

    /// A hidden logic-only tick after the hook runs no egui input pass. Repeated hidden ticks that
    /// re-hook the retained host input must leave the physical key state and one pending batch
    /// untouched.
    #[test]
    fn a_hidden_logic_tick_after_the_hook_keeps_the_held_key_and_pending_batch() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let visible = egui::ViewportInfo { inner_rect: Some(rect), ..Default::default() };
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let raw = |viewport: egui::ViewportInfo| egui::RawInput {
            screen_rect: Some(rect),
            viewports: [(egui::ViewportId::ROOT, viewport)].into_iter().collect(),
            ..Default::default()
        };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(visible.clone()), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        let depth = app.engine.undo_history().len();
        assert!(depth > 0, "the test needs an undoable edit");
        // Alt+Z keeps Z physically down without firing the plain-Z Commands Focus undo.
        let mut held = raw(visible.clone());
        held.events.push(egui::Event::Key {
            key: egui::Key::Z,
            physical_key: Some(egui::Key::Z),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
        });
        let mut out = ctx.run_ui(held, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
        assert_eq!(app.engine.undo_history().len(), depth, "holding Alt+Z is not a command");
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
        }
        // eframe retains a hook-mutated input across hidden ticks and hands it back to the hook.
        let mut retained = raw(hidden);
        for tick in 0..3 {
            app.raw_input_hook(&ctx, &mut retained);
            assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "hook tick {tick} mutated the held state");
            assert!(app.synthetic_batch.is_some(), "hook tick {tick} dropped the batch");
            assert_eq!(app.engine.undo_history().len(), depth, "hook tick {tick} ran a shortcut");
            let _ = ctx.run_logic(&retained, |ctx| app.logic(ctx));
            assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "hidden tick {tick} erased the physically held key");
            assert!(app.synthetic_batch.is_some(), "hidden tick {tick} applied or dropped the batch");
            assert_eq!(app.engine.undo_history().len(), depth, "hidden tick {tick} ran a shortcut");
        }
        // The deferred batch still reaches the next UI pass exactly once.
        let mut out = ctx.run_ui(retained, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none());
        assert_eq!(app.engine.undo_history().len(), depth - 1, "the deferred scripted Cmd+Z ran exactly once");
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the held key survives the deferred pass too");
    }

    /// A frame that asks for a second layout pass receives the injected key and dispatches it in
    /// the first pass. The batch must be acknowledged once, and the physically held key must still
    /// be down in the settled pass rather than being cleared by the scripted release.
    #[test]
    fn a_multi_pass_frame_keeps_the_held_key_and_applies_the_scripted_press_once() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        let depth = app.engine.undo_history().len();
        assert!(depth > 0, "the test needs an undoable edit");
        let mut held = raw();
        held.events.push(egui::Event::Key {
            key: egui::Key::Z,
            physical_key: Some(egui::Key::Z),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
        });
        let mut out = ctx.run_ui(held, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
        }
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut first_pass = true;
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            if first_pass {
                first_pass = false;
                // A measurement or grid asks for another layout pass after the widgets ran.
                ui.ctx().request_discard("discard the pass that received the synthetic key");
            }
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(out.platform_output.num_completed_passes >= 2, "the frame really ran more than one pass");
        assert!(app.synthetic_batch.is_none(), "the pass that received the batch consumed it");
        assert_eq!(app.engine.undo_history().len(), depth - 1, "one scripted Cmd+Z undid exactly one edit");
        assert_eq!(app.synthetic_applied_batches, 1);
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)), "the held key survives the settled multi-pass frame");
    }

    /// Losing focus clears the physical key state even when a scripted key batch is in flight. The
    /// injected press still runs once, and restoring the host-only state must not bring the stale
    /// held key back.
    #[test]
    fn a_focus_loss_clears_a_held_key_with_a_pending_scripted_press() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        let depth = app.engine.undo_history().len();
        assert!(depth > 0, "the test needs an undoable edit");
        let mut held = raw();
        held.events.push(egui::Event::Key {
            key: egui::Key::Z,
            physical_key: Some(egui::Key::Z),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers { alt: true, ..egui::Modifiers::NONE },
        });
        let mut out = ctx.run_ui(held, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(ctx.input(|i| i.key_down(egui::Key::Z)));
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed, repeat: false, modifiers });
        }
        let mut input = raw();
        input.events.push(egui::Event::WindowFocused(false));
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none(), "the injected batch still ran once after focus loss");
        assert_eq!(app.engine.undo_history().len(), depth - 1);
        assert!(!ctx.input(|i| i.key_down(egui::Key::Z)), "focus loss cleared the held key and the injected release did not resurrect it");
        // The next host frame must not bring the stale key back either.
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(!ctx.input(|i| i.key_down(egui::Key::Z)));
        assert_eq!(app.engine.undo_history().len(), depth - 1);
    }

    /// A host key with the same signature as the pending injection does not acknowledge a batch
    /// that was never actually appended. Otherwise a lookalike event could hide a lost scripted
    /// action.
    #[test]
    fn a_host_key_with_the_same_signature_does_not_acknowledge_the_batch() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        for _ in 0..2 {
            app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        }
        let depth = app.engine.undo_history().len();
        assert!(depth >= 2, "the test needs two undoable edits");
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        // The batch is pending but this pass was not prepared by the hook, so it carries no injected
        // copy. The identical host press is a real action, not the injected one.
        app.synthetic_batch =
            Some(vec![egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers }]);
        let mut input = raw();
        input.events.push(egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers });
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_some(), "a lookalike host event must not acknowledge the batch");
        assert_eq!(app.synthetic_applied_batches, 0);
        assert_eq!(app.engine.undo_history().len(), depth - 1, "only the real host press ran");
        // The hook then appends the actual injection; that pass acknowledges and runs it once.
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none());
        assert_eq!(app.synthetic_applied_batches, 1);
        assert_eq!(app.engine.undo_history().len(), depth - 2, "the actual injected press ran in addition to the host press");
    }

    /// The acknowledgement compares key events without egui's rewritten `repeat` flag. Every other
    /// identifying field — the logical key, physical key, press/release state and modifiers — still
    /// has to match.
    #[test]
    fn synthetic_key_acknowledgement_ignores_only_the_rewritten_repeat_flag() {
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        let sent = egui::Event::Key { key: egui::Key::Z, physical_key: Some(egui::Key::Z), pressed: true, repeat: false, modifiers };
        let mut received = sent.clone();
        if let egui::Event::Key { repeat, .. } = &mut received {
            *repeat = true;
        }
        assert!(super::SoundApp::synthetic_event_received(&sent, &received), "egui's repeat rewrite does not hide the press");
        let mut wrong = received.clone();
        if let egui::Event::Key { key, .. } = &mut wrong {
            *key = egui::Key::X;
        }
        assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a different logical key is not the scripted press");
        let mut wrong = received.clone();
        if let egui::Event::Key { physical_key, .. } = &mut wrong {
            *physical_key = None;
        }
        assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a different physical key is not the scripted press");
        let mut wrong = received.clone();
        if let egui::Event::Key { pressed, .. } = &mut wrong {
            *pressed = false;
        }
        assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "a release is not the scripted press");
        let mut wrong = received;
        if let egui::Event::Key { modifiers: got, .. } = &mut wrong {
            *got = egui::Modifiers::NONE;
        }
        assert!(!super::SoundApp::synthetic_event_received(&sent, &wrong), "different modifiers are not the scripted press");
        assert!(!super::SoundApp::synthetic_event_received(&sent, &egui::Event::Text("z".into())), "other variants match exactly");
    }

    /// A pass that receives the synthetic events and then asks for another layout pass has already
    /// dispatched their widget side effects. It must acknowledge the batch: leaving it pending
    /// makes the next host frame inject the same click/key again (a double undo, a toggled command,
    /// a dialog action run twice).
    #[test]
    fn a_discarded_pass_acknowledges_the_synthetic_batch_it_received() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        app.run("track.new", json!({"count": 1, "format": "stereo"})).unwrap();
        let depth = app.engine.undo_history().len();
        assert!(depth > 0, "the test needs an undoable edit");
        // Cmd+Z, queued as egui delivers a real key: press and release together.
        let modifiers = egui::Modifiers { command: true, ..egui::Modifiers::NONE };
        for pressed in [true, false] {
            app.synthetic.push(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed, repeat: false, modifiers });
        }
        let mut first_pass = true;
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            if first_pass {
                first_pass = false;
                // A measurement or grid asks for another layout pass after the widgets ran.
                ui.ctx().request_discard("discard the pass that received the synthetic key");
            }
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_none(), "the pass that received the batch consumed it");
        assert_eq!(app.engine.undo_history().len(), depth - 1, "one Cmd+Z undid exactly one edit");
        assert_eq!(app.synthetic_applied_batches, 1);
        // The following host frame must not replay the same key.
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.engine.undo_history().len(), depth - 1, "a later frame must not undo a second time");
        assert_eq!(app.synthetic_applied_batches, 1);
    }

    /// The real toolbar tempo `DragValue`/`TextEdit` consumes scripted keys from
    /// `InputState.events` (arrow navigation), yet each scripted edit, navigation step and cut is
    /// dispatched exactly once. A post-widget acknowledgement would miss the consumed press, keep
    /// the batch pending and apply it again on the next frame.
    #[test]
    fn real_widgets_apply_a_scripted_edit_navigation_and_cut_once_despite_consuming_the_events() {
        struct Rig {
            ctx: egui::Context,
            app: SoundApp,
            shapes: Vec<egui::epaint::ClippedShape>,
        }

        impl Rig {
            /// A 1600x1000 rig over a one-track session, warmed for four frames.
            fn new() -> Self {
                let mut engine = soundcraft_engine::Engine::default();
                engine.execute("track.new", &json!({})).unwrap();
                let mut rig = Self { ctx: egui::Context::default(), app: SoundApp::new(engine, None, Services::default()), shapes: Vec::new() };
                rig.frames(4);
                rig
            }

            /// A blank 1600x1000 host frame.
            fn raw(&self) -> egui::RawInput {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
                egui::RawInput { screen_rect: Some(rect), ..Default::default() }
            }

            /// Run one UI pass with `events`, keeping the emitted shapes for position lookups.
            fn frame(&mut self, events: Vec<egui::Event>) {
                let mut input = self.raw();
                input.events = events;
                self.app.raw_input_hook(&self.ctx, &mut input);
                let mut out = self.ctx.run_ui(input, |ui| {
                    self.app.logic(ui.ctx());
                    self.app.ui(ui);
                });
                out.textures_delta.clear();
                self.shapes = out.shapes;
            }

            /// Advance `n` blank frames.
            fn frames(&mut self, n: usize) {
                for _ in 0..n {
                    self.frame(Vec::new());
                }
            }

            /// Queue events as scripted input and run one frame.
            fn synthetic(&mut self, events: Vec<egui::Event>) {
                self.app.synthetic.extend(events);
                self.frame(Vec::new());
            }

            /// The session tempo at tick 0.
            fn bpm(&self) -> f64 {
                self.app.engine.session().tempo.tempo_at_tick(0)
            }

            /// Centre of the toolbar tempo, which shows `bpm` with four decimals.
            fn field(&self, bpm: f64) -> egui::Pos2 {
                let text = format!("{bpm:.4}");
                self.shapes
                    .iter()
                    .find_map(|c| match &c.shape {
                        egui::Shape::Text(t) if t.galley.text() == text => Some(t.galley.rect.translate(t.pos.to_vec2()).center()),
                        _ => None,
                    })
                    .expect("tempo not in the toolbar")
            }

            /// Send a primary press or release at `at` in its own frame.
            fn button(&mut self, at: egui::Pos2, pressed: bool) {
                self.frame(vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
                ]);
            }

            /// Press and release at `at`, then let the UI settle for two frames.
            fn click(&mut self, at: egui::Pos2) {
                self.button(at, true);
                self.button(at, false);
                self.frames(2);
            }
        }

        let key = |key: egui::Key, modifiers: egui::Modifiers| {
            [
                egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
                egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
            ]
        };
        let mut rig = Rig::new();
        let mut applied = rig.app.synthetic_applied_batches;

        // Edit: typing replaces the selected tempo, and Enter commits it once.
        let at = rig.field(120.0);
        rig.click(at);
        assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo TextEdit has focus");
        rig.synthetic(vec![egui::Event::Text("98.5".into())]);
        rig.synthetic(key(egui::Key::Enter, egui::Modifiers::NONE).to_vec());
        rig.frames(2);
        assert!((rig.bpm() - 98.5).abs() < 1e-9, "the scripted edit applied once: {}", rig.bpm());
        assert!(rig.app.synthetic_batch.is_none());
        assert_eq!(rig.app.synthetic_applied_batches, applied + 2);
        applied = rig.app.synthetic_applied_batches;

        // Navigation: the focused DragValue consumes ArrowUp out of InputState.events.
        rig.frames(6); // The scripted gesture's pointer grace must end before a real click.
        rig.click(rig.field(98.5));
        assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo DragValue is editing");
        rig.synthetic(key(egui::Key::ArrowUp, egui::Modifiers::NONE).to_vec());
        assert!(
            !rig.ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::ArrowUp, pressed: true, .. }))),
            "the real DragValue consumed the injected ArrowUp press"
        );
        assert!(rig.app.synthetic_batch.is_none(), "the pass that let DragValue consume ArrowUp still acknowledged it");
        let stepped = rig.bpm();
        assert!((stepped - 98.6).abs() < 1e-9, "one scripted navigation step: {stepped}");
        rig.frames(2);
        assert!((rig.bpm() - stepped).abs() < 1e-9, "a later frame must not step again");
        assert_eq!(rig.app.synthetic_applied_batches, applied + 1);
        applied = rig.app.synthetic_applied_batches;

        // Cut: select the tempo text and cut it once. The value only changes on commit, so cancel
        // with Escape and keep the previous tempo.
        rig.frames(6);
        rig.click(rig.field(stepped));
        assert!(rig.ctx.egui_wants_keyboard_input(), "the real tempo TextEdit is editing again");
        rig.synthetic(key(egui::Key::A, egui::Modifiers { command: true, ..egui::Modifiers::NONE }).to_vec());
        rig.synthetic(vec![egui::Event::Cut]);
        assert!(rig.app.synthetic_batch.is_none(), "the scripted cut was acknowledged");
        assert!(
            !rig.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == "98.6000")),
            "the scripted cut removed the selected tempo text"
        );
        assert_eq!(rig.app.synthetic_applied_batches, applied + 2);
        rig.synthetic(key(egui::Key::Escape, egui::Modifiers::NONE).to_vec());
        rig.frames(2);
        assert!((rig.bpm() - stepped).abs() < 1e-9, "cutting the edit text and cancelling leaves the tempo unchanged");
    }

    /// A hidden logic-only tick has no fresh input: the last pass's pointer must not write Touch
    /// automation or leave the fader gesture stuck.
    #[test]
    fn a_hidden_tick_does_not_write_automation_from_the_last_pass_pointer() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "touch"})).unwrap();
        app.run("transport.locate", json!({"at": 0})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let pos = egui::pos2(700.0, 450.0);
        let press = egui::RawInput {
            screen_rect: Some(rect),
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            ],
            ..Default::default()
        };
        let mut out = ctx.run_ui(press, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(ctx.input(|i| i.pointer.any_down()), "the visible pass left the pointer down");
        let start_db = app.engine.session().track(kick).unwrap().mixer.volume_db;
        app.gesture = Some(super::Gesture::Fader { track: kick, start_db });
        app.last_write_at = i64::MIN;
        let points = |app: &super::SoundApp| {
            app.engine.session().track(kick).and_then(|t| t.lane(&soundcraft_model::AutoParam::Volume)).map_or(0, |l| l.points.len())
        };
        let before = points(&app);
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let raw =
            egui::RawInput { screen_rect: Some(rect), viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
        let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
        assert!(app.gesture.is_none(), "visibility loss drops the transient fader gesture");
        assert_eq!(points(&app), before, "a hidden tick wrote automation from the stale pointer");
        app.run("transport.stop", json!({})).unwrap();
    }

    /// The font-gated first pass returns before layout, so it cannot have dispatched the batch
    /// and must leave it pending for the first laid-out pass.
    #[test]
    fn a_font_gated_pass_does_not_ack_the_pending_synthetic_batch() {
        let ctx = egui::Context::default();
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        app.synthetic.push(egui::Event::Text("x".into()));
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(!app.fonts_ready, "the first pass installs fonts but does not lay out");
        assert!(app.synthetic_batch.is_some(), "the font gate returns before the acknowledgement");
        assert_eq!(app.synthetic_applied_batches, 0);
        // The next hook offers the same batch to the first laid-out pass, which applies it once.
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.fonts_ready);
        assert!(app.synthetic_batch.is_none());
        assert_eq!(app.synthetic_applied_batches, 1);
        // And the following frame must not replay it.
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut out = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.synthetic_applied_batches, 1);
    }

    /// A settled pass that never received the injected events (for example a forced second pass
    /// after a font-gated first pass) must not consume the pending batch.
    #[test]
    fn a_settled_pass_that_did_not_receive_the_batch_does_not_ack_it() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw(), |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
        app.synthetic_batch = Some(vec![egui::Event::Text("x".into())]);
        let mut out = ctx.run_ui(raw(), |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert!(app.synthetic_batch.is_some(), "a pass without the events keeps the batch pending");
        assert_eq!(app.synthetic_applied_batches, 0);
        let mut injected = raw();
        app.raw_input_hook(&ctx, &mut injected);
        let mut out = ctx.run_ui(injected, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
        assert_eq!(app.synthetic_applied_batches, 1);
        assert!(app.synthetic_batch.is_none());
    }

    /// The transport simulation advances by real elapsed monotonic time, not by a full frame step
    /// per call and not by the stale UI input clock.
    #[test]
    fn transport_simulation_advances_by_monotonic_elapsed_time_only() {
        let ctx = egui::Context::default();
        let mut app = super::SoundApp::new(soundcraft_engine::demo::demo_engine(), None, super::Services::default());
        app.sim = Some((0, None, None));
        app.engine.transport.playing = true;
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() };
        let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
        // Two passes of one host frame must not each advance a full predicted frame.
        let before = app.position();
        let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
        let same_frame = app.position() - before;
        assert!(same_frame.abs() < 480, "a repeated pass advanced {same_frame} samples");
        app.advance_test_clock(std::time::Duration::from_millis(200));
        let _ = ctx.run_logic(&raw, |ctx| app.logic(ctx));
        let elapsed = app.position() - before;
        let expected = (0.2 * app.engine.session().sample_rate.as_f64()) as i64;
        assert!((elapsed - expected).abs() < expected / 20, "200 ms advanced {elapsed} samples, expected about {expected}");
        app.engine.transport.playing = false;
        app.sim = None;
    }
    /// Plugin queues are drained once per host iteration, rather than once per discarded pass.
    /// The fixture replaces only native polling; the real raw hook, logic/UI, scheduler and
    /// engine state/undo commands run normally.
    #[test]
    fn editor_gesture_polling_survives_discarded_and_hidden_host_iterations() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        let ctx = egui::Context::default();
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let raw = || egui::RawInput { screen_rect: Some(rect), ..Default::default() };
        for _ in 0..4 {
            let mut input = raw();
            app.raw_input_hook(&ctx, &mut input);
            let mut output = ctx.run_ui(input, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            output.textures_delta.clear();
        }
        let keys = app.engine.session().track_by_name("Keys").unwrap().id;
        let original = instrument_state(&app, keys);
        let revision = app.engine.revision;
        let undo = app.engine.undo_history().len();
        let history = app.engine.undo_history();
        let journal = app.engine.journal.clone();
        let state_command = soundcraft_engine::find_command("mix.insert_state").unwrap();
        assert!(!state_command.undoable && !state_command.journal, "this fixture exercises state-only instrument persistence");
        assert!(!app.engine.is_dirty());
        let reports = Rc::new(RefCell::new(Vec::<soundcraft_playback::EditorEdit>::new()));
        let latest = Rc::new(Cell::new(1u8));
        let polls = Rc::new(Cell::new(0));
        let captures = Rc::new(RefCell::new(Vec::new()));
        let (poll_reports, poll_latest, poll_count, poll_captures) = (reports.clone(), latest.clone(), polls.clone(), captures.clone());
        app.editor_poll_fixture = Some(Box::new(move |pending| {
            poll_count.set(poll_count.get() + 1);
            let edits = std::mem::take(&mut *poll_reports.borrow_mut());
            super::editor_changes(
                pending,
                edits,
                Vec::new(),
                |_| false,
                |due| {
                    poll_captures.borrow_mut().extend_from_slice(due);
                    due.iter().map(|&(track, slot)| (track, slot, vec![poll_latest.get(), 1])).collect()
                },
            )
        }));
        reports.borrow_mut().push((keys, soundcraft_mix::INSTRUMENT_SLOT, "cutoff".into(), 1.0));
        let mut input = raw();
        app.raw_input_hook(&ctx, &mut input);
        let mut passes = 0;
        let mut output = ctx.run_ui(input, |ui| {
            app.logic(ui.ctx());
            passes += 1;
            if passes == 1 {
                ui.ctx().request_discard("plugin gesture must survive a second layout pass");
            }
            app.ui(ui);
        });
        output.textures_delta.clear();
        assert!(passes >= 2, "the regression must actually force a discarded pass");
        assert_eq!(polls.get(), 1, "the rerun cannot drain an empty editor queue");
        assert!(captures.borrow().is_empty(), "the ongoing gesture has not ended");
        assert_eq!(app.editor_states, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        assert_eq!(instrument_state(&app, keys), original);
        assert_eq!(app.engine.revision, revision);
        assert_eq!(app.engine.undo_history().len(), undo);
        assert!(app.has_unsaved_changes(), "pending editor work still needs autosave/recovery");

        // The next host hook is hidden: it still polls the continuing gesture despite a stale
        // discarded-pass index and without drawing any UI.
        latest.set(2);
        reports.borrow_mut().push((keys, soundcraft_mix::INSTRUMENT_SLOT, "cutoff".into(), 2.0));
        let mut hidden = raw();
        hidden.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().minimized = Some(true);
        app.raw_input_hook(&ctx, &mut hidden);
        let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
        let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
        assert_eq!(polls.get(), 2, "hidden hook rearms exactly one poll, not one per logic callback");
        assert!(captures.borrow().is_empty());
        assert_eq!(app.editor_states, [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        assert_eq!(instrument_state(&app, keys), original);
        assert_eq!(app.engine.revision, revision);

        // A genuine next-iteration empty poll ends the gesture and persists its latest state.
        app.raw_input_hook(&ctx, &mut hidden);
        let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
        assert_eq!(polls.get(), 3);
        assert_eq!(*captures.borrow(), [(keys, soundcraft_mix::INSTRUMENT_SLOT)]);
        assert!(app.editor_states.is_empty());
        assert_eq!(instrument_state(&app, keys), Some(vec![2, 1]));
        assert_eq!(app.engine.revision, revision.wrapping_add(1), "the completed gesture persists exactly one document revision");
        assert!(app.engine.is_dirty(), "captured instrument state is unsaved document work");
        assert_eq!(app.engine.undo_history().len(), undo, "state-only instrument persistence does not create an undo step");
        assert_eq!(app.engine.undo_history(), history, "state capture does not replace another history entry");
        assert_eq!(app.engine.journal, journal, "plugin-state blobs are not journaled");
        let settled_revision = app.engine.revision;
        app.raw_input_hook(&ctx, &mut hidden);
        let _ = ctx.run_logic(&hidden, |ctx| app.logic(ctx));
        assert_eq!(polls.get(), 4, "a fresh empty host iteration is still live");
        assert_eq!(captures.borrow().len(), 1, "the completed gesture is captured once");
        assert_eq!(app.engine.revision, settled_revision);
        assert_eq!(app.engine.undo_history().len(), undo);
        assert_eq!(app.engine.undo_history(), history);
        assert_eq!(app.engine.journal, journal);
    }
}
