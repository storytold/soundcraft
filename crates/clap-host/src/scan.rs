//! Finding CLAP plugins on disk (safe, pure `std::fs`) and mapping CLAP features to categories.

use soundcraft_dsp::Category;
use std::path::{Path, PathBuf};

/// Directory recursion limit while scanning (CLAP folders may be nested, never deeply).
const MAX_DEPTH: usize = 8;
/// Upper bound on bundles collected by one scan (hostile or huge folders).
const MAX_BUNDLES: usize = 4096;
/// Upper bound on directory entries visited by one scan.
const MAX_ENTRIES: usize = 100_000;

/// The standard CLAP search locations for this OS (existing or not), followed by every entry
/// of the `CLAP_PATH` environment variable.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut v = os_search_paths(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("COMMONPROGRAMFILES").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    );
    if let Some(cp) = std::env::var_os("CLAP_PATH") {
        v.extend(std::env::split_paths(&cp).filter(|p| !p.as_os_str().is_empty()));
    }
    v
}

/// The OS-specific standard locations, given the relevant environment values.
pub fn os_search_paths(home: Option<PathBuf>, common_program_files: Option<PathBuf>, local_app_data: Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(h) = home {
            v.push(h.join("Library/Audio/Plug-Ins/CLAP"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    } else if cfg!(windows) {
        if let Some(c) = common_program_files {
            v.push(c.join("CLAP"));
        }
        if let Some(l) = local_app_data {
            v.push(l.join("Programs").join("Common").join("CLAP"));
        }
    } else {
        if let Some(h) = home {
            v.push(h.join(".clap"));
        }
        v.push(PathBuf::from("/usr/lib/clap"));
        v.push(PathBuf::from("/usr/local/lib/clap"));
    }
    v
}

/// True when the path names a CLAP plugin (`*.clap`, case-insensitive).
pub fn is_clap_path(p: &Path) -> bool {
    p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| e.eq_ignore_ascii_case("clap"))
}

/// Recursively collects `.clap` files and bundles under `dirs` (missing directories are skipped).
/// A `.clap` directory is a bundle and is not descended into. The result is sorted and deduplicated.
pub fn find_bundles(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut budget = MAX_ENTRIES;
    for d in dirs {
        if is_clap_path(d) && d.exists() {
            out.push(d.clone());
        } else {
            walk(d, 0, &mut out, &mut budget);
        }
    }
    out.sort();
    out.dedup();
    out.truncate(MAX_BUNDLES);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, budget: &mut usize) {
    if depth > MAX_DEPTH || out.len() >= MAX_BUNDLES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if *budget == 0 || out.len() >= MAX_BUNDLES {
            return;
        }
        *budget -= 1;
        let p = entry.path();
        // `metadata` follows symlinks (plugins are often symlinked in); errors are skipped.
        let Ok(md) = std::fs::metadata(&p) else { continue };
        if is_clap_path(&p) {
            out.push(p);
        } else if md.is_dir() {
            walk(&p, depth + 1, out, budget);
        }
    }
}

/// The loadable binary of a plugin: the file itself, or for a macOS bundle directory
/// `Contents/MacOS/<name>` (the executable named like the bundle, else the only/first file there).
pub fn bundle_executable(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    let macos = path.join("Contents").join("MacOS");
    if let Some(stem) = path.file_stem() {
        let p = macos.join(stem);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&macos).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file()).take(64).collect();
    files.sort();
    files.into_iter().next()
}

/// Maps CLAP feature strings to a SoundCraft insert-menu category.
pub fn category_from_features<S: AsRef<str>>(features: &[S]) -> Category {
    let has = |names: &[&str]| features.iter().any(|f| names.iter().any(|n| f.as_ref().eq_ignore_ascii_case(n)));
    if has(&["instrument", "synthesizer", "sampler", "drum", "drum-machine"]) {
        Category::Instrument
    } else if has(&["equalizer", "filter"]) {
        Category::Eq
    } else if has(&["compressor", "limiter", "expander", "gate", "de-esser", "transient-shaper"]) {
        Category::Dynamics
    } else if has(&["reverb"]) {
        Category::Reverb
    } else if has(&["delay"]) {
        Category::Delay
    } else if has(&["chorus", "flanger", "phaser", "tremolo"]) {
        Category::Modulation
    } else if has(&["distortion"]) {
        Category::Harmonic
    } else if has(&["pitch-shifter", "pitch-correction"]) {
        Category::PitchShift
    } else {
        Category::Other
    }
}

/// True when the features mark an instrument (CLAP `instrument`).
pub fn is_instrument<S: AsRef<str>>(features: &[S]) -> bool {
    features.iter().any(|f| f.as_ref().eq_ignore_ascii_case("instrument"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-clap-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_files_and_bundles_recursively_but_not_inside_bundles() {
        let d = tmp("find");
        std::fs::write(d.join("a.clap"), b"x").unwrap();
        std::fs::write(d.join("readme.txt"), b"x").unwrap();
        std::fs::create_dir_all(d.join("vendor/sub")).unwrap();
        std::fs::write(d.join("vendor/sub/B.CLAP"), b"x").unwrap();
        std::fs::create_dir_all(d.join("c.clap/Contents/MacOS")).unwrap();
        std::fs::write(d.join("c.clap/Contents/MacOS/c"), b"x").unwrap();
        std::fs::write(d.join("c.clap/Contents/inner.clap"), b"x").unwrap();
        let found = find_bundles(&[d.clone(), d.join("missing")]);
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, vec!["a.clap", "c.clap", "vendor/sub/B.CLAP"]);
        assert_eq!(bundle_executable(&d.join("c.clap")), Some(d.join("c.clap/Contents/MacOS/c")));
        assert_eq!(bundle_executable(&d.join("a.clap")), Some(d.join("a.clap")));
        assert_eq!(bundle_executable(&d.join("nope.clap")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bundle_executable_falls_back_to_first_file() {
        let d = tmp("exe");
        std::fs::create_dir_all(d.join("X.clap/Contents/MacOS")).unwrap();
        std::fs::write(d.join("X.clap/Contents/MacOS/other"), b"x").unwrap();
        assert_eq!(bundle_executable(&d.join("X.clap")), Some(d.join("X.clap/Contents/MacOS/other")));
        std::fs::create_dir_all(d.join("Empty.clap")).unwrap();
        assert_eq!(bundle_executable(&d.join("Empty.clap")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn os_paths_cover_the_standard_locations() {
        let v = os_search_paths(Some("/home/u".into()), Some("C:/CPF".into()), Some("C:/LAD".into()));
        if cfg!(target_os = "macos") {
            assert_eq!(v, vec![PathBuf::from("/home/u/Library/Audio/Plug-Ins/CLAP"), PathBuf::from("/Library/Audio/Plug-Ins/CLAP")]);
        } else if cfg!(windows) {
            assert_eq!(v.len(), 2);
            assert!(v[0].ends_with("CLAP") && v[1].ends_with("CLAP"));
        } else {
            assert_eq!(v, vec![PathBuf::from("/home/u/.clap"), PathBuf::from("/usr/lib/clap"), PathBuf::from("/usr/local/lib/clap")]);
        }
        assert!(default_search_paths().len() >= v.len().saturating_sub(1));
    }

    #[test]
    fn features_map_to_categories() {
        assert_eq!(category_from_features(&["audio-effect", "reverb", "stereo"]), Category::Reverb);
        assert_eq!(category_from_features(&["instrument", "synthesizer"]), Category::Instrument);
        assert_eq!(category_from_features(&["audio-effect", "Compressor"]), Category::Dynamics);
        assert_eq!(category_from_features(&["audio-effect", "equalizer"]), Category::Eq);
        assert_eq!(category_from_features(&["chorus"]), Category::Modulation);
        assert_eq!(category_from_features(&["distortion"]), Category::Harmonic);
        assert_eq!(category_from_features(&["pitch-shifter"]), Category::PitchShift);
        assert_eq!(category_from_features(&["delay"]), Category::Delay);
        assert_eq!(category_from_features::<&str>(&[]), Category::Other);
        assert!(is_instrument(&["instrument"]));
        assert!(!is_instrument(&["audio-effect"]));
    }
}
