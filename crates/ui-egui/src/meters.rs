//! Meter ballistics, scale and preferences, in one place.
//!
//! The audio thread hands the UI the highest sample peak (and gain reduction) of every block since
//! the last poll (`soundcraft_playback::Player::take_meters`). This module turns those readings
//! into what the meters show: instant attack, a release in dB per second, a peak hold line, a clip
//! light and the highest peak as a number, each following the meter preferences in
//! [`MeterPrefs`] (`ui.meters`). Clicking a meter resets its holds and clip light
//! (`ui.meters.reset`); Alt-click resets every meter.
//!
//! Every number below is a named constant so it can be checked against Pro Tools and changed in
//! one place. Where a value comes from a published standard rather than from observing Pro Tools,
//! its comment says so.

use crate::{MeterDisplay, SoundApp};
use serde_json::{Value, json};
use soundcraft_model::TrackId;

/// Readings at or above this linear sample peak (0 dBFS) light the clip indicator.
pub const CLIP_LEVEL: f32 = 1.0;

/// Seconds a peak hold line or clip light stays lit with the "timed" setting. Pro Tools' Display
/// preferences offer a 3 second hold (assumed equal for peak hold and clip indication).
pub const HOLD_SECS: f32 = 3.0;

/// Levels below this (linear, about -100 dBFS) read as silence, so a decaying bar ends.
pub const SILENCE: f32 = 1e-5;

/// Release (fall-back) rates, dB per second. `fast` is SoundCraft's original rate, kept as the
/// default until it is checked against Pro Tools' sample-peak meter. The others are the decay
/// rates of the IEC 60268-10 peak programme meters: Type I (DIN/Nordic, 20 dB in 1.7 s) and
/// Type II (BBC, 24 dB in 2.8 s).
pub const RELEASE_FAST_DB_S: f32 = 26.0;
pub const RELEASE_MEDIUM_DB_S: f32 = 20.0 / 1.7;
pub const RELEASE_SLOW_DB_S: f32 = 24.0 / 2.8;

/// The scale: dB marks (numbered and ruled across the bar), from the top down.
pub const SCALE_MARKS: [f32; 8] = [0.0, -5.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0];
/// The bottom of the scale.
pub const SCALE_FLOOR_DB: f32 = -80.0;
/// The scale's segments: `(dB at the top of the segment, position there)`, from the bottom up,
/// linear in dB within each: -80..-40 dB fills the bottom 10 %, -40..-20 the next 20 %, -20..0
/// the top 70 %.
pub const SCALE_KNEES: [(f32, f32); 3] = [(-40.0, 0.1), (-20.0, 0.3), (0.0, 1.0)];
/// Where the bar turns from green to yellow, and from yellow to red (dBFS).
pub const YELLOW_FROM_DB: f32 = -12.0;
pub const RED_FROM_DB: f32 = -3.0;

/// How long a peak hold line or a clip light stays lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Not held: the hold line follows the bar; the clip light is lit only while the signal clips.
    None,
    /// [`HOLD_SECS`] after the last peak or clip.
    Timed,
    /// Until reset: click the meter or `ui.meters.reset` (hold line, clip light and peak
    /// readout), or Track › Clear All Clip Indicators (Alt+C; clip lights only).
    Infinite,
}

impl Hold {
    const ALL: [Hold; 3] = [Hold::None, Hold::Timed, Hold::Infinite];

    pub fn id(self) -> &'static str {
        match self {
            Hold::None => "none",
            Hold::Timed => "timed",
            Hold::Infinite => "infinite",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Hold::None => "None",
            Hold::Timed => "3 Seconds",
            Hold::Infinite => "Infinite",
        }
    }

    fn parse(s: &str) -> Option<Hold> {
        Hold::ALL.into_iter().find(|h| h.id() == s)
    }
}

/// How fast a meter falls back after a peak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Release {
    #[default]
    Fast,
    Medium,
    Slow,
}

impl Release {
    const ALL: [Release; 3] = [Release::Fast, Release::Medium, Release::Slow];

    pub fn id(self) -> &'static str {
        match self {
            Release::Fast => "fast",
            Release::Medium => "medium",
            Release::Slow => "slow",
        }
    }

    pub fn db_per_s(self) -> f32 {
        match self {
            Release::Fast => RELEASE_FAST_DB_S,
            Release::Medium => RELEASE_MEDIUM_DB_S,
            Release::Slow => RELEASE_SLOW_DB_S,
        }
    }

    pub fn label(self) -> String {
        let name = match self {
            Release::Fast => "Fast",
            Release::Medium => "Medium",
            Release::Slow => "Slow",
        };
        format!("{name} ({:.1} dB/s)", self.db_per_s())
    }

    fn parse(s: &str) -> Option<Release> {
        Release::ALL.into_iter().find(|r| r.id() == s)
    }

    /// The linear factor a level falls by over `dt` seconds.
    pub fn factor(self, dt: f32) -> f32 {
        10f32.powf(-self.db_per_s() * dt.max(0.0) / 20.0)
    }
}

/// Meter preferences (per machine, saved with the UI state; `ui.meters`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MeterPrefs {
    /// 3 seconds by default.
    pub peak_hold: Hold,
    /// Clip lights latch until cleared by default, as they always have.
    pub clip_hold: Hold,
    pub release: Release,
}

impl Default for MeterPrefs {
    fn default() -> Self {
        MeterPrefs { peak_hold: Hold::Timed, clip_hold: Hold::Infinite, release: Release::default() }
    }
}

/// Which meter a click or `ui.meters.reset` acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterTarget {
    Track(TrackId),
    /// The main output meters (Transport, toolbar).
    Main,
}

/// One poll's reading of a meter (linear sample peaks; gain reduction in dB).
#[derive(Debug, Clone, Copy, Default)]
pub struct Reading {
    /// The two sides the bars show (mono strips repeat channel 0).
    pub peak: [f32; 2],
    /// The highest peak over every channel of the strip (surround strips have more than the
    /// two bars of a stereo meter), for the clip light and the peak readout.
    pub peak_all: f32,
    pub gr: f32,
}

impl Reading {
    pub fn of(m: &soundcraft_mix::StripMeter) -> Reading {
        Reading { peak: m.peak, peak_all: m.peak_all, gr: m.gain_reduction_db }
    }
}

fn sane(v: Option<f32>) -> f32 {
    v.filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0)
}

/// Feed one poll's reading into a meter's display state, `dt` seconds after the previous frame.
pub fn feed(d: &mut MeterDisplay, r: &Reading, dt: f32, prefs: &MeterPrefs) {
    let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    let fall = prefs.release.factor(dt);
    let all = r.peak.iter().copied().map(Some).map(sane).fold(sane(Some(r.peak_all)), f32::max);
    d.max_peak = d.max_peak.max(all);
    let clipping = all >= CLIP_LEVEL;
    for c in 0..2 {
        let v = sane(r.peak.get(c).copied());
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < SILENCE { 0.0 } else { nv };
        }
        // Each side holds on its own: a silent side (a hard-panned source, a stopped transport)
        // must not keep restarting the other side's hold time.
        let (Some(h), Some(age)) = (d.hold.get_mut(c), d.hold_age.get_mut(c)) else { continue };
        if v > 0.0 && v >= *h {
            *h = v;
            *age = 0.0;
        } else {
            *age += dt;
        }
        let expired = match prefs.peak_hold {
            Hold::None => true,
            Hold::Timed => *age > HOLD_SECS,
            Hold::Infinite => false,
        };
        if expired {
            *h = d.level.get(c).copied().unwrap_or(0.0);
        }
    }
    if clipping {
        d.clip = true;
        d.clip_age = 0.0;
    } else {
        d.clip_age += dt;
        let clip_expired = match prefs.clip_hold {
            Hold::None => true,
            Hold::Timed => d.clip_age > HOLD_SECS,
            Hold::Infinite => false,
        };
        if clip_expired {
            d.clip = false;
        }
    }
    let gr = if r.gr.is_finite() { r.gr } else { 0.0 };
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// Whether a stopped meter still changes on its own: a bar falling, or a timed hold or clip light
/// waiting to expire. The UI keeps repainting until it settles.
pub fn settling(d: &MeterDisplay, prefs: &MeterPrefs) -> bool {
    let timed_hold = prefs.peak_hold == Hold::Timed && d.hold.iter().zip(&d.level).any(|(h, l)| h > l);
    d.level.iter().any(|l| *l > 0.0001) || timed_hold || (prefs.clip_hold == Hold::Timed && d.clip)
}

/// Clear a meter's peak hold, clip light and highest-peak readout (the bar keeps falling; the
/// readout starts again from the next reading).
pub fn reset(d: &mut MeterDisplay) {
    d.hold = [0.0; 2];
    d.hold_age = [0.0; 2];
    d.clip = false;
    d.clip_age = 0.0;
    d.max_peak = 0.0;
}

/// A linear peak as the readout shows it: dBFS with one decimal, or "-inf".
pub fn peak_text(v: f32) -> String {
    if !v.is_finite() || v <= SILENCE {
        return "-inf".into();
    }
    format!("{:.1}", 20.0 * v.log10())
}

/// `ui.meters`: read or set the meter preferences. `{peak_hold?, clip_hold?: none|timed|infinite,
/// release?: fast|medium|slow}`; nothing changes unless every given value is valid.
pub fn prefs_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let mut next = app.ui.meters;
    for (key, slot) in [("peak_hold", &mut next.peak_hold), ("clip_hold", &mut next.clip_hold)] {
        if let Some(v) = p.get(key) {
            *slot = v.as_str().and_then(Hold::parse).ok_or_else(|| format!("`{key}` must be none, timed or infinite"))?;
        }
    }
    if let Some(v) = p.get("release") {
        next.release = v.as_str().and_then(Release::parse).ok_or("`release` must be fast, medium or slow")?;
    }
    app.ui.meters = next;
    Ok(prefs_json(&next))
}

fn prefs_json(m: &MeterPrefs) -> Value {
    json!({
        "peak_hold": m.peak_hold.id(),
        "clip_hold": m.clip_hold.id(),
        "release": m.release.id(),
        "release_db_per_s": m.release.db_per_s(),
        "hold_seconds": HOLD_SECS,
    })
}

/// `ui.meters.reset`: clear peak holds, clip lights and peak readouts. `{track?: id | name}` or
/// `{main: true}` (the output meters); with neither, every meter. `{track: "main"}` also means the
/// output meters unless a track is called that.
pub fn reset_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let session = app.engine.session();
    let by_name = |name: &str| session.track_by_name(name).map(|t| MeterTarget::Track(t.id));
    let target = match p.get("track") {
        _ if p.get("main").and_then(Value::as_bool) == Some(true) => Some(MeterTarget::Main),
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(match by_name(name) {
            Some(t) => t,
            None if name == "main" => MeterTarget::Main,
            None => return Err(format!("no track named `{name}`")),
        }),
        Some(v) => {
            let id = TrackId(v.as_u64().ok_or("`track` must be a track id or name")?);
            if session.track(id).is_none() {
                return Err(format!("no track {}", id.0));
            }
            Some(MeterTarget::Track(id))
        }
    };
    match target {
        None => app.meters.values_mut().chain([&mut app.main_meter]).for_each(reset),
        Some(MeterTarget::Main) => reset(&mut app.main_meter),
        Some(MeterTarget::Track(id)) => reset(app.meters.entry(id).or_default()),
    }
    Ok(json!({"reset": target.map_or_else(|| "all".to_string(), |t| match t {
        MeterTarget::Main => "main".into(),
        MeterTarget::Track(id) => id.0.to_string(),
    })}))
}

/// Make a drawn meter clickable: click resets it, Alt-click resets every meter, right-click opens
/// the meter options.
pub fn interact(app: &mut SoundApp, ui: &mut egui::Ui, r: egui::Rect, target: MeterTarget) {
    let key = match target {
        MeterTarget::Track(id) => ("meter", id.0),
        MeterTarget::Main => ("meter", u64::MAX),
    };
    let resp = ui.interact(r, ui.scope_id().with(key), egui::Sense::click());
    if resp.clicked() {
        let all = ui.input(|i| i.modifiers.alt);
        let params = match target {
            _ if all => json!({}),
            MeterTarget::Main => json!({"main": true}),
            MeterTarget::Track(id) => json!({"track": id.0}),
        };
        let _ = app.run("ui.meters.reset", params);
    }
    let resp = resp.on_hover_text("Click: reset peak hold and clip · Alt-click: reset all meters · right-click: meter options");
    resp.context_menu(|ui| options_menu(app, ui));
}

/// The meter options (right-click a meter).
pub fn options_menu(app: &mut SoundApp, ui: &mut egui::Ui) {
    let cur = app.ui.meters;
    for (key, title, now) in [("peak_hold", "Peak Hold", cur.peak_hold), ("clip_hold", "Clip Indication", cur.clip_hold)] {
        ui.menu_button(title, |ui| {
            for h in Hold::ALL {
                if ui.radio(now == h, h.label()).clicked() {
                    let _ = app.run("ui.meters", json!({ key: h.id() }));
                    ui.close();
                }
            }
        });
    }
    ui.menu_button("Release", |ui| {
        for r in Release::ALL {
            if ui.radio(cur.release == r, r.label()).clicked() {
                let _ = app.run("ui.meters", json!({"release": r.id()}));
                ui.close();
            }
        }
    });
    ui.separator();
    if ui.button("Reset All Meters").clicked() {
        let _ = app.run("ui.meters.reset", json!({}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;

    const FRAME: f32 = 1.0 / 60.0;

    fn pk(v: [f32; 2]) -> Reading {
        Reading { peak: v, peak_all: v[0].max(v[1]), gr: 0.0 }
    }

    fn run_for_each(d: &mut MeterDisplay, secs: f32, peak: [f32; 2], prefs: &MeterPrefs) {
        let n = (secs / FRAME).round() as usize;
        for _ in 0..n {
            feed(d, &pk(peak), FRAME, prefs);
        }
    }

    fn run_for(d: &mut MeterDisplay, secs: f32, peak: f32, prefs: &MeterPrefs) {
        let n = (secs / FRAME).round() as usize;
        for _ in 0..n {
            feed(d, &pk([peak; 2]), FRAME, prefs);
        }
    }

    #[test]
    fn timed_hold_lasts_hold_secs_and_infinite_until_reset() {
        let timed = MeterPrefs::default();
        let mut d = MeterDisplay::default();
        feed(&mut d, &pk([0.5; 2]), FRAME, &timed);
        run_for(&mut d, HOLD_SECS - 0.1, 0.0, &timed);
        assert_eq!(d.hold[0], 0.5, "still held");
        run_for(&mut d, 0.2, 0.0, &timed);
        assert!(d.hold[0] < 0.5, "released after the hold time");
        let inf = MeterPrefs { peak_hold: Hold::Infinite, ..MeterPrefs::default() };
        let mut d = MeterDisplay::default();
        feed(&mut d, &pk([0.5; 2]), FRAME, &inf);
        run_for(&mut d, 60.0, 0.0, &inf);
        assert_eq!(d.hold[0], 0.5);
        assert_eq!(d.level[0], 0.0, "the bar fell back");
        reset(&mut d);
        assert_eq!(d.hold[0], 0.0);
        let none = MeterPrefs { peak_hold: Hold::None, ..MeterPrefs::default() };
        feed(&mut d, &pk([0.5; 2]), FRAME, &none);
        run_for(&mut d, 0.5, 0.0, &none);
        assert_eq!(d.hold, d.level);
    }

    #[test]
    fn a_silent_side_does_not_keep_the_other_sides_hold() {
        // Hard-panned mono on a stereo meter: the right side reads nothing at all.
        let prefs = MeterPrefs::default();
        let mut d = MeterDisplay::default();
        feed(&mut d, &pk([0.5, 0.0]), FRAME, &prefs);
        run_for_each(&mut d, HOLD_SECS + 0.2, [0.01, 0.0], &prefs);
        assert!(d.hold[0] < 0.5, "the left hold released: {d:?}");
    }

    #[test]
    fn clip_light_follows_clip_indication() {
        for (hold, lit_after_10s) in [(Hold::Infinite, true), (Hold::Timed, false), (Hold::None, false)] {
            let prefs = MeterPrefs { clip_hold: hold, ..MeterPrefs::default() };
            let mut d = MeterDisplay::default();
            feed(&mut d, &pk([1.2, 0.1]), FRAME, &prefs);
            assert!(d.clip);
            feed(&mut d, &pk([0.1; 2]), FRAME, &prefs);
            assert_eq!(d.clip, hold != Hold::None, "{hold:?} right after");
            run_for(&mut d, 10.0, 0.1, &prefs);
            assert_eq!(d.clip, lit_after_10s, "{hold:?}");
        }
    }

    #[test]
    fn release_rates_fall_by_their_db_per_second() {
        for r in Release::ALL {
            let prefs = MeterPrefs { release: r, ..MeterPrefs::default() };
            let mut d = MeterDisplay::default();
            feed(&mut d, &pk([1.0; 2]), FRAME, &prefs);
            run_for(&mut d, 1.0, 0.0, &prefs);
            let fell = -20.0 * d.level[0].log10();
            assert!((fell - r.db_per_s()).abs() < 0.5, "{r:?}: {fell}");
        }
    }

    #[test]
    fn hostile_readings_do_not_stick() {
        let mut d = MeterDisplay::default();
        feed(&mut d, &Reading { peak: [f32::NAN, f32::INFINITY], peak_all: f32::NAN, gr: f32::NAN }, f32::NAN, &MeterPrefs::default());
        assert!(d.level.iter().chain(&d.hold).all(|x| x.is_finite()) && d.gr.is_finite() && d.max_peak.is_finite());
    }

    #[test]
    fn a_clip_on_any_channel_lights_the_clip_and_the_readout() {
        // A 5.1 strip whose bars (L, R) are quiet while the LFE clips.
        let mut d = MeterDisplay::default();
        feed(&mut d, &Reading { peak: [0.1; 2], peak_all: 1.2, gr: 0.0 }, FRAME, &MeterPrefs::default());
        assert!(d.clip && d.max_peak == 1.2 && d.level == [0.1; 2], "{d:?}");
    }

    #[test]
    fn clear_all_clip_indicators_leaves_holds_and_readouts() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        let d = app.meters.entry(kick).or_default();
        (d.clip, d.hold, d.max_peak) = (true, [0.9; 2], 1.1);
        app.run("track.clear_clip_indicators", json!({})).unwrap();
        let d = app.meters[&kick];
        assert!(!d.clip && d.hold == [0.9; 2] && d.max_peak == 1.1, "{d:?}");
    }

    #[test]
    fn defaults_latch_clips_and_hold_peaks_for_three_seconds() {
        let prefs = MeterPrefs::default();
        assert_eq!((prefs.peak_hold, prefs.clip_hold), (Hold::Timed, Hold::Infinite));
        let ui: crate::UiState = serde_json::from_value(json!({"meters": {"release": "slow"}})).unwrap();
        assert_eq!(ui.meters.clip_hold, Hold::Infinite, "missing fields take the defaults");
    }

    #[test]
    fn a_reset_drops_the_hold_line_of_a_falling_bar() {
        let prefs = MeterPrefs { peak_hold: Hold::Infinite, ..MeterPrefs::default() };
        let mut d = MeterDisplay { level: [0.5; 2], hold: [0.9; 2], ..MeterDisplay::default() };
        reset(&mut d);
        assert_eq!(d.hold, [0.0; 2]);
        feed(&mut d, &Reading::default(), 0.01, &prefs);
        assert_eq!(d.hold, [0.0; 2], "no line stays behind at the height the bar had");
    }

    #[test]
    fn stopped_meters_settle_only_when_nothing_is_left_to_expire() {
        let timed = MeterPrefs { peak_hold: Hold::Timed, clip_hold: Hold::Timed, ..MeterPrefs::default() };
        let latched = MeterPrefs { peak_hold: Hold::Infinite, clip_hold: Hold::Infinite, ..MeterPrefs::default() };
        assert!(!settling(&MeterDisplay::default(), &timed));
        let right_only = MeterDisplay { level: [0.0, 0.5], hold: [0.0, 0.5], ..MeterDisplay::default() };
        assert!(settling(&right_only, &latched), "either side's bar still falls");
        let held = MeterDisplay { hold: [0.5, 0.0], ..MeterDisplay::default() };
        let clipped = MeterDisplay { clip: true, ..MeterDisplay::default() };
        assert!(settling(&held, &timed) && settling(&clipped, &timed), "waiting to expire");
        assert!(!settling(&held, &latched) && !settling(&clipped, &latched), "latched: nothing will change");
    }

    #[test]
    fn peak_readout_keeps_the_highest_peak_until_reset() {
        let prefs = MeterPrefs::default();
        let mut d = MeterDisplay::default();
        feed(&mut d, &pk([0.5, 0.25]), FRAME, &prefs);
        run_for(&mut d, 10.0, 0.01, &prefs);
        assert_eq!(peak_text(d.max_peak), "-6.0");
        reset(&mut d);
        assert_eq!(peak_text(d.max_peak), "-inf", "cleared");
        feed(&mut d, &pk([0.01; 2]), FRAME, &prefs);
        assert_eq!(peak_text(d.max_peak), "-40.0", "and counting again");
        assert_eq!(peak_text(0.0), "-inf");
    }

    #[test]
    fn commands_set_prefs_and_reset_meters() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let r = app.run("ui.meters", json!({"peak_hold": "infinite", "release": "slow"})).unwrap();
        assert_eq!((r["peak_hold"].as_str(), r["clip_hold"].as_str(), r["release"].as_str()), (Some("infinite"), Some("infinite"), Some("slow")));
        assert!(app.run("ui.meters", json!({"clip_hold": "forever", "peak_hold": "none"})).is_err());
        assert_eq!(app.ui.meters.peak_hold, Hold::Infinite, "a refused call changes nothing");
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        let snare = app.engine.session().track_by_name("Snare").unwrap().id;
        for id in [kick, snare] {
            let d = app.meters.entry(id).or_default();
            (d.clip, d.hold, d.max_peak) = (true, [0.9; 2], 1.1);
        }
        app.main_meter.clip = true;
        app.run("ui.meters.reset", json!({"track": kick.0})).unwrap();
        assert!(!app.meters[&kick].clip && app.meters[&kick].hold == [0.0; 2] && app.meters[&kick].max_peak == 0.0);
        assert!(app.meters[&snare].clip && app.main_meter.clip, "only that meter");
        app.run("ui.meters.reset", json!({"track": "main"})).unwrap();
        app.meters.entry(kick).or_default().clip = true;
        app.run("ui.meters.reset", json!({"track": "Kick"})).unwrap();
        assert!(!app.meters[&kick].clip, "by name");
        assert!(app.run("ui.meters.reset", json!({"track": "Nobody"})).is_err());
        app.main_meter.clip = true;
        app.run("ui.meters.reset", json!({"main": true})).unwrap();
        assert!(!app.main_meter.clip);
        // A track called "main" is reachable by name; {"main": true} still means the output.
        let main_track = app.engine.session().track_by_name("Bass").unwrap().id;
        app.engine.execute("track.rename", &json!({"track": main_track.0, "name": "main"})).unwrap();
        (app.meters.entry(main_track).or_default().clip, app.main_meter.clip) = (true, true);
        app.run("ui.meters.reset", json!({"track": "main"})).unwrap();
        assert!(!app.meters[&main_track].clip && app.main_meter.clip);
        assert!(app.meters[&snare].clip, "the others stay lit");
        assert!(app.run("ui.meters.reset", json!({"track": 999_999})).is_err());
        app.run("ui.meters.reset", json!({})).unwrap();
        assert!(!app.meters[&snare].clip);
        // Saved with the UI state, and older prefs without it get the defaults.
        let ui: crate::UiState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(ui.meters, MeterPrefs::default());
        let back: crate::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
        assert_eq!(back.meters, app.ui.meters);
    }
}
