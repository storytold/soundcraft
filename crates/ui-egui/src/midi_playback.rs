//! MIDI viewport follow and its stopped cue. All positions come from the shared transport.

use crate::{SoundApp, midi_editor};
use serde_json::{Value, json};
use soundcraft_model::ClipId;
use soundcraft_time::{MAX_POSITION, Samples};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiScrollingMode {
    #[serde(rename = "none")]
    NoScrolling,
    AfterPlayback,
    #[default]
    Page,
    Continuous,
    Center,
    FollowEdit,
}

impl MidiScrollingMode {
    pub const ALL: [Self; 6] = [Self::NoScrolling, Self::AfterPlayback, Self::Page, Self::Continuous, Self::Center, Self::FollowEdit];

    pub fn id(self) -> &'static str {
        match self {
            Self::NoScrolling => "none",
            Self::AfterPlayback => "after_playback",
            Self::Page => "page",
            Self::Continuous => "continuous",
            Self::Center => "center",
            Self::FollowEdit => "follow_edit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NoScrolling => "No Scrolling",
            Self::AfterPlayback => "After Playback",
            Self::Page => "Page",
            Self::Continuous => "Continuous",
            Self::Center => "Center Playhead",
            Self::FollowEdit => "Follow Edit Window",
        }
    }

    pub fn centered(self) -> bool {
        matches!(self, Self::Continuous | Self::Center)
    }
}

#[derive(Debug, Default, Clone)]
pub struct FollowState {
    pub hold: bool,
    pub(crate) held_offscreen: bool,
    pub owns_transport: bool,
    pub(crate) effective: Option<MidiScrollingMode>,
    pub(crate) cue_ready: bool,
    pub(crate) pending_stop: Option<(ClipId, Samples)>,
}

pub fn effective(app: &SoundApp) -> MidiScrollingMode {
    let choice = app.ui.midi_scrolling;
    if choice != MidiScrollingMode::FollowEdit {
        return choice;
    }
    match app.engine.session().edit.scrolling.as_str() {
        "after_playback" => MidiScrollingMode::AfterPlayback,
        "page" => MidiScrollingMode::Page,
        "continuous" => MidiScrollingMode::Continuous,
        "center" => MidiScrollingMode::Center,
        _ => MidiScrollingMode::NoScrolling,
    }
}

fn timeline_start(app: &SoundApp) -> Samples {
    let e = &app.engine.session().edit;
    if e.link_timeline_edit { e.selection.start } else { e.timeline_selection.start }
}

pub(crate) fn initialize_cue(app: &mut SoundApp) {
    let at = if app.is_playing() { app.position() } else { timeline_start(app) };
    let _ = app.engine.execute("transport.set_play_start", &json!({"at": at}));
    app.midi.follow.cue_ready = true;
}

pub fn scrolling_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    if let Some(value) = p.get("mode") {
        let mode = value.as_str().and_then(|id| MidiScrollingMode::ALL.into_iter().find(|m| m.id() == id)).ok_or("Unknown MIDI scrolling mode")?;
        app.ui.midi_scrolling = mode;
        app.midi.follow.hold = false;
        app.midi.follow.effective = Some(effective(app));
        app.midi.follow.pending_stop = None;
        app.midi.follow.owns_transport = effective(app) == MidiScrollingMode::Center;
        if app.midi.follow.owns_transport {
            initialize_cue(app);
        }
    }
    Ok(json!({"mode": app.ui.midi_scrolling.id(), "effective": effective(app).id()}))
}

pub(crate) fn play_start(app: &SoundApp) -> Option<Samples> {
    (app.ui.show_midi_editor
        && app.midi.follow.owns_transport
        && app.midi.follow.cue_ready
        && effective(app) == MidiScrollingMode::Center
        && app.midi.clip.is_some()
        && midi_editor::target_clip(app) == app.midi.clip)
        .then(|| app.engine.session().edit.playhead)
}

pub(crate) fn display_position(app: &SoundApp) -> Samples {
    if app.is_playing() { app.position() } else { play_start(app).unwrap_or_else(|| timeline_start(app)) }
}

/// Absolute session bounds, expressed relative to the edited clip. The half-screen blank space
/// lets a centered cursor stay centered even at sample zero; follow can pass the clip's end.
pub(crate) fn bounds(app: &SoundApp, base: f64, span: f64) -> (f64, f64) {
    let s = app.engine.session();
    let end = s.tempo.samples_to_ticks_f(MAX_POSITION, s.sample_rate);
    (-base - span * 0.5, (end - base - span * 0.5).max(-base - span * 0.5))
}

pub(crate) fn manual_view_changed(app: &mut SoundApp, horizontal: bool) {
    if !horizontal {
        return;
    }
    app.midi.follow.owns_transport = true;
    if app.is_playing() {
        if !app.midi.follow.hold {
            app.midi.follow.held_offscreen = false;
        }
        app.midi.follow.hold = true;
    } else if effective(app) == MidiScrollingMode::Center {
        let Some(cid) = app.midi.clip else { return };
        let s = app.engine.session();
        let Some((_, clip)) = s.find_clip(cid) else { return };
        let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate) as f64;
        let tick = base + app.midi.scroll_ticks + app.midi.len_ticks / app.midi.zoom * 0.5;
        let at = s.tempo.fractional_tick_to_samples(tick, s.sample_rate).clamp(0, MAX_POSITION);
        let _ = app.engine.execute("transport.set_play_start", &json!({"at": at}));
        app.midi.follow.cue_ready = true;
    }
}

/// Update before drawing notes so the cursor, grid, and notes share this frame's transform.
pub(crate) fn update_view(app: &mut SoundApp, base: f64) {
    let mode = effective(app);
    if app.midi.follow.effective != Some(mode) {
        app.midi.follow.effective = Some(mode);
        app.midi.follow.hold = false;
        if mode == MidiScrollingMode::Center && !app.midi.follow.cue_ready {
            initialize_cue(app);
        }
    }
    let span = app.midi.len_ticks / app.midi.zoom;
    if !span.is_finite() || span <= 0.0 {
        return;
    }
    let s = app.engine.session();
    let at = s.tempo.samples_to_ticks_f(display_position(app), s.sample_rate) - base;
    let scroll = app.midi.scroll_ticks;
    let visible = at >= scroll && at < scroll + span;
    let (min, max) = bounds(app, base, span);
    let playing = app.is_playing();
    let pending = app.midi.follow.pending_stop.take();
    let stopped_tick =
        pending.filter(|(id, _)| Some(*id) == app.midi.clip).map(|(_, sample)| s.tempo.samples_to_ticks_f(sample, s.sample_rate) - base);
    if !visible && app.midi.follow.hold {
        app.midi.follow.held_offscreen = true;
    }
    if visible && app.midi.follow.hold && app.midi.follow.held_offscreen {
        app.midi.follow.hold = false;
    }
    let next = if mode == MidiScrollingMode::AfterPlayback {
        stopped_tick.filter(|tick| *tick < scroll || *tick >= scroll + span)
    } else if !app.midi.follow.hold && (playing || mode == MidiScrollingMode::Center && app.midi.follow.owns_transport) {
        match mode {
            MidiScrollingMode::Continuous | MidiScrollingMode::Center => Some(at - span * 0.5),
            MidiScrollingMode::Page if !visible => Some(if at < scroll { at } else { scroll + ((at - scroll) / span).floor() * span }),
            _ => None,
        }
    } else {
        None
    };
    app.midi.scroll_ticks = next.unwrap_or(scroll).clamp(min, max);
}

pub(crate) fn playback_started(app: &mut SoundApp) {
    app.midi.follow.hold = false;
    app.midi.follow.held_offscreen = false;
    app.midi.follow.pending_stop = None;
}

pub(crate) fn playback_stopped(app: &mut SoundApp, at: Samples) {
    app.midi.follow.hold = false;
    app.midi.follow.pending_stop = app.midi.clip.map(|id| (id, at));
    if play_start(app).is_some() {
        let _ = app.engine.execute("transport.set_play_start", &json!({"at": at}));
    }
}

pub(crate) fn release_transport(app: &mut SoundApp) {
    app.midi.follow.owns_transport = false;
    app.midi.follow.cue_ready = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, midi_editor::MidiEditorState};

    fn app() -> SoundApp {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
        let start = app.engine.session().edit.selection.start;
        app.run("edit.select", json!({"start": start, "end": start + 192_000})).unwrap();
        app.ui.show_midi_editor = true;
        let clip = midi_editor::target_clip(&app).unwrap();
        let s = app.engine.session();
        let c = s.find_clip(clip).unwrap().1;
        let len = s.tempo.samples_to_ticks(c.end(), s.sample_rate) - s.tempo.samples_to_ticks(c.start, s.sample_rate);
        app.midi = MidiEditorState { clip: Some(clip), len_ticks: len as f64, zoom: 4.0, roll: [0.0, 22.0, 800.0, 400.0], ..Default::default() };
        app.engine.mark_clean();
        app
    }

    fn base(app: &SoundApp) -> f64 {
        let s = app.engine.session();
        s.tempo.samples_to_ticks(s.find_clip(app.midi.clip.unwrap()).unwrap().1.start, s.sample_rate) as f64
    }

    #[test]
    fn centered_cue_starts_space_and_half_speed_without_replacing_a_selection() {
        let mut app = app();
        let sel = app.engine.session().edit.selection;
        let timeline = app.engine.session().edit.timeline_selection;
        let notes = app.engine.session().find_clip(app.midi.clip.unwrap()).unwrap().1.clone();
        let undo = app.engine.undo_history().len();
        app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
        app.run("ui.midi_scroll", json!({"by_px": 200})).unwrap();
        let cue = app.engine.session().edit.playhead;
        assert_ne!(cue, sel.start);
        for id in ["transport.toggle", "transport.half_speed", "transport.play"] {
            app.run(id, json!({})).unwrap();
            app.handle_transport(0.0);
            assert_eq!(app.position(), cue, "{id}");
            app.run("transport.stop", json!({})).unwrap();
            app.handle_transport(0.0);
        }
        assert_eq!(app.engine.session().edit.selection, sel);
        assert_eq!(app.engine.session().edit.timeline_selection, timeline);
        assert_eq!(app.engine.session().find_clip(app.midi.clip.unwrap()).unwrap().1, &notes);
        assert_eq!(app.engine.undo_history().len(), undo);
        assert!(!app.engine.is_dirty());
    }

    #[test]
    fn explicit_from_and_play_selection_override_the_centered_cue() {
        let mut app = app();
        app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
        app.run("ui.midi_scroll", json!({"by_px": 200})).unwrap();
        app.run("transport.play", json!({"from": 12_345})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.position(), 12_345);
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.run("transport.play_selection", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.position(), app.engine.session().edit.selection.start);
    }

    #[test]
    fn edit_selection_close_and_session_reset_release_center_ownership() {
        let mut app = app();
        app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
        app.run("edit.select", json!({"start": 50_000})).unwrap();
        assert_eq!(play_start(&app), None);
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.position(), 50_000);
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
        assert!(play_start(&app).is_some());
        app.run("window.midi_back", json!({})).unwrap();
        assert_eq!(play_start(&app), None);
        app.run("window.midi_front", json!({})).unwrap();
        assert_eq!(play_start(&app), None);
        app.run("session.new", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.midi.clip, None);
        assert!(!app.midi.follow.owns_transport);
    }

    #[test]
    fn after_playback_uses_final_time_for_stop_and_natural_end_with_both_insertion_modes() {
        for follow_insertion in [false, true] {
            for natural in [false, true] {
                let mut app = app();
                app.ui.midi_scrolling = MidiScrollingMode::AfterPlayback;
                app.engine.session_mut().edit.insertion_follows_playback = follow_insertion;
                let sel = app.engine.session().edit.selection;
                let start = sel.start;
                let base = base(&app);
                let end = app.engine.session().tempo.tick_to_samples(
                    soundcraft_time::to_samples(base + app.midi.len_ticks / app.midi.zoom + 1920.0),
                    app.engine.session().sample_rate,
                );
                app.start_play(start, natural.then_some(end), None);
                if natural {
                    app.handle_transport((end - start) as f32 / app.engine.session().sample_rate.as_f64() as f32 + 0.01);
                } else {
                    app.engine.transport.position = end;
                    app.run("transport.stop", json!({})).unwrap();
                    app.handle_transport(0.0);
                }
                assert!(!app.is_playing());
                let final_sample = app.engine.transport.position;
                update_view(&mut app, base);
                let s = app.engine.session();
                let final_tick = s.tempo.samples_to_ticks_f(final_sample, s.sample_rate) - base;
                assert!(
                    (app.midi.scroll_ticks - final_tick).abs() < 0.01,
                    "natural={natural} insertion={follow_insertion}: {} vs {final_tick}",
                    app.midi.scroll_ticks
                );
                assert_eq!(s.edit.selection.start, if follow_insertion { final_sample } else { sel.start });
            }
        }
    }

    #[test]
    fn selection_playback_and_loop_keep_their_ranges_in_center_mode() {
        let mut app = app();
        app.run("ui.midi_scrolling", json!({"mode": "center"})).unwrap();
        app.run("ui.midi_scroll", json!({"by_px": 200})).unwrap();
        app.engine.session_mut().edit.loop_playback = true;
        let sel = app.engine.session().edit.selection;
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.position(), sel.start);
        app.handle_transport(sel.len() as f32 / app.engine.session().sample_rate.as_f64() as f32 + 0.01);
        assert!(app.is_playing());
        assert_eq!(app.position(), sel.start);
    }
}
