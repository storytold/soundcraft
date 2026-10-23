//! The on-disk scan cache: what each `.vst3` held the last time it was loaded, keyed by the
//! bundle path plus its binary's size and modification time. A bundle whose binary is unchanged
//! is listed from the cache without loading it, so a launch with a cache loads no plugin code
//! until a plugin is actually used. Bundles that failed to load are remembered too (not retried
//! until their binary changes or the user rescans). Safe, pure `std::fs` + serde.

use crate::Vst3Descriptor;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Bumped whenever the entry layout or the meaning of a descriptor changes; other versions are ignored.
const VERSION: u32 = 1;
/// Largest cache file read (a hostile or corrupt file must not exhaust memory).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The size and modification time of a bundle's binary (what decides whether it changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub modified_ns: u64,
}

impl Stamp {
    /// The stamp of `bundle`'s binary; `None` when it has none or cannot be read.
    pub fn of(bundle: &Path) -> Option<Stamp> {
        let exe = crate::scan::bundle_binary(bundle)?;
        let md = std::fs::metadata(exe).ok()?;
        let modified = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Stamp { size: md.len(), modified_ns: u64::try_from(modified.as_nanos()).unwrap_or(u64::MAX) })
    }
}

/// One bundle as it was last loaded: its plugin classes, or why it would not load.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub descriptors: Vec<Vst3Descriptor>,
    pub error: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct File {
    version: u32,
    entries: Vec<Entry>,
}

/// Reads a cache file; empty when it is missing, too big, unreadable or another version.
pub fn read(path: &Path) -> HashMap<PathBuf, Entry> {
    let ok = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES);
    let parsed = ok.then(|| std::fs::read(path).ok()).flatten().and_then(|b| serde_json::from_slice::<File>(&b).ok());
    match parsed {
        Some(f) if f.version == VERSION => f.entries.into_iter().map(|e| (e.path.clone(), e)).collect(),
        _ => HashMap::new(),
    }
}

/// Writes the cache atomically (a temporary file renamed over the old one).
pub fn write(path: &Path, entries: Vec<Entry>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(&File { version: VERSION, entries }).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::Category;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-vst3-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(path: &Path) -> Entry {
        Entry {
            path: path.to_path_buf(),
            stamp: Stamp { size: 3, modified_ns: 7 },
            descriptors: vec![Vst3Descriptor {
                id: "vst3:00".into(),
                class_id: "00".into(),
                name: "Bass".into(),
                vendor: "V".into(),
                version: "1".into(),
                sdk_version: "VST 3.7".into(),
                sub_categories: vec!["Instrument".into()],
                category: Category::Instrument,
                is_instrument: true,
                path: path.to_path_buf(),
            }],
            error: None,
        }
    }

    #[test]
    fn round_trips() {
        let d = tmp("rt");
        let f = d.join("sub").join("vst3-cache.json");
        let e = entry(&d.join("Bass.vst3"));
        write(&f, vec![e.clone()]).unwrap();
        let back = read(&f);
        assert_eq!(back.get(&e.path), Some(&e));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn junk_missing_and_other_versions_read_empty() {
        let d = tmp("junk");
        assert!(read(&d.join("missing.json")).is_empty());
        std::fs::write(d.join("junk.json"), b"{not json").unwrap();
        assert!(read(&d.join("junk.json")).is_empty());
        std::fs::write(d.join("v0.json"), br#"{"version":0,"entries":[]}"#).unwrap();
        assert!(read(&d.join("v0.json")).is_empty());
        assert!(read(&d).is_empty(), "a directory is not a cache file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stamp_follows_the_binary() {
        let d = tmp("stamp");
        let b = d.join("one.vst3");
        assert_eq!(Stamp::of(&b), None);
        std::fs::write(&b, b"abc").unwrap();
        let s = Stamp::of(&b).unwrap();
        assert_eq!(s.size, 3);
        std::fs::write(&b, b"abcdef").unwrap();
        assert_ne!(Stamp::of(&b), Some(s));
        let _ = std::fs::remove_dir_all(&d);
    }
}
