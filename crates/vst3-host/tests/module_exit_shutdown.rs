//! A plugin that sets up state while in use (HALion Sonic does) needs its module exit before the
//! process exit tears that state down: `shutdown()` runs the exits up front, while an exit
//! handler comes too late (`harness = false`, see `module_exit.rs`). Each case runs in a child
//! process with the fixture's late state on (`VST3_FIXTURE_LATE_STATE`); the fixture ends the
//! process with status 3 when its module exit comes after the teardown.

mod common;

use soundcraft_dsp::Plugin;
use std::process::{Command, Output};

/// Makes, uses and releases a plugin, then shuts plugin hosting down unless `exit_handler_only`.
fn child(exit_handler_only: bool) {
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    let mut p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    drop(p);
    if !exit_handler_only {
        soundcraft_vst3_host::shutdown();
        assert!(soundcraft_vst3_host::instantiate_plugin(&gain).is_err(), "loads fail after shutdown");
    }
}

fn run(case: &str) -> Output {
    Command::new(std::env::current_exe().expect("test binary")).arg(case).env("VST3_FIXTURE_LATE_STATE", "1").output().expect("run the child")
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("shutdown") => return child(false),
        Some("exit-handler") => return child(true),
        _ => {}
    }
    let up_front = run("shutdown");
    assert!(up_front.status.success(), "shutdown before exit: {:?}\n{}", up_front.status, String::from_utf8_lossy(&up_front.stderr));
    let too_late = run("exit-handler");
    let err = String::from_utf8_lossy(&too_late.stderr);
    assert_eq!(too_late.status.code(), Some(3), "the exit handler alone comes too late: {err}");
    assert!(err.contains("module exit after the process exit tore down state"), "{err}");
    println!("module_exit_shutdown: ok");
}
