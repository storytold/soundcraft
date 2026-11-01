//! Finds JSFX files in REAPER's Effects folders.

use crate::ID_PREFIX;
use std::path::{Path, PathBuf};

/// Directory depth and entry caps keep a scan of a hostile or huge tree bounded.
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 20_000;
/// A JSFX header is tiny; only the start of a file is inspected.
const SNIFF_BYTES: usize = 8 * 1024;

/// One effect found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct JsfxEntry {
    /// SoundCraft id: `jsfx:<path relative to the scanned root>`.
    pub id: String,
    /// The `desc:` line (or the file name when there is none).
    pub name: String,
    /// The top-level folder it lives in ("" for files directly in the root); menus group by it.
    pub vendor: String,
    /// Always `false`: JSFX instruments are not offered as instruments yet.
    pub is_instrument: bool,
    /// Why this host cannot run the effect (an unsupported function, a missing import, ...), set by
    /// the registry's scan; `None` when it compiles.
    pub unsupported: Option<String>,
    pub path: PathBuf,
}

/// SoundCraft's per-user data folder (`SOUNDCRAFT_DATA_DIR` overrides it): `%APPDATA%\SoundCraft`,
/// `~/Library/Application Support/SoundCraft` or `$XDG_DATA_HOME/soundcraft`.
pub fn soundcraft_data_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = env("SOUNDCRAFT_DATA_DIR") {
        return Some(d);
    }
    if cfg!(target_os = "windows") {
        env("APPDATA").map(|p| p.join("SoundCraft"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library/Application Support/SoundCraft"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|p| p.join(".local/share"))).map(|p| p.join("soundcraft"))
    }
}

/// The Effects folder SoundCraft owns: JSFX installed from repositories go here, so REAPER's own
/// folder is never modified. It is always part of the search path.
pub fn soundcraft_effects_dir() -> Option<PathBuf> {
    soundcraft_data_dir().map(|d| d.join("Effects"))
}

/// REAPER's per-user Effects folder for this OS, if its location can be worked out.
pub fn default_effects_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(target_os = "windows") {
        dirs.extend(env("APPDATA").map(|p| p.join("REAPER").join("Effects")));
    } else if cfg!(target_os = "macos") {
        dirs.extend(env("HOME").map(|p| p.join("Library/Application Support/REAPER/Effects")));
    } else {
        dirs.extend(env("XDG_CONFIG_HOME").map(|p| p.join("REAPER/Effects")));
        dirs.extend(env("HOME").map(|p| p.join(".config/REAPER/Effects")));
    }
    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}

/// The `desc:` of a JSFX file's first lines, `None` when the file is not a JSFX effect.
fn sniff(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path).ok()?.take(SNIFF_BYTES as u64).read_to_end(&mut buf).ok()?;
    if buf.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    text.lines().take(60).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("desc").then(|| v.trim().to_string())
    })
}

/// Scans `root` recursively for JSFX effects (symlinks are not followed), sorted by id.
pub fn scan(root: &Path) -> Vec<JsfxEntry> {
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out.sort_by_key(|e| e.id.to_lowercase());
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<JsfxEntry>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk(root, &path, depth + 1, out);
        } else if ft.is_file() {
            // Effects have no extension (or `.jsfx`); everything else is data, includes or art.
            let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), None | Some("jsfx")) {
                continue;
            }
            let Some(desc) = sniff(&path) else { continue };
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let name = if desc.is_empty() { name } else { desc };
            let vendor = rel.split_once('/').map(|(v, _)| v.to_string()).unwrap_or_default();
            out.push(JsfxEntry { id: format!("{ID_PREFIX}{rel}"), name, vendor, is_instrument: false, unsupported: None, path });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_effects_and_skips_data() {
        let root = std::env::temp_dir().join(format!("sc-jsfx-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dynamics")).unwrap();
        std::fs::write(root.join("dynamics/comp"), "desc:My Comp\n@sample\nspl0*=1;\n").unwrap();
        std::fs::write(root.join("plain.jsfx"), "desc:Plain\n").unwrap();
        std::fs::write(root.join("inc.jsfx-inc"), "desc:include\n").unwrap();
        std::fs::write(root.join("pic.png"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("notes"), "just some text\n").unwrap();
        std::fs::write(root.join(".hidden"), "desc:Hidden\n").unwrap();
        let found = scan(&root);
        let ids: Vec<_> = found.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["jsfx:dynamics/comp", "jsfx:plain.jsfx"]);
        assert_eq!(found[0].name, "My Comp");
        assert_eq!((found[0].vendor.as_str(), found[1].vendor.as_str()), ("dynamics", ""));
        let p = crate::JsfxPlugin::from_file(&root, &found[0].path).unwrap();
        assert_eq!(soundcraft_dsp::Plugin::info(&p).id, "jsfx:dynamics/comp");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_root_is_empty() {
        assert!(scan(Path::new("/definitely/not/here")).is_empty());
    }
}
