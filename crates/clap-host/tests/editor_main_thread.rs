//! Plugin editors must be driven from the process main thread, which the standard test harness
//! never uses, so this test binary has its own `main` (`harness = false`). The fixture's GUI is
//! window-less: nothing appears on screen.

mod common;

use soundcraft_dsp::Plugin;
use std::sync::Mutex;

const GAIN: &str = "clap:org.soundcraft.test.gain";

/// Runs one 64-frame stereo block of ones through `p` and returns the output.
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

/// `(process calls, params.flush calls)` of every gain fixture so far in this process, as the
/// fixture publishes them (it shares our process, so the file is named after our pid).
fn counters() -> (u64, u64) {
    let path = std::env::temp_dir().join(format!("clap-fixture-counters-{}", std::process::id()));
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut it = text.split_whitespace().map(|w| w.parse::<u64>().expect("counter"));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// A prepared gain fixture (48 kHz, 64 frames, 2 channels) and its editor handle, not yet opened.
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

/// A request made before `prepare` survives activation: the pending edit reaches `idle` through
/// the first flush, without `process`. After a processed block (the plugin is then in the
/// processing state) a flush leaves that state first (the fixture asserts `params.flush` never
/// runs while processing).
fn flush_guards() {
    let (p0, f0) = counters();
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    let mut ed = p.editor().expect("editor handle");
    ed.open().expect("open: requests a flush");
    p.prepare(48_000.0, 64, 2);
    p.flush();
    assert_eq!(counters(), (p0, f0 + 1), "the request survived prepare: one flush, no process");
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)], "the pending edit got through");
    ed.close();
    let mut ed = p.editor().expect("editor handle");
    ed.open().expect("open");
    let _ = block(&mut p);
    ed.close();
    ed.open().expect("reopen: requests a flush again");
    p.flush();
    assert_eq!(counters().1 - f0, 2, "flushed after a processed block, outside the processing state");
    let _ = block(&mut p);
    assert_eq!(counters().0 - p0, 2, "processing restarts on the next block");
    ed.close();
}

/// Smoke test: the main thread opens, services and closes the editor a fixed number of times while
/// the audio thread alternates blocks and flushes until told to stop. (Overlap is impossible by
/// type, `&mut self`; the fixture still asserts it.) The editor is opened before the audio thread
/// starts, so its first block reports the pending edit; the main thread waits for that block (and
/// the flush after it) before its first `idle`, because `idle` holds the edit queue and an edit
/// published while it does is dropped by design. The first `idle` therefore returns that edit
/// deterministically. The audio thread keeps running for the whole churn, so every close/open/idle
/// cycle really overlaps it.
fn editor_alongside_block_and_flush() {
    const CYCLES: u32 = 50;
    let (mut p, mut ed) = gain_with_editor();
    let (p0, _) = counters();
    // Opened before the audio thread starts, so an edit is pending whatever the scheduling: the
    // first block applies it.
    ed.open().expect("open");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (first_done, first_signal) = std::sync::mpsc::channel();
    let audio = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut blocks = 0u32;
            while blocks == 0 || !stop.load(std::sync::atomic::Ordering::Acquire) {
                let _ = block(&mut p);
                p.flush();
                if blocks == 0 {
                    first_done.send(()).expect("the main thread is waiting");
                }
                blocks += 1;
            }
            (p, blocks)
        })
    };
    first_signal.recv().expect("the audio thread ran its first block");
    let first = ed.idle();
    assert_eq!(first, vec![("7".to_string(), 0.25)], "the first block's edit reaches the first idle");
    for _ in 0..CYCLES {
        ed.close();
        ed.open().expect("reopen");
        let _ = ed.idle();
    }
    stop.store(true, std::sync::atomic::Ordering::Release);
    let (p, blocks) = audio.join().expect("the fixture saw an overlap or panicked");
    ed.close();
    // No concurrent writer is left, so the counters file reads whole.
    assert!(blocks >= 1);
    assert_eq!(counters().0 - p0, u64::from(blocks), "every block was processed");
    drop(p);
}

/// Custom harness entry point: answers nextest's `--list`, then drives the editor lifecycle on the main
/// thread and runs the flush scenarios above.
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
    // #901: a plugin dropped on another thread with its editor open (an audio callback dropped
    // by the device's thread) does not destroy the editor there, clap.gui being main-thread only:
    // it warns and leaves it. Earlier scenarios' warnings are dropped first, so only this one counts.
    WARNINGS.lock().unwrap().clear();
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
