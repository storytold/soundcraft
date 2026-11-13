//! VST3 editors on the process main thread (`harness = false`: the standard harness never runs
//! tests there). Opening a real window would flash on screen, so this only checks what the
//! main thread changes without one: the view probe and the parameter round trip through the
//! controller. The window itself is exercised manually (see `installed_plugins_smoke`).

mod common;

fn main() {
    // nextest lists custom-harness tests with `--list`; each binary here is one test.
    if std::env::args().any(|a| a == "--list") {
        println!("editor_main_thread: test");
        return;
    }
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    let mut p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    let info = soundcraft_vst3_host::plugin_info(&gain).expect("info");
    assert_eq!(info.param("3").expect("mode").choices, &["A", "B", "C"], "the main thread reads the plugin's choice text");
    // Editor windows are hosted on macOS and Windows; elsewhere the probe reports "unsupported".
    if cfg!(any(target_os = "macos", windows)) {
        assert_eq!(p.editor_size(), Ok((320, 240)));
    } else {
        assert!(p.editor_size().is_err());
    }
    let mut ed = soundcraft_dsp::Plugin::editor(&mut p).expect("editor handle");
    assert!(!ed.is_open());
    assert_eq!(ed.idle().len(), 1, "the fixture's setup edit");
    // Session → editor: the controller follows (seen through the component handler round trip
    // below and through the editor's own value).
    ed.set_param("7", 1.5);
    ed.set_param("nope", 1.0);
    assert!(ed.idle().is_empty(), "set_param is not reported back as an edit");
    // #599: an identity-mapped (JUCE-style) parameter shows the plugin's own text, verbatim, with
    // its units appended once; correctly mapped parameters and choices keep SoundCraft's format.
    assert_eq!(ed.value_text("13", 0.375).as_deref(), Some("\u{2212}6.0 dB"));
    assert_eq!(ed.value_text("13", 1.0).as_deref(), Some("24.0 dB"));
    assert_eq!(ed.value_text("14", 0.25).as_deref(), Some("3.0 dB"), "units already in the text");
    assert_eq!(ed.value_text("7", 1.5), None);
    assert_eq!(ed.value_text("3", 1.0), None);
    assert_eq!(ed.value_text("nope", 0.5), None);
    assert!((soundcraft_dsp::Plugin::value_from_text(&mut p, "13", "-6.0 dB").unwrap_or(-1.0) - 0.375).abs() < 1e-6);
    assert_eq!(soundcraft_dsp::Plugin::value_from_text(&mut p, "13", "bad"), None);
    // The fixture's text for "Level" has no unit, and its parser does not strip "dB". The host
    // appends the unit on the way out and takes that one suffix back off on the way in.
    let shown = soundcraft_dsp::Plugin::value_text(&mut p, "13", 0.375).expect("level text");
    assert_eq!(shown, "\u{2212}6.0 dB");
    let back = soundcraft_dsp::Plugin::value_from_text(&mut p, "13", &shown).expect("round trip");
    assert!((back - 0.375).abs() < 1e-6, "{shown} -> {back}");
    assert_eq!(soundcraft_dsp::Plugin::value_from_text(&mut p, "13", &"x".repeat(257)), None, "over 256 scalars is refused before the plugin parse");
    assert_eq!(ed.value_from_text("13", &"x".repeat(257)), None, "over 256 scalars is refused before the editor parse");
    // The editor handle parses every mapping (not only the identity display) and reads the live
    // value, on the main thread, through the same controller. It strips one appended unit too.
    assert!((ed.value_from_text("13", "-6.0 dB").unwrap_or(-1.0) - 0.375).abs() < 1e-6);
    let editor_back = ed.value_from_text("13", &shown).expect("editor round trip");
    assert!((editor_back - back).abs() < 1e-6, "{shown} -> {editor_back}");
    assert_eq!(ed.value_from_text("13", "bad"), None);
    assert!((ed.param("7").unwrap_or(-1.0) - 1.5).abs() < 1e-6);
    drop(p);
    assert_eq!(ed.value_text("13", 0.375), None, "the plugin is gone");
    assert_eq!(ed.param("7"), None, "the plugin is gone");
    assert_eq!(ed.value_from_text("13", "-6.0 dB"), None, "the plugin is gone");
    assert!(!ed.is_open());
    assert!(ed.open().is_err(), "the plugin is gone");
    println!("editor_main_thread: ok");
}
