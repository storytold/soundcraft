//! VST3 editors on the process main thread (`harness = false`: the standard harness never runs
//! tests there). Opening a real window would flash on screen, so this only checks what the
//! main thread changes without one: the view probe and the parameter round trip through the
//! controller. The window itself is exercised manually (see `installed_plugins_smoke`).

mod common;

fn main() {
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    let mut p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    // Editor windows are hosted on macOS only; elsewhere the probe reports "unsupported".
    if cfg!(target_os = "macos") {
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
    drop(p);
    assert!(!ed.is_open());
    assert!(ed.open().is_err(), "the plugin is gone");
    println!("editor_main_thread: ok");
}
