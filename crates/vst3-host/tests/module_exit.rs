//! At process exit the host runs the module exit of every module it loaded, after releasing the
//! factory and before the module's statics are torn down (`harness = false`: the process exit is
//! what is under test). The fixture aborts the process if that order breaks, much as real plugins
//! crash: HALion Sonic segfaulted in its static destructors after a plain plugin scan.

mod common;

fn main() {
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    let p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    drop(p);
    println!("module_exit: ok");
}
