//! A module first loaded from a short-lived thread is entered and, at process exit, exited on
//! one thread that outlives it (`harness = false`, see `module_exit.rs`): the asking thread is
//! gone by then, and modules tie state to the thread of their entry (Massive X crashed in a
//! module exit run on another thread). The fixture aborts the process if the threads differ.

mod common;

fn main() {
    std::thread::spawn(|| {
        let dir = common::fixture_dir();
        let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
        drop(soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate"));
    })
    .join()
    .expect("worker thread");
    println!("module_exit_worker_thread: ok");
}
