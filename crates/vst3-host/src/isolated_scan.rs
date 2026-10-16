//! Desktop discovery runs each untrusted module in a disposable child process.
use crate::{Vst3Descriptor, Vst3Error};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

static HELPER: OnceLock<PathBuf> = OnceLock::new();
static CACHE: OnceLock<PathBuf> = OnceLock::new();
static VALIDATION_CACHE: OnceLock<PathBuf> = OnceLock::new();
static VALIDATED: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static SKIPPED: AtomicU64 = AtomicU64::new(0);
const CACHE_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 16 * 1024 * 1024;

/// Bundles skipped by the isolated scanner in this app run.
pub fn skipped_count() -> u64 {
    SKIPPED.load(Ordering::Relaxed)
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// The desktop executable handles `--scan-vst3 bundle output.json` before app startup.
pub fn configure(executable: PathBuf) {
    let _ = HELPER.set(executable);
}

/// Selects the persistent discovery cache written by the desktop host.
pub fn configure_cache(path: PathBuf) {
    let _ = CACHE.set(path);
}

/// Selects the persistent per-instance safety-validation cache.
pub fn configure_validation_cache(path: PathBuf) {
    let _ = VALIDATION_CACHE.set(path);
}

fn validation_key(path: &Path, id: &str) -> String {
    let stamp = bundle_stamp(path);
    format!("v1\n{}\n{id}\n{stamp:?}", path.display())
}

fn load_validations() -> HashMap<String, bool> {
    let Some(path) = VALIDATION_CACHE.get() else { return HashMap::new() };
    let Ok(file) = std::fs::File::open(path) else { return HashMap::new() };
    if file.metadata().ok().is_none_or(|m| m.len() > MAX_CACHE_BYTES) {
        return HashMap::new();
    }
    serde_json::from_reader(file).unwrap_or_default()
}

fn save_validations(values: &HashMap<String, bool>) {
    let Some(path) = VALIDATION_CACHE.get() else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = parent.join(format!(".plugin-validation-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = std::fs::File::create(&temp)
        .and_then(|mut f| serde_json::to_writer(&mut f, values).map_err(std::io::Error::other))
        .and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
}

/// Instantiates one plugin class in a disposable child before the host loads it. A plug-in that
/// aborts, hangs, or is killed by macOS code-signing enforcement is skipped for this app run.
pub(crate) fn validate_instance(path: &Path, id: &str) -> Result<(), Vst3Error> {
    let Some(exe) = HELPER.get() else {
        return crate::registry::validate_bundle_instance(path, id);
    };
    let key = validation_key(path, id);
    let validations = VALIDATED.get_or_init(|| Mutex::new(load_validations()));
    if let Some(ok) = validations.lock().unwrap_or_else(PoisonError::into_inner).get(&key).copied() {
        return if ok { Ok(()) } else { Err(Vst3Error::Instantiate(id.to_string(), "failed isolated validation before".into())) };
    }
    let validation = validate_with(exe, path, id, Duration::from_secs(15));
    if let Some(ok) = validation.cache {
        let mut values = validations.lock().unwrap_or_else(PoisonError::into_inner);
        values.insert(key, ok);
        save_validations(&values);
    }
    validation.result
}

struct Validation {
    result: Result<(), Vst3Error>,
    /// Successful validation and a definite child rejection are stable for this bundle stamp.
    /// Host errors and timeouts are transient and must be retried on the next scan.
    cache: Option<bool>,
}

fn validate_with(exe: &Path, path: &Path, id: &str, timeout: Duration) -> Validation {
    let fail = |message: String| Vst3Error::Instantiate(id.to_string(), message);
    let mut child =
        match Command::new(exe).arg("--validate-vst3").arg(path).arg(id).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => child,
            Err(e) => return Validation { result: Err(fail(e.to_string())), cache: None },
        };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Validation { result: Ok(()), cache: Some(true) },
            Ok(Some(status)) => {
                return Validation {
                    result: Err(fail(format!("isolated validator crashed or rejected plugin ({status}); skipped"))),
                    cache: Some(false),
                };
            }
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Validation {
                    result: Err(fail(if other.is_err() { "validator wait failed".into() } else { "validator timed out; plugin skipped".into() })),
                    cache: None,
                };
            }
        }
    }
}

pub(crate) fn clear_failed_validations() {
    let validations = VALIDATED.get_or_init(|| Mutex::new(load_validations()));
    let mut values = validations.lock().unwrap_or_else(PoisonError::into_inner);
    values.retain(|_, ok| *ok);
    save_validations(&values);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BundleStamp {
    bytes: u64,
    modified_secs: u64,
    modified_nanos: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBundle {
    path: PathBuf,
    stamp: BundleStamp,
    descriptors: Vec<Vst3Descriptor>,
    failed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScanCache {
    version: u32,
    bundles: Vec<CachedBundle>,
}

fn bundle_stamp(path: &Path) -> Option<BundleStamp> {
    let subject = crate::scan::bundle_binary(path).unwrap_or_else(|| path.to_path_buf());
    let metadata = std::fs::metadata(subject).ok()?;
    let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(BundleStamp { bytes: metadata.len(), modified_secs: modified.as_secs(), modified_nanos: modified.subsec_nanos() })
}

fn load_cache(path: &Path) -> Option<ScanCache> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_CACHE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    let cache: ScanCache = serde_json::from_slice(&bytes).ok()?;
    if cache.version != CACHE_VERSION || cache.bundles.len() > 16_384 {
        return None;
    }
    for cached in &cache.bundles {
        if cached.descriptors.len() > 4096 || cached.descriptors.iter().any(|d| d.path != cached.path || crate::parse_id(&d.id).is_none()) {
            return None;
        }
    }
    Some(cache)
}

fn read_cache(path: &Path, bundles: &[PathBuf]) -> Option<ScanCache> {
    let cache = load_cache(path)?;
    if cache.bundles.len() != bundles.len() {
        return None;
    }
    for (cached, current) in cache.bundles.iter().zip(bundles) {
        if &cached.path != current || bundle_stamp(current).as_ref() != Some(&cached.stamp) {
            return None;
        }
    }
    Some(cache)
}

fn write_cache(path: &Path, cache: &ScanCache) -> std::io::Result<()> {
    let Some(parent) = path.parent() else { return Ok(()) };
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(cache).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(std::io::Error::other("VST3 scan cache is too large"));
    }
    let temp = parent.join(format!(".vst3-scan-cache-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

fn flatten(cache: &ScanCache) -> Vec<Vst3Descriptor> {
    let mut out = Vec::new();
    for bundle in &cache.bundles {
        for descriptor in &bundle.descriptors {
            if !out.iter().any(|existing: &Vst3Descriptor| existing.id == descriptor.id) {
                out.push(descriptor.clone());
            }
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    out
}

fn admitted(cache: &ScanCache) -> Vec<Vst3Descriptor> {
    let mut descriptors = flatten(cache);
    if HELPER.get().is_some() {
        descriptors.retain(|descriptor| match validate_instance(&descriptor.path, &descriptor.id) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("{e}");
                false
            }
        });
    }
    descriptors
}

fn refresh_cache<F>(bundles: Vec<PathBuf>, previous: Option<&ScanCache>, force: bool, mut probe_bundle: F) -> ScanCache
where
    F: FnMut(&Path) -> Result<Vec<Vst3Descriptor>, Vst3Error>,
{
    let mut cached = Vec::with_capacity(bundles.len());
    for path in bundles {
        let stamp = bundle_stamp(&path).unwrap_or(BundleStamp { bytes: 0, modified_secs: 0, modified_nanos: 0 });
        if !force && let Some(old) = previous.and_then(|cache| cache.bundles.iter().find(|old| old.path == path && old.stamp == stamp)) {
            cached.push(old.clone());
            continue;
        }
        let (descriptors, failed) = match probe_bundle(&path) {
            Ok(found) => (found, false),
            Err(e) => {
                log::warn!("{e}");
                (Vec::new(), true)
            }
        };
        cached.push(CachedBundle { path, stamp, descriptors, failed });
    }
    ScanCache { version: CACHE_VERSION, bundles: cached }
}

/// Reuses unchanged entries from the on-disk catalog and probes only new or changed bundles.
pub(crate) fn scan_cached(dirs: &[PathBuf], force: bool) -> Vec<Vst3Descriptor> {
    let bundles = crate::scan::find_bundles(dirs);
    if !force
        && let Some(path) = CACHE.get()
        && let Some(cache) = read_cache(path, &bundles)
    {
        SKIPPED.store(cache.bundles.iter().filter(|b| b.failed).count() as u64, Ordering::Relaxed);
        return admitted(&cache);
    }

    let previous = CACHE.get().and_then(|path| load_cache(path));
    let cache = refresh_cache(bundles, previous.as_ref(), force, probe);
    SKIPPED.store(cache.bundles.iter().filter(|bundle| bundle.failed).count() as u64, Ordering::Relaxed);
    if let Some(path) = CACHE.get()
        && let Err(e) = write_cache(path, &cache)
    {
        log::warn!("could not save VST3 scan cache {}: {e}", path.display());
    }
    admitted(&cache)
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn probe(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    // Library users without a configured helper retain their explicit in-process API.
    let Some(exe) = HELPER.get() else {
        return crate::load_bundle(path);
    };
    let result = probe_with(exe, path, Duration::from_secs(10));
    if result.is_err() {
        SKIPPED.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn probe_with(exe: &Path, path: &Path, timeout: Duration) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    let fail = |message: String| Vst3Error::Load(path.display().to_string(), message);
    let dir = std::env::temp_dir().join(format!("soundcraft-vst3-probe-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&dir).map_err(|e| fail(e.to_string()))?;
    let scratch = Scratch(dir);
    let output = scratch.0.join("plugins.json");
    let mut child = Command::new(exe)
        .arg("--scan-vst3")
        .arg(path)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| fail(e.to_string()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(if other.is_err() { "scanner wait failed".into() } else { "scanner timed out; plugin skipped".into() }));
            }
        }
    };
    if !status.success() {
        return Err(fail(format!("scanner crashed or rejected plugin ({status}); skipped")));
    }
    let file = std::fs::File::open(&output).map_err(|e| fail(e.to_string()))?;
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| fail(e.to_string()))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(fail("scanner response too large".into()));
    }
    let ds: Vec<Vst3Descriptor> = serde_json::from_slice(&bytes).map_err(|e| fail(e.to_string()))?;
    if ds.len() > 4096 || ds.iter().any(|d| d.path != path || crate::parse_id(&d.id).is_none()) {
        return Err(fail("invalid scanner descriptors".into()));
    }
    Ok(ds)
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn helper(body: &str) -> (Scratch, PathBuf) {
        let d = std::env::temp_dir().join(format!("sc-scan-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&d).unwrap();
        let p = d.join("helper");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        (Scratch(d), p)
    }
    #[test]
    fn crashed_scanner_does_not_crash_host() {
        let (_d, exe) = helper("kill -KILL $$");
        assert!(probe_with(&exe, Path::new("broken.vst3"), Duration::from_secs(1)).is_err());
        let (_d, exe) = helper("printf '[]' > \"$3\"");
        assert!(probe_with(&exe, Path::new("healthy.vst3"), Duration::from_secs(1)).unwrap().is_empty());
    }
    #[test]
    fn hung_scanner_is_terminated() {
        let (_d, exe) = helper("exec sleep 30");
        let start = Instant::now();
        assert!(probe_with(&exe, Path::new("hung.vst3"), Duration::from_millis(50)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn malformed_scanner_output_is_rejected() {
        let (_d, exe) = helper("printf 'garbage' > \"$3\"");
        assert!(probe_with(&exe, Path::new("bad.vst3"), Duration::from_secs(1)).is_err());
    }

    #[test]
    fn crashed_or_hung_instance_validator_is_contained() {
        let (_d, exe) = helper("kill -KILL $$");
        let crashed = validate_with(&exe, Path::new("broken.vst3"), "vst3:00000000000000000000000000000000", Duration::from_secs(1));
        assert!(crashed.result.is_err());
        assert_eq!(crashed.cache, Some(false));
        let (_d, exe) = helper("exit 0");
        let healthy = validate_with(&exe, Path::new("healthy.vst3"), "vst3:00000000000000000000000000000000", Duration::from_secs(1));
        assert!(healthy.result.is_ok());
        assert_eq!(healthy.cache, Some(true));
        let (_d, exe) = helper("exec sleep 30");
        let hung = validate_with(&exe, Path::new("hung.vst3"), "vst3:00000000000000000000000000000000", Duration::from_millis(50));
        assert!(hung.result.is_err());
        assert_eq!(hung.cache, None);
    }

    #[test]
    fn disk_cache_is_used_only_while_bundle_is_unchanged() {
        let dir = std::env::temp_dir().join(format!("sc-cache-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let scratch = Scratch(dir.clone());
        let bundle = dir.join("Test.vst3");
        let cache_path = dir.join("cache.json");
        std::fs::write(&bundle, b"one").unwrap();
        let cache = ScanCache {
            version: CACHE_VERSION,
            bundles: vec![CachedBundle { path: bundle.clone(), stamp: bundle_stamp(&bundle).unwrap(), descriptors: Vec::new(), failed: true }],
        };
        write_cache(&cache_path, &cache).unwrap();
        assert!(read_cache(&cache_path, std::slice::from_ref(&bundle)).is_some());
        std::fs::write(&bundle, b"changed plugin binary").unwrap();
        assert!(read_cache(&cache_path, std::slice::from_ref(&bundle)).is_none());
        drop(scratch);
    }

    #[test]
    fn first_scan_and_new_bundles_are_probed_without_force() {
        let dir = std::env::temp_dir().join(format!("sc-refresh-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let scratch = Scratch(dir.clone());
        let first = dir.join("First.vst3");
        let second = dir.join("Second.vst3");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let mut probed = Vec::new();
        let initial = refresh_cache(vec![first.clone()], None, false, |path| {
            probed.push(path.to_path_buf());
            Ok(Vec::new())
        });
        assert_eq!(probed, vec![first.clone()]);

        probed.clear();
        let _updated = refresh_cache(vec![first, second.clone()], Some(&initial), false, |path| {
            probed.push(path.to_path_buf());
            Ok(Vec::new())
        });
        assert_eq!(probed, vec![second]);
        drop(scratch);
    }
}
