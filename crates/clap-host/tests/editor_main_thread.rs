//! Plugin editors must be driven from the process main thread, which the standard test harness
//! never uses, so this test binary has its own `main` (`harness = false`). The fixture's GUI is
//! window-less: nothing appears on screen.

mod common;

use soundcraft_dsp::Plugin;
use std::sync::Mutex;

const GAIN: &str = "clap:org.soundcraft.test.gain";

fn block(p: &mut Box<dyn Plugin>) -> Vec<Vec<f32>> {
    let mut io = vec![vec![1.0f32; 64]; 2];
    p.process(&mut io, 64);
    io
}

/// The warnings logged, to tell which way the host went.
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Warnings;

impl log::Log for Warnings {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            WARNINGS.lock().unwrap().push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}

fn main() {
    // nextest lists custom-harness tests with `--list`; each binary here is one test.
    if std::env::args().any(|a| a == "--list") {
        println!("editor_main_thread: test");
        return;
    }
    log::set_logger(&Warnings).expect("logger");
    log::set_max_level(log::LevelFilter::Warn);
    common::fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    plugin_text_on_the_main_thread(p.as_mut());
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("editor handle");
    assert!(!ed.is_open());
    editor_text_before_any_edit(ed.as_mut());
    ed.open().expect("open on the main thread");
    assert!(ed.is_open());
    ed.open().expect("opening again just raises it");
    // The fixture "edits" the gain in its GUI; the edit arrives as an output event of the next
    // block and is reported (in SoundCraft units) by idle.
    let io = block(&mut p);
    assert!((io[0][10] - 0.25).abs() < 1e-6, "{}", io[0][10]);
    // Only the main thread drives the editor, on every platform (#901). Another thread, named or
    // not, is refused: it neither takes the edit idle reports nor closes or reopens the editor.
    for name in [None, Some("soundcraft-ui")] {
        let mut other = p.editor().expect("editor handle");
        let thread = match name {
            Some(n) => std::thread::Builder::new().name(n.to_string()),
            None => std::thread::Builder::new(),
        };
        let seen = thread
            .spawn(move || {
                let edits = other.idle();
                other.close();
                (edits, other.open().map_err(|e| e.contains("main thread")))
            })
            .expect("spawn")
            .join()
            .expect("no panic");
        assert_eq!(seen, (Vec::new(), Err(true)), "{name:?}");
        assert!(ed.is_open(), "{name:?}");
    }
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)]);
    assert!((ed.param("7").unwrap_or(-1.0) - 0.25).abs() < 1e-6, "the live value follows the edit");
    assert!(ed.idle().is_empty(), "drained");
    ed.close();
    assert!(!ed.is_open());
    // Through the Plugin trait as well.
    p.open_editor().expect("open via the trait");
    assert!(ed.is_open(), "the handle sees the same editor");
    p.close_editor();
    assert!(!ed.is_open());
    // Dropping the instance with its editor open destroys the GUI first (the fixture asserts
    // that); the handle then refuses politely.
    ed.open().expect("reopen");
    drop(p);
    assert!(!ed.is_open());
    assert_eq!(ed.param("7"), None, "the plugin is gone");
    assert_eq!(ed.value_from_text("3", "b"), None, "the plugin is gone");
    assert!(ed.open().is_err());
    assert!(ed.idle().is_empty());
    ed.close();
    // #901: a plugin dropped on another thread with its editor open (an audio callback dropped
    // by the device's thread) does not destroy the editor there, clap.gui being main-thread only:
    // it warns and leaves it.
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("editor handle");
    ed.open().expect("open on the main thread");
    std::thread::Builder::new().name("audio".into()).spawn(move || drop(p)).expect("spawn").join().expect("no panic");
    let left = "clap: editor still open while its plugin is destroyed off the main thread";
    assert!(WARNINGS.lock().unwrap().iter().any(|w| w == left), "{:?}", WARNINGS.lock().unwrap());
    assert!(!ed.is_open());
    assert!(ed.open().is_err());
    println!("editor_main_thread: ok");
}

/// The plugin answers choice and value text on the main thread, and refuses text it can't parse.
fn plugin_text_on_the_main_thread(p: &mut dyn Plugin) {
    let info = soundcraft_clap_host::plugin_info(GAIN).expect("info");
    assert_eq!(info.param("3").expect("mode").choices, &["A", "B", "C"], "the main thread reads the plugin's choice text");
    assert_eq!(p.value_text("3", 1.0).as_deref(), Some("B"));
    assert_eq!(p.value_from_text("3", "b"), Some(1.0));
    assert_eq!(p.value_from_text("3", "not a mode"), None);
    assert_eq!(p.value_from_text("3", &"x".repeat(257)), None);
}

/// Text and the live value go through the same main-thread handle as the window, before any GUI
/// edit.
fn editor_text_before_any_edit(ed: &mut dyn soundcraft_dsp::PluginEditor) {
    assert_eq!(ed.value_text("3", 1.0).as_deref(), Some("B"));
    assert_eq!(ed.value_from_text("3", "b"), Some(1.0));
    assert_eq!(ed.value_from_text("3", "nope"), None);
    assert!((ed.param("7").unwrap_or(-1.0) - 1.0).abs() < 1e-6);
}
