//! Preflight Audio Unit creation in a disposable process before loading it in SoundCraft.

use crate::AuError;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

static HELPER: OnceLock<PathBuf> = OnceLock::new();
static CACHE: OnceLock<PathBuf> = OnceLock::new();
static VALIDATED: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024;

/// The desktop executable handles `--validate-au id` before app startup.
pub fn configure(executable: PathBuf) {
    let _ = HELPER.set(executable);
}

/// Selects the persistent Audio Unit instance-validation cache.
pub fn configure_cache(path: PathBuf) {
    let _ = CACHE.set(path);
}

fn load_cache() -> HashMap<String, bool> {
    let Some(path) = CACHE.get() else { return HashMap::new() };
    let Ok(file) = std::fs::File::open(path) else { return HashMap::new() };
    if file.metadata().ok().is_none_or(|m| m.len() > MAX_CACHE_BYTES) {
        return HashMap::new();
    }
    serde_json::from_reader(file).unwrap_or_default()
}

fn save_cache(values: &HashMap<String, bool>) {
    use std::sync::atomic::Ordering;
    let Some(path) = CACHE.get() else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = parent.join(format!(".au-validation-{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = std::fs::File::create(&temp)
        .and_then(|mut f| {
            let bytes = serde_json::to_vec(values).map_err(std::io::Error::other)?;
            f.write_all(&bytes)
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
}

/// Checks one component in a child. Crashes, code-signing kills and hangs remain in that child.
pub(crate) fn validate_instance(id: &str, version: &str) -> Result<(), AuError> {
    // Library users do not have the desktop helper available. Preserve the original single
    // in-process instantiation path instead of creating and immediately recreating the AU.
    let Some(exe) = HELPER.get() else { return Ok(()) };
    let key = format!("v1\n{id}\n{version}");
    let validations = VALIDATED.get_or_init(|| Mutex::new(load_cache()));
    if let Some(ok) = validations.lock().unwrap_or_else(PoisonError::into_inner).get(&key).copied() {
        return if ok { Ok(()) } else { Err(failed(id, "failed isolated validation before".into())) };
    }
    let result = validate_with(exe, id, Duration::from_secs(15));
    let mut values = validations.lock().unwrap_or_else(PoisonError::into_inner);
    values.insert(key, result.is_ok());
    save_cache(&values);
    result
}

fn failed(id: &str, message: String) -> AuError {
    AuError::Instantiate(id.to_string(), message)
}

fn validate_with(exe: &Path, id: &str, timeout: Duration) -> Result<(), AuError> {
    let mut child = Command::new(exe)
        .arg("--validate-au")
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| failed(id, e.to_string()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(failed(id, format!("isolated validator crashed or rejected plugin ({status}); skipped"))),
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(id, if other.is_err() { "validator wait failed".into() } else { "validator timed out; plugin skipped".into() }));
            }
        }
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn helper(body: &str) -> (Scratch, PathBuf) {
        let dir = std::env::temp_dir().join(format!("sc-au-validator-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("helper");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (Scratch(dir), path)
    }

    #[test]
    fn crashed_or_hung_validator_is_contained() {
        let (_dir, exe) = helper("kill -KILL $$");
        assert!(validate_with(&exe, "au:aufx:test:test", Duration::from_secs(1)).is_err());
        let (_dir, exe) = helper("exit 0");
        assert!(validate_with(&exe, "au:aufx:test:test", Duration::from_secs(1)).is_ok());
        let (_dir, exe) = helper("exec sleep 30");
        assert!(validate_with(&exe, "au:aufx:test:test", Duration::from_millis(50)).is_err());
    }
}
