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
pub mod fonts;
pub mod icons;
pub mod import;
pub mod menus;
pub mod mix_window;
pub mod palette;
pub mod panels;
pub mod shortcuts;
pub mod theme;
pub mod toolbar;
pub mod updates_ui;
pub mod video_track;
pub mod waveform;
pub mod widgets;

use serde_json::{Value, json};
use soundcraft_engine::{Engine, TransportRequest};
use soundcraft_model::TrackId;
use soundcraft_playback::record::Trouble;
use soundcraft_playback::{OutputTrouble, Player};
use soundcraft_time::{Range, Samples};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;

pub use control::ControlRequest;
pub use dialogs::ops_windows;
use dialogs::recent;
pub use dialogs::recovery;
pub use dialogs::spot;
pub use panels::extra_windows;
pub use panels::midi_editor;
pub use panels::sampler_window;
pub use panels::score_editor;
pub use panels::video_window;
pub use panels::windows;

mod app;
mod services;
mod state;

#[cfg(test)]
use app::{automation_step_due, editor_changes, editor_states_due, failed_plugin_status};
pub use services::Services;
pub use state::{DragSource, MainWindow};

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
    /// Whether the system fonts are loaded next to the bundled ones; set before the first frame.
    system_fonts: bool,
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

#[cfg(test)]
type EditorPollFixture = Box<dyn FnMut(&mut Vec<(TrackId, usize)>) -> (Vec<soundcraft_playback::EditorEdit>, soundcraft_playback::PluginStates)>;

#[cfg(test)]
mod tests;
