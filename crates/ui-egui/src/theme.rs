//! Design tokens. Colours were chosen by eye to give a dark, studio-style look; they are ours.

use egui::{Color32, FontFamily, FontId};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Whether the Light palette is active. Dark is the default (the incumbent is dark-only).
static LIGHT_THEME: AtomicBool = AtomicBool::new(false);

/// The operating system's appearance, asked once off the UI thread.
static SYSTEM: AtomicU8 = AtomicU8::new(SYSTEM_UNKNOWN);
const SYSTEM_UNKNOWN: u8 = 0;
const SYSTEM_ASKING: u8 = 1;
const SYSTEM_DARK: u8 = 2;
const SYSTEM_LIGHT: u8 = 3;

/// The Appearance setting (`ui.theme {mode}`). Dark by default; System and Light are opt-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    Light,
    #[default]
    Dark,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::System];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The id used in `ui.theme {"mode": …}` and in the saved UI prefs.
    pub const fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub window_bg: Color32,
    pub panel_bg: Color32,
    pub panel_bg2: Color32,
    pub toolbar_bg: Color32,
    pub toolbar_group: Color32,
    pub border: Color32,
    pub border_light: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_dark: Color32,
    pub header_text: Color32,
    pub counter_bg: Color32,
    pub counter_text: Color32,
    pub counter_label: Color32,
    pub accent: Color32,
    pub accent_dark: Color32,
    pub mode_on: Color32,
    pub mode_on_text: Color32,
    pub mode_off_text: Color32,
    pub button: Color32,
    pub button_hi: Color32,
    pub button_border: Color32,
    pub name_field: Color32,
    pub name_field_sel: Color32,
    pub ruler_bg: Color32,
    pub ruler_label_bg: Color32,
    pub ruler_text: Color32,
    pub ruler_tick: Color32,
    pub tempo_ruler: Color32,
    pub meter_ruler: Color32,
    pub marker_ruler: Color32,
    pub playlist_bg: Color32,
    pub playlist_alt: Color32,
    pub grid_line: Color32,
    pub bar_line: Color32,
    pub selection: Color32,
    pub playhead: Color32,
    pub insertion: Color32,
    pub solo: Color32,
    pub mute: Color32,
    pub rec: Color32,
    pub input: Color32,
    pub auto_read: Color32,
    pub auto_write: Color32,
    pub auto_touch: Color32,
    pub auto_latch: Color32,
    pub meter_green: Color32,
    pub meter_yellow: Color32,
    pub meter_red: Color32,
    pub fader_track: Color32,
    pub strip_bg: Color32,
    pub strip_section: Color32,
    pub slot_bg: Color32,
    pub automation_line: Color32,
    /// Menu bar background.
    pub menu_bar: Color32,
    /// Outline of toolbar groups.
    pub group_border: Color32,
    /// Edit-mode cell, not selected.
    pub mode_off: Color32,
    /// Counter-panel toggle (Grid, Count Off), off.
    pub counter_off: Color32,
    /// Outline of the counter panels.
    pub counter_border: Color32,
    /// Divider inside the counter panels.
    pub counter_rule: Color32,
    /// Smart-tool bracket, off.
    pub tool_bracket_off: Color32,
    /// Selected row in the Tracks, Groups and Clips lists.
    pub row_selected: Color32,
    /// Track-visible dot in the Tracks list.
    pub visible_dot: Color32,
    /// Big Counter background.
    pub big_counter_bg: Color32,
    /// Edit-window track header, selected.
    pub header_selected: Color32,
    /// Mix-window strip, selected.
    pub strip_selected: Color32,
    /// Marker names on the marker ruler.
    pub marker_text: Color32,
    /// Alternate-playlist lanes.
    pub playlist_lane: Color32,
    /// Universe overview background.
    pub universe_bg: Color32,
    /// Universe visible-area outline.
    pub universe_view: Color32,
    /// Meter background.
    pub meter_bg: Color32,
    /// Meter clip indicator, not clipped.
    pub meter_clip_off: Color32,
    /// Pan knob body.
    pub knob_bg: Color32,
    /// Pan knob outline.
    pub knob_ring: Color32,
    /// Pan knob pointer.
    pub knob_pointer: Color32,
    /// Mix insert slot holding a plugin.
    pub insert_on: Color32,
    /// Mix insert slot, bypassed.
    pub insert_bypass: Color32,
    /// Mix send slot in use.
    pub send_on: Color32,
    /// Mix insert/send slot outline.
    pub slot_border: Color32,
    /// Mix-window section captions (INSERTS A-E, I/O…).
    pub section_label: Color32,
}

impl Tokens {
    pub const DARK: Tokens = Tokens {
        window_bg: Color32::from_rgb(30, 30, 30),
        panel_bg: Color32::from_rgb(37, 37, 38),
        panel_bg2: Color32::from_rgb(44, 44, 45),
        toolbar_bg: Color32::from_rgb(30, 30, 31),
        toolbar_group: Color32::from_rgb(24, 24, 25),
        border: Color32::from_rgb(14, 14, 14),
        border_light: Color32::from_rgb(64, 64, 66),
        text: Color32::from_rgb(214, 214, 214),
        text_dim: Color32::from_rgb(150, 150, 152),
        text_dark: Color32::from_rgb(16, 16, 16),
        header_text: Color32::from_rgb(196, 196, 198),
        counter_bg: Color32::from_rgb(4, 4, 4),
        counter_text: Color32::from_rgb(104, 220, 120),
        counter_label: Color32::from_rgb(200, 200, 200),
        accent: Color32::from_rgb(64, 132, 196),
        accent_dark: Color32::from_rgb(40, 92, 146),
        mode_on: Color32::from_rgb(92, 196, 108),
        mode_on_text: Color32::from_rgb(10, 30, 12),
        mode_off_text: Color32::from_rgb(98, 196, 112),
        button: Color32::from_rgb(58, 58, 60),
        button_hi: Color32::from_rgb(78, 78, 80),
        button_border: Color32::from_rgb(20, 20, 20),
        name_field: Color32::from_rgb(208, 208, 208),
        name_field_sel: Color32::from_rgb(232, 232, 232),
        ruler_bg: Color32::from_rgb(40, 40, 41),
        ruler_label_bg: Color32::from_rgb(40, 40, 41),
        ruler_text: Color32::from_rgb(178, 178, 180),
        ruler_tick: Color32::from_rgb(96, 96, 98),
        tempo_ruler: Color32::from_rgb(52, 128, 88),
        meter_ruler: Color32::from_rgb(44, 108, 140),
        marker_ruler: Color32::from_rgb(46, 46, 48),
        playlist_bg: Color32::from_rgb(36, 36, 37),
        playlist_alt: Color32::from_rgb(40, 40, 41),
        grid_line: Color32::from_rgb(52, 52, 54),
        bar_line: Color32::from_rgb(66, 66, 70),
        selection: Color32::from_rgba_premultiplied(40, 70, 110, 110),
        playhead: Color32::from_rgb(232, 60, 50),
        insertion: Color32::from_rgb(230, 230, 230),
        solo: Color32::from_rgb(222, 196, 52),
        mute: Color32::from_rgb(232, 148, 40),
        rec: Color32::from_rgb(214, 52, 46),
        input: Color32::from_rgb(70, 170, 90),
        auto_read: Color32::from_rgb(96, 200, 110),
        auto_write: Color32::from_rgb(220, 70, 60),
        auto_touch: Color32::from_rgb(230, 170, 50),
        auto_latch: Color32::from_rgb(160, 120, 220),
        meter_green: Color32::from_rgb(60, 200, 80),
        meter_yellow: Color32::from_rgb(230, 210, 60),
        meter_red: Color32::from_rgb(230, 50, 40),
        fader_track: Color32::from_rgb(12, 12, 12),
        strip_bg: Color32::from_rgb(46, 46, 48),
        strip_section: Color32::from_rgb(36, 36, 38),
        slot_bg: Color32::from_rgb(28, 28, 29),
        automation_line: Color32::from_rgb(240, 240, 240),
        menu_bar: Color32::from_rgb(22, 22, 23),
        group_border: Color32::from_rgb(8, 8, 8),
        mode_off: Color32::from_rgb(20, 36, 22),
        counter_off: Color32::from_rgb(30, 50, 32),
        counter_border: Color32::from_rgb(50, 50, 50),
        counter_rule: Color32::from_rgb(40, 40, 40),
        tool_bracket_off: Color32::from_rgb(64, 64, 66),
        row_selected: Color32::from_rgb(52, 70, 96),
        visible_dot: Color32::from_rgb(200, 200, 200),
        big_counter_bg: Color32::from_rgb(0, 0, 0),
        header_selected: Color32::from_rgb(52, 58, 66),
        strip_selected: Color32::from_rgb(54, 58, 64),
        marker_text: Color32::from_rgb(236, 236, 236),
        playlist_lane: Color32::from_rgb(30, 30, 31),
        universe_bg: Color32::from_rgb(18, 18, 19),
        universe_view: Color32::from_rgb(255, 255, 255),
        meter_bg: Color32::from_rgb(8, 8, 8),
        meter_clip_off: Color32::from_rgb(60, 20, 20),
        knob_bg: Color32::from_rgb(26, 26, 28),
        knob_ring: Color32::from_rgb(90, 90, 94),
        knob_pointer: Color32::from_rgb(230, 230, 230),
        insert_on: Color32::from_rgb(52, 62, 80),
        insert_bypass: Color32::from_rgb(70, 56, 30),
        send_on: Color32::from_rgb(46, 66, 56),
        slot_border: Color32::from_rgb(16, 16, 16),
        section_label: Color32::from_rgb(200, 200, 200),
    };

    pub const LIGHT: Tokens = Tokens {
        window_bg: Color32::from_rgb(238, 239, 242),
        panel_bg: Color32::from_rgb(248, 249, 251),
        panel_bg2: Color32::from_rgb(255, 255, 255),
        toolbar_bg: Color32::from_rgb(230, 232, 236),
        toolbar_group: Color32::from_rgb(242, 243, 246),
        border: Color32::from_rgb(190, 193, 200),
        border_light: Color32::from_rgb(210, 213, 220),
        text: Color32::from_rgb(35, 36, 40),
        text_dim: Color32::from_rgb(95, 98, 106),
        text_dark: Color32::from_rgb(20, 21, 24),
        header_text: Color32::from_rgb(45, 47, 53),
        counter_bg: Color32::from_rgb(224, 226, 231),
        counter_text: Color32::from_rgb(22, 116, 45),
        counter_label: Color32::from_rgb(65, 68, 75),
        accent: Color32::from_rgb(62, 116, 180),
        accent_dark: Color32::from_rgb(43, 88, 145),
        mode_on: Color32::from_rgb(75, 158, 88),
        mode_on_text: Color32::from_rgb(255, 255, 255),
        mode_off_text: Color32::from_rgb(48, 122, 62),
        button: Color32::from_rgb(224, 226, 230),
        button_hi: Color32::from_rgb(210, 213, 219),
        button_border: Color32::from_rgb(175, 179, 187),
        name_field: Color32::from_rgb(255, 255, 255),
        name_field_sel: Color32::from_rgb(236, 242, 252),
        ruler_bg: Color32::from_rgb(226, 228, 233),
        ruler_label_bg: Color32::from_rgb(226, 228, 233),
        ruler_text: Color32::from_rgb(70, 73, 80),
        ruler_tick: Color32::from_rgb(130, 134, 142),
        tempo_ruler: Color32::from_rgb(104, 170, 124),
        meter_ruler: Color32::from_rgb(92, 158, 184),
        marker_ruler: Color32::from_rgb(210, 213, 219),
        playlist_bg: Color32::from_rgb(246, 247, 249),
        playlist_alt: Color32::from_rgb(238, 240, 244),
        grid_line: Color32::from_rgb(210, 213, 219),
        bar_line: Color32::from_rgb(185, 189, 197),
        selection: Color32::from_rgba_premultiplied(100, 145, 205, 90),
        playhead: Color32::from_rgb(210, 48, 42),
        insertion: Color32::from_rgb(35, 36, 40),
        solo: Color32::from_rgb(178, 145, 20),
        mute: Color32::from_rgb(194, 106, 22),
        rec: Color32::from_rgb(190, 42, 38),
        input: Color32::from_rgb(45, 126, 64),
        auto_read: Color32::from_rgb(52, 142, 68),
        auto_write: Color32::from_rgb(190, 50, 45),
        auto_touch: Color32::from_rgb(180, 122, 24),
        auto_latch: Color32::from_rgb(118, 80, 170),
        meter_green: Color32::from_rgb(36, 160, 58),
        meter_yellow: Color32::from_rgb(190, 162, 20),
        meter_red: Color32::from_rgb(195, 42, 35),
        fader_track: Color32::from_rgb(198, 201, 207),
        strip_bg: Color32::from_rgb(235, 237, 241),
        strip_section: Color32::from_rgb(244, 245, 247),
        slot_bg: Color32::from_rgb(248, 249, 251),
        automation_line: Color32::from_rgb(40, 42, 48),
        menu_bar: Color32::from_rgb(222, 224, 229),
        group_border: Color32::from_rgb(190, 193, 200),
        mode_off: Color32::from_rgb(236, 240, 237),
        counter_off: Color32::from_rgb(214, 230, 217),
        counter_border: Color32::from_rgb(190, 193, 200),
        counter_rule: Color32::from_rgb(204, 207, 213),
        tool_bracket_off: Color32::from_rgb(200, 203, 210),
        row_selected: Color32::from_rgb(198, 216, 240),
        visible_dot: Color32::from_rgb(110, 113, 120),
        big_counter_bg: Color32::from_rgb(224, 226, 231),
        header_selected: Color32::from_rgb(206, 219, 238),
        strip_selected: Color32::from_rgb(206, 219, 238),
        marker_text: Color32::from_rgb(35, 36, 40),
        playlist_lane: Color32::from_rgb(232, 234, 238),
        universe_bg: Color32::from_rgb(228, 230, 234),
        universe_view: Color32::from_rgb(40, 42, 48),
        meter_bg: Color32::from_rgb(196, 199, 206),
        meter_clip_off: Color32::from_rgb(232, 200, 198),
        knob_bg: Color32::from_rgb(252, 252, 253),
        knob_ring: Color32::from_rgb(150, 154, 162),
        knob_pointer: Color32::from_rgb(40, 42, 48),
        insert_on: Color32::from_rgb(206, 218, 236),
        insert_bypass: Color32::from_rgb(240, 222, 186),
        send_on: Color32::from_rgb(204, 230, 214),
        slot_border: Color32::from_rgb(175, 179, 187),
        section_label: Color32::from_rgb(95, 98, 106),
    };

    pub fn current() -> Tokens {
        if is_light() { Self::LIGHT } else { Self::DARK }
    }
}

/// Whether the Light palette is the active one.
pub fn is_light() -> bool {
    LIGHT_THEME.load(Ordering::Relaxed)
}

/// Whether `mode` resolves to the Light palette right now. Never blocks.
pub fn wants_light(ctx: &egui::Context, mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Light => true,
        ThemeMode::Dark => false,
        ThemeMode::System => system_is_light(ctx),
    }
}

/// Forget the cached system appearance so the next System lookup asks again.
pub fn refresh_system() {
    let _ = SYSTEM.compare_exchange(SYSTEM_DARK, SYSTEM_UNKNOWN, Ordering::Relaxed, Ordering::Relaxed);
    let _ = SYSTEM.compare_exchange(SYSTEM_LIGHT, SYSTEM_UNKNOWN, Ordering::Relaxed, Ordering::Relaxed);
}

/// The OS appearance. The query (a D-Bus call on Linux) runs once on a background thread; until it
/// answers this reports dark, and `ctx` is repainted when the answer arrives.
#[cfg(not(target_arch = "wasm32"))]
fn system_is_light(ctx: &egui::Context) -> bool {
    if SYSTEM.compare_exchange(SYSTEM_UNKNOWN, SYSTEM_ASKING, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("soundcraft-theme".into()).spawn(move || {
            let light = matches!(dark_light::detect(), Ok(dark_light::Mode::Light));
            SYSTEM.store(if light { SYSTEM_LIGHT } else { SYSTEM_DARK }, Ordering::Relaxed);
            ctx.request_repaint();
        });
        if spawned.is_err() {
            SYSTEM.store(SYSTEM_DARK, Ordering::Relaxed);
        }
    }
    SYSTEM.load(Ordering::Relaxed) == SYSTEM_LIGHT
}

/// On the web the query is a synchronous `matchMedia` lookup (no threads there).
#[cfg(target_arch = "wasm32")]
fn system_is_light(_ctx: &egui::Context) -> bool {
    if SYSTEM.compare_exchange(SYSTEM_UNKNOWN, SYSTEM_ASKING, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        let light = matches!(dark_light::detect(), Ok(dark_light::Mode::Light));
        SYSTEM.store(if light { SYSTEM_LIGHT } else { SYSTEM_DARK }, Ordering::Relaxed);
    }
    SYSTEM.load(Ordering::Relaxed) == SYSTEM_LIGHT
}

/// Track colours tinted for clip bodies.
pub fn clip_colors(rgb: [u8; 3], selected: bool) -> (Color32, Color32, Color32) {
    let [r, g, b] = rgb;
    let k = if selected { 0.95 } else { 0.62 };
    let body = Color32::from_rgb((f32::from(r) * k) as u8, (f32::from(g) * k) as u8, (f32::from(b) * k) as u8);
    let bar = Color32::from_rgb((f32::from(r) * 0.9) as u8, (f32::from(g) * 0.9) as u8, (f32::from(b) * 0.9) as u8);
    if is_light() {
        // A paler body and a waveform in a deep shade of the clip's own colour.
        let tint = |c: u8, k: f32| (f32::from(c) + (255.0 - f32::from(c)) * k) as u8;
        let k = if selected { 0.2 } else { 0.45 };
        let body = Color32::from_rgb(tint(r, k), tint(g, k), tint(b, k));
        let d = if selected { 0.22 } else { 0.32 };
        let wave = Color32::from_rgb((f32::from(r) * d) as u8, (f32::from(g) * d) as u8, (f32::from(b) * d) as u8);
        return (body, bar, wave);
    }
    let wave = if selected { Color32::from_rgb(20, 20, 24) } else { Color32::from_rgb(12, 12, 14) };
    (body, bar, wave)
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Make `light` (or dark) the active palette and apply its visuals. Call only when it changes.
pub fn apply(ctx: &egui::Context, light: bool) {
    LIGHT_THEME.store(light, Ordering::Relaxed);
    let t = Tokens::current();
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg2;
    v.extreme_bg_color = t.slot_bg;
    v.faint_bg_color = t.panel_bg2;
    v.selection.bg_fill = t.accent;
    v.widgets.inactive.weak_bg_fill = t.button;
    v.widgets.inactive.bg_fill = t.button;
    v.widgets.hovered.weak_bg_fill = t.button_hi;
    v.widgets.noninteractive.fg_stroke.color = t.text;
    v.widgets.inactive.fg_stroke.color = t.text;
    v.window_corner_radius = egui::CornerRadius::same(6);
    v.menu_corner_radius = egui::CornerRadius::same(4);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.interaction.tooltip_delay = 0.4;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp, UiState};
    use serde_json::json;

    #[test]
    fn dark_is_the_default() {
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
        assert_eq!(UiState::default().theme, ThemeMode::Dark);
        // Prefs saved before the setting existed load as Dark.
        let old: UiState = serde_json::from_value(json!({"show_tracks_list": true})).unwrap_or_default();
        assert_eq!(old.theme, ThemeMode::Dark);
    }

    #[test]
    fn modes_round_trip_through_prefs() {
        for mode in ThemeMode::ALL {
            assert_eq!(ThemeMode::parse(mode.id()), Some(mode));
            let ui = UiState { theme: mode, ..UiState::default() };
            let back: UiState = serde_json::from_value(serde_json::to_value(&ui).unwrap_or_default()).unwrap_or_default();
            assert_eq!(back.theme, mode);
        }
        assert_eq!(ThemeMode::parse(" Light "), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::parse("sepia"), None);
    }

    #[test]
    fn theme_command_sets_and_reports_the_mode() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        assert_eq!(app.run("ui.theme", json!({})), Ok(json!({"mode": "dark"})));
        assert_eq!(app.run("ui.theme", json!({"mode": "light"})), Ok(json!({"mode": "light"})));
        assert_eq!(app.ui.theme, ThemeMode::Light);
        assert!(app.run("ui.theme", json!({"mode": "sepia"})).is_err());
        assert_eq!(app.ui.theme, ThemeMode::Light);
        assert_eq!(app.run("ui.theme", json!({"mode": "system"})), Ok(json!({"mode": "system"})));
        assert!(crate::menus::UI_COMMANDS.iter().any(|c| c.0 == "ui.theme"));
    }
}
