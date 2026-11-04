//! Plugin editors must be driven from the process main thread, which the standard test harness
//! never uses, so this test binary has its own `main` (`harness = false`). The fixture's GUI is
//! window-less: nothing appears on screen.

mod common;

use soundcraft_dsp::Plugin;

const GAIN: &str = "clap:org.soundcraft.test.gain";

fn block(p: &mut Box<dyn Plugin>) -> Vec<Vec<f32>> {
    let mut io = vec![vec![1.0f32; 64]; 2];
    p.process(&mut io, 64);
    io
}

/// `(process calls, params.flush calls)` of every gain fixture so far in this process, as the
/// fixture publishes them (it shares our process, so the file is named after our pid).
fn counters() -> (u64, u64) {
    let path = std::env::temp_dir().join(format!("clap-fixture-counters-{}", std::process::id()));
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut it = text.split_whitespace().map(|w| w.parse::<u64>().expect("counter"));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn gain_with_editor() -> (Box<dyn Plugin>, Box<dyn soundcraft_dsp::PluginEditor>) {
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    p.prepare(48_000.0, 64, 2);
    let ed = p.editor().expect("editor handle");
    (p, ed)
}

/// An edit made in the plugin's editor while the transport is stopped (no `process`) reaches the
/// UI through `Plugin::flush`: the fixture asks for `params.request_flush` from `gui.show`.
fn gui_edit_applies_without_process() {
    let (p0, f0) = counters();
    let (mut p, mut ed) = gain_with_editor();
    ed.open().expect("open");
    p.flush();
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)], "the edit is reported without any process call");
    assert!(ed.idle().is_empty(), "drained");
    let (p1, f1) = counters();
    assert_eq!((p1 - p0, f1 - f0), (0, 1), "one flush, no process");
    ed.close();
}

/// Requests are coalesced, and `flush` does nothing without one.
fn flush_is_coalesced_and_conditional() {
    let (mut p, mut ed) = gain_with_editor();
    let (_, f0) = counters();
    p.flush();
    p.flush();
    assert_eq!(counters().1 - f0, 0, "no request, no flush");
    ed.open().expect("open");
    p.flush();
    p.flush();
    assert_eq!(counters().1 - f0, 1, "one request, one flush");
    assert_eq!(ed.idle().len(), 1);
    // A processed block applies the pending edit itself and clears the request.
    ed.close();
    ed.open().expect("reopen: the fixture edits again and requests a flush");
    let (pc, fc) = counters();
    let _ = block(&mut p);
    p.flush();
    assert_eq!(counters().1 - fc, 0, "process cleared the request");
    assert_eq!(counters().0 - pc, 1);
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)]);
    ed.close();
}

/// `clap_host.request_process` alone neither flushes nor processes.
fn request_process_is_ignored() {
    let (mut p, _ed) = gain_with_editor();
    let (p0, f0) = counters();
    p.reset(); // the fixture calls request_process from `reset`
    p.flush();
    p.flush();
    assert_eq!(counters(), (p0, f0));
}

/// `prepare` forgets a stale request. After a processed block (the plugin is then in the
/// processing state) a flush leaves that state first (the fixture asserts `params.flush` never
/// runs while processing).
fn flush_guards() {
    let (p0, f0) = counters();
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    let mut ed = p.editor().expect("editor handle");
    ed.open().expect("open: requests a flush");
    p.prepare(48_000.0, 64, 2);
    p.flush();
    assert_eq!(counters().1 - f0, 0, "prepare forgot the stale request");
    ed.close();
    let mut ed = p.editor().expect("editor handle");
    ed.open().expect("open");
    let _ = block(&mut p);
    ed.close();
    ed.open().expect("reopen: requests a flush again");
    p.flush();
    assert_eq!(counters().1 - f0, 1, "flushed after a processed block, outside the processing state");
    let _ = block(&mut p);
    assert_eq!(counters().0 - p0, 2, "processing restarts on the next block");
}

/// Smoke test: the main thread opens, services and closes the editor while the audio thread
/// alternates blocks and flushes. (Overlap is impossible by type, `&mut self`; the fixture still
/// asserts it.)
fn editor_alongside_block_and_flush() {
    let (mut p, mut ed) = gain_with_editor();
    let (p0, _) = counters();
    let audio = std::thread::spawn(move || {
        for _ in 0..2_000 {
            let _ = block(&mut p);
            p.flush();
        }
        p
    });
    let mut edits = 0;
    while !audio.is_finished() {
        ed.open().expect("open");
        edits += ed.idle().len();
        ed.close();
    }
    let p = audio.join().expect("the fixture saw an overlap or panicked");
    edits += ed.idle().len();
    assert!(edits > 0, "some edit got through");
    assert_eq!(counters().0 - p0, 2_000);
    drop(p);
}

fn main() {
    // nextest lists custom-harness tests with `--list`; each binary here is one test.
    if std::env::args().any(|a| a == "--list") {
        println!("editor_main_thread: test");
        return;
    }
    common::fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("editor handle");
    assert!(!ed.is_open());
    ed.open().expect("open on the main thread");
    assert!(ed.is_open());
    ed.open().expect("opening again just raises it");
    // The fixture "edits" the gain in its GUI; the edit arrives as an output event of the next
    // block and is reported (in SoundCraft units) by idle.
    let io = block(&mut p);
    assert!((io[0][10] - 0.25).abs() < 1e-6, "{}", io[0][10]);
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)]);
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
    assert!(ed.open().is_err());
    assert!(ed.idle().is_empty());
    ed.close();
    gui_edit_applies_without_process();
    flush_is_coalesced_and_conditional();
    request_process_is_ignored();
    flush_guards();
    editor_alongside_block_and_flush();
    println!("editor_main_thread: ok");
}
