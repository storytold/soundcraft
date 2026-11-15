//! The set of installed JSFX effects: scanned on first use from REAPER's Effects folder, the
//! `JSFX_PATH` folders and any added with [`add_search_dir`], then cached. Lookups by plugin id
//! (`jsfx:<path relative to its Effects folder>`) create instances from the files on disk.

use crate::scan::{JsfxEntry, default_effects_dirs, scan as scan_dir, soundcraft_effects_dir};
use crate::{ID_PREFIX, JsfxPlugin};
use soundcraft_dsp::{Plugin, PluginInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

type Found = Arc<Vec<(PathBuf, JsfxEntry)>>;

struct State {
    dirs: Vec<PathBuf>,
    /// Bumped whenever `dirs` changes, so a scan of the old folders isn't stored.
    generation: u64,
    /// Each effect with the Effects folder its id is relative to; `None` until the first scan.
    /// A rescan replaces it only once done, so lookups keep answering meanwhile.
    found: Option<Found>,
    /// `plugin_info` answers by id, cleared with every new scan (`None`: unknown or fails to load).
    infos: HashMap<String, Option<&'static PluginInfo>>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    let m = STATE.get_or_init(|| {
        // SoundCraft's own folder comes first (it may not exist yet: installs create it), so what
        // was installed from a repository wins over a same-named file in REAPER's folder.
        let mut dirs: Vec<PathBuf> = soundcraft_effects_dir().into_iter().collect();
        dirs.extend(default_effects_dirs());
        if let Some(extra) = std::env::var_os("JSFX_PATH") {
            dirs.extend(std::env::split_paths(&extra).filter(|d| d.is_dir()));
        }
        Mutex::new(State { dirs, generation: 0, found: None, infos: HashMap::new() })
    });
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Scans `dirs` and compiles each effect to see whether it loads. Slow (every file is read and
/// compiled), so it runs without the registry's lock: the Mix window asks it every frame.
fn build(dirs: &[PathBuf]) -> Vec<(PathBuf, JsfxEntry)> {
    let mut all: Vec<(PathBuf, JsfxEntry)> = Vec::new();
    for root in dirs {
        for mut e in scan_dir(root) {
            // An earlier folder wins when two hold the same relative path.
            if !all.iter().any(|(_, x)| x.id == e.id) {
                e.unsupported = JsfxPlugin::check_file(root, &e.path).err().map(|x| x.to_string());
                all.push((root.clone(), e));
            }
        }
    }
    all
}

/// Scans the current folders and stores the result, again if the folders changed meanwhile.
fn scan_and_store() -> Found {
    loop {
        let (dirs, generation) = {
            let st = state();
            (st.dirs.clone(), st.generation)
        };
        let all: Found = Arc::new(build(&dirs));
        let mut st = state();
        if st.generation == generation {
            st.found = Some(Arc::clone(&all));
            st.infos.clear();
            return all;
        }
    }
}

/// The scan, made on first use.
fn scanned() -> Found {
    if let Some(f) = &state().found {
        return Arc::clone(f);
    }
    scan_and_store()
}

/// The JSFX id's path part (`None` for other ids or an empty path).
pub fn parse_id(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX).filter(|s| !s.is_empty() && !s.contains('\0'))
}

/// Every installed JSFX effect, scanned on the first call and cached.
pub fn scan() -> Vec<JsfxEntry> {
    scanned().iter().map(|(_, e)| e.clone()).collect()
}

/// Scans again; lookups answer from the previous scan until this one is done.
pub fn rescan() -> Vec<JsfxEntry> {
    scan_and_store().iter().map(|(_, e)| e.clone()).collect()
}

/// Adds an Effects folder to the search path. Returns how many effects the folder holds.
pub fn add_search_dir(dir: &Path) -> usize {
    let mut st = state();
    if !st.dirs.iter().any(|d| d == dir) {
        st.dirs.push(dir.to_path_buf());
        st.generation += 1;
        st.found = None;
    }
    scan_dir(dir).len()
}

/// Creates an effect by `jsfx:<path>`, prepared for 48 kHz stereo with 1024-frame blocks (like
/// `soundcraft_dsp::create`); `None` if unknown or it fails to load.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    parse_id(id)?;
    let (root, path) = {
        let found = scanned();
        let (r, e) = found.iter().find(|(_, e)| e.id == id)?;
        if let Some(why) = &e.unsupported {
            log::warn!("cannot load {id}: {why}");
            return None;
        }
        (r.clone(), e.path.clone())
    };
    match JsfxPlugin::from_file(&root, &path) {
        Ok(mut p) => {
            p.prepare(48_000.0, 1024, 2);
            Some(Box::new(p))
        }
        Err(e) => {
            log::warn!("cannot load {id}: {e}");
            None
        }
    }
}

/// The description of a `jsfx:<path>` effect. The Mix window asks for it every frame, so it reads
/// only the file's header (no compiling, no `@init`, no effect memory) and is cached per scan; the
/// `PluginInfo` itself is leaked once per file contents (see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    parse_id(id)?;
    if let Some(&cached) = state().infos.get(id) {
        return cached;
    }
    let found = scanned();
    let info = match found.iter().find(|(_, e)| e.id == id) {
        Some((root, e)) if e.unsupported.is_none() => match JsfxPlugin::info_from_file(root, &e.path) {
            Ok(i) => Some(i),
            Err(err) => {
                log::warn!("cannot read {id}: {err}");
                None
            }
        },
        _ => None,
    };
    state().infos.insert(id.to_string(), info);
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        assert_eq!(parse_id("jsfx:dynamics/comp"), Some("dynamics/comp"));
        assert_eq!(parse_id("jsfx:"), None);
        assert_eq!(parse_id("clap:x"), None);
        assert_eq!(parse_id("jsfx:a\0b"), None);
    }

    #[test]
    fn non_jsfx_and_unknown_ids_are_rejected() {
        assert!(create("eq_7band").is_none());
        assert!(create("jsfx:no/such/effect").is_none());
        assert!(plugin_info("clap:x").is_none());
    }

    #[test]
    fn an_added_folder_is_scanned_and_creates_working_plugins() {
        let root = std::env::temp_dir().join(format!("sc-jsfx-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("util")).unwrap();
        std::fs::write(root.join("util/half"), "desc:Half\n@sample\nspl0*=0.5;spl1*=0.5;\n").unwrap();
        assert_eq!(add_search_dir(&root), 1);
        let id = "jsfx:util/half";
        assert!(scan().iter().any(|e| e.id == id && e.vendor == "util"));
        let info = plugin_info(id).unwrap();
        assert_eq!(info.name, "Half");
        let mut p = create(id).unwrap();
        assert!(std::ptr::eq(info, p.info()), "the header-only description is the instance's");
        let mut io = vec![vec![1.0f32; 4], vec![-1.0f32; 4]];
        p.process(&mut io, 4);
        assert_eq!((io[0][0], io[1][3]), (0.5, -0.5));
        // Later calls answer from the cache, without touching the file.
        std::fs::remove_file(root.join("util/half")).unwrap();
        assert!(std::ptr::eq(plugin_info(id).unwrap(), info));
        let _ = std::fs::remove_dir_all(&root);
    }
}
