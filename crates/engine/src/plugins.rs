//! Apps call [`init`] at the top of `main`: a plugin-scan child reads its one plugin file and exits there.

use soundcraft_plugin_scan as scan;
use std::path::PathBuf;

pub fn init() {
    if let Some((format, path)) = scan::child_request() {
        soundcraft_vst3_host::scan_child(|| match format.as_str() {
            "clap" => scan::child_reply(soundcraft_clap_host::load_bundle(&path)),
            "vst3" => scan::child_reply(soundcraft_vst3_host::load_bundle(&path)),
            other => scan::child_reply::<(), _>(Err(format!("unknown plugin format `{other}`"))),
        });
    }
    match std::env::current_exe() {
        Ok(program) => scan::in_children(program, cache_dir()),
        Err(e) => log::warn!("plugin scanning stays off: {e}"),
    }
}

fn cache_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches/SoundCraft"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("SoundCraft"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache"))).map(|d| d.join("soundcraft"))
    }
}
