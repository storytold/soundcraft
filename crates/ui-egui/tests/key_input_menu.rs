//! The Key menu of a dynamics plug-in window picks the bus its detector listens to; a bus that
//! already carries the track's own signal can't be picked.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

#[test]
fn the_key_menu_sets_a_bus_and_greys_out_a_feedback_loop() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("mix.send", json!({"track": "Kick", "slot": 1, "bus": "Kick Key", "pre_fader": true})).unwrap();
    app.run("mix.send", json!({"track": "Bass", "slot": 1, "bus": "Bass Key"})).unwrap();
    let s = app.engine.session();
    let bass = s.track_by_name("Bass").unwrap().id;
    let kick_key = s.bus_by_name("Kick Key").unwrap().id;
    assert_eq!(s.track(bass).unwrap().mixer.inserts[0].as_ref().unwrap().plugin, "compressor");
    app.ui.plugin_windows.push((bass, 0));
    let proof_dir = std::env::var_os("SOUNDCRAFT_KEY_PROOF_DIR").map(std::path::PathBuf::from);
    let mut builder = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(2.0);
    if proof_dir.is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    h.get_by_label("Key Input").click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(h.get_by_label("Bass Key").accesskit_node().is_disabled(), "the bass feeds Bass Key: picking it would feed back");
    assert!(!h.get_by_label("Kick Key").accesskit_node().is_disabled());
    if let Some(dir) = &proof_dir {
        h.render().unwrap().save(dir.join("key-menu.png")).unwrap();
    }
    h.get_by_label("Kick Key").click();
    for _ in 0..3 {
        step(&mut h);
    }
    let ins = h.state().engine.session().track(bass).unwrap().mixer.inserts[0].clone().unwrap();
    assert_eq!(ins.key, Some(kick_key));
    assert_eq!(ins.params.get("ext_key"), Some(&1.0));
}
