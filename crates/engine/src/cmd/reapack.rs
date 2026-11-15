//! ReaPack repositories: search them and install JSFX effects into SoundCraft's own Effects folder
//! (`reapack.*`). The client itself lives in `soundcraft-reapack`. None of these touch the session,
//! so none are journaled or undoable. `refresh`, `add_repo` and `install` download (through the
//! system `curl`, https only) and block until done; the UI runs them on a worker thread.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_reapack::{Manager, ReapackError};

fn convert(id: &str, e: ReapackError) -> EngineError {
    match e {
        ReapackError::Invalid(m) => bad(id, m),
        ReapackError::NotFound(m) => EngineError::NotFound(m),
        ReapackError::Unsupported(m) => EngineError::Unsupported(m),
        other => EngineError::Io(other.to_string()),
    }
}

fn manager(id: &str) -> Result<&'static Manager> {
    soundcraft_reapack::system().map_err(|e| convert(id, e))
}

fn text<'a>(id: &str, p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(id, format!("`{key}` (a non-empty string) is required")))
}

/// The three values that name a package.
fn package<'a>(id: &str, p: &'a Value) -> Result<(&'a str, &'a str, &'a str)> {
    Ok((text(id, p, "repo")?, text(id, p, "category")?, text(id, p, "package")?))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "reapack.repos",
            "List ReaPack Repositories",
            [],
            None,
            "{} → [{name, url, cached: bool, packages: n}]",
            always,
            |_, _| Ok(json!(manager("reapack.repos")?.repos()))
        ),
        cmd!(
            query "reapack.add_repo",
            "Add ReaPack Repository",
            [],
            None,
            "{url: \"https://…/index.xml\"} → {name, url}; downloads the index",
            always,
            |_, p| {
                let r = manager("reapack.add_repo")?.add_repo(text("reapack.add_repo", p, "url")?).map_err(|e| convert("reapack.add_repo", e))?;
                Ok(json!(r))
            }
        ),
        cmd!(
            query "reapack.remove_repo",
            "Remove ReaPack Repository",
            [],
            None,
            "{repo: url or name} → {}; installed packages stay installed",
            always,
            |_, p| {
                manager("reapack.remove_repo")?.remove_repo(text("reapack.remove_repo", p, "repo")?).map_err(|e| convert("reapack.remove_repo", e))?;
                Ok(json!({}))
            }
        ),
        cmd!(
            query "reapack.refresh",
            "Refresh ReaPack Repositories",
            [],
            None,
            "{repo?: url or name} → [{url, name, packages, error}]; downloads the index of one repository or all",
            always,
            |_, p| {
                let only = p.get("repo").and_then(Value::as_str).filter(|s| !s.is_empty());
                let out = manager("reapack.refresh")?.refresh(only).map_err(|e| convert("reapack.refresh", e))?;
                Ok(json!(out))
            }
        ),
        cmd!(
            query "reapack.search",
            "Search ReaPack JSFX",
            [],
            None,
            "{query?: string, repo?: url or name, limit?: number (default 50)} → [{repo, repo_name, category, package, desc, author, version, installed_version, update_available}]; searches the downloaded indexes",
            always,
            |_, p| {
                let query = p.get("query").and_then(Value::as_str).unwrap_or("");
                let repo = p.get("repo").and_then(Value::as_str).filter(|s| !s.is_empty());
                let limit = p.get("limit").and_then(Value::as_u64).map_or(50, |n| n.min(1000) as usize);
                let hits = manager("reapack.search")?.search(query, repo, limit).map_err(|e| convert("reapack.search", e))?;
                Ok(json!(hits))
            }
        ),
        cmd!(
            query "reapack.installed",
            "List Installed ReaPack JSFX",
            [],
            None,
            "{} → [{repo, repo_name, category, package, version, id: \"jsfx:…\", files}]",
            always,
            |_, _| Ok(json!(manager("reapack.installed")?.installed()))
        ),
        cmd!(
            query "reapack.install",
            "Install ReaPack JSFX",
            [],
            None,
            "{repo: url or name, category, package} → {…installed record, id: \"jsfx:…\"}; installs or updates to the newest stable version, then rescans the JSFX list",
            always,
            |_, p| {
                let (repo, category, package) = package("reapack.install", p)?;
                let rec = manager("reapack.install")?.install(repo, category, package).map_err(|e| convert("reapack.install", e))?;
                soundcraft_jsfx_host::rescan();
                Ok(json!(rec))
            }
        ),
        cmd!(
            query "reapack.uninstall",
            "Uninstall ReaPack JSFX",
            [],
            None,
            "{repo: url or name, category, package} → {removed: n files}",
            always,
            |_, p| {
                let (repo, category, package) = package("reapack.uninstall", p)?;
                let n = manager("reapack.uninstall")?.uninstall(repo, category, package).map_err(|e| convert("reapack.uninstall", e))?;
                soundcraft_jsfx_host::rescan();
                Ok(json!({"removed": n}))
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    // These only read, or fail before touching the disk or the network, so they are safe to run
    // against the real per-user folders.

    #[test]
    fn repositories_and_installed_lists_are_arrays() {
        let mut e = Engine::default();
        assert!(e.execute("reapack.repos", &json!({})).unwrap().is_array());
        assert!(e.execute("reapack.installed", &json!({})).unwrap().is_array());
        assert!(e.execute("reapack.search", &json!({"query": "zz-no-such-effect-zz"})).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn bad_arguments_are_rejected_before_any_download() {
        let mut e = Engine::default();
        for (cmd, params) in [
            ("reapack.add_repo", json!({})),
            ("reapack.add_repo", json!({"url": "http://insecure.example/index.xml"})),
            ("reapack.add_repo", json!({"url": "file:///etc/passwd"})),
            ("reapack.install", json!({"repo": "nope"})),
            ("reapack.install", json!({"repo": "nope", "category": "C", "package": "p"})),
            ("reapack.uninstall", json!({"repo": "nope", "category": "C", "package": "p"})),
            ("reapack.remove_repo", json!({"repo": "nope"})),
        ] {
            assert!(e.execute(cmd, &params).is_err(), "{cmd} {params}");
        }
    }
}
