//! A module with a plugin instance still alive at process exit keeps running: its module exit
//! would pull the module out from under that instance (`harness = false`, see `module_exit.rs`).
//! The fixture aborts the process if its module exit runs while any of its objects are alive.

mod common;

fn main() {
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    let p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    // Still alive at exit, like a plugin held by a mixer that is never torn down.
    std::mem::forget(p);
    println!("module_exit_live_instance: ok");
}
