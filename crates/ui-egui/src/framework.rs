//! The craft-ui runtime, hosted from SoundCraft's own eframe `App` (craft-libs docs/craft-framework.md, section 2.2).
//! The desktop app calls [`install`] once, [`SoundApp::before_input`] in `raw_input_hook`, [`SoundApp::close_path`]
//! last in `logic`, and [`before_ui`] first in `ui`; the browser build and tests install with [`install_headless`].
//! [`SoundApp::run_framework_dialogs`] runs the framework dialogs after SoundCraft's own UI.
//!
//! The runtime takes Escape, Tab and the clipboard events for the whole window unless one of SoundCraft's own
//! egui widgets has the keyboard. The window's Escape is read with [`escape_pressed`]; the session view is declared
//! to the runtime with [`host_canvas`], which keeps the edit chords and the clipboard events in egui's input for
//! SoundCraft's shortcuts, and Tab while the focused view has work for it. A focused view leaves the keyboard to the
//! shortcuts ([`widget_has_keyboard`]).

use std::sync::{Arc, OnceLock};

use craft_host::target::ShellInfo;
use craft_host::{AppInfo, Host};
use craft_keymap::{Chord, Key, Mods, Overrides};
use craft_ui::fonts::{I18nScripts, SystemFonts, UiFonts};
use craft_ui::runtime::commands::{CommandInput, CommandSet, NavCommand, QuitCommand};
use craft_ui::runtime::env::{RootEnv, Scale};
use craft_ui::runtime::focus::{EditSet, FocusTarget, KeepKeys, KeyUse};
use craft_ui::runtime::frame::{FrameInput, FrameOutput};
use craft_ui::runtime::keys::{Intent, with_window_intents};
use craft_ui::runtime::overlay::{self, OpenFilter};
use craft_ui::shell::embed::{self, CloseDecision};
use craft_window::system::WindowSystem;

use crate::SoundApp;

/// Each pass of the root window, first in `App::ui`: hands the runtime the native window it answers the hit test of
/// (Windows) and fits the window to its monitor once.
#[cfg(not(target_arch = "wasm32"))]
pub use craft_ui::shell::embed::before_ui;

/// The scripts of the interface languages, for the font stack.
static SCRIPTS: I18nScripts = I18nScripts;

/// The runtime's first font stack: egui's bundled faces and no installed system fonts. SoundCraft's own definitions
/// ([`crate::fonts::install`]) replace it on the first frame, before anything is drawn.
fn ui_fonts() -> UiFonts<'static> {
    UiFonts::new(&SCRIPTS).system_fonts(SystemFonts::Off)
}

fn install_with(ctx: &egui::Context, shell: ShellInfo, windows: Arc<dyn WindowSystem>) -> Result<(), String> {
    let spec = craft_ui::InstallSpec::new(shell, windows).app_id(crate::os_window::app_id());
    embed::install(ctx, spec, &ui_fonts()).map(drop).map_err(|error| format!("the UI runtime could not be installed: {error}"))
}

/// Installs the runtime on the desktop window's context, once, before the first pass. `forced_x11` when the app
/// opens its window through XWayland. SoundCraft keeps the system's title bar on every platform.
#[cfg(not(target_arch = "wasm32"))]
pub fn install(ctx: &egui::Context, forced_x11: bool) -> Result<(), String> {
    use craft_host::target::Backend;
    let backend = if forced_x11 { Backend::X11 } else { Backend::from_env() };
    install_with(ctx, ShellInfo::default().backend(backend), Arc::new(craft_window::system::SystemWindows))
}

/// Installs the runtime on a context with no native window of its own: the browser build's canvas and test windows.
pub fn install_headless(ctx: &egui::Context) {
    if let Err(error) = install_with(ctx, ShellInfo::default(), Arc::new(craft_window::system::NoWindows)) {
        log::warn!("{error}");
    }
}

/// Who SoundCraft is to the framework's dialogs (the update dialogs show its name and icon).
pub(crate) fn host() -> Arc<Host> {
    Arc::new(
        Host::builder(AppInfo { id: "soundcraft", name: "SoundCraft", version: env!("CARGO_PKG_VERSION") })
            .icon_png(include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.soundcraft.png"))
            .build(),
    )
}

/// The runtime's default keys SoundCraft's own shortcuts use: F6 and Shift+F6 move between panes by default, and F6
/// is the Trim tool in SoundCraft; Cmd+Q (Ctrl+Q) is the runtime's Quit, and SoundCraft quits through its own
/// `app.quit`, which keeps its recording and recovery rules. Overrides are keyed by command id, which the command set
/// gives.
fn command_overrides() -> Arc<Overrides> {
    static OVERRIDES: OnceLock<Arc<Overrides>> = OnceLock::new();
    OVERRIDES
        .get_or_init(|| {
            let mut overrides = Overrides::new();
            overrides.insert(NavCommand::NextPane.id(), Vec::new());
            overrides.insert(NavCommand::PreviousPane.id(), Vec::new());
            overrides.insert(QuitCommand::Quit.id(), Vec::new());
            Arc::new(overrides)
        })
        .clone()
}

impl SoundApp {
    /// The framework's part of `App::raw_input_hook`, once per frame before the first pass: takes what the runtime
    /// gathered since the last frame and gives the coming passes the root values (theme mode, interface scale).
    pub fn before_input(&mut self, ctx: &egui::Context) {
        embed::before_input(ctx, |output| self.frame_input(output));
    }

    /// The root values of this frame. SoundCraft registers no commands with the runtime and lets it toggle no
    /// chrome, so the output carries nothing it acts on. The interface scale is the one SoundCraft applied last
    /// (`extra_windows`), so the runtime's zoom and SoundCraft's agree.
    fn frame_input(&mut self, _output: FrameOutput) -> FrameInput {
        let mode = if crate::theme::is_light() { craft_theme::Mode::Light } else { craft_theme::Mode::Dark };
        // SoundCraft zooms with Cmd/Ctrl+scroll itself, so a smooth Ctrl+wheel stays a wheel event.
        let root = RootEnv::new().mode(mode).scale(Scale::new(self.extra.applied_scale())).ctrl_wheel_pinch(false);
        FrameInput::new(root).commands(CommandInput::new().overrides(command_overrides())).host(Arc::clone(&self.framework_host))
    }

    /// The framework's part of `App::logic`, after SoundCraft's own: the root window's close path. A close the app
    /// asked for itself goes through; otherwise unsaved changes hold the window open while Save Changes asks, and
    /// its answer quits the app.
    pub fn close_path(&mut self, ctx: &egui::Context) {
        embed::logic(ctx, || {
            if self.quit_requested || !crate::dialogs::ask_to_save(self, "app.quit", &serde_json::json!({})) {
                CloseDecision::Close
            } else {
                CloseDecision::Hold
            }
        });
    }

    /// The framework dialogs open on [`SoundApp::framework_dialogs`], once per pass after SoundCraft's own UI.
    pub(crate) fn run_framework_dialogs(&mut self, ui: &mut egui::Ui) {
        self.framework_dialogs.run(ui);
    }
}

/// The edit-mode keys (Alt+1 to Alt+4, Pro Tools' Shuffle, Slip, Spot and Grid) the session view claims, so Alt-code
/// entry leaves them to SoundCraft's shortcuts.
const EDIT_MODE_KEYS: [Chord; 4] =
    [Chord::new(Mods::ALT, Key::Num1), Chord::new(Mods::ALT, Key::Num2), Chord::new(Mods::ALT, Key::Num3), Chord::new(Mods::ALT, Key::Num4)];

/// The widget of the session view: the Edit window's tracks and timeline (`edit`) or the Mix window's strips.
pub fn session_view_id(edit: bool) -> egui::Id {
    egui::Id::unique(("soundcraft-session-view", edit))
}

/// Declares the session view `id`, drawn in this pass, as the window's document canvas. With nothing focused it is
/// the window's key target: the edit chords, the clipboard events and the edit-mode keys stay in egui's input for
/// SoundCraft's shortcuts, as do the arrows, Enter and the page keys. Tab and Shift+Tab stop at the view; with
/// `keeps_tab` the focused view keeps them in egui's input, else they move on from it.
pub(crate) fn host_canvas(ctx: &egui::Context, id: egui::Id, keeps_tab: bool) {
    const KEYS: KeyUse = KeyUse::CANVAS.with_edits(EditSet::ALL).with_claims(&EDIT_MODE_KEYS);
    let keys = if keeps_tab { KEYS.with_keep(KeepKeys::Tab) } else { KEYS };
    craft_ui::runtime::focus::host_canvas(ctx, id, keys);
}

/// Gives the session view `id`, declared in this pass, the keyboard focus.
pub(crate) fn focus_session_view(ctx: &egui::Context, id: egui::Id) {
    craft_ui::runtime::focus::request(ctx, FocusTarget::Id(id));
}

/// Whether a widget other than the session view has the keyboard. A focused session view leaves it to SoundCraft's
/// shortcuts, as with nothing focused.
pub(crate) fn widget_has_keyboard(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.focused()).is_some_and(|focused| ![true, false].map(session_view_id).contains(&focused))
}

/// Whether a modal dialog, SoundCraft's, egui's or the framework's, or an egui menu or popup is up. A framework menu
/// holds the focus in its own layer.
pub(crate) fn overlay_open(app: &SoundApp, ctx: &egui::Context) -> bool {
    app.dialogs.open.is_some()
        || app.ui.show_preferences
        || app.ui.show_search
        || egui::Popup::is_any_open(ctx)
        || ctx.memory(|memory| memory.top_modal_layer()).is_some()
        || overlay::any_open(ctx, OpenFilter::Modal)
}

/// Whether Escape was pressed in this pass: from egui while one of SoundCraft's own widgets or popups has the
/// keyboard, otherwise the window's cancel, which the runtime routes to no component. Leaves it for later readers.
pub(crate) fn escape_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape)) || window_cancel_pending(ctx)
}

/// Whether the window's cancel (an Escape while nothing has framework focus) is there and untaken.
fn window_cancel_pending(ctx: &egui::Context) -> bool {
    let mut pending = false;
    with_window_intents(ctx, false, |intents| {
        pending = intents.untaken(&Intent::Cancel);
        false
    });
    pending
}
