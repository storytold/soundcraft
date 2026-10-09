//! End to end: build the test VST3 plugin in `tests/fixture` (a cdylib outside the workspace),
//! install it as a `.vst3` bundle in a temp folder, then scan → instantiate → process → params →
//! notes.

use soundcraft_dsp::{Category, Taper, Unit};

mod common;
use common::fixture_dir;

fn id_of(name: &str) -> String {
    let found = soundcraft_vst3_host::scan_paths(&[fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == name).unwrap().id
}

#[test]
fn scan_lists_both_audio_modules_but_not_the_controller_class() {
    let dir = fixture_dir();
    let found = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]);
    assert_eq!(found.len(), 2, "{found:?}");
    let g = found.iter().find(|d| d.name == "Fixture Gain").unwrap();
    // `uid()` lays out the first 8 bytes differently on Windows (COM GUID order).
    let want = if cfg!(windows) { "vst3:001D0A5C111122223333444455556666" } else { "vst3:5C0A1D00111122223333444455556666" };
    assert_eq!(g.id, want);
    assert_eq!(g.vendor, "SoundCraft tests", "factory vendor fills an empty class vendor");
    assert_eq!(g.version, "1.2.3");
    assert_eq!(g.sdk_version, "VST 3.8.0");
    assert_eq!(g.sub_categories, vec!["Fx"]);
    assert_eq!(g.category, Category::Other);
    assert!(!g.is_instrument);
    assert!(g.path.ends_with("Fixture.vst3"));
    let s = found.iter().find(|d| d.name == "Fixture Synth").unwrap();
    assert_eq!(s.category, Category::Instrument);
    assert!(s.is_instrument);
    let global = soundcraft_vst3_host::scan();
    assert!(global.iter().any(|d| d.id == g.id) && global.iter().any(|d| d.id == s.id));
}

#[test]
fn info_exposes_visible_params_with_plain_ranges() {
    let gain = id_of("Fixture Gain");
    let info = soundcraft_vst3_host::plugin_info(&gain).unwrap();
    assert_eq!(info.id, gain);
    assert_eq!(info.short_name, "Fixture ");
    let ids: Vec<&str> = info.params.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec!["7", "3", "12"], "hidden 9 and bypass 11 are skipped");
    let g = info.param("7").unwrap();
    assert_eq!((g.name, g.min, g.max, g.default, g.unit, g.taper), ("Gain", 0.0, 2.0, 1.0, Unit::Db, Taper::Linear));
    let mode = info.param("3").unwrap();
    assert_eq!((mode.unit, mode.min, mode.max), (Unit::Choice, 0.0, 2.0));
    assert_eq!(mode.choices, &["A", "B", "C"]);
    let f = info.param("12").unwrap();
    assert_eq!((f.unit, f.taper), (Unit::Hz, Taper::Log));
    assert!((f.min - 20.0).abs() < 1e-3 && (f.max - 20_000.0).abs() < 1e-1, "{f:?}");
    // Cached: the same leaked info every time.
    assert!(std::ptr::eq(info, soundcraft_vst3_host::plugin_info(&gain).unwrap()));
}

#[test]
fn gain_processes_audio_and_follows_params() {
    let mut p = soundcraft_vst3_host::create(&id_of("Fixture Gain")).unwrap();
    p.prepare(44_100.0, 256, 2);
    assert_eq!(p.latency(), 3);
    // The controller was synced from the component state (gain 0.5), not the declared default.
    assert_eq!(p.param("7"), Some(0.5));
    let mut io = vec![vec![0.5f32; 256], vec![-0.25f32; 256]];
    p.process(&mut io, 256);
    assert!(io[0].iter().all(|&x| (x - 0.25).abs() < 1e-6), "{:?}", &io[0][..4]);
    assert!(p.set_param("7", 2.0));
    assert!(!p.set_param("9", 1.0), "hidden params are not exposed");
    assert!(!p.set_param("Gain", 1.0), "ids are VST3 ParamIDs");
    assert!(p.set_param("7", 5.0), "clamped");
    assert_eq!(p.param("7"), Some(2.0));
    assert!(p.set_param("12", 2000.0));
    assert!((p.param("12").unwrap() - 2000.0).abs() < 1e-2);
    assert!(p.set_param("3", 1.6));
    assert_eq!(p.param("3"), Some(2.0), "choices round");
    // A block larger than max_block is chunked.
    let mut io = vec![vec![0.5f32; 700], vec![-0.25f32; 700]];
    p.process(&mut io, 700);
    assert!(io[0].iter().all(|&x| (x - 1.0).abs() < 1e-6));
    assert!(io[1].iter().all(|&x| (x + 0.5).abs() < 1e-6));
    // Mono strip: the plugin accepts a mono arrangement; the session value is re-sent.
    p.prepare(48_000.0, 128, 1);
    let mut mono = vec![vec![0.25f32; 128]];
    p.process(&mut mono, 128);
    assert!(mono[0].iter().all(|&x| (x - 0.5).abs() < 1e-6), "{:?}", &mono[0][..4]);
    // Hostile shapes don't crash.
    p.process(&mut [], 64);
    let mut short = vec![vec![1.0f32; 4]];
    p.process(&mut short, 1_000_000);
    assert!(p.set_param("7", f32::NAN), "NaN becomes the default");
    assert_eq!(p.param("7"), Some(1.0));
    p.reset();
    p.prepare(f32::NAN, 0, 0);
    let mut io = vec![vec![0.1f32; 8]];
    p.process(&mut io, 8);
}

#[test]
fn synth_plays_notes_at_their_offsets() {
    let id = id_of("Fixture Synth");
    let info = soundcraft_vst3_host::plugin_info(&id).unwrap();
    assert!(info.is_instrument);
    assert!(info.params.is_empty());
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![9.0f32; 64], vec![9.0f32; 64]];
    p.note_on(10, 60, 127);
    p.note_off(40, 60);
    p.process(&mut io, 64);
    for (i, &x) in io[0].iter().enumerate() {
        let want = if (10..40).contains(&i) { 1.0 } else { 0.0 };
        assert!((x - want).abs() < 1e-6, "frame {i}: {x}");
    }
    assert_eq!(io[0], io[1]);
    // Offsets past the block are clamped into it; all_notes_off releases held notes.
    p.note_on(1000, 61, 64);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!(io[0][62].abs() < 1e-6 && (io[0][63] - 64.0 / 127.0).abs() < 1e-6);
    p.all_notes_off();
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6));
    // Notes in a later sub-block of a chunked call land at the right frame.
    p.prepare(48_000.0, 32, 2);
    let mut io = vec![vec![0.0f32; 96], vec![0.0f32; 96]];
    p.note_on(50, 62, 127);
    p.process(&mut io, 96);
    assert!(io[0][49].abs() < 1e-6 && (io[0][50] - 1.0).abs() < 1e-6 && (io[0][95] - 1.0).abs() < 1e-6);
    // A mono strip downmixes the stereo output.
    p.prepare(48_000.0, 64, 1);
    let mut mono = vec![vec![0.0f32; 64]];
    p.process(&mut mono, 64);
    assert!(mono[0].iter().all(|&x| (x - 1.0).abs() < 1e-6), "the note is still held");
}

#[test]
fn many_instances_create_and_drop_cleanly() {
    let (g, s) = (id_of("Fixture Gain"), id_of("Fixture Synth"));
    for _ in 0..50 {
        let mut a = soundcraft_vst3_host::create(&g).unwrap();
        let b = soundcraft_vst3_host::create(&s).unwrap();
        let mut io = vec![vec![0.0f32; 32]; 2];
        a.process(&mut io, 32);
        drop(b);
    }
}

/// Smoke test against whatever real VST3 plugins are installed (none are required): scan, then
/// instantiate, prepare and process each one briefly. Set `SOUNDCRAFT_VST3_SMOKE=1` to run it.
#[test]
fn installed_plugins_smoke() {
    if std::env::var_os("SOUNDCRAFT_VST3_SMOKE").is_none() {
        return;
    }
    for d in soundcraft_vst3_host::scan() {
        eprintln!("found {} ({}) {:?} [{}] at {}", d.name, d.vendor, d.category, d.sub_categories.join("|"), d.path.display());
        let info = soundcraft_vst3_host::plugin_info(&d.id);
        eprintln!(
            "  info: {:?}",
            info.map(|i| (i.params.len(), i.params.iter().take(5).map(|p| (p.id, p.name, p.min, p.max, p.unit)).collect::<Vec<_>>()))
        );
        match soundcraft_vst3_host::instantiate(&d.id) {
            Ok(mut p) => {
                p.prepare(48_000.0, 512, 2);
                let mut io = vec![vec![0.1f32; 512], vec![0.1f32; 512]];
                p.process(&mut io, 512);
                eprintln!("  processed; latency {} out[0] {}", p.latency(), io[0][0]);
            }
            Err(e) => eprintln!("  instantiate: {e}"),
        }
    }
}

#[test]
fn state_saves_component_and_controller_and_restores_them() {
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    let blob = p.save_state().expect("component state");
    assert!(blob.starts_with(b"SCV3"));
    let mut q = soundcraft_vst3_host::create(&id).unwrap();
    q.prepare(48_000.0, 64, 2);
    assert_eq!(q.param("7"), Some(0.5));
    assert!(q.load_state(&blob));
    assert_eq!(q.param("7"), Some(1.5), "controller synced from the restored component state");
    let mut io = vec![vec![0.5f32; 64]; 2];
    q.process(&mut io, 64);
    assert!(io[0].iter().all(|&x| (x - 0.75).abs() < 1e-6), "{:?}", &io[0][..4]);
    // The controller part (Freq) round-trips too: write one by hand.
    let mut custom = b"SCV3".to_vec();
    custom.extend_from_slice(&8u32.to_le_bytes());
    custom.extend_from_slice(&0.25f64.to_le_bytes());
    custom.extend_from_slice(&8u32.to_le_bytes());
    custom.extend_from_slice(&1.0f64.to_le_bytes());
    assert!(q.load_state(&custom));
    assert_eq!(q.param("7"), Some(0.5));
    assert!((q.param("12").unwrap() - 20_000.0).abs() < 1.0, "{:?}", q.param("12"));
    // A bare component state is accepted; hostile blobs are refused.
    assert!(q.load_state(&0.5f64.to_le_bytes()));
    assert_eq!(q.param("7"), Some(1.0));
    for bad in [&b""[..], b"SCV3", b"SCV3\xff\xff\xff\xff", &[0xffu8; 8][..], b"abc"] {
        assert!(!q.load_state(bad), "{bad:?}");
    }
    assert_eq!(q.param("7"), Some(1.0));
}

#[test]
fn editor_view_is_probed_and_edits_come_back_as_plain_values() {
    let id = id_of("Fixture Gain");
    let mut p = soundcraft_vst3_host::instantiate_plugin(&id).unwrap();
    // Editor windows are hosted on macOS only; elsewhere the probe reports "unsupported".
    if cfg!(target_os = "macos") {
        assert_eq!(p.editor_size(), Ok((320, 240)));
    } else {
        assert!(p.editor_size().is_err());
    }
    let mut ed = soundcraft_dsp::Plugin::editor(&mut p).expect("editor handle");
    // The fixture "edits" Gain to 0.9 (normalized) when it gets the component handler.
    assert_eq!(ed.idle(), vec![("7".to_string(), 1.8)]);
    assert!(ed.idle().is_empty());
    if cfg!(target_os = "macos") {
        assert!(ed.open().unwrap_err().contains("main thread"), "test threads are not the main thread");
    } else {
        assert!(ed.open().is_err(), "unsupported elsewhere");
    }
    assert!(!ed.is_open());
    // The synth's controller has no view.
    let synth = soundcraft_vst3_host::instantiate_plugin(&id_of("Fixture Synth")).unwrap();
    assert!(synth.editor_size().is_err());
}
