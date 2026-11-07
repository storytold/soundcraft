//! Repositories, search and install/uninstall, on top of a [`Fetch`] and two folders: where the
//! client keeps its state (repository list, cached indexes, what is installed) and the Effects
//! folder the files are installed into.

use crate::index::{self, Index};
use crate::{Fetch, ReapackError, folder_name, safe_rel, sha256, valid_url};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

type R<T> = Result<T, ReapackError>;

/// The REAPER community's public JSFX repository, offered out of the box (nothing is downloaded
/// until the user refreshes it).
pub const DEFAULT_REPO_URL: &str = "https://github.com/ReaTeam/JSFX/raw/master/index.xml";

const MAX_INDEX: usize = 32 << 20;
const MAX_FILE: usize = 8 << 20;
const MAX_TOTAL: usize = 64 << 20;
const MAX_FILES: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepoInfo {
    pub name: String,
    pub url: String,
    /// An index for it has been downloaded.
    pub cached: bool,
    pub packages: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefreshOutcome {
    pub url: String,
    pub name: String,
    pub packages: usize,
    pub error: Option<String>,
}

/// A search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    /// The repository URL (what `install` takes).
    pub repo: String,
    pub repo_name: String,
    pub category: String,
    pub package: String,
    pub desc: String,
    pub author: String,
    pub version: String,
    pub installed_version: Option<String>,
    pub update_available: bool,
}

/// An installed package and the files it put in the Effects folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub repo: String,
    pub repo_name: String,
    pub category: String,
    pub package: String,
    pub version: String,
    /// The effect to insert: `jsfx:<path relative to the Effects folder>`.
    pub id: String,
    /// Every file, relative to the Effects folder, with `/` separators.
    pub files: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct RepoFile {
    repos: Vec<Repo>,
}

#[derive(Default, Serialize, Deserialize)]
struct InstalledFile {
    installed: Vec<Installed>,
}

pub struct Manager {
    state: PathBuf,
    effects: PathBuf,
    fetch: Box<dyn Fetch>,
    lock: Mutex<()>,
}

/// Platform names a `<source platform>` may carry for this build.
fn platform_tags() -> &'static [&'static str] {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        &["all", "windows", "win64"]
    } else if cfg!(target_os = "windows") {
        &["all", "windows", "win32"]
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        &["all", "darwin", "darwin-arm64"]
    } else if cfg!(target_os = "macos") {
        &["all", "darwin", "darwin64"]
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        &["all", "linux", "linux64"]
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        &["all", "linux", "linux-aarch64"]
    } else if cfg!(target_os = "linux") {
        &["all", "linux"]
    } else {
        &["all"]
    }
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn io<T>(r: std::io::Result<T>, what: &str) -> R<T> {
    r.map_err(|e| ReapackError::Io(format!("{what}: {e}")))
}

/// Writes `bytes` to `path` through a temporary file in the same folder.
fn write_atomic(path: &Path, bytes: &[u8]) -> R<()> {
    if let Some(dir) = path.parent() {
        io(std::fs::create_dir_all(dir), "creating a folder")?;
    }
    let tmp = tmp_path(path);
    io(std::fs::write(&tmp, bytes), "writing a file")?;
    io(std::fs::rename(&tmp, path), "replacing a file").inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".sctmp");
    path.with_file_name(name)
}

/// Where a file this install replaces is kept while the install is in flight (same folder, so the
/// move back is a rename on the same volume).
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".scbak");
    path.with_file_name(name)
}

/// Files an install has moved out of the way, so a failure half way puts the folder back exactly
/// as it was: the old version's files, and the ones the new version no longer has.
#[derive(Default)]
struct Aside {
    /// `(backup, original)`: `original` was renamed to `backup`.
    moved: Vec<(PathBuf, PathBuf)>,
    /// Files this install put in place; undoing removes them before restoring the backups.
    placed: Vec<PathBuf>,
}

impl Aside {
    /// Moves `dest` aside when it exists, then puts `tmp` in its place.
    fn swap_in(&mut self, tmp: &Path, dest: &Path) -> R<()> {
        self.move_aside(dest)?;
        io(std::fs::rename(tmp, dest), &format!("installing {}", dest.display()))?;
        self.placed.push(dest.to_path_buf());
        Ok(())
    }

    /// Renames a file aside; a file that is not there is nothing to keep.
    fn move_aside(&mut self, path: &Path) -> R<()> {
        if !path.exists() {
            return Ok(());
        }
        let backup = backup_path(path);
        io(std::fs::rename(path, &backup), &format!("replacing {}", path.display()))?;
        self.moved.push((backup, path.to_path_buf()));
        Ok(())
    }

    /// Undoes the install: removes the new files, then puts the old ones back.
    fn restore(self) {
        for path in &self.placed {
            let _ = std::fs::remove_file(path);
        }
        for (backup, original) in &self.moved {
            let _ = std::fs::rename(backup, original);
        }
    }

    /// The install succeeded: the files kept aside are no longer needed.
    fn discard(self) {
        for (backup, _) in &self.moved {
            let _ = std::fs::remove_file(backup);
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> R<()> {
    let bytes = serde_json::to_vec_pretty(v).map_err(|e| ReapackError::Io(e.to_string()))?;
    write_atomic(path, &bytes)
}

fn verify(url: &str, hash: Option<&str>, bytes: &[u8]) -> R<()> {
    let Some(h) = hash else { return Ok(()) };
    let Some(expected) = h.strip_prefix("1220") else {
        return Err(ReapackError::Unsupported(format!("hash algorithm of {url} (only SHA-256 multihashes are checked)")));
    };
    let got = sha256::hex(&sha256::sha256(bytes));
    if got == expected { Ok(()) } else { Err(ReapackError::Hash { url: url.to_string(), expected: expected.to_string(), got }) }
}

impl Manager {
    /// `state` holds the client's own files, `effects` is where packages are installed.
    pub fn new(state: PathBuf, effects: PathBuf, fetch: Box<dyn Fetch>) -> Self {
        Self { state, effects, fetch, lock: Mutex::new(()) }
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn repos_path(&self) -> PathBuf {
        self.state.join("repositories.json")
    }

    fn installed_path(&self) -> PathBuf {
        self.state.join("installed.json")
    }

    fn index_path(&self, url: &str) -> PathBuf {
        self.state.join("index").join(format!("{:016x}.xml", fnv1a(url)))
    }

    fn load_repos(&self) -> Vec<Repo> {
        let path = self.repos_path();
        if !path.exists() {
            return vec![Repo { name: "ReaTeam JSFX".into(), url: DEFAULT_REPO_URL.into() }];
        }
        read_json::<RepoFile>(&path).repos
    }

    fn save_repos(&self, repos: Vec<Repo>) -> R<()> {
        write_json(&self.repos_path(), &RepoFile { repos })
    }

    fn load_installed(&self) -> Vec<Installed> {
        read_json::<InstalledFile>(&self.installed_path()).installed
    }

    fn save_installed(&self, installed: Vec<Installed>) -> R<()> {
        write_json(&self.installed_path(), &InstalledFile { installed })
    }

    fn find_repo(&self, url_or_name: &str) -> R<Repo> {
        self.load_repos()
            .into_iter()
            .find(|r| r.url == url_or_name || r.name.eq_ignore_ascii_case(url_or_name))
            .ok_or_else(|| ReapackError::NotFound(format!("repository `{url_or_name}`")))
    }

    fn cached_index(&self, url: &str) -> Option<Index> {
        let text = std::fs::read_to_string(self.index_path(url)).ok()?;
        index::parse(&text).ok()
    }

    /// Downloads and parses a repository's index; returns it with its raw text for caching.
    fn download_index(&self, url: &str) -> R<(Index, Vec<u8>)> {
        if !valid_url(url) {
            return Err(ReapackError::Invalid(format!("a repository URL must be https://…: {url}")));
        }
        let bytes = self.fetch.get(url, MAX_INDEX)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| ReapackError::Parse("the index is not UTF-8".into()))?;
        Ok((index::parse(text)?, bytes))
    }

    fn refresh_one(&self, repo: &Repo) -> R<Index> {
        let (idx, bytes) = self.download_index(&repo.url)?;
        write_atomic(&self.index_path(&repo.url), &bytes)?;
        Ok(idx)
    }

    /// The configured repositories, with whether each has a downloaded index.
    pub fn repos(&self) -> Vec<RepoInfo> {
        let _g = self.guard();
        self.load_repos()
            .into_iter()
            .map(|r| {
                let idx = self.cached_index(&r.url);
                RepoInfo { cached: idx.is_some(), packages: idx.map_or(0, |i| i.packages.len()), name: r.name, url: r.url }
            })
            .collect()
    }

    /// Adds a repository (downloading its index to learn its name). Adding a known URL is a no-op.
    pub fn add_repo(&self, url: &str) -> R<Repo> {
        let _g = self.guard();
        let url = url.trim();
        let mut repos = self.load_repos();
        if let Some(r) = repos.iter().find(|r| r.url == url) {
            return Ok(r.clone());
        }
        let (idx, bytes) = self.download_index(url)?;
        write_atomic(&self.index_path(url), &bytes)?;
        let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("repository");
        let repo = Repo { name: if idx.name.trim().is_empty() { host.to_string() } else { idx.name.clone() }, url: url.to_string() };
        repos.push(repo.clone());
        self.save_repos(repos)?;
        Ok(repo)
    }

    /// Removes a repository and its cached index. Packages already installed stay installed.
    pub fn remove_repo(&self, url_or_name: &str) -> R<()> {
        let _g = self.guard();
        let repo = self.find_repo(url_or_name)?;
        let repos: Vec<Repo> = self.load_repos().into_iter().filter(|r| r.url != repo.url).collect();
        let _ = std::fs::remove_file(self.index_path(&repo.url));
        self.save_repos(repos)
    }

    /// Downloads the index of one repository, or of all of them. One failing does not stop the rest.
    pub fn refresh(&self, only: Option<&str>) -> R<Vec<RefreshOutcome>> {
        let _g = self.guard();
        let repos = match only {
            Some(r) => vec![self.find_repo(r)?],
            None => self.load_repos(),
        };
        let mut renamed = self.load_repos();
        let mut out = Vec::new();
        for repo in repos {
            match self.refresh_one(&repo) {
                Ok(idx) => {
                    if !idx.name.trim().is_empty()
                        && let Some(r) = renamed.iter_mut().find(|r| r.url == repo.url)
                    {
                        r.name = idx.name.clone();
                    }
                    out.push(RefreshOutcome { url: repo.url, name: idx.name, packages: idx.packages.len(), error: None });
                }
                Err(e) => out.push(RefreshOutcome { url: repo.url, name: repo.name, packages: 0, error: Some(e.to_string()) }),
            }
        }
        self.save_repos(renamed)?;
        Ok(out)
    }

    /// Searches the downloaded indexes. Every word of `query` must appear in a package's name,
    /// description, category or author; an empty query lists everything.
    pub fn search(&self, query: &str, only_repo: Option<&str>, limit: usize) -> R<Vec<Hit>> {
        let _g = self.guard();
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let repos = match only_repo {
            Some(r) => vec![self.find_repo(r)?],
            None => self.load_repos(),
        };
        let installed = self.load_installed();
        let mut hits: Vec<(u32, Hit)> = Vec::new();
        for repo in repos {
            let Some(idx) = self.cached_index(&repo.url) else { continue };
            for p in &idx.packages {
                let Some(v) = p.latest() else { continue };
                let (name, desc, cat, author) = (p.name.to_lowercase(), p.desc.to_lowercase(), p.category.to_lowercase(), v.author.to_lowercase());
                let mut score = 0;
                let mut all = true;
                for w in &words {
                    let s = if name.contains(w.as_str()) { 6 } else { 0 }
                        + if desc.contains(w.as_str()) { 4 } else { 0 }
                        + if cat.contains(w.as_str()) { 2 } else { 0 }
                        + if author.contains(w.as_str()) { 1 } else { 0 };
                    all &= s > 0;
                    score += s;
                }
                if !all {
                    continue;
                }
                let have = installed.iter().find(|i| i.repo == repo.url && i.category == p.category && i.package == p.name);
                hits.push((
                    score,
                    Hit {
                        repo: repo.url.clone(),
                        repo_name: idx.name.clone(),
                        category: p.category.clone(),
                        package: p.name.clone(),
                        desc: p.desc.clone(),
                        author: v.author.clone(),
                        version: v.name.clone(),
                        installed_version: have.map(|i| i.version.clone()),
                        update_available: have.is_some_and(|i| index::compare_versions(&i.version, &v.name).is_lt()),
                    },
                ));
            }
        }
        hits.sort_by(|(sa, a), (sb, b)| {
            sb.cmp(sa)
                .then_with(|| a.category.to_lowercase().cmp(&b.category.to_lowercase()))
                .then_with(|| a.package.to_lowercase().cmp(&b.package.to_lowercase()))
        });
        Ok(hits.into_iter().take(limit.clamp(1, 1000)).map(|(_, h)| h).collect())
    }

    pub fn installed(&self) -> Vec<Installed> {
        let _g = self.guard();
        self.load_installed()
    }

    /// Installs (or updates to) the newest stable version of a package.
    pub fn install(&self, repo: &str, category: &str, package: &str) -> R<Installed> {
        let _g = self.guard();
        let repo = self.find_repo(repo)?;
        let idx = match self.cached_index(&repo.url) {
            Some(i) => i,
            None => self.refresh_one(&repo)?,
        };
        let pkg = idx
            .packages
            .iter()
            .find(|p| p.category == category && p.name == package)
            .ok_or_else(|| ReapackError::NotFound(format!("package `{category}/{package}` in {}", repo.name)))?;
        let ver = pkg.latest().ok_or_else(|| ReapackError::NotFound(format!("a version of `{package}`")))?;

        // Plan: which files, where. Everything is validated before anything is downloaded.
        let top = folder_name(&idx.name);
        let tags = platform_tags();
        let mut plan: Vec<(String, &index::Source)> = Vec::new();
        for s in &ver.sources {
            if !tags.contains(&s.platform.as_str()) || s.kind.as_deref().is_some_and(|k| k != "effect") {
                continue;
            }
            let rel = format!("{top}/{category}/{}", s.file.as_deref().unwrap_or(&pkg.name));
            if safe_rel(&rel).is_none() {
                return Err(ReapackError::Invalid(format!("the install path `{rel}` is not allowed")));
            }
            if !valid_url(&s.url) {
                return Err(ReapackError::Invalid(format!("not an https URL: {}", s.url)));
            }
            if let Some(h) = &s.hash {
                // Fail before downloading when a checksum cannot be checked.
                let sha = h.strip_prefix("1220").filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()));
                if sha.is_none() {
                    return Err(ReapackError::Unsupported(format!("the checksum of {} (only SHA-256 multihashes are checked)", s.url)));
                }
            }
            if plan.iter().any(|(r, _)| r.eq_ignore_ascii_case(&rel)) {
                return Err(ReapackError::Invalid(format!("two files would install to `{rel}`")));
            }
            plan.push((rel, s));
        }
        if plan.is_empty() {
            return Err(ReapackError::Unsupported(format!("`{package}` has no files for this platform")));
        }
        if plan.len() > MAX_FILES {
            return Err(ReapackError::Invalid(format!("`{package}` has more than {MAX_FILES} files")));
        }
        // A file another installed package owns would be overwritten now and deleted with this
        // package later: a conflict, refused like ReaPack does.
        let this = |i: &Installed| i.repo == repo.url && i.category == category && i.package == package;
        for i in self.load_installed().iter().filter(|i| !this(i)) {
            if let Some((rel, _)) = plan.iter().find(|(rel, _)| i.files.iter().any(|f| f.eq_ignore_ascii_case(rel))) {
                return Err(ReapackError::Invalid(format!("`{rel}` belongs to the installed package `{}/{}`", i.category, i.package)));
            }
        }

        // Download and verify everything in memory first.
        let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
        let mut total = 0usize;
        for (rel, s) in &plan {
            let bytes = self.fetch.get(&s.url, MAX_FILE)?;
            total += bytes.len();
            if total > MAX_TOTAL {
                return Err(ReapackError::Invalid(format!("`{package}` is larger than {} MiB", MAX_TOTAL >> 20)));
            }
            verify(&s.url, s.hash.as_deref(), &bytes)?;
            blobs.push((rel.clone(), bytes));
        }

        // Write every file next to its destination first, so no existing file is touched until all
        // the new bytes are on disk.
        let mut tmps: Vec<(PathBuf, PathBuf)> = Vec::new();
        let cleanup = |tmps: &[(PathBuf, PathBuf)]| {
            for (t, _) in tmps {
                let _ = std::fs::remove_file(t);
            }
        };
        for (rel, bytes) in &blobs {
            let dest = self.effects.join(rel);
            let tmp = tmp_path(&dest);
            let res = dest.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, bytes));
            tmps.push((tmp, dest));
            if let Err(e) = res {
                cleanup(&tmps);
                return Err(ReapackError::Io(format!("writing {rel}: {e}")));
            }
        }

        let main =
            plan.iter().find(|(_, s)| s.file.as_deref().is_none_or(|f| f == pkg.name)).or(plan.first()).map(|(r, _)| r.clone()).unwrap_or_default();
        let record = Installed {
            repo: repo.url.clone(),
            repo_name: idx.name.clone(),
            category: category.to_string(),
            package: package.to_string(),
            version: ver.name.clone(),
            id: format!("{}{main}", soundcraft_jsfx_host::ID_PREFIX),
            files: plan.iter().map(|(r, _)| r.clone()).collect(),
        };
        let mut installed = self.load_installed();
        // Files the previous version had and this one does not are stale.
        let stale: Vec<String> = installed
            .iter()
            .find(|i| i.repo == record.repo && i.category == record.category && i.package == record.package)
            .map(|old| old.files.iter().filter(|f| !record.files.contains(f)).cloned().collect())
            .unwrap_or_default();
        installed.retain(|i| !(i.repo == record.repo && i.category == record.category && i.package == record.package));
        installed.push(record.clone());

        // Move the files in with everything they replace (and the stale ones) kept aside: a failed
        // rename, deletion or record save puts the folder back as it was, never a mix of both
        // versions next to the old record.
        if let Err(e) = self.commit_install(&tmps, &stale, installed) {
            cleanup(&tmps);
            return Err(e);
        }
        Ok(record)
    }

    /// Moves the staged files into place and saves the new record, undoing every step if any of
    /// them fails, so an interrupted update never leaves half of a version installed.
    fn commit_install(&self, tmps: &[(PathBuf, PathBuf)], stale: &[String], installed: Vec<Installed>) -> R<()> {
        let mut aside = Aside::default();
        let step = (|| -> R<()> {
            // The stale files go first: on a case-insensitive filesystem (Windows, and macOS by
            // default) a file the new version only renames in case is the very file about to be
            // written, so swapping in first would move the NEW file aside and then delete it with
            // the backups, losing both versions.
            for rel in stale.iter().filter(|r| safe_rel(r).is_some()) {
                aside.move_aside(&self.effects.join(rel))?;
            }
            for (tmp, dest) in tmps {
                aside.swap_in(tmp, dest)?;
            }
            self.save_installed(installed)
        })();
        if let Err(e) = step {
            aside.restore();
            return Err(e);
        }
        aside.discard();
        for rel in stale.iter().filter(|r| safe_rel(r).is_some()) {
            self.prune_dirs(&self.effects.join(rel));
        }
        Ok(())
    }

    /// Removes an installed package's files (and any folders that become empty).
    pub fn uninstall(&self, repo: &str, category: &str, package: &str) -> R<usize> {
        let _g = self.guard();
        let url = self.find_repo(repo).map_or_else(|_| repo.to_string(), |r| r.url);
        let mut installed = self.load_installed();
        let pos = installed
            .iter()
            .position(|i| i.repo == url && i.category == category && i.package == package)
            .ok_or_else(|| ReapackError::NotFound(format!("`{category}/{package}` is not installed")))?;
        let rec = installed.remove(pos);
        let n = self.delete_files(&rec.files);
        self.save_installed(installed)?;
        Ok(n)
    }

    /// Deletes files (re-checking that each stays inside the Effects folder) and prunes folders.
    fn delete_files(&self, files: &[String]) -> usize {
        let mut n = 0;
        for f in files {
            if safe_rel(f).is_none() {
                continue;
            }
            let path = self.effects.join(f);
            if std::fs::remove_file(&path).is_ok() {
                n += 1;
            }
            self.prune_dirs(&path);
        }
        n
    }

    /// Removes `path`'s folder and its parents while they are empty, never the Effects folder.
    fn prune_dirs(&self, path: &Path) {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == self.effects || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

/// The client for this user: state in SoundCraft's data folder, installs into its Effects folder,
/// downloads with `curl`. `Err` when the platform has no per-user data folder.
pub fn system() -> R<&'static Manager> {
    static M: OnceLock<Option<Manager>> = OnceLock::new();
    M.get_or_init(|| {
        let data = soundcraft_jsfx_host::soundcraft_data_dir()?;
        let effects = soundcraft_jsfx_host::soundcraft_effects_dir()?;
        Some(Manager::new(data.join("ReaPack"), effects, Box::new(crate::CurlFetch)))
    })
    .as_ref()
    .ok_or_else(|| ReapackError::Io("this platform has no per-user data folder".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        files: Mutex<HashMap<String, Vec<u8>>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn put(&self, url: &str, body: &str) {
            self.files.lock().unwrap().insert(url.to_string(), body.as_bytes().to_vec());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetch for Fake {
        fn get(&self, url: &str, max_bytes: usize) -> R<Vec<u8>> {
            self.calls.lock().unwrap().push(url.to_string());
            let b = self.files.lock().unwrap().get(url).cloned().ok_or_else(|| ReapackError::Network(format!("404 {url}")))?;
            if b.len() > max_bytes { Err(ReapackError::Invalid("too large".into())) } else { Ok(b) }
        }
    }

    const REPO: &str = "https://repo.test/index.xml";

    fn setup() -> (Manager, Arc<Fake>, PathBuf) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("sc-reapack-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        let fake = Arc::new(Fake::default());
        let m = Manager::new(root.join("state"), root.join("Effects"), Box::new(Arc::clone(&fake)));
        (m, fake, root)
    }

    fn index_xml(extra_sources: &str, version: &str) -> String {
        format!(
            r#"<index version="1" name="Test Repo"><category name="Dynamics">
              <reapack name="comp.jsfx" type="effect" desc="Simple Compressor"><version name="{version}" author="Ann">
                <source>https://repo.test/comp.jsfx</source>{extra_sources}
              </version></reapack>
              <reapack name="gate.jsfx" type="effect" desc="Noise Gate"><version name="1" author="Bob"><source>https://repo.test/gate.jsfx</source></version></reapack>
            </category></index>"#
        )
    }

    fn with_repo(extra: &str, version: &str) -> (Manager, Arc<Fake>, PathBuf) {
        let (m, f, root) = setup();
        f.put(REPO, &index_xml(extra, version));
        f.put("https://repo.test/comp.jsfx", "desc:Comp\n@sample\nspl0*=0.5;\n");
        f.put("https://repo.test/gate.jsfx", "desc:Gate\n");
        m.add_repo(REPO).unwrap();
        (m, f, root)
    }

    #[test]
    fn a_fresh_manager_offers_the_default_repository() {
        let (m, _, _) = setup();
        let r = m.repos();
        assert_eq!((r.len(), r[0].url.as_str(), r[0].cached), (1, DEFAULT_REPO_URL, false));
    }

    #[test]
    fn adding_a_repository_learns_its_name_and_caches_the_index() {
        let (m, f, _) = setup();
        f.put(REPO, &index_xml("", "1.0"));
        let r = m.add_repo(REPO).unwrap();
        assert_eq!(r.name, "Test Repo");
        let infos = m.repos();
        assert!(infos.iter().any(|i| i.url == REPO && i.cached && i.packages == 2));
        assert_eq!(m.add_repo(REPO).unwrap(), r, "adding twice is a no-op");
        assert_eq!(f.calls().len(), 1);
        assert!(matches!(m.add_repo("http://repo.test/index.xml"), Err(ReapackError::Invalid(_))));
        assert!(matches!(m.add_repo("https://nope.test/index.xml"), Err(ReapackError::Network(_))));
        m.remove_repo("test repo").unwrap();
        assert!(m.repos().iter().all(|i| i.url != REPO));
    }

    #[test]
    fn search_matches_every_word_and_ranks_names_first() {
        let (m, _, _) = with_repo("", "1.0");
        let names = |q: &str| m.search(q, None, 50).unwrap().into_iter().map(|h| h.package).collect::<Vec<_>>();
        assert_eq!(names("comp"), ["comp.jsfx"]);
        assert_eq!(names("noise gate"), ["gate.jsfx"]);
        assert_eq!(names("compressor ann"), ["comp.jsfx"]);
        assert_eq!(names("dynamics"), ["comp.jsfx", "gate.jsfx"]);
        assert_eq!(names("compressor bob"), Vec::<String>::new());
        assert_eq!(names("").len(), 2);
        assert_eq!(m.search("", None, 1).unwrap().len(), 1);
    }

    #[test]
    fn install_puts_files_under_the_repository_and_category_and_records_them() {
        let (m, f, root) = with_repo(
            r#"<source file="lib/helper.jsfx-inc">https://repo.test/helper</source><source file="../Data/x.rpl" type="data">https://repo.test/x</source><source platform="nonexistent-os">https://repo.test/other</source>"#,
            "1.0",
        );
        f.put("https://repo.test/helper", "// helper\n");
        let rec = m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        assert_eq!(rec.id, "jsfx:Test Repo/Dynamics/comp.jsfx");
        assert_eq!(rec.files, ["Test Repo/Dynamics/comp.jsfx", "Test Repo/Dynamics/lib/helper.jsfx-inc"]);
        let eff = root.join("Effects/Test Repo/Dynamics");
        assert_eq!(std::fs::read_to_string(eff.join("comp.jsfx")).unwrap(), "desc:Comp\n@sample\nspl0*=0.5;\n");
        assert!(eff.join("lib/helper.jsfx-inc").exists());
        assert!(!root.join("Effects/Test Repo/Data").exists(), "data files are not installed");
        assert!(!f.calls().iter().any(|c| c.ends_with("/x") || c.ends_with("/other")));
        assert_eq!(m.installed(), vec![rec]);
        let hit = &m.search("comp", None, 5).unwrap()[0];
        assert_eq!((hit.installed_version.as_deref(), hit.update_available), (Some("1.0"), false));
        assert!(std::fs::read_dir(&eff).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".sctmp")));
    }

    #[test]
    fn installed_effects_load_in_the_jsfx_host() {
        let (m, _, root) = with_repo("", "1.0");
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        let eff = root.join("Effects");
        assert_eq!(soundcraft_jsfx_host::scan::scan(&eff).len(), 1, "only the installed effect is on disk");
        let p = soundcraft_jsfx_host::JsfxPlugin::from_file(&eff, &eff.join("Test Repo/Dynamics/comp.jsfx")).unwrap();
        assert_eq!(soundcraft_dsp::Plugin::info(&p).id, "jsfx:Test Repo/Dynamics/comp.jsfx");
    }

    #[test]
    fn a_checksum_is_enforced_and_a_failed_install_leaves_nothing() {
        let body = "// lib\n";
        let good = format!("1220{}", sha256::hex(&sha256::sha256(body.as_bytes())));
        let (m, f, root) = with_repo(&format!(r#"<source file="lib.jsfx-inc" hash="{good}">https://repo.test/lib</source>"#), "1.0");
        f.put("https://repo.test/lib", body);
        assert!(m.install(REPO, "Dynamics", "comp.jsfx").is_ok());
        let _ = std::fs::remove_dir_all(root.join("Effects"));
        let _ = m.uninstall(REPO, "Dynamics", "comp.jsfx");

        f.put("https://repo.test/lib", "// tampered\n");
        let err = m.install(REPO, "Dynamics", "comp.jsfx").unwrap_err();
        assert!(matches!(err, ReapackError::Hash { .. }), "{err}");
        assert!(!root.join("Effects").exists(), "nothing was written");
        assert!(m.installed().is_empty());
    }

    #[test]
    fn unknown_hash_algorithms_and_missing_downloads_refuse_the_install() {
        let (m, _, root) = with_repo(r#"<source file="a.jsfx-inc" hash="1114abcd">https://repo.test/a</source>"#, "1.0");
        assert!(matches!(m.install(REPO, "Dynamics", "comp.jsfx"), Err(ReapackError::Unsupported(_))));
        let (m, _, root2) = with_repo(r#"<source file="missing.jsfx-inc">https://repo.test/missing</source>"#, "1.0");
        assert!(matches!(m.install(REPO, "Dynamics", "comp.jsfx"), Err(ReapackError::Network(_))));
        assert!(!root.join("Effects").exists() && !root2.join("Effects").exists());
    }

    #[test]
    fn hostile_paths_and_urls_are_refused_before_any_download() {
        for (extra, what) in [
            (r#"<source file="../../evil.jsfx">https://repo.test/e</source>"#, "dot-dot"),
            (r#"<source file="/abs/evil">https://repo.test/e</source>"#, "absolute"),
            (r#"<source file="C:/evil">https://repo.test/e</source>"#, "drive"),
            (r#"<source file="a\evil">https://repo.test/e</source>"#, "backslash"),
            (r#"<source file="ok.jsfx-inc">http://repo.test/e</source>"#, "plain http"),
            (r#"<source file="ok.jsfx-inc">file:///etc/passwd</source>"#, "file url"),
            (r#"<source file="comp.jsfx">https://repo.test/dup</source>"#, "duplicate destination"),
        ] {
            let (m, f, root) = with_repo(extra, "1.0");
            let before = f.calls().len();
            assert!(matches!(m.install(REPO, "Dynamics", "comp.jsfx"), Err(ReapackError::Invalid(_))), "{what}");
            assert_eq!(f.calls().len(), before, "{what}: nothing downloaded");
            assert!(!root.join("Effects").exists(), "{what}");
        }
    }

    #[test]
    fn a_category_cannot_escape_the_effects_folder() {
        let (m, f, root) = setup();
        f.put(REPO, r#"<index version="1" name="R"><category name="../../Evil"><reapack name="x.jsfx" type="effect"><version name="1"><source>https://repo.test/x</source></version></reapack></category></index>"#);
        f.put("https://repo.test/x", "desc:x\n");
        m.add_repo(REPO).unwrap();
        assert!(matches!(m.install(REPO, "../../Evil", "x.jsfx"), Err(ReapackError::Invalid(_))));
        assert!(!root.join("Evil").exists() && !root.join("Effects").exists());
    }

    #[test]
    fn a_repository_name_cannot_escape_either() {
        let (m, f, root) = setup();
        f.put(REPO, r#"<index version="1" name="../../Evil"><category name="C"><reapack name="x.jsfx" type="effect"><version name="1"><source>https://repo.test/x</source></version></reapack></category></index>"#);
        f.put("https://repo.test/x", "desc:x\n");
        m.add_repo(REPO).unwrap();
        let rec = m.install(REPO, "C", "x.jsfx").unwrap();
        assert!(rec.files[0].starts_with("_.._Evil/"), "{:?}", rec.files);
        assert!(root.join("Effects").join(&rec.files[0]).exists());
        assert!(!root.join("Evil").exists());
    }

    #[test]
    fn updating_replaces_files_and_removes_stale_ones() {
        let (m, f, root) = with_repo(r#"<source file="old.jsfx-inc">https://repo.test/old</source>"#, "1.0");
        f.put("https://repo.test/old", "// old\n");
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        let eff = root.join("Effects/Test Repo/Dynamics");
        assert!(eff.join("old.jsfx-inc").exists());

        f.put(REPO, &index_xml("", "1.1"));
        f.put("https://repo.test/comp.jsfx", "desc:Comp 2\n");
        assert!(m.search("comp", None, 5).unwrap()[0].version == "1.0", "the cached index is still the old one");
        m.refresh(None).unwrap();
        let hit = m.search("comp", None, 5).unwrap().remove(0);
        assert_eq!((hit.version.as_str(), hit.installed_version.as_deref(), hit.update_available), ("1.1", Some("1.0"), true));
        let rec = m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        assert_eq!((rec.version.as_str(), rec.files.len()), ("1.1", 1));
        assert!(!eff.join("old.jsfx-inc").exists(), "stale file removed");
        assert_eq!(std::fs::read_to_string(eff.join("comp.jsfx")).unwrap(), "desc:Comp 2\n");
        assert_eq!(m.installed().len(), 1);
    }

    #[test]
    fn a_failed_record_save_puts_the_previous_version_back() {
        let (m, f, root) = with_repo(r#"<source file="old.jsfx-inc">https://repo.test/old</source>"#, "1.0");
        f.put("https://repo.test/old", "// old\n");
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        let eff = root.join("Effects/Test Repo/Dynamics");
        let before = std::fs::read_to_string(eff.join("comp.jsfx")).unwrap();

        // The record cannot be written (a folder sits where its temporary file goes) but it can
        // still be read, so the update gets as far as swapping the files in.
        std::fs::create_dir_all(root.join("state/installed.json.sctmp")).unwrap();
        f.put(REPO, &index_xml("", "1.1"));
        f.put("https://repo.test/comp.jsfx", "desc:Comp 2\n");
        m.refresh(None).unwrap();
        let err = m.install(REPO, "Dynamics", "comp.jsfx").unwrap_err();
        assert!(matches!(err, ReapackError::Io(_)), "{err:?}");

        assert_eq!(std::fs::read_to_string(eff.join("comp.jsfx")).unwrap(), before, "the old file is back");
        assert!(eff.join("old.jsfx-inc").exists(), "the stale file was put back too");
        let mut names: Vec<String> = std::fs::read_dir(&eff).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["comp.jsfx", "old.jsfx-inc"], "no temporary or backup files are left behind");
        assert_eq!(m.installed().len(), 1);
        assert_eq!(m.installed()[0].version, "1.0", "the old record stays");
    }

    #[test]
    fn a_case_only_rename_keeps_the_new_file_and_drops_the_old_one() {
        let (m, f, root) = setup();
        let index = |file: &str, version: &str| {
            format!(
                r#"<index version="1" name="Test Repo"><category name="Dynamics">
                  <reapack name="comp.jsfx" type="effect"><version name="{version}"><source file="{file}">https://repo.test/comp.jsfx</source></version></reapack>
                </category></index>"#
            )
        };
        f.put(REPO, &index("Comp.jsfx", "1"));
        f.put("https://repo.test/comp.jsfx", "desc:Old\n");
        m.add_repo(REPO).unwrap();
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();

        // Only the case of the file name changes: on a case-insensitive filesystem this is the
        // same file the install is about to write, and the old one must not come back.
        f.put(REPO, &index("comp.jsfx", "2"));
        f.put("https://repo.test/comp.jsfx", "desc:New\n");
        m.refresh(None).unwrap();
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();

        let eff = root.join("Effects/Test Repo/Dynamics");
        let names: Vec<String> = std::fs::read_dir(&eff).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 1, "one file, no backups left: {names:?}");
        assert_eq!(std::fs::read_to_string(eff.join("comp.jsfx")).unwrap(), "desc:New\n");
        assert_eq!(m.installed()[0].files, ["Test Repo/Dynamics/comp.jsfx"]);
    }

    #[test]
    fn a_file_another_installed_package_owns_refuses_the_install() {
        let (m, f, root) = setup();
        let helper = r#"<source file="lib/helper.jsfx-inc">https://repo.test/helper</source>"#;
        f.put(
            REPO,
            &format!(
                r#"<index version="1" name="Test Repo"><category name="Dynamics">
                  <reapack name="comp.jsfx" type="effect"><version name="1"><source>https://repo.test/comp.jsfx</source>{helper}</version></reapack>
                  <reapack name="gate.jsfx" type="effect"><version name="1"><source>https://repo.test/gate.jsfx</source>{helper}</version></reapack>
                </category></index>"#
            ),
        );
        f.put("https://repo.test/comp.jsfx", "desc:Comp\n");
        f.put("https://repo.test/gate.jsfx", "desc:Gate\n");
        f.put("https://repo.test/helper", "// helper\n");
        m.add_repo(REPO).unwrap();
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        let err = m.install(REPO, "Dynamics", "gate.jsfx").unwrap_err();
        assert!(matches!(&err, ReapackError::Invalid(s) if s.contains("comp.jsfx")), "{err:?}");
        assert!(!f.calls().iter().any(|c| c.ends_with("/gate.jsfx")), "refused before any download");
        assert_eq!(m.installed().len(), 1);
        // Uninstalling the owner leaves nothing behind, as before.
        m.uninstall(REPO, "Dynamics", "comp.jsfx").unwrap();
        assert!(!root.join("Effects/Test Repo").exists());
    }

    #[test]
    fn uninstall_removes_files_and_empty_folders_and_survives_a_restart() {
        let (m, _, root) = with_repo("", "1.0");
        m.install(REPO, "Dynamics", "comp.jsfx").unwrap();
        m.install(REPO, "Dynamics", "gate.jsfx").unwrap();
        let m2 = Manager::new(root.join("state"), root.join("Effects"), Box::new(Arc::new(Fake::default())));
        assert_eq!(m2.installed().len(), 2, "state persists");
        assert_eq!(m2.uninstall(REPO, "Dynamics", "comp.jsfx").unwrap(), 1);
        assert!(root.join("Effects/Test Repo/Dynamics/gate.jsfx").exists());
        assert_eq!(m2.uninstall("test repo", "Dynamics", "gate.jsfx").unwrap(), 1);
        assert!(!root.join("Effects/Test Repo").exists(), "empty folders are pruned");
        assert!(root.join("Effects").exists(), "the Effects folder itself stays");
        assert!(matches!(m2.uninstall(REPO, "Dynamics", "gate.jsfx"), Err(ReapackError::NotFound(_))));
    }

    #[test]
    fn a_tampered_install_record_cannot_delete_outside_the_effects_folder() {
        let (m, _, root) = with_repo("", "1.0");
        std::fs::create_dir_all(root.join("state")).unwrap();
        let victim = root.join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        let rec = Installed {
            repo: REPO.into(),
            repo_name: "x".into(),
            category: "C".into(),
            package: "p".into(),
            version: "1".into(),
            id: "jsfx:x".into(),
            files: vec!["../victim.txt".into(), "/abs".into()],
        };
        m.save_installed(vec![rec]).unwrap();
        assert_eq!(m.uninstall(REPO, "C", "p").unwrap(), 0);
        assert!(victim.exists());
    }

    #[test]
    fn refresh_reports_failures_without_stopping() {
        let (m, f, _) = with_repo("", "1.0");
        m.add_repo("https://other.test/index.xml").unwrap_err();
        f.put("https://other.test/index.xml", "<index version=\"1\" name=\"Other\"></index>");
        m.add_repo("https://other.test/index.xml").unwrap();
        f.files.lock().unwrap().remove(REPO);
        let out = m.refresh(None).unwrap();
        assert_eq!(out.len(), 3, "default repo + two added");
        assert!(out.iter().any(|o| o.url == REPO && o.error.is_some()));
        assert!(out.iter().any(|o| o.name == "Other" && o.error.is_none()));
        assert!(m.search("comp", None, 5).unwrap().len() == 1, "the old cached index still serves searches");
    }
}
