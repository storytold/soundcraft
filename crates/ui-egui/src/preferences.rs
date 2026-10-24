//! SoundCraft / Setup › Preferences: the complete settings surface, including placeholders for features whose
//! underlying behaviour does not exist yet. A placeholder is never interactive: enabling a row
//! requires a real command and persistence policy first.

use crate::SoundApp;
use crate::theme::{ThemeMode, Tokens};
use egui::{Key, Modifiers, RichText, Stroke, vec2};
use serde_json::json;

const PREFERENCES_WINDOW_WIDTH: f32 = 900.0;
const PREFERENCES_WINDOW_HEIGHT: f32 = 945.0;
const PREFERENCES_BODY_HEIGHT: f32 = 800.0;
const PREFERENCES_SCREEN_MARGIN: f32 = 24.0;
const PREFERENCES_MIN_HEIGHT: f32 = 320.0;

/// One of the pages exposed by the Preferences window, in shortcut order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferencesPage {
    #[default]
    Display,
    Operation,
    Editing,
    Mixing,
    Metering,
    Processing,
    Midi,
    Collaboration,
    Synchronization,
}

/// Controls when optional interface animation and decoration are reduced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReduceVisualEffects {
    #[default]
    Never,
    Playback,
    Always,
}

impl ReduceVisualEffects {
    pub const ALL: [Self; 3] = [Self::Never, Self::Playback, Self::Always];

    /// Returns the text shown for this mode in preference selectors.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::Playback => "Only During Playback",
            Self::Always => "Always",
        }
    }

    /// Returns the stable command and persistence identifier for this mode.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Playback => "playback",
            Self::Always => "always",
        }
    }

    /// Parses a command or persistence identifier into a visual-effects mode.
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "never" => Some(Self::Never),
            "playback" => Some(Self::Playback),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

impl PreferencesPage {
    pub const ALL: [Self; 9] = [
        Self::Display,
        Self::Operation,
        Self::Editing,
        Self::Mixing,
        Self::Metering,
        Self::Processing,
        Self::Midi,
        Self::Collaboration,
        Self::Synchronization,
    ];

    /// Returns the page title shown in the tab strip.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Display => "Display",
            Self::Operation => "Operation",
            Self::Editing => "Editing",
            Self::Mixing => "Mixing",
            Self::Metering => "Metering",
            Self::Processing => "Processing",
            Self::Midi => "MIDI",
            Self::Collaboration => "Collaboration",
            Self::Synchronization => "Synchronization",
        }
    }

    /// Returns the static section catalog belonging to this page.
    const fn sections(self) -> &'static [Section] {
        match self {
            Self::Display => DISPLAY,
            Self::Operation => OPERATION,
            Self::Editing => EDITING,
            Self::Mixing => MIXING,
            Self::Metering => METERING,
            Self::Processing => PROCESSING,
            Self::Midi => MIDI,
            Self::Collaboration => COLLABORATION,
            Self::Synchronization => SYNCHRONIZATION,
        }
    }
}

/// Values edited in the window before the user applies or cancels them.
#[derive(Debug, Clone, Copy)]
struct PreferencesDraft {
    theme: ThemeMode,
    reduce_visual_effects: ReduceVisualEffects,
    undo_levels: usize,
    theme_edited: bool,
    reduce_visual_effects_edited: bool,
    undo_levels_edited: bool,
}

impl PreferencesDraft {
    /// Starts a clean draft from the current live, per-machine settings.
    fn from_live(app: &SoundApp) -> Self {
        Self {
            theme: app.ui.theme,
            reduce_visual_effects: app.ui.reduce_visual_effects,
            undo_levels: app.ui.undo_levels,
            theme_edited: false,
            reduce_visual_effects_edited: false,
            undo_levels_edited: false,
        }
    }

    /// Pulls in scripted setting changes while retaining fields edited in this window.
    fn sync_unedited_from_live(&mut self, app: &SoundApp) {
        if !self.theme_edited {
            self.theme = app.ui.theme;
        }
        if !self.reduce_visual_effects_edited {
            self.reduce_visual_effects = app.ui.reduce_visual_effects;
        }
        if !self.undo_levels_edited {
            self.undo_levels = app.ui.undo_levels;
        }
    }

    /// Remembers which values the user changed during the current UI frame.
    fn remember_user_edits(&mut self, before: Self) {
        self.theme_edited |= self.theme != before.theme;
        self.reduce_visual_effects_edited |= self.reduce_visual_effects != before.reduce_visual_effects;
        self.undo_levels_edited |= self.undo_levels != before.undo_levels;
    }
}

/// Transient Preferences-window state that is never stored in a session.
#[derive(Debug, Default)]
pub struct PreferencesState {
    search: String,
    draft: Option<PreferencesDraft>,
    popup_open: bool,
}

impl PreferencesState {
    /// Drops unapplied values and search text when the window is closed by any route.
    pub(crate) fn discard_draft(&mut self) {
        self.draft = None;
        self.search.clear();
        self.popup_open = false;
    }
}

/// A declarative preference control, including disabled placeholders without a backend yet.
#[derive(Clone, Copy)]
enum Control {
    Check(&'static str),
    Choice(&'static str, &'static str),
    ChoiceOptions(&'static str, &'static str, &'static [&'static str]),
    Value(&'static str, &'static str),
    Button(&'static str),
    Radios(&'static str, &'static [&'static str]),
    Theme,
    ReduceVisualEffects,
    UndoLevels,
}

impl Control {
    /// Returns the user-facing name used by the control and search index.
    const fn label(self) -> &'static str {
        match self {
            Self::Check(label)
            | Self::Button(label)
            | Self::Choice(label, _)
            | Self::ChoiceOptions(label, _, _)
            | Self::Value(label, _)
            | Self::Radios(label, _) => label,
            Self::Theme => "UI Theme",
            Self::ReduceVisualEffects => "Reduce Visual Effects",
            Self::UndoLevels => "Levels of Undo",
        }
    }

    /// Builds the searchable text from a control's label, value and available choices.
    fn search_text(self) -> String {
        match self {
            Self::Choice(label, value) | Self::Value(label, value) => format!("{label} {value}"),
            Self::ChoiceOptions(label, value, values) => format!("{label} {value} {}", values.join(" ")),
            Self::Radios(label, values) => format!("{label} {}", values.join(" ")),
            Self::ReduceVisualEffects => format!("{} Never Only During Playback Always", self.label()),
            _ => self.label().to_string(),
        }
    }
}

/// A titled group of controls within a preference page.
#[derive(Clone, Copy)]
struct Section {
    title: &'static str,
    controls: &'static [Control],
}

const DISPLAY: &[Section] = &[
    Section {
        title: "Basics",
        controls: &[
            Control::Check("Track Position Numbers Stay with Hidden Tracks"),
            Control::Check("Function Tool Tips"),
            Control::Check("Detailed Tool Tips"),
            Control::Value("Edit Window Default Length", "00:00:00:00"),
            Control::Choice("Organize Plugin Menus By", "Category & Manufacturer"),
            Control::Choice("Organize Track I/O Menus By", "Type"),
            Control::Check("Auto-Switch Input Language"),
            Control::Choice("Language", "English"),
            Control::Check("Default Automatic Naming to English"),
            Control::Theme,
            Control::ReduceVisualEffects,
        ],
    },
    Section {
        title: "General Color",
        controls: &[
            Control::Check("MIDI Note Color Shows Velocity"),
            Control::Choice("Default Track Color Coding", "Track Type"),
            Control::Choice("Default Clip Color Coding", "Track Color"),
        ],
    },
    Section {
        title: "Warnings & Dialogs",
        controls: &[
            Control::Button("Reset “Don’t Show This Again” Settings"),
            Control::Check("Show Dashboard Window at Startup"),
            Control::Check("Show Dashboard After Closing a Session"),
        ],
    },
    Section {
        title: "Marker Colors",
        controls: &[
            Control::Check("Randomize Marker Color Upon Creation"),
            Control::Check("Show Marker Lane Colors in Timeline Rulers"),
            Control::Check("Show Marker Lane Colors in Tracks"),
        ],
    },
    Section {
        title: "Pinned Tracks",
        controls: &[
            Control::Check("Auto-Adjust Pinned Area for Tracks"),
            Control::Check("Adjusting Pinned Area Resizes Tracks"),
            Control::Choice("Original Track Height While Pinned", "Small"),
        ],
    },
];

const OPERATION: &[Section] = &[
    Section {
        title: "Transport",
        controls: &[
            Control::Check("Timeline Insertion and Play Start Follow Playback"),
            Control::Check("Edit Insertion Follows Scrub and Shuttle"),
            Control::Check("Audio During Fast Forward and Rewind"),
            Control::Check("Latch Fast Forward and Rewind"),
            Control::Check("Play Start Follows Timeline Selection"),
            Control::Check("Dynamic Transport Follows Selection During Playback"),
            Control::Check("Reserve Voices for Preview in Context"),
            Control::Value("Custom Shuttle Lock Speed", "800 %"),
            Control::Value("Custom Fast Forward/Rewind Speed", "300 %"),
            Control::Value("Back/Forward Amount", "2|0|000"),
            Control::Choice("Back/Forward Time Scale", "Bars|Beats"),
            Control::Radios("Numeric Keypad", &["Classic", "Transport", "Shuttle"]),
            Control::Check("Use Separate Play and Stop Keys"),
        ],
    },
    Section {
        title: "Record",
        controls: &[
            Control::Choice("Record Enable Buttons", "Latch Audio Tracks"),
            Control::Check("Enable Automation in Record"),
            Control::Check("Link Record and Play Faders"),
            Control::Check("Audio Track Record Lock"),
            Control::Check("Transport Record Lock"),
            Control::Check("Disable Input Monitoring when Disarming a Stopped Track"),
            Control::Check("Mute Record-Armed Tracks While Stopped"),
            Control::Check("Enable DSP Mode for Record-Armed or Input-Monitored Tracks"),
            Control::Check("Disable DSP Mode when Disabling Record and Input"),
            Control::Check("Console-Style Input Monitoring Indicators"),
            Control::Check("Create New Playlists when Loop Recording"),
            Control::Radios("Online Recording Starts", &["At Timecode Lock", "At Insertion or Selection"]),
            Control::Value("Destructive Punch File Length", "25 minutes"),
        ],
    },
    Section {
        title: "Auto Backup",
        controls: &[
            Control::Check("Enable Session File Auto Backup"),
            Control::Value("Keep Most Recent Session Backups", "10"),
            Control::Value("Backup Every", "5 minutes"),
        ],
    },
    Section {
        title: "Miscellaneous",
        controls: &[
            Control::Check("Mouse Wheel Scrolling Snaps to Track"),
            Control::Value("Clip Auto Fade In/Out Length", "0 ms"),
            Control::Value("Local Waveform Cache Versions", "0"),
        ],
    },
    Section {
        title: "User Media and Settings Location",
        controls: &[
            Control::Button("Change Documentation and Settings Location"),
            Control::Button("Change Project Media Cache Location"),
            Control::Button("Change Sound Libraries Location"),
            Control::Check("Show Sound Libraries in Workspace Locations"),
        ],
    },
    Section {
        title: "Video",
        controls: &[
            Control::Check("Enable GPU Processing"),
            Control::Check("Prevent Tearing"),
            Control::Choice("Play Start Latency", "1 — Shortest"),
            Control::Choice("Slewing", "Disabled While Looping"),
        ],
    },
    Section {
        title: "Data Collection and Personalization",
        controls: &[Control::Check("Anonymous Usage Data"), Control::Check("Personalize the Experience")],
    },
    Section {
        title: "Logging",
        controls: &[Control::Button("Save Logs"), Control::Button("Reset Logging"), Control::Check("Advanced Video Logging")],
    },
];

const EDITING: &[Section] = &[
    Section {
        title: "Clips",
        controls: &[
            Control::Check("Clip List Selection Follows Edit Selection"),
            Control::Check("Edit Selection Follows Clip List Selection"),
            Control::Check("Auto-Name Separated Clips"),
            Control::Check("Separate Clip Operates on All Related Takes"),
            Control::Check("Send Fully Overlapped Clips to a Playlist While Editing"),
            Control::Check("Send Fully Overlapped Clips to a Playlist While Recording"),
            Control::Check("Tab Respects Breakpoints in Automation Lanes"),
            Control::Check("Double-Click Selects Automation Breakpoints"),
            Control::Value("Clip Gain Nudge Value", "0.5 dB"),
            Control::Choice("Number Keys 1–5 Control", "Zoom Presets"),
            Control::Choice("Double-Clicking an Audio Clip Opens", "Name Dialog"),
        ],
    },
    Section {
        title: "Fades",
        controls: &[
            Control::Value("Fades Dialog Preview Pre-Roll", "3000 ms"),
            Control::Value("Fades Dialog Preview Post-Roll", "3000 ms"),
            Control::Value("Punch Recording Crossfade Length", "0 ms"),
            Control::Check("Preserve Fades when Editing"),
            Control::Check("Auto Accept Adjust Bounds"),
            Control::Choice("Smart Tool Fade Adjustment", "Always On"),
            Control::Button("Default Fade In"),
            Control::Button("Default Fade Out"),
            Control::Button("Default Crossfade"),
            Control::Button("Default Sliced-Loop Fade"),
        ],
    },
    Section {
        title: "Tracks",
        controls: &[
            Control::Check("New Tracks Default to Tick Timebase"),
            Control::Check("Suppress Name Dialog when Creating New Playlists"),
            Control::Check("Show Target Playlists after Sending a Clip Selection"),
            Control::Choice("Add New Playlists", "At Bottom"),
        ],
    },
    Section {
        title: "Zoom Toggle",
        controls: &[
            Control::Choice("Vertical MIDI Zoom", "Selection"),
            Control::Choice("Horizontal Zoom", "Selection"),
            Control::Check("Remove Range Selection after Zooming In"),
            Control::Choice("Track Height", "Fit to Window"),
            Control::Choice("Track View", "No Change"),
            Control::Check("Separate Grid Settings when Zoomed In"),
            Control::Check("Zoom Toggle Follows Edit Selection"),
        ],
    },
    Section {
        title: "Memory Locations",
        controls: &[
            Control::Check("Auto-Name Memory Locations when Playing"),
            Control::Check("Recall Memory Location at Original Track"),
            Control::Check("Marker Target Follows First Selected Track"),
        ],
    },
    Section { title: "Undo", controls: &[Control::UndoLevels] },
];

const MIXING: &[Section] = &[
    Section {
        title: "Setup",
        controls: &[
            Control::Value("Sends Default to", "−∞ dB"),
            Control::Check("Send Pans Default to Follow Main Pan"),
            Control::Check("Use Absolute Pan Linking"),
            Control::Check("Allow Sends to Persist During Low-Latency Monitoring"),
            Control::Choice("Default EQ", "None"),
            Control::Choice("Default EQ Insert Slot", "None"),
            Control::Choice("Default Dynamics", "None"),
            Control::Choice("Default Dynamics Insert Slot", "None"),
            Control::Check("Auto Insert Default Plugins from Control Surfaces"),
        ],
    },
    Section {
        title: "Controllers",
        controls: &[
            Control::Check("Edit Window Follows Banking"),
            Control::Check("Edit Window Follows Selection"),
            Control::Check("Mix Window Follows Banking"),
            Control::Check("Mix Window Follows Selection"),
            Control::Check("Scroll to Track Banks Controllers"),
            Control::Check("Always Fill Channel Strips when Banking"),
            Control::Check("Always Show Assignable Knob Highlight"),
            Control::Value("Touch Timeout", "1000 ms"),
            Control::Choice("EQ and Dynamics Cycling Starts at Insert", "None"),
        ],
    },
    Section {
        title: "Automation",
        controls: &[
            Control::Check("Smooth and Thin Data after Pass"),
            Control::Choice("Degree of Thinning", "Some"),
            Control::Check("Plugin Controls Default to Auto-Enabled"),
            Control::Check("Suppress Automation Write-To Warnings"),
            Control::Check("Latch Switch Controls in Touch Mode"),
            Control::Check("Allow Latch Prime in Stop"),
            Control::Check("Coalesce when Removing Members from a VCA Group"),
            Control::Check("Standard VCA Logic for Group Attributes"),
            Control::Check("Include Sends in Trim Mode"),
            Control::Check("Include Control Changes in Undo Queue"),
            Control::Check("Link Object and Audio Recording"),
            Control::Radios("After Write Pass, Switch To", &["Touch", "Latch", "No Change"]),
            Control::Radios("Coalesce Trim Automation", &["After Every Pass", "On Exiting Trim Mode", "Manually"]),
            Control::Value("Steep Breakpoint Smoothing Time", "0 ms"),
            Control::Value("Automation Match Time", "250 ms"),
            Control::Value("Automation Glide Time", "1000 ms"),
        ],
    },
    Section {
        title: "Delay Compensation",
        controls: &[Control::Choice("Time Mode", "Samples"), Control::Check("Compensate Side Chains"), Control::Check("Compensate Hardware Inputs")],
    },
    Section {
        title: "Routing Coefficients",
        controls: &[
            Control::Choice("New Session Fold Down", "Modern"),
            Control::Choice("New Session 5.x to 7.x/9.x", "Direct to Rear Surrounds"),
            Control::Choice("New Session 7.x/9.x to 5.x", "Downmix to Surrounds"),
            Control::Value("New Session Surround to Stereo", "−3.0 dB"),
            Control::Check("New Session Includes LFE in Fold Down"),
            Control::Choice("Current Session Fold Down", "Modern"),
            Control::Choice("Current Session 5.x to 7.x/9.x", "Direct to Rear Surrounds"),
            Control::Choice("Current Session 7.x/9.x to 5.x", "Downmix to Surrounds"),
            Control::Value("Current Session Surround to Stereo", "−3.0 dB"),
            Control::Check("Current Session Includes LFE in Fold Down"),
        ],
    },
];

const METER_TYPES: &[&str] = &[
    "Sample Peak",
    "Classic",
    "Linear",
    "Linear Extended",
    "RMS",
    "VU",
    "Digital VU",
    "PPM Digital",
    "PPM BBC",
    "PPM Nordic",
    "PPM EBU",
    "PPM DIN",
    "K-12",
    "K-14",
    "K-20",
    "Live Peak",
    "Live RMS",
];

const METERING: &[Section] = &[
    Section {
        title: "Track and Master Meter Types",
        controls: &[
            Control::Check("Track and Master Meter Types Linked"),
            Control::ChoiceOptions("Track Meters", "Digital VU", METER_TYPES),
            Control::ChoiceOptions("Master Meters", "Linear", METER_TYPES),
        ],
    },
    Section {
        title: "Advanced Meter Type Settings",
        controls: &[
            Control::Choice("Meter Type", "Digital VU"),
            Control::Value("Decay", "100 dB in 1.00 s"),
            Control::Value("0 dB Reference", "0 dBFS"),
            Control::Value("Integration Time", "— ms"),
            Control::Value("Color Break High", "−6 dB"),
            Control::Value("Color Break Low", "−20 dB"),
            Control::Button("Reset Advanced Meter Settings"),
        ],
    },
    Section {
        title: "Peak / Clip",
        controls: &[
            Control::Radios("Peak Hold", &["3 Seconds", "Infinite", "None"]),
            Control::Radios("Clip Indication", &["3 Seconds", "Infinite", "None"]),
            Control::Value("Calibration Reference Level", "−18 dB"),
        ],
    },
    Section {
        title: "Display",
        controls: &[
            Control::Check("Show Send Assignment Level Meter"),
            Control::Check("Show Insert Assignment Gain Reduction Meter"),
            Control::Check("Show Track Gain Reduction Meter"),
            Control::Radios(
                "Gain Reduction Meter Type",
                &["Compressor/Limiter Only", "Expander/Gate Only", "Priority Compressor/Limiter", "Priority Expander/Gate", "All — Summed"],
            ),
        ],
    },
];

const PROCESSING: &[Section] = &[
    Section {
        title: "Offline Processing",
        controls: &[Control::Radios("Default Handle Length", &["Whole File", "Length"]), Control::Value("Handle Length", "2.00 seconds")],
    },
    Section {
        title: "Time Compression / Expansion",
        controls: &[Control::Choice("Processor", "Time Stretch"), Control::Choice("Default Settings", "Factory Default")],
    },
    Section {
        title: "Import",
        controls: &[
            Control::Check("Convert Imported WAV Files to Broadcast Wave"),
            Control::Check("Automatically Copy Files on Import"),
            Control::Check("Convert Copied Files to Session Format"),
            Control::Check("Do Not Convert Sample Rate on Import"),
            Control::Check("Import Sliced Loops as Clip Groups"),
            Control::Check("Automatically Create Fades"),
            Control::Radios("Desktop Drag-and-Drop Conforms to Session Tempo", &["No Files", "Tagged Loop Files Only", "All Files"]),
            Control::Choice("Sample Rate Conversion Quality", "Highest"),
        ],
    },
    Section {
        title: "Elastic Audio",
        controls: &[
            Control::Choice("Default Plugin", "Polyphonic"),
            Control::Value("Default Input Gain", "0.0 dB"),
            Control::Check("Enable Elastic Audio on New Tracks"),
        ],
    },
    Section { title: "Commit", controls: &[Control::Radios("Rendered File Bit Depth", &["Always Use 32-bit", "Follow Session Settings"])] },
    Section {
        title: "DSP Management",
        controls: &[Control::Check("Open Unresourced DSP Plugins as Native"), Control::Check("Enable Analog Console Processing in New Sessions")],
    },
    Section { title: "ARA", controls: &[Control::Check("Automatic Analysis on Tick-Based Tracks")] },
    Section {
        title: "Miscellaneous",
        controls: &[
            Control::Choice("Parallel Task Optimization", "High"),
            Control::Check("Display Waveforms in Low Resolution While Loading"),
            Control::Check("Noise Burst Peak Protection"),
        ],
    },
];

const MIDI: &[Section] = &[
    Section {
        title: "Basics",
        controls: &[
            Control::Check("Remove Range Selection after Opening a MIDI or Score Editor"),
            Control::Check("Use MIDI to Tap Tempo"),
            Control::Check("Display Events as Modified by Real-Time Properties"),
            Control::Check("Use F11 for Wait for Note"),
            Control::Check("Automatically Create Click Track in New Sessions"),
            Control::Check("MIDI Input Monitoring Follows Selected Track"),
            Control::Value("Pencil Resolution for Controller Data", "30 ms"),
            Control::Value("Global MIDI Playback Offset", "0 samples"),
            Control::Choice("Double-Clicking a MIDI Clip Opens", "Docked MIDI Editor"),
            Control::Choice("MIDI Merge Release Mode", "Latch Until Next Event"),
        ],
    },
    Section { title: "Note Display", controls: &[Control::Radios("Middle C", &["C3", "C4", "MIDI Note 60"])] },
    Section {
        title: "Delay Compensation for External Devices",
        controls: &[Control::Check("MIDI Beat Clock"), Control::Check("MIDI Timecode"), Control::Check("MIDI Notes and Controllers")],
    },
    Section { title: "MIDI / Score Editor Display", controls: &[Control::Value("Additional Empty Bars in the Score Editor", "8")] },
];

const COLLABORATION: &[Section] = &[
    Section {
        title: "Track Collaboration",
        controls: &[
            Control::Check("New Tracks Are Shared"),
            Control::Check("First Invite Shares All Tracks"),
            Control::Check("Joining a Project Downloads All Shared Tracks"),
            Control::Check("Enable Track Notes Dialog"),
            Control::Check("Automatically Download New Shared Tracks"),
        ],
    },
    Section {
        title: "Cloud Storage Services",
        controls: &[
            Control::Check("Prevent Others from Overwriting an Open Session"),
            Control::Choice("Lock File Extension", ".lock"),
            Control::Check("Keep Unused Folders on Session Close"),
        ],
    },
];

const SYNCHRONIZATION: &[Section] = &[
    Section {
        title: "Machine Control",
        controls: &[
            Control::Check("Machine Chases Memory Location"),
            Control::Check("Machine Follows Edit Insertion and Scrub"),
            Control::Choice("Controlled Device Type", "Non-Linear Device"),
            Control::Check("Machine Cues Intelligently"),
            Control::Check("Stop at Shuttle Speed Zero"),
            Control::Check("Non-Linear Transport Error Suppression"),
        ],
    },
    Section {
        title: "Remote Mode",
        controls: &[
            Control::Check("Ignore Track Arming"),
            Control::Check("Set Servo Lock at Play"),
            Control::Check("Allow External Track-Arm Commands in Local Mode"),
            Control::Check("Allow External Transport Commands in Local Mode"),
            Control::Value("Punch In Frame Offset", "0 frames"),
            Control::Value("Punch Out Frame Offset", "0 frames"),
            Control::Value("Delay after Play Command", "0 frames"),
        ],
    },
    Section {
        title: "Synchronization",
        controls: &[
            Control::Value("Minimum Sync Delay", "30 frames"),
            Control::Value("Delay before Locking to Longitudinal Timecode", "0 frames"),
            Control::Value("Delay before Locking to Serial Timecode", "0 frames"),
        ],
    },
    Section {
        title: "Linked Systems",
        controls: &[
            Control::Check("Transmit Solos"),
            Control::Check("Receive Solos"),
            Control::Check("Solo Independent of Linked State"),
            Control::Check("Audio Engine Errors Stop All Linked Systems"),
            Control::Check("Transmit Play Selections"),
            Control::Check("Receive Play Selections"),
        ],
    },
];

enum Action {
    Apply,
    Cancel,
}

/// Draws the modal Preferences window and applies or discards its staged values.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_preferences;
    if !open {
        app.preferences.discard_draft();
        return;
    }

    let mut draft = app.preferences.draft.unwrap_or_else(|| PreferencesDraft::from_live(app));
    draft.sync_unedited_from_live(app);
    let draft_before_ui = draft;
    let mut search = std::mem::take(&mut app.preferences.search);
    let mut page = app.ui.preferences_page;
    let mut action = None;
    select_page_from_keyboard(ctx, &mut page);
    // A single-line text edit releases focus on Enter in the frame that receives the key. Capture
    // ownership before drawing so Enter in Search cannot also apply and close the window.
    let typing = ctx.egui_wants_keyboard_input();
    // egui closes a ComboBox before this window observes Escape, so preserve the previous frame's
    // popup state to keep the same key from also canceling the complete Preferences window.
    let popup_was_open = app.preferences.popup_open;

    // A transparent middle-layer area makes this a modal window without borrowing the app while
    // the window body is drawn. The Preferences window itself is on the foreground layer.
    let screen = ctx.content_rect();
    let window_height = preferences_window_height(screen.height());
    let body_height = (PREFERENCES_BODY_HEIGHT - (PREFERENCES_WINDOW_HEIGHT - window_height)).max(160.0);
    egui::Area::new(egui::Id::new("preferences_modal_blocker")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
    });

    let mut window = egui::Window::new("SoundCraft Preferences")
        .id(egui::Id::new("preferences_window"))
        .open(&mut open)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .fixed_size(vec2(PREFERENCES_WINDOW_WIDTH, window_height));
    if let Some([x, y]) = app.ui.preferences_window_pos {
        window = window.default_pos(egui::pos2(x, y));
    }

    let shown = window.show(ctx, |ui| {
        ui.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
            crate::icons::draw(ui.painter(), icon_rect.shrink(1.0), "zoom", Tokens::current().text_dim);
            let edit = egui::TextEdit::singleline(&mut search)
                .id(egui::Id::new("preferences_search"))
                .hint_text("Type to search preferences")
                .desired_width(f32::INFINITY);
            ui.add(edit);
            if !search.is_empty() && ui.small_button("Clear").clicked() {
                search.clear();
            }
        });
        ui.add_space(4.0);
        tabs(ui, &mut page, &search);
        ui.separator();

        // Reserve the same body height on every page. Only screens shorter than the normal window
        // get an emergency scroll area, keeping the footer reachable without changing tall-screen
        // layout or making the window resizable.
        let body_top = ui.cursor().top();
        if window_height < PREFERENCES_WINDOW_HEIGHT {
            egui::ScrollArea::vertical()
                .id_salt("preferences_body")
                .max_height(body_height)
                .min_scrolled_height(body_height)
                .auto_shrink([false, false])
                .show(ui, |ui| page_body(ui, page, &search, &mut draft));
        } else {
            page_body(ui, page, &search, &mut draft);
        }
        let used_height = ui.cursor().top() - body_top;
        ui.add_space((body_height - used_height).max(0.0));

        ui.separator();
        ui.horizontal(|ui| {
            ui.add_space(ui.available_width() - 176.0);
            if ui.add_sized([80.0, 24.0], egui::Button::new("Cancel")).clicked() {
                action = Some(Action::Cancel);
            }
            if ui.add_sized([80.0, 24.0], egui::Button::new("OK").fill(Tokens::current().accent_dark)).clicked() {
                action = Some(Action::Apply);
            }
        });
    });
    if let Some(shown) = shown {
        let rect = shown.response.rect;
        if rect.min.x.is_finite() && rect.min.y.is_finite() {
            app.ui.preferences_window_pos = Some([rect.min.x, rect.min.y]);
        }
    }
    draft.remember_user_edits(draft_before_ui);
    app.preferences.popup_open = ctx.any_popup_open();

    if ctx.input(|i| i.key_pressed(Key::Escape)) && !popup_was_open {
        action = Some(Action::Cancel);
    } else if action.is_none() && ctx.input(|i| i.key_pressed(Key::Enter)) && !typing {
        action = Some(Action::Apply);
    }

    if !open && action.is_none() {
        action = Some(Action::Cancel);
    }
    match action {
        Some(Action::Apply) => {
            let _ = app.run("ui.theme", json!({"mode": draft.theme.id()}));
            let _ = app.run("ui.reduce_visual_effects", json!({"mode": draft.reduce_visual_effects.id()}));
            let _ = app.run("ui.undo_levels", json!({"levels": draft.undo_levels}));
            open = false;
            app.preferences.discard_draft();
        }
        Some(Action::Cancel) => {
            open = false;
            app.preferences.discard_draft();
        }
        None => {
            app.preferences.draft = Some(draft);
            app.preferences.search = search;
        }
    }
    app.ui.preferences_page = page;
    app.ui.show_preferences = open;
}

/// Caps the normal fixed window height so its frame and footer remain inside short screens.
fn preferences_window_height(screen_height: f32) -> f32 {
    (screen_height - PREFERENCES_SCREEN_MARGIN).clamp(PREFERENCES_MIN_HEIGHT, PREFERENCES_WINDOW_HEIGHT)
}

/// Draws the fixed-order page tabs and highlights pages containing a search match.
fn tabs(ui: &mut egui::Ui, page: &mut PreferencesPage, search: &str) {
    let t = Tokens::current();
    egui::ScrollArea::horizontal().id_salt("preferences_tabs").show(ui, |ui| {
        ui.horizontal(|ui| {
            for candidate in PreferencesPage::ALL {
                let hit = page_matches(candidate, search);
                let text =
                    if !search.is_empty() && hit { RichText::new(candidate.label()).color(t.meter_yellow) } else { RichText::new(candidate.label()) };
                if ui.selectable_label(*page == candidate, text).clicked() {
                    *page = candidate;
                }
            }
        });
    });
}

/// Draws one page using its standard two-column section layout.
fn page_body(ui: &mut egui::Ui, page: PreferencesPage, search: &str, draft: &mut PreferencesDraft) {
    if page == PreferencesPage::Mixing {
        mixing_page_body(ui, search, draft);
        return;
    }

    let sections = page.sections();
    ui.columns(2, |columns| {
        for (index, section) in sections.iter().enumerate() {
            let column = index % 2;
            section_ui(&mut columns[column], page, *section, search, draft);
            columns[column].add_space(6.0);
        }
    });
}

/// Draws the Mixing page whose larger sections use explicit column splits.
fn mixing_page_body(ui: &mut egui::Ui, search: &str, draft: &mut PreferencesDraft) {
    ui.columns(2, |columns| {
        section_ui(&mut columns[0], PreferencesPage::Mixing, MIXING[0], search, draft);
        section_ui(&mut columns[1], PreferencesPage::Mixing, MIXING[1], search, draft);
    });
    ui.add_space(6.0);
    section_ui_columns(ui, PreferencesPage::Mixing, MIXING[2], 11, search, draft);
    ui.add_space(6.0);
    section_ui_columns(ui, PreferencesPage::Mixing, MIXING[3], 1, search, draft);
    ui.add_space(6.0);
    section_ui_columns(ui, PreferencesPage::Mixing, MIXING[4], 5, search, draft);
}

/// Draws every control in one section inside its shared frame.
fn section_ui(ui: &mut egui::Ui, page: PreferencesPage, section: Section, search: &str, draft: &mut PreferencesDraft) {
    section_frame(ui, section, search, |ui| {
        for (index, control) in section.controls.iter().copied().enumerate() {
            control_ui(ui, page, section.title, index, control, search, draft);
        }
    });
}

/// Draws a section's title and border, including search-match highlighting.
fn section_frame(ui: &mut egui::Ui, section: Section, search: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::current();
    let section_hit = text_matches(section.title, search);
    let stroke = if !search.is_empty() && section_hit { Stroke::new(1.5, t.meter_yellow) } else { Stroke::new(1.0, t.border) };
    egui::Frame::new().fill(t.panel_bg2).stroke(stroke).corner_radius(3.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        ui.set_min_width((ui.available_width() - 12.0).max(280.0));
        egui::Frame::new().fill(t.toolbar_group).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
            let title = if !search.is_empty() && section_hit {
                RichText::new(section.title).strong().color(t.meter_yellow)
            } else {
                RichText::new(section.title).strong()
            };
            ui.label(title);
        });
        ui.add_space(3.0);
        add_contents(ui);
    });
}

/// Draws one long section in two columns at a stable catalog index.
fn section_ui_columns(ui: &mut egui::Ui, page: PreferencesPage, section: Section, split_at: usize, search: &str, draft: &mut PreferencesDraft) {
    section_frame(ui, section, search, |ui| {
        let split_at = split_at.min(section.controls.len());
        ui.columns(2, |columns| {
            for (index, control) in section.controls[..split_at].iter().copied().enumerate() {
                control_ui(&mut columns[0], page, section.title, index, control, search, draft);
            }
            for (offset, control) in section.controls[split_at..].iter().copied().enumerate() {
                control_ui(&mut columns[1], page, section.title, split_at + offset, control, search, draft);
            }
        });
    });
}

/// Draws an implemented control or a disabled placeholder for unavailable behavior.
fn control_ui(ui: &mut egui::Ui, page: PreferencesPage, section: &str, index: usize, control: Control, search: &str, draft: &mut PreferencesDraft) {
    let t = Tokens::current();
    let hit = text_matches(&control.search_text(), search);
    let label = |text: &'static str| {
        if !search.is_empty() && hit { RichText::new(text).color(t.meter_yellow) } else { RichText::new(text) }
    };
    let id = ("preference", page, section, index);
    match control {
        Control::Theme => {
            ui.horizontal(|ui| {
                ui.label(label("UI Theme"));
                egui::ComboBox::from_id_salt(id).selected_text(draft.theme.label()).show_ui(ui, |ui| {
                    for mode in ThemeMode::ALL {
                        ui.selectable_value(&mut draft.theme, mode, mode.label());
                    }
                });
            });
        }
        Control::ReduceVisualEffects => {
            ui.horizontal(|ui| {
                ui.label(label("Reduce Visual Effects"));
                egui::ComboBox::from_id_salt(id).selected_text(draft.reduce_visual_effects.label()).show_ui(ui, |ui| {
                    for mode in ReduceVisualEffects::ALL {
                        ui.selectable_value(&mut draft.reduce_visual_effects, mode, mode.label());
                    }
                });
            });
        }
        Control::UndoLevels => {
            ui.horizontal(|ui| {
                ui.label(label("Levels of Undo"));
                ui.add(egui::DragValue::new(&mut draft.undo_levels).range(1..=64));
            });
        }
        Control::Check(text) => {
            let mut value = false;
            ui.add_enabled(false, egui::Checkbox::new(&mut value, label(text))).on_disabled_hover_text("This preference is not available yet.");
        }
        Control::Choice(text, value) | Control::ChoiceOptions(text, value, _) => {
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label(label(text));
                    egui::ComboBox::from_id_salt(id).selected_text(value).show_ui(ui, |_| {});
                });
            })
            .response
            .on_disabled_hover_text("This preference is not available yet.");
        }
        Control::Value(text, value) => {
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label(label(text));
                    let mut shown = value.to_string();
                    ui.add(egui::TextEdit::singleline(&mut shown).desired_width(96.0));
                });
            })
            .response
            .on_disabled_hover_text("This preference is not available yet.");
        }
        Control::Button(text) => {
            ui.add_enabled(false, egui::Button::new(label(text))).on_disabled_hover_text("This preference is not available yet.");
        }
        Control::Radios(text, values) => {
            ui.add_enabled_ui(false, |ui| {
                ui.label(label(text));
                let mut selected = 0usize;
                ui.indent(id, |ui| {
                    for (value_index, value) in values.iter().enumerate() {
                        ui.radio_value(&mut selected, value_index, *value);
                    }
                });
            })
            .response
            .on_disabled_hover_text("This preference is not available yet.");
        }
    }
}

/// Selects a page from the platform's primary-modifier plus number shortcuts.
fn select_page_from_keyboard(ctx: &egui::Context, page: &mut PreferencesPage) {
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
    let selected = ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if page_modifiers(*modifiers, mac) => {
                keys.iter().position(|candidate| candidate == key)
            }
            _ => None,
        })
    });
    if let Some(index) = selected.and_then(|index| PreferencesPage::ALL.get(index).copied()) {
        *page = index;
    }
}

/// Reports whether modifiers represent the platform-specific page shortcut chord.
fn page_modifiers(modifiers: Modifiers, mac: bool) -> bool {
    !modifiers.alt && !modifiers.shift && if mac { modifiers.command && !modifiers.ctrl } else { modifiers.command && modifiers.ctrl }
}

/// Reports whether a page title, section or control matches the search terms.
fn page_matches(page: PreferencesPage, search: &str) -> bool {
    text_matches(page.label(), search)
        || page.sections().iter().any(|section| {
            text_matches(section.title, search) || section.controls.iter().copied().any(|control| text_matches(&control.search_text(), search))
        })
}

/// Performs a case-insensitive AND match over whitespace-separated search terms.
fn text_matches(text: &str, search: &str) -> bool {
    let search = search.trim();
    if search.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    search.split_whitespace().all(|term| text.contains(&term.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use soundcraft_engine::Engine;

    /// Creates a complete application harness with Preferences already open.
    fn preferences_harness(size: egui::Vec2) -> Harness<'static, SoundApp> {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        app.ui.show_preferences = true;
        Harness::builder().with_size(size).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        )
    }

    /// Search includes page, section, control, choice and value text.
    #[test]
    fn search_matches_pages_sections_labels_and_values() {
        assert!(page_matches(PreferencesPage::Display, "theme"));
        assert!(page_matches(PreferencesPage::Editing, "zoom selection"));
        assert!(page_matches(PreferencesPage::Metering, "K-20"));
        assert!(page_matches(PreferencesPage::Synchronization, "linked systems"));
        assert!(!page_matches(PreferencesPage::Midi, "waveform cache"));
    }

    /// Implemented preferences use commands without becoming document edits.
    #[test]
    fn applying_live_preferences_uses_ui_commands_without_dirtying_the_session() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        assert_eq!(app.ui.theme, ThemeMode::Dark);
        assert!(app.run("ui.theme", json!({"mode": ThemeMode::Light.id()})).is_ok());
        assert!(app.run("ui.reduce_visual_effects", json!({"mode": "playback"})).is_ok());
        assert!(app.run("ui.undo_levels", json!({"levels": 32})).is_ok());
        assert_eq!(app.ui.theme, ThemeMode::Light);
        assert_eq!(app.ui.reduce_visual_effects, ReduceVisualEffects::Playback);
        assert_eq!((app.ui.undo_levels, app.engine.history_limit), (32, 32));
        assert!(!app.engine.is_dirty());
    }

    /// An unchanged open draft follows newer scripted values instead of restoring stale ones.
    #[test]
    fn unchanged_draft_tracks_live_commands_before_apply() {
        let mut harness = preferences_harness(egui::vec2(1280.0, 1000.0));
        harness.run_steps(4);
        {
            let app = harness.state_mut();
            assert!(app.run("ui.theme", json!({"mode": ThemeMode::Light.id()})).is_ok());
            assert!(app.run("ui.reduce_visual_effects", json!({"mode": "always"})).is_ok());
            assert!(app.run("ui.undo_levels", json!({"levels": 17})).is_ok());
        }
        harness.run_steps(2);
        harness.get_by_label("OK").click();
        harness.run_steps(2);

        let app = harness.state();
        assert_eq!(app.ui.theme, ThemeMode::Light);
        assert_eq!(app.ui.reduce_visual_effects, ReduceVisualEffects::Always);
        assert_eq!((app.ui.undo_levels, app.engine.history_limit), (17, 17));
        assert!(!app.ui.show_preferences);
    }

    /// Fields edited in the window win over later live changes while untouched fields follow them.
    #[test]
    fn draft_sync_preserves_only_user_edited_fields() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        let mut draft = PreferencesDraft::from_live(&app);
        let before = draft;
        draft.theme = ThemeMode::Light;
        draft.remember_user_edits(before);

        app.ui.theme = ThemeMode::System;
        app.ui.reduce_visual_effects = ReduceVisualEffects::Always;
        app.ui.undo_levels = 17;
        draft.sync_unedited_from_live(&app);

        assert_eq!(draft.theme, ThemeMode::Light);
        assert_eq!(draft.reduce_visual_effects, ReduceVisualEffects::Always);
        assert_eq!(draft.undo_levels, 17);
    }

    /// Command inputs reject unknown modes and bound undo depth safely.
    #[test]
    fn live_preferences_reject_or_clamp_hostile_values() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        assert!(app.run("ui.reduce_visual_effects", json!({"mode": "sometimes"})).is_err());
        assert_eq!(app.run("ui.undo_levels", json!({"levels": 10_000})).unwrap()["levels"], 64);
    }

    /// The Setup menu maps to the same scriptable command as every other entry point.
    #[test]
    fn setup_menu_opens_preferences_through_the_catalog_command() {
        let aliases = crate::menus::ui_aliases();
        assert_eq!(crate::menus::command_for_path("Setup > Preferences...", &aliases).as_deref(), Some("setup.preferences"));
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        assert_eq!(app.run("setup.preferences", json!({"value": true})), Ok(json!({"value": true})));
        assert!(app.ui.show_preferences);
    }

    /// Closing by command cannot leak staged values into a later window instance.
    #[test]
    fn command_closes_discard_staged_preferences_before_a_reopen() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        app.ui.show_preferences = true;
        app.preferences.search = "theme".into();
        let mut draft = PreferencesDraft::from_live(&app);
        draft.theme = ThemeMode::Light;
        draft.reduce_visual_effects = ReduceVisualEffects::Always;
        draft.undo_levels = 1;
        app.preferences.draft = Some(draft);

        assert_eq!(app.run("setup.preferences", json!({})), Ok(json!({"value": false})));
        assert!(app.preferences.draft.is_none());
        assert!(app.preferences.search.is_empty());
        assert_eq!(app.run("setup.preferences", json!({})), Ok(json!({"value": true})));

        app.preferences.search = "undo".into();
        app.preferences.draft = Some(draft);
        assert!(app.run("window.close", json!({})).is_ok());
        assert!(app.preferences.draft.is_none());
        assert!(app.preferences.search.is_empty());
    }

    /// Enter belongs to the focused search field and must not apply its window.
    #[test]
    fn enter_in_search_does_not_apply_or_close_preferences() {
        let mut harness = preferences_harness(egui::vec2(1280.0, 1000.0));
        harness.run_steps(4);
        harness.ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("preferences_search")));
        harness.run_steps(1);
        assert!(harness.ctx.memory(|memory| memory.has_focus(egui::Id::new("preferences_search"))));
        let state = harness.state_mut();
        state.preferences.draft.as_mut().unwrap().theme = ThemeMode::Light;

        harness.key_press(Key::Enter);
        harness.run_steps(2);

        assert!(harness.state().ui.show_preferences);
        assert_eq!(harness.state().ui.theme, ThemeMode::Dark);
    }

    /// Enter cannot replace a Cancel button action received in the same frame.
    #[test]
    fn enter_and_cancel_in_the_same_frame_discard_the_draft() {
        let mut harness = preferences_harness(egui::vec2(1280.0, 1000.0));
        harness.run_steps(4);
        harness.state_mut().preferences.draft.as_mut().unwrap().theme = ThemeMode::Light;

        harness.get_by_label("Cancel").click();
        harness.key_press(Key::Enter);
        harness.run_steps(2);

        assert!(!harness.state().ui.show_preferences);
        assert_eq!(harness.state().ui.theme, ThemeMode::Dark);
    }

    /// Escape closes an open choice popup before it can cancel the complete window.
    #[test]
    fn escape_closes_a_choice_popup_before_preferences() {
        let mut harness = preferences_harness(egui::vec2(1280.0, 1000.0));
        harness.run_steps(4);
        harness.get_by_value("Dark").click();
        harness.run_steps(2);
        assert!(harness.ctx.any_popup_open());

        harness.key_press(Key::Escape);
        harness.run_steps(2);

        assert!(!harness.ctx.any_popup_open());
        assert!(harness.state().ui.show_preferences);
    }

    /// The adaptive fixed height keeps both footer buttons inside a short viewport.
    #[test]
    fn short_screens_keep_the_preferences_footer_reachable() {
        assert_eq!(preferences_window_height(1000.0), PREFERENCES_WINDOW_HEIGHT);
        assert_eq!(preferences_window_height(800.0), 776.0);

        let mut harness = preferences_harness(egui::vec2(1280.0, 800.0));
        harness.run_steps(4);
        let ok = harness.get_by_label("OK").rect();
        let cancel = harness.get_by_label("Cancel").rect();
        assert!(ok.max.y <= 800.0, "OK button is outside the screen: {ok:?}");
        assert!(cancel.max.y <= 800.0, "Cancel button is outside the screen: {cancel:?}");
    }

    /// The application menu invokes the same Preferences command as Setup.
    #[test]
    fn soundcraft_menu_opens_preferences_through_the_same_command() {
        let app = SoundApp::new(Engine::default(), None, crate::Services::default());
        let mut harness = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        harness.run_steps(4);
        harness.get_by_label("SoundCraft").click();
        harness.run_steps(2);
        harness.get_by_label("Preferences...").click();
        harness.run_steps(2);
        assert!(harness.state().ui.show_preferences);
    }

    /// The in-window application menu visibly disables Preferences during playback.
    #[test]
    fn soundcraft_menu_disables_preferences_during_playback() {
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        app.engine.transport.playing = true;
        let mut harness = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        harness.run_steps(4);
        harness.get_by_label("SoundCraft").click();
        harness.run_steps(2);
        assert!(harness.get_by_label("Preferences...").accesskit_node().is_disabled());
    }

    /// Page declaration order remains identical to number-shortcut order.
    #[test]
    fn page_order_matches_the_platform_shortcuts() {
        assert_eq!(PreferencesPage::ALL[0], PreferencesPage::Display);
        assert_eq!(PreferencesPage::ALL[4], PreferencesPage::Metering);
        assert_eq!(PreferencesPage::ALL[8], PreferencesPage::Synchronization);
    }

    /// An empty query does not mark every page and section as a hit.
    #[test]
    fn empty_search_does_not_highlight_everything() {
        assert!(!text_matches("Display", ""));
        assert!(!text_matches("Display", "   "));
    }

    /// Page shortcuts accept only the documented primary modifier for each platform.
    #[test]
    fn page_shortcuts_require_the_platform_primary_modifier_only() {
        let command = Modifiers { command: true, ..Modifiers::NONE };
        let off_mac_control = Modifiers { command: true, ctrl: true, ..Modifiers::NONE };
        assert!(page_modifiers(command, true));
        assert!(!page_modifiers(command, false));
        assert!(page_modifiers(off_mac_control, false));
        assert!(!page_modifiers(off_mac_control, true));
    }
}
