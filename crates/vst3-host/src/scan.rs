//! Finding VST3 plugins on disk (safe, pure `std::fs`), resolving bundles to their binaries and
//! mapping VST3 sub-categories to SoundCraft categories.

use soundcraft_dsp::Category;
use std::path::{Path, PathBuf};

/// Directory recursion limit while scanning (vendors nest folders, never deeply).
const MAX_DEPTH: usize = 8;
/// Upper bound on bundles collected by one scan (hostile or huge folders).
const MAX_BUNDLES: usize = 4096;
/// Upper bound on directory entries visited by one scan.
const MAX_ENTRIES: usize = 100_000;

/// The operating systems whose VST3 bundle layouts we know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
    /// Linux and the BSDs (same bundle layout).
    Unix,
}

impl Os {
    /// The OS this build runs on.
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Unix
        }
    }
}

/// The standard VST3 search locations for this OS (existing or not), followed by every entry of
/// the `VST3_PATH` environment variable.
pub fn default_search_paths() -> Vec<PathBuf> {
    let mut v = os_search_paths(
        Os::current(),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("COMMONPROGRAMFILES").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
    );
    if let Some(cp) = std::env::var_os("VST3_PATH") {
        v.extend(std::env::split_paths(&cp).filter(|p| !p.as_os_str().is_empty()));
    }
    v
}

/// The OS-specific standard locations, given the relevant environment values.
pub fn os_search_paths(os: Os, home: Option<PathBuf>, common_program_files: Option<PathBuf>, local_app_data: Option<PathBuf>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    match os {
        Os::MacOs => {
            if let Some(h) = home {
                v.push(h.join("Library/Audio/Plug-Ins/VST3"));
            }
            v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        }
        Os::Windows => {
            if let Some(c) = common_program_files {
                v.push(c.join("VST3"));
            }
            if let Some(l) = local_app_data {
                v.push(l.join("Programs").join("Common").join("VST3"));
            }
        }
        Os::Unix => {
            if let Some(h) = home {
                v.push(h.join(".vst3"));
            }
            v.push(PathBuf::from("/usr/lib/vst3"));
            v.push(PathBuf::from("/usr/local/lib/vst3"));
        }
    }
    v
}

/// True when the path names a VST3 plugin (`*.vst3`, case-insensitive).
pub fn is_vst3_path(p: &Path) -> bool {
    p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
}

/// Recursively collects `.vst3` bundles and files under `dirs` (missing directories are skipped).
/// A `.vst3` directory is a bundle and is not descended into. The result is sorted and deduplicated.
pub fn find_bundles(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut budget = MAX_ENTRIES;
    for d in dirs {
        if is_vst3_path(d) && d.exists() {
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
        if is_vst3_path(&p) {
            out.push(p);
        } else if md.is_dir() {
            walk(&p, depth + 1, out, budget);
        }
    }
}

/// The `Contents/<dir>` folder names that hold this machine's binaries, most specific first.
fn arch_dirs(os: Os) -> &'static [&'static str] {
    match os {
        Os::MacOs => &["MacOS"],
        Os::Windows => {
            if cfg!(target_arch = "aarch64") {
                &["arm64-win", "arm64ec-win", "arm64x-win"]
            } else if cfg!(target_arch = "x86") {
                &["x86-win"]
            } else {
                &["x86_64-win"]
            }
        }
        Os::Unix => {
            if cfg!(target_arch = "aarch64") {
                &["aarch64-linux"]
            } else if cfg!(target_arch = "x86") {
                &["i386-linux"]
            } else {
                &["x86_64-linux"]
            }
        }
    }
}

/// The loadable binary of a plugin for this OS: a single-file `.vst3` itself, or inside a bundle
/// directory `Contents/MacOS/<name>` (macOS), `Contents/x86_64-win/<name>.vst3` (Windows) or
/// `Contents/x86_64-linux/<name>.so` (Linux/BSD), falling back to the only/first fitting file there.
pub fn bundle_binary(path: &Path) -> Option<PathBuf> {
    bundle_binary_for(Os::current(), path)
}

/// [`bundle_binary`] for a given OS layout (testable everywhere).
pub fn bundle_binary_for(os: Os, path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if !path.is_dir() {
        return None;
    }
    let stem = path.file_stem()?;
    let ext = match os {
        Os::MacOs => None,
        Os::Windows => Some("vst3"),
        Os::Unix => Some("so"),
    };
    for dir in arch_dirs(os) {
        let folder = path.join("Contents").join(dir);
        let mut named = folder.join(stem);
        if let Some(e) = ext {
            named.set_extension(e);
        }
        if named.is_file() {
            return Some(named);
        }
        let Ok(rd) = std::fs::read_dir(&folder) else { continue };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| ext.is_none_or(|e| p.extension().and_then(std::ffi::OsStr::to_str).is_some_and(|x| x.eq_ignore_ascii_case(e))))
            .take(64)
            .collect();
        files.sort();
        if let Some(f) = files.into_iter().next() {
            return Some(f);
        }
    }
    None
}

/// The `|`-separated VST3 sub-category tokens (`"Fx|Reverb"` → `["Fx", "Reverb"]`).
pub fn split_sub_categories(s: &str) -> Vec<String> {
    s.split('|').map(str::trim).filter(|t| !t.is_empty()).take(32).map(str::to_string).collect()
}

/// Maps VST3 sub-categories (`Fx|EQ`, `Instrument|Synth`, …) to a SoundCraft insert-menu category.
pub fn category_from_sub_categories<S: AsRef<str>>(subs: &[S]) -> Category {
    let has = |names: &[&str]| subs.iter().any(|f| names.iter().any(|n| f.as_ref().eq_ignore_ascii_case(n)));
    if is_instrument(subs) {
        Category::Instrument
    } else if has(&["EQ", "Filter"]) {
        Category::Eq
    } else if has(&["Dynamics"]) {
        Category::Dynamics
    } else if has(&["Reverb"]) {
        Category::Reverb
    } else if has(&["Delay"]) {
        Category::Delay
    } else if has(&["Modulation"]) {
        Category::Modulation
    } else if has(&["Distortion"]) {
        Category::Harmonic
    } else if has(&["Pitch Shift"]) {
        Category::PitchShift
    } else {
        Category::Other
    }
}

/// True when the sub-categories mark an instrument (`Instrument`, `Instrument|Synth`, …).
pub fn is_instrument<S: AsRef<str>>(subs: &[S]) -> bool {
    subs.first().is_some_and(|f| f.as_ref().eq_ignore_ascii_case("Instrument"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_files_and_bundles_recursively_but_not_inside_bundles() {
        let d = tmp("find");
        std::fs::write(d.join("a.vst3"), b"x").unwrap();
        std::fs::write(d.join("readme.txt"), b"x").unwrap();
        std::fs::create_dir_all(d.join("vendor/sub")).unwrap();
        std::fs::write(d.join("vendor/sub/B.VST3"), b"x").unwrap();
        std::fs::create_dir_all(d.join("c.vst3/Contents/MacOS")).unwrap();
        std::fs::write(d.join("c.vst3/Contents/MacOS/c"), b"x").unwrap();
        std::fs::write(d.join("c.vst3/Contents/inner.vst3"), b"x").unwrap();
        let found = find_bundles(&[d.clone(), d.join("missing")]);
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, vec!["a.vst3", "c.vst3", "vendor/sub/B.VST3"]);
        assert_eq!(find_bundles(&[d.join("a.vst3")]), vec![d.join("a.vst3")]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bundles_resolve_per_os_layout() {
        let d = tmp("layout");
        // macOS
        std::fs::create_dir_all(d.join("M.vst3/Contents/MacOS")).unwrap();
        std::fs::write(d.join("M.vst3/Contents/MacOS/M"), b"x").unwrap();
        std::fs::write(d.join("M.vst3/Contents/MacOS/aaa"), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::MacOs, &d.join("M.vst3")), Some(d.join("M.vst3/Contents/MacOS/M")));
        // macOS fallback: first file when the name differs.
        std::fs::create_dir_all(d.join("N.vst3/Contents/MacOS")).unwrap();
        std::fs::write(d.join("N.vst3/Contents/MacOS/other"), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::MacOs, &d.join("N.vst3")), Some(d.join("N.vst3/Contents/MacOS/other")));
        // Windows
        let win = arch_dirs(Os::Windows)[0];
        std::fs::create_dir_all(d.join(format!("W.vst3/Contents/{win}"))).unwrap();
        std::fs::write(d.join(format!("W.vst3/Contents/{win}/W.vst3")), b"x").unwrap();
        std::fs::write(d.join(format!("W.vst3/Contents/{win}/helper.dll")), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::Windows, &d.join("W.vst3")), Some(d.join(format!("W.vst3/Contents/{win}/W.vst3"))));
        // Linux
        let lin = arch_dirs(Os::Unix)[0];
        std::fs::create_dir_all(d.join(format!("L.vst3/Contents/{lin}"))).unwrap();
        std::fs::write(d.join(format!("L.vst3/Contents/{lin}/notes.txt")), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::Unix, &d.join("L.vst3")), None, "only .so files qualify");
        std::fs::write(d.join(format!("L.vst3/Contents/{lin}/Renamed.so")), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::Unix, &d.join("L.vst3")), Some(d.join(format!("L.vst3/Contents/{lin}/Renamed.so"))));
        std::fs::write(d.join(format!("L.vst3/Contents/{lin}/L.so")), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::Unix, &d.join("L.vst3")), Some(d.join(format!("L.vst3/Contents/{lin}/L.so"))));
        // Single-file plugin, wrong layout, missing.
        std::fs::write(d.join("single.vst3"), b"x").unwrap();
        assert_eq!(bundle_binary_for(Os::Windows, &d.join("single.vst3")), Some(d.join("single.vst3")));
        assert_eq!(bundle_binary_for(Os::Windows, &d.join("M.vst3")), None);
        assert_eq!(bundle_binary_for(Os::MacOs, &d.join("nope.vst3")), None);
        std::fs::create_dir_all(d.join("Empty.vst3")).unwrap();
        assert_eq!(bundle_binary(&d.join("Empty.vst3")), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn os_paths_cover_the_standard_locations() {
        let mac = os_search_paths(Os::MacOs, Some("/Users/u".into()), None, None);
        assert_eq!(mac, vec![PathBuf::from("/Users/u/Library/Audio/Plug-Ins/VST3"), PathBuf::from("/Library/Audio/Plug-Ins/VST3")]);
        let win = os_search_paths(Os::Windows, None, Some("C:/CPF".into()), Some("C:/LAD".into()));
        assert_eq!(win[0], PathBuf::from("C:/CPF").join("VST3"));
        assert_eq!(win.len(), 2);
        let unix = os_search_paths(Os::Unix, Some("/home/u".into()), None, None);
        assert_eq!(unix, vec![PathBuf::from("/home/u/.vst3"), PathBuf::from("/usr/lib/vst3"), PathBuf::from("/usr/local/lib/vst3")]);
        assert!(!default_search_paths().is_empty());
    }

    #[test]
    fn sub_categories_map_to_categories() {
        let c = |s: &str| category_from_sub_categories(&split_sub_categories(s));
        assert_eq!(c("Fx|Reverb"), Category::Reverb);
        assert_eq!(c("Instrument|Synth"), Category::Instrument);
        assert_eq!(c("Fx|Dynamics"), Category::Dynamics);
        assert_eq!(c("Fx|EQ"), Category::Eq);
        assert_eq!(c("Fx|Filter"), Category::Eq);
        assert_eq!(c("Fx|Modulation"), Category::Modulation);
        assert_eq!(c("Fx|Distortion"), Category::Harmonic);
        assert_eq!(c("Fx|Pitch Shift"), Category::PitchShift);
        assert_eq!(c("Fx|Delay"), Category::Delay);
        assert_eq!(c("Fx"), Category::Other);
        assert_eq!(c(""), Category::Other);
        assert_eq!(split_sub_categories(" Fx || Delay |"), vec!["Fx", "Delay"]);
        assert!(is_instrument(&split_sub_categories("Instrument|Sampler")));
        assert!(!is_instrument(&split_sub_categories("Fx|Instrument")));
    }
}
