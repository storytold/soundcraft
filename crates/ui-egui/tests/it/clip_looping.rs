use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

/// Clip > Loop... opens Clip Looping instead of looping right away, and Loop Until End of Session
/// or Next Clip fills the gap up to the next clip, the last loop cut short to meet it.
#[test]
fn clip_looping_fills_up_to_the_next_clip() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap();
    let (track, clip) = (kick.id, kick.clips()[0].clone());
    let after = kick.clips().iter().filter(|c| c.start >= clip.end()).min_by_key(|c| c.start).unwrap().clone();
    // Trim the next clip's start so the gap is not a whole number of loops.
    let next = after.start + clip.length / 2;
    assert!(next < after.end());
    app.run("edit.trim_clip", json!({"clip": after.id.0, "start": next})).unwrap();
    assert_ne!((next - clip.end()) % clip.length, 0);
    app.run("edit.select", json!({"clips": [clip.id.0]})).unwrap();
    let before = app.engine.session().track(track).unwrap().clips().len();
    menus::invoke_menu(&mut app, "clip.loop", "Clip > Loop...");
    assert_eq!(app.dialogs.open_name(), Some("clip_looping"));
    assert_eq!(app.engine.session().track(track).unwrap().clips().len(), before);

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_role_and_label(Role::RadioButton, "Loop Until End of Session or Next Clip").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);

    assert!(h.state().dialogs.open_name().is_none());
    let clips = h.state().engine.session().track(track).unwrap().clips();
    let loops: Vec<_> = clips.iter().filter(|c| c.start >= clip.end() && c.start < next).collect();
    let whole = (next - clip.end()) / clip.length;
    assert_eq!(loops.len() as i64, whole + 1);
    assert!(loops.iter().all(|c| c.loop_id.is_some() && c.loop_id == loops[0].loop_id));
    assert!(loops.windows(2).all(|w| w[0].end() == w[1].start));
    let last = loops.last().unwrap();
    assert_eq!(last.end(), next);
    assert_eq!(last.length, (next - clip.end()) % clip.length);
}

/// Clip > Loop... with Fit to Grid stretches an off-grid clip to a whole number of beats first, so
/// every copy lands on the tempo grid instead of repeating at the clip's own length (#1404).
#[test]
fn clip_looping_fit_to_grid_lands_the_copies_on_the_beat() {
    use soundcraft_time::Samples;
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap();
    let (track, clip) = (kick.id, kick.clips()[0].clone());
    // The clip spans exactly 8 bars (32 beats at 104 BPM in 4/4). Trim its end so it no longer
    // spans whole beats, the way an imported loop that misses the grid does.
    app.run("edit.trim_clip", json!({"clip": clip.id.0, "end": clip.end() - 5000})).unwrap();
    assert_ne!((clip.length - 5000) % (clip.length / 32), 0);
    app.run("edit.select", json!({"clips": [clip.id.0]})).unwrap();
    menus::invoke_menu(&mut app, "clip.loop", "Clip > Loop...");
    assert_eq!(app.dialogs.open_name(), Some("clip_looping"));

    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_role_and_label(Role::CheckBox, "Fit to Grid").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(2);

    assert!(h.state().dialogs.open_name().is_none());
    let s = h.state().engine.session();
    let sr = s.sample_rate;
    let clips = s.track(track).unwrap().clips();
    let origin = clips.iter().find(|c| c.id == clip.id).unwrap();
    let lid = origin.loop_id.expect("the trimmed clip is looped");
    let copies: Vec<_> = clips.iter().filter(|c| c.loop_id == Some(lid) && c.id != origin.id).collect();
    assert_eq!(copies.len(), 3);
    assert!(copies.iter().all(|c| c.length == origin.length), "each copy is the fitted clip");
    assert!(copies.windows(2).all(|w| w[0].end() == w[1].start), "the copies butt up");
    // Measured on the tempo map, the fitted clip and every copy start a whole number of beats
    // after the origin's start.
    let beats_from_origin =
        |at: Samples| (s.tempo.samples_to_ticks_f(at, sr) - s.tempo.samples_to_ticks_f(origin.start, sr)) / soundcraft_time::TICKS_PER_QUARTER as f64;
    let fitted_beats = beats_from_origin(origin.start + origin.length);
    assert!((fitted_beats - fitted_beats.round()).abs() < 0.01, "the clip spans {fitted_beats} beats, not a whole number");
    for c in &copies {
        let beats = beats_from_origin(c.start);
        assert!((beats - beats.round()).abs() < 0.01, "a copy starts {beats} beats after the origin, off the grid");
    }
}
