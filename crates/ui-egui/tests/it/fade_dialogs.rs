//! The Fade In, Fade Out and Crossfade dialogs: double-clicking a fade or crossfade, or Edit › Fades ›
//! Create… over a selection, opens the dialog for it; OK applies what it shows in one undo step.

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceId, TrackId};
use soundcraft_time::Range;
use soundcraft_ui_egui::dialogs::{Dialog, FadeTarget};
use soundcraft_ui_egui::fade_dialog::{FadeEditor, Kind, Link, Settings, Shape, Side, Slope};
use soundcraft_ui_egui::{Services, SoundApp, edit_window, menus};

const SEC: i64 = 48_000;
/// Every fade here is 20 000 samples long (50 px at the zoom used).
const FADE: i64 = 20_000;

struct Rig {
    h: Harness<'static, SoundApp>,
    track: TrackId,
}

impl Rig {
    /// One track with `clips` (start, length, fade-in shape, fade-out shape; a shape means a `FADE`-long fade).
    fn new(tool: &str, clips: &[(i64, i64, Option<FadeShape>, Option<FadeShape>)]) -> (Rig, Vec<ClipId>) {
        let mut e = Engine::default();
        let track = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
        let s = e.session_mut();
        s.sources.push(Source {
            id: SourceId(7),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames: u64::try_from(30 * SEC).unwrap(),
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let mut ids = Vec::new();
        for &(start, len, fade_in, fade_out) in clips {
            let id = s.new_clip_id();
            let mut c = Clip::audio(id, "c", SourceId(7), SEC + start, start, len);
            let fade = |shape: Option<FadeShape>| shape.map_or(Fade::default(), |shape| Fade { len: FADE, shape, curvature: 0.0 });
            c.fade_in = fade(fade_in);
            c.fade_out = fade(fade_out);
            s.track_mut(track).and_then(|t| t.playlist_mut()).unwrap().clips.push(c);
            ids.push(id);
        }
        e.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
        e.execute("edit.tool", &json!({"tool": tool})).unwrap();
        e.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
        let app = SoundApp::new(e, None, Services::default());
        let mut r = Rig { h: Rig::harness(app), track };
        r.frames(4);
        (r, ids)
    }

    /// 60 frames a second: egui counts two clicks within 0.3 s as a double-click.
    fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
        Rig::sized(app, vec2(1600.0, 1000.0), false)
    }

    fn sized(app: SoundApp, size: egui::Vec2, render: bool) -> Harness<'static, SoundApp> {
        let builder = Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).with_pixels_per_point(if render { 2.0 } else { 1.0 });
        let builder = if render { builder.wgpu() } else { builder };
        builder.build_ui_state(
            |ui, app: &mut SoundApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        )
    }

    fn app(&self) -> &SoundApp {
        self.h.state()
    }

    fn frames(&mut self, n: usize) {
        self.h.run_steps(n);
    }

    fn frame(&mut self, events: Vec<Event>) {
        for e in events {
            self.h.event(e);
        }
        self.h.step();
    }

    /// The point at sample `at` on the track, `down` of the way from the top of its row to the bottom.
    fn point(&self, at: i64, down: f32) -> Pos2 {
        let s = self.app().engine.session();
        let [x0, y0, x1, y1] = self.app().edit_layout.timeline;
        let row = self.app().edit_layout.rows.iter().find(|(id, _)| *id == self.track.0).unwrap().1;
        let x = edit_window::x_of(s, Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), at);
        pos2(x, row[1] + s.track(self.track).unwrap().height.points() * down)
    }

    fn double_click(&mut self, at: Pos2) {
        // A second of frames first: egui counts a click within 0.6 s of the one before last as a triple-click.
        self.frames(60);
        for pressed in [true, false, true, false] {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
            ]);
        }
        self.frames(3);
    }

    fn editor(&self) -> &FadeEditor {
        match &self.app().dialogs.open {
            Some(Dialog::FadeEditor(ed)) => ed,
            other => panic!("no fade dialog open: {other:?}"),
        }
    }

    fn target(&self) -> Option<FadeTarget> {
        match &self.app().dialogs.open {
            Some(Dialog::FadeEditor(ed)) => Some(ed.target()),
            _ => None,
        }
    }

    fn settings(&self) -> Settings {
        self.editor().settings()
    }

    /// Opens the dialog for `target` the way a double-click does.
    fn open(&mut self, target: FadeTarget) {
        let ed = FadeEditor::for_target(self.app().engine.session(), target).unwrap();
        self.h.state_mut().dialogs.open = Some(Dialog::FadeEditor(Box::new(ed)));
        self.frames(3);
    }

    /// Click the `nth` control with this role and label.
    fn click(&mut self, role: Role, label: &str, nth: usize) {
        self.h.get_all_by_role_and_label(role, label).nth(nth).unwrap_or_else(|| panic!("no {label} #{nth}")).click();
        self.frames(3);
    }

    fn radio(&mut self, label: &str, nth: usize) {
        self.click(Role::RadioButton, label, nth);
    }

    fn button(&mut self, label: &str, nth: usize) {
        self.click(Role::Button, label, nth);
    }

    fn disabled(&self, role: Role, label: &str, nth: usize) -> bool {
        self.h.get_all_by_role_and_label(role, label).nth(nth).unwrap_or_else(|| panic!("no {label} #{nth}")).accesskit_node().is_disabled()
    }

    /// Picks a preset curve of the `nth` shape group from its menu.
    fn preset_curve(&mut self, nth: usize, name: &str) {
        self.button("Choose a preset curve", nth);
        self.button(name, 0);
    }

    fn ok(&mut self) {
        self.button("OK", 0);
        self.frames(2);
    }

    fn enter(&mut self) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }]);
        }
        self.frames(2);
    }

    fn escape(&mut self) {
        for pressed in [true, false] {
            self.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }]);
        }
        self.frames(2);
    }

    fn clip(&self, id: ClipId) -> Clip {
        self.app().engine.session().find_clip(id).unwrap().1.clone()
    }

    fn steps(&self) -> usize {
        self.app().engine.undo_history().len()
    }
}

fn shaped(shape: FadeShape) -> Fade {
    Fade { len: FADE, shape, curvature: 0.0 }
}

const STANDARD_POWER: Side = Side { shape: Shape::Standard, slope: Slope::EqualPower, bend: 0.0 };

/// A crossfade over 76 000..96 000, both fades Linear: clip A is the first id, clip B the second.
fn crossfade_rig(tool: &str) -> (Rig, ClipId, ClipId, i64) {
    let b_start = 2 * SEC - FADE;
    let spans = [(0, 2 * SEC, None, Some(FadeShape::Linear)), (b_start, 2 * SEC, Some(FadeShape::Linear), None)];
    let (r, ids) = Rig::new(tool, &spans);
    (r, ids[0], ids[1], b_start)
}

#[test]
fn double_clicking_a_fade_aims_the_dialog_at_that_fade_and_ok_changes_only_it() {
    for tool in ["grabber", "selector"] {
        for fade_in in [true, false] {
            let (mut r, ids) = Rig::new(tool, &[(0, 2 * SEC, Some(FadeShape::EqualPower), Some(FadeShape::Linear))]);
            let x = ids[0];
            let at = if fade_in { FADE / 2 } else { 2 * SEC - FADE / 2 };
            r.double_click(r.point(at, 0.75));
            assert_eq!(r.target(), Some(FadeTarget::Single { clip: x, fade_in }), "{tool} {fade_in}");
            assert_eq!(r.app().dialogs.open_name(), Some(if fade_in { "fade_in" } else { "fade_out" }));
            assert_eq!(r.app().engine.session().edit.selected_clips, vec![x], "the double-click still selects the clip");
            // It opens on the shape that fade has: Equal Power in, Linear out.
            let side = if fade_in { r.settings().input } else { r.settings().output };
            let expected = if fade_in { STANDARD_POWER } else { Side { slope: Slope::EqualGain, ..STANDARD_POWER } };
            assert_eq!(side, expected, "{tool} {fade_in}");
            // OK changes the shape of that fade, and only that fade, in one undo step.
            r.radio("S-Curve", 0);
            let before = r.app().engine.session().clone();
            let steps = r.steps();
            r.ok();
            let c = r.clip(x);
            let (hit, other) = if fade_in { (c.fade_in, c.fade_out) } else { (c.fade_out, c.fade_in) };
            assert_eq!(hit, shaped(if fade_in { FadeShape::SCurvePower } else { FadeShape::SCurve }), "{tool} {fade_in}");
            assert_eq!(other, if fade_in { shaped(FadeShape::Linear) } else { shaped(FadeShape::EqualPower) });
            assert_eq!(r.steps(), steps + 1);
            assert!(r.app().dialogs.open.is_none());
            assert!(r.h.state_mut().engine.undo());
            assert_eq!(r.app().engine.session(), &before);
        }
    }
}

#[test]
fn double_clicking_a_crossfade_opens_its_dialog_and_s_curve_with_link_equal_power_gives_both_fades_the_new_shape() {
    for tool in ["grabber", "smart"] {
        let (mut r, a, b, b_start) = crossfade_rig(tool);
        r.double_click(r.point(b_start + FADE / 2, 0.75));
        assert_eq!(r.target(), Some(FadeTarget::Crossfade { later: b }), "{tool}");
        assert_eq!((r.editor().kind(), r.app().dialogs.open_name()), (Kind::Crossfade, Some("crossfade")));
        // Two Linear fades come up as Standard with Link Equal Gain.
        assert_eq!(r.settings().link, Link::EqualGain);
        r.radio("S-Curve", 0);
        r.radio("Equal Power", 0);
        assert_eq!((r.settings().input.fade().shape, r.settings().output.fade().shape), (FadeShape::SCurvePower, FadeShape::SCurvePower));
        let before = r.app().engine.session().clone();
        let steps = r.steps();
        r.ok();
        assert_eq!((r.clip(a).fade_out, r.clip(b).fade_in), (shaped(FadeShape::SCurvePower), shaped(FadeShape::SCurvePower)), "{tool}");
        assert_eq!((r.clip(a).end() - r.clip(b).start, r.clip(a).start, r.clip(b).end()), (FADE, 0, b_start + 2 * SEC));
        assert_eq!(r.steps(), steps + 1, "both fades in one undo step");
        assert!(r.h.state_mut().engine.undo());
        assert_eq!(r.app().engine.session(), &before, "one undo restores both fades");
    }
}

#[test]
fn fade_in_standard_with_equal_gain_makes_a_linear_fade_and_cancel_changes_nothing() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    let target = FadeTarget::Single { clip: ids[0], fade_in: true };
    r.open(target);
    let (before, steps) = (r.app().engine.session().clone(), r.steps());
    r.radio("Standard", 0);
    r.radio("Equal Gain", 0);
    r.button("Cancel", 0);
    r.frames(2);
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps), "Cancel changes nothing");
    // Esc does the same.
    r.open(target);
    r.radio("S-Curve", 0);
    r.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    r.frames(2);
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps));
    // OK applies Standard with Equal Gain: the Linear shape.
    r.open(target);
    r.radio("Equal Gain", 0);
    r.ok();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::Linear));
    assert_eq!(r.steps(), steps + 1);
}

#[test]
fn enter_is_ok_and_ok_without_a_change_does_nothing() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    r.app_mut_bend(ids[0], 0.35);
    r.frames(2);
    r.double_click(r.point(FADE / 2, 0.75));
    assert!(r.target().is_some());
    // A bent fade opens on its preset curve, and OK on it leaves it exactly as it was, bend included.
    assert_eq!(r.settings().input.shape, Shape::Preset(FadeShape::EqualPower));
    assert_eq!(r.settings().input.bend, 0.35);
    let (before, steps) = (r.app().engine.session().clone(), r.steps());
    r.enter();
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps));
    // Standard takes the bend off; Enter applies it.
    r.open(FadeTarget::Single { clip: ids[0], fade_in: true });
    r.radio("Standard", 0);
    r.enter();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::EqualPower));
    assert_eq!(r.steps(), steps + 1);
}

impl Rig {
    fn app_mut_bend(&mut self, clip: ClipId, curvature: f32) {
        self.h.state_mut().engine.session_mut().find_clip_mut(clip).unwrap().fade_in.curvature = curvature;
    }
}

#[test]
fn an_open_menu_has_enter_and_esc_before_the_dialog() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    r.open(FadeTarget::Single { clip: ids[0], fade_in: true });
    r.radio("S-Curve", 0);
    let (before, steps) = (r.app().engine.session().clone(), r.steps());
    r.button("Choose a preset curve", 0);
    r.enter();
    assert!(r.app().dialogs.open.is_some(), "Enter with a menu open does not press OK");
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps));
    // Esc closes the menu and leaves the dialog and its edit alone.
    r.escape();
    assert!(r.app().dialogs.open.is_some(), "Esc closes the menu first");
    assert_eq!(r.settings().input.shape, Shape::SCurve);
    // With the menu shut, Esc cancels and Enter would have applied.
    r.escape();
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps));
    r.open(FadeTarget::Single { clip: ids[0], fade_in: true });
    r.radio("S-Curve", 0);
    r.enter();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::SCurvePower));
}

/// The preset-curve menu is as wide as its rows (an icon and a name), not as wide as the room there is, and hangs
/// under the thumbnail it opens from, inside the dialog.
#[test]
fn the_preset_curve_menu_is_as_wide_as_its_rows_and_hangs_under_its_thumbnail_inside_the_dialog() {
    let (mut r, _, b, _) = crossfade_rig("grabber");
    r.open(FadeTarget::Crossfade { later: b });
    r.radio("None", 0);
    let dialog = r.h.query_all_by_role(Role::Window).map(|w| w.rect()).next_back().unwrap();
    // Both thumbnails: the In one on the left and the Out one at the right, next to the dialog's edge.
    for nth in [0, 1] {
        let thumb = r.h.get_all_by_role_and_label(Role::Button, "Choose a preset curve").nth(nth).unwrap().rect();
        r.button("Choose a preset curve", nth);
        let rows: Vec<Rect> =
            ["Exponential", "Logarithmic"].map(|name| r.h.get_all_by_role_and_label(Role::Button, name).next().unwrap().rect()).into();
        for row in &rows {
            let w = row.width();
            assert!(w >= thumb.width() && w < 3.0 * thumb.width(), "#{nth}: a row is {w} px wide, the thumbnail {}", thumb.width());
            assert!(row.min.y >= thumb.max.y, "#{nth}: under the thumbnail");
            assert!((row.min.x - thumb.min.x).abs() < 12.0, "#{nth}: anchored to the thumbnail: {row:?} vs {thumb:?}");
            assert!(row.max.x <= dialog.max.x, "#{nth}: inside the dialog: {row:?} vs {dialog:?}");
        }
        assert_eq!(rows[0].width(), rows[1].width(), "#{nth}: the rows share the menu's width");
        r.escape();
        assert!(r.h.query_all_by_role_and_label(Role::Button, "Logarithmic").next().is_none(), "#{nth}: Esc closes the menu");
    }
}

#[test]
fn the_smart_tool_opens_the_fade_dialog_from_both_halves_of_the_clip() {
    for down in [0.2, 0.75] {
        let (mut r, ids) = Rig::new("smart", &[(0, 2 * SEC, Some(FadeShape::SCurve), None)]);
        r.double_click(r.point(FADE / 2, down));
        assert_eq!(r.target(), Some(FadeTarget::Single { clip: ids[0], fade_in: true }), "down {down}");
    }
}

#[test]
fn double_clicking_a_clip_outside_its_fades_still_only_selects_it() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), Some(FadeShape::EqualPower))]);
    r.double_click(r.point(SEC, 0.75));
    assert_eq!(r.app().dialogs.open_name(), None);
    assert_eq!(r.app().engine.session().edit.selected_clips, vec![ids[0]]);
    // A clip without fades has no fade to aim at, at its edges either.
    let (mut r, _) = Rig::new("grabber", &[(0, 2 * SEC, None, None)]);
    r.double_click(r.point(FADE / 2, 0.75));
    assert_eq!(r.app().dialogs.open_name(), None);
}

#[test]
fn the_menu_still_opens_the_simple_fades_dialog_when_the_selection_is_not_one_fade() {
    let (mut r, _) = Rig::new("grabber", &[(0, 2 * SEC, None, None)]);
    menus::invoke_menu(r.h.state_mut(), "edit.fades_create", "Edit > Fades > Create...");
    r.frames(2);
    assert!(matches!(&r.app().dialogs.open, Some(Dialog::Fades { shape }) if shape == "equal power"));
}

#[test]
fn only_the_tools_that_select_open_the_dialog_and_the_gain_icon_keeps_its_own_double_click() {
    for tool in ["zoom", "scrubber", "pencil"] {
        let (mut r, _) = Rig::new(tool, &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
        r.double_click(r.point(FADE / 2, 0.75));
        assert_eq!(r.app().dialogs.open_name(), None, "{tool}");
    }
    // The clip-gain icon sits at the bottom left of the clip, inside its fade-in.
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    r.h.state_mut().engine.session_mut().find_clip_mut(ids[0]).unwrap().gain_db = 3.0;
    r.frames(2);
    let beside_the_icon = r.point(0, 0.5) + vec2(8.0, 0.0);
    let on_the_icon = r.point(0, 1.0) + vec2(8.0, -9.0);
    r.double_click(on_the_icon);
    assert_eq!(r.app().dialogs.open_name(), None);
    r.double_click(beside_the_icon);
    assert_eq!(r.target(), Some(FadeTarget::Single { clip: ids[0], fade_in: true }), "the same stretch of fade opens the dialog elsewhere");
}

#[test]
fn link_none_frees_the_out_column_and_each_side_keeps_its_own_shape() {
    let (mut r, a, b, _) = crossfade_rig("grabber");
    r.open(FadeTarget::Crossfade { later: b });
    // Linked: Out follows In, its column is greyed, and so is the preset curve of the In column.
    for label in ["Standard", "S-Curve"] {
        assert!(r.disabled(Role::RadioButton, label, 1), "Out {label} is greyed while linked");
        assert!(!r.disabled(Role::RadioButton, label, 0), "In {label} is free");
    }
    assert!(r.disabled(Role::RadioButton, "Preset curve", 0) && r.disabled(Role::RadioButton, "Preset curve", 1));
    r.radio("None", 0);
    assert_eq!(r.settings().link, Link::None);
    for label in ["Standard", "S-Curve", "Preset curve"] {
        assert!(!r.disabled(Role::RadioButton, label, 1), "Out {label} is free with Link None");
    }
    // In takes S-Curve, Out a preset curve from the menu: two different shapes.
    r.radio("S-Curve", 0);
    r.preset_curve(1, "Exponential");
    assert_eq!(r.settings().output.shape, Shape::Preset(FadeShape::Exponential));
    assert_eq!(r.settings().input.shape, Shape::SCurve);
    let steps = r.steps();
    r.ok();
    assert_eq!((r.clip(a).fade_out, r.clip(b).fade_in), (shaped(FadeShape::Exponential), shaped(FadeShape::SCurve)));
    assert_eq!(r.steps(), steps + 1);
    // Linking again puts Out back on In.
    r.open(FadeTarget::Crossfade { later: b });
    assert_eq!(r.settings().link, Link::None, "different shapes open unlinked");
    r.radio("Equal Gain", 0);
    assert_eq!(r.settings().output, r.settings().input);
}

#[test]
fn a_preset_curve_greys_the_slope_and_a_standard_shape_takes_the_bend_off() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    r.app_mut_bend(ids[0], -0.4);
    r.open(FadeTarget::Single { clip: ids[0], fade_in: true });
    assert_eq!(r.settings().input.shape, Shape::Preset(FadeShape::EqualPower));
    assert!(r.disabled(Role::RadioButton, "Equal Gain", 0) && r.disabled(Role::RadioButton, "Equal Power", 0), "the preset fixes the slope");
    r.preset_curve(0, "Logarithmic");
    assert_eq!(r.settings().input, Side { shape: Shape::Preset(FadeShape::Logarithmic), bend: 0.0, ..STANDARD_POWER }, "another curve starts unbent");
    r.radio("S-Curve", 0);
    assert!(!r.disabled(Role::RadioButton, "Equal Gain", 0));
    r.ok();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::SCurvePower), "the bend is off with the new shape");
}

#[test]
fn a_preset_slot_stores_on_cmd_click_and_recalls_on_click() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), Some(FadeShape::EqualPower))]);
    let target = FadeTarget::Single { clip: ids[0], fade_in: true };
    r.open(target);
    // Empty slots do nothing on a plain click.
    r.button("Preset 3", 0);
    assert_eq!(r.settings().input, STANDARD_POWER);
    assert_eq!(r.app().ui.fade_presets.slot(Kind::In, 2), None);
    // Cmd-click stores the current settings.
    r.radio("S-Curve", 0);
    r.radio("Equal Gain", 0);
    let stored = Side { shape: Shape::SCurve, slope: Slope::EqualGain, bend: 0.0 };
    assert_eq!(r.settings().input, stored);
    r.h.get_by_role_and_label(Role::Button, "Preset 3").click_modifiers(Modifiers::COMMAND);
    r.frames(3);
    assert_eq!(r.app().ui.fade_presets.slot(Kind::In, 2).map(|s| s.input), Some(stored));
    assert_eq!(r.settings().input, stored, "storing changes nothing in the dialog");
    // The other dialogs' slots are their own.
    assert_eq!((r.app().ui.fade_presets.slot(Kind::Out, 2), r.app().ui.fade_presets.slot(Kind::Crossfade, 2)), (None, None));
    // A plain click recalls it, here or in the next dialog.
    r.radio("Standard", 0);
    r.radio("Equal Power", 0);
    assert_eq!(r.settings().input, STANDARD_POWER);
    r.button("Preset 3", 0);
    assert_eq!(r.settings().input, stored);
    r.button("Cancel", 0);
    r.frames(2);
    r.open(target);
    assert_eq!(r.settings().input, STANDARD_POWER);
    r.button("Preset 3", 0);
    assert_eq!(r.settings().input, stored);
    // The factory default entry puts the dialog back to Standard with Equal Power.
    let menu = r.h.query_all_by_role(Role::ComboBox).find(|n| n.accesskit_node().value().as_deref() == Some("<factory default>")).unwrap();
    menu.click();
    r.frames(3);
    r.button("<factory default>", 0);
    assert_eq!(r.settings().input, STANDARD_POWER);
    r.ok();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::EqualPower), "nothing changed from the fade it opened on");
}

#[test]
fn compare_shows_the_original_settings_and_ok_applies_the_edit_not_the_original() {
    let (mut r, ids) = Rig::new("grabber", &[(0, 2 * SEC, Some(FadeShape::EqualPower), None)]);
    r.open(FadeTarget::Single { clip: ids[0], fade_in: true });
    r.radio("S-Curve", 0);
    let edited = r.settings();
    assert_eq!(r.editor().shown(), edited);
    r.button("COMPARE", 0);
    assert_eq!(r.editor().shown().input, STANDARD_POWER, "while COMPARE is on, the settings it opened with");
    assert_eq!(r.settings(), edited, "the edits are kept");
    // The controls are greyed meanwhile, and say so.
    assert!(r.disabled(Role::RadioButton, "S-Curve", 0) && r.disabled(Role::Button, "Preset 1", 0));
    r.radio("Standard", 0);
    assert_eq!(r.settings(), edited, "a click on a greyed control changes nothing");
    r.button("COMPARE", 0);
    assert_eq!(r.editor().shown(), edited, "back to the edits");
    // OK with COMPARE on applies the edits.
    r.button("COMPARE", 0);
    r.ok();
    assert_eq!(r.clip(ids[0]).fade_in, shaped(FadeShape::SCurvePower));
}

/// Three clips in a row on one track: 0..2 s, 2..4 s and 4..6 s, with source before and after each so a
/// crossfade can reach into them.
fn selection_rig() -> (Rig, [ClipId; 3]) {
    let spans = [(0, 2 * SEC, None, None), (2 * SEC, 2 * SEC, None, None), (4 * SEC, 2 * SEC, None, None)];
    let (mut r, ids) = Rig::new("grabber", &spans);
    r.h.state_mut().engine.execute("edit.select", &json!({"tracks": [r.track.0]})).unwrap();
    (r, [ids[0], ids[1], ids[2]])
}

fn select(r: &mut Rig, from: i64, to: i64) {
    let track = r.track.0;
    r.h.state_mut().engine.execute("edit.select", &json!({"tracks": [track], "start": from, "end": to, "exact": true})).unwrap();
}

fn open_create(r: &mut Rig) {
    r.h.state_mut().dialogs.open = None;
    menus::invoke_menu(r.h.state_mut(), "edit.fades_create", "Edit > Fades > Create...");
    r.frames(3);
}

#[test]
fn create_over_one_clip_boundary_opens_the_crossfade_dialog_and_ok_makes_the_crossfade() {
    let (mut r, [a, b, _]) = selection_rig();
    select(&mut r, 2 * SEC - 1000, 2 * SEC + 1000);
    open_create(&mut r);
    assert_eq!(r.app().dialogs.open_name(), Some("crossfade"));
    assert_eq!(r.target(), Some(FadeTarget::Crossfade { later: b }));
    // No fade yet: the dialog opens on the factory settings, Link Equal Power.
    assert_eq!(r.settings(), Settings::FACTORY);
    r.radio("S-Curve", 0);
    r.ok();
    assert!(r.app().dialogs.open.is_none());
    // The range form made the crossfade, with the shape the link gives each side.
    assert_eq!((r.clip(a).fade_out.shape, r.clip(b).fade_in.shape), (FadeShape::SCurvePower, FadeShape::SCurvePower));
    assert!(r.clip(a).fade_out.len > 0 && r.clip(b).fade_in.len > 0);
}

#[test]
fn create_at_a_clip_start_or_end_opens_fade_in_or_fade_out() {
    let (mut r, [a, _, c]) = selection_rig();
    select(&mut r, 0, 1000);
    open_create(&mut r);
    assert_eq!((r.app().dialogs.open_name(), r.target()), (Some("fade_in"), Some(FadeTarget::Single { clip: a, fade_in: true })));
    r.radio("Equal Gain", 0);
    r.ok();
    assert_eq!(r.clip(a).fade_in, Fade { len: 1000, shape: FadeShape::Linear, curvature: 0.0 });
    select(&mut r, 6 * SEC - 1000, 6 * SEC);
    open_create(&mut r);
    assert_eq!((r.app().dialogs.open_name(), r.target()), (Some("fade_out"), Some(FadeTarget::Single { clip: c, fade_in: false })));
    r.radio("S-Curve", 0);
    r.ok();
    assert_eq!(r.clip(c).fade_out, Fade { len: 1000, shape: FadeShape::SCurvePower, curvature: 0.0 });
}

#[test]
fn create_over_a_selection_makes_nothing_if_the_clips_moved_while_the_dialog_was_open() {
    let (mut r, [a, b, _]) = selection_rig();
    select(&mut r, 2 * SEC - 1000, 2 * SEC + 1000);
    open_create(&mut r);
    assert_eq!(r.app().dialogs.open_name(), Some("crossfade"));
    // The dialog does not hold the Edit window: the later clip moves off the boundary under the dialog.
    r.h.state_mut().engine.session_mut().find_clip_mut(b).unwrap().start += SEC;
    r.frames(2);
    let (before, steps) = (r.app().engine.session().clone(), r.steps());
    r.ok();
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps), "the range would have made something else");
    assert_eq!(r.clip(a).fade_out, Fade::default());
}

#[test]
fn create_over_a_selection_makes_nothing_if_its_clip_was_trimmed_while_the_dialog_was_open() {
    let (mut r, [a, _, _]) = selection_rig();
    select(&mut r, 0, 1000);
    open_create(&mut r);
    assert_eq!(r.app().dialogs.open_name(), Some("fade_in"));
    // Still the one fade-in of the same clip, but half as long as the graph the dialog opened on.
    r.h.state_mut().engine.session_mut().find_clip_mut(a).unwrap().trim_start_to(500);
    r.frames(2);
    let (before, steps) = (r.app().engine.session().clone(), r.steps());
    r.ok();
    assert!(r.app().dialogs.open.is_none());
    assert_eq!((r.app().engine.session(), r.steps()), (&before, steps), "the fade would not have been the one shown");
}

#[test]
fn create_over_two_boundaries_or_nothing_keeps_the_simple_dialog() {
    let (mut r, _) = selection_rig();
    // Over both boundaries: not one fade.
    select(&mut r, 2 * SEC - 1000, 4 * SEC + 1000);
    open_create(&mut r);
    assert_eq!(r.app().dialogs.open_name(), Some("fades"));
    assert!(matches!(&r.app().dialogs.open, Some(Dialog::Fades { .. })));
    // Over the middle of a clip: no fade at all.
    select(&mut r, SEC / 2, SEC);
    open_create(&mut r);
    assert!(matches!(&r.app().dialogs.open, Some(Dialog::Fades { .. })));
}

/// With `SOUNDCRAFT_FADE_DIALOG_PNG_DIR` set, the three dialogs on the demo session are rendered there.
#[test]
fn the_three_dialogs_render_on_the_demo_session() {
    let mut e = soundcraft_engine::demo::demo_engine();
    // Two Kick clips (the demo's first track) joined by a crossfade, a fade-in on the first and the demo's own fade-out.
    let track = e.session().track_by_name("Kick").unwrap().clone();
    let first = track.clips()[0].clone();
    let at = first.start + first.length / 2;
    e.execute("edit.select", &json!({"tracks": [track.id.0], "start": at, "end": at, "exact": true})).unwrap();
    e.execute("edit.separate", &json!({"tracks": [track.id.0], "at": at})).unwrap();
    let range = Range::new(at - first.length / 8, at + first.length / 8);
    e.execute(
        "edit.fades_create",
        &json!({"tracks": [track.id.0], "start": range.start, "end": range.end, "in_shape": "s-curve", "out_shape": "s-curve"}),
    )
    .unwrap();
    e.execute("edit.fades_create", &json!({"clip": first.id.0, "side": "in", "length": first.length / 4, "shape": "equal power"})).unwrap();
    let clips = e.session().track_by_name("Kick").unwrap().clips().to_vec();
    let (a, b) = (clips[0].id, clips[1].id);
    let out_clip = clips[1].id;
    e.execute("edit.fades_create", &json!({"clip": out_clip.0, "side": "out", "length": clips[1].length / 4, "shape": "exponential"})).unwrap();
    let dir = std::env::var_os("SOUNDCRAFT_FADE_DIALOG_PNG_DIR").map(std::path::PathBuf::from);
    let app = SoundApp::new(e, None, Services::default());
    let mut r = Rig { h: Rig::sized(app, vec2(900.0, 640.0), dir.is_some()), track: track.id };
    r.frames(4);
    let targets = [
        ("in", FadeTarget::Single { clip: a, fade_in: true }),
        ("out", FadeTarget::Single { clip: out_clip, fade_in: false }),
        ("xfade", FadeTarget::Crossfade { later: b }),
    ];
    for (name, target) in targets {
        r.open(target);
        assert!(r.app().dialogs.open_name().is_some(), "{name}");
        if let Some(dir) = &dir {
            r.shoot(&dir.join(format!("{name}_dark.png")));
        }
        r.button("Cancel", 0);
        r.frames(2);
    }
    let Some(dir) = dir else { return };
    // States worth a look: the preset-curve menu open, COMPARE over an unlinked crossfade, and the Light theme.
    r.open(targets[0].1);
    r.button("Choose a preset curve", 0);
    r.shoot(&dir.join("in_menu_dark.png"));
    r.open(targets[2].1);
    r.radio("None", 0);
    r.radio("S-Curve", 1);
    r.button("Cancel", 0);
    r.open(targets[2].1);
    r.radio("None", 0);
    r.radio("S-Curve", 1);
    r.button("COMPARE", 0);
    r.shoot(&dir.join("xfade_none_compare_dark.png"));
    r.button("Cancel", 0);
    r.h.state_mut().ui.theme = soundcraft_ui_egui::theme::ThemeMode::Light;
    r.frames(4);
    for (name, target) in targets {
        r.open(target);
        r.shoot(&dir.join(format!("{name}_light.png")));
        r.button("Cancel", 0);
        r.frames(2);
    }
    r.open(targets[0].1);
    r.button("Choose a preset curve", 0);
    r.shoot(&dir.join("in_menu_light.png"));
    r.h.state_mut().ui.theme = soundcraft_ui_egui::theme::ThemeMode::Dark;
    r.frames(4);
}

impl Rig {
    /// The open dialog alone, cropped out of the render: the Edit window behind it stays out of the picture.
    fn shoot(&mut self, path: &std::path::Path) {
        self.frame(vec![Event::PointerGone]);
        self.frames(3);
        let window = self.h.query_all_by_role(Role::Window).map(|w| w.rect()).next_back().unwrap();
        let image = self.h.render().unwrap();
        let k = 2.0;
        let (x, y) = ((window.min.x * k) as u32, (window.min.y * k) as u32);
        let (w, h) = ((window.width() * k) as u32, (window.height() * k) as u32);
        image::imageops::crop_imm(&image, x, y, w.min(image.width() - x), h.min(image.height() - y)).to_image().save(path).unwrap();
    }
}
