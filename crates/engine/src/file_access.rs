//! The folders automation may read and write.
//!
//! `soundcraft-cli mcp --automation-read-root <dir> --automation-write-root <dir>` confines the
//! files an agent's commands reach; each flag may be given more than once (#874). [`AutomationRoots`]
//! holds the folders of each kind. While they are in force, every path a command reads goes through
//! [`check_read`] and every path it writes through [`check_write`]:
//!
//! - The path is made absolute against the working directory, as the file system reads it. Its
//!   links are followed: for a file that doesn't exist yet, the links of its nearest existing
//!   folder, the rest of the path being plain names (`..` below a missing folder is refused, and so
//!   is a link that leads nowhere).
//! - The result must lie inside one of the roots of its kind, compared by whole components
//!   (`/work2` isn't inside `/work`), without regard to case on Windows.
//! - On Windows a network path (`\\server\share`, `\\?\UNC\...`, WebDAV's `\\host@SSL\share`), a
//!   device path (`\\.\...`) or a verbatim path that isn't a disk is refused by its name before
//!   the file system is asked about it, unless a root is on that same server and share (or that
//!   same device or prefix). Looking such a path up would contact a host the agent chose. A link
//!   whose text leads to one is refused before it is followed.
//! - Read and write access are separate: a root given alone grants only its own kind of access.
//!
//! Roots are in force on a thread inside [`confine`], and in the whole process after
//! [`confine_process`] (`soundcraft-cli mcp`), except inside [`unconfined`]. Without roots
//! nothing is checked.
//!
//! A path is checked, then opened: a process that can change the folders in between (swapping in
//! a link) can still win that race. The roots keep an agent to the files it was given; they don't
//! guard against other programs on the computer.

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Read access or write access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// The folders automation may read and write; a kind with no root grants no access of its kind.
#[derive(Debug)]
pub struct AutomationRoots {
    read: Vec<Root>,
    write: Vec<Root>,
}

#[derive(Debug)]
struct Root {
    /// The folder made absolute, links followed: what paths are compared with.
    resolved: PathBuf,
    /// The same without Windows' `\\?\` prefix, for people.
    shown: PathBuf,
}

/// A root [`AutomationRoots::new`] can't open: missing, unreadable, or not a folder.
#[derive(Debug, thiserror::Error)]
#[error("cannot open the automation {} root `{}`: {source}", kind(.access), .path.display())]
pub struct RootError {
    access: Access,
    path: PathBuf,
    source: std::io::Error,
}

fn kind(access: &Access) -> &'static str {
    match access {
        Access::Read => "read",
        Access::Write => "write",
    }
}

impl Root {
    /// The folder at `path`, which must exist.
    fn open(path: &Path, access: Access) -> Result<Self, RootError> {
        let resolved = std::fs::canonicalize(path)
            .and_then(|p| if p.is_dir() { Ok(p) } else { Err(std::io::ErrorKind::NotADirectory.into()) })
            .map_err(|source| RootError { access, path: path.to_owned(), source })?;
        Ok(Self { shown: plain(resolved.clone()), resolved })
    }
}

impl AutomationRoots {
    /// The roots `read` and `write`, which must be existing folders (one given twice counts once);
    /// `None` when neither kind has any (nothing is confined then).
    pub fn new(read: &[&Path], write: &[&Path]) -> Result<Option<Arc<Self>>, RootError> {
        if read.is_empty() && write.is_empty() {
            return Ok(None);
        }
        let open = |paths: &[&Path], access: Access| -> Result<Vec<Root>, RootError> {
            let mut roots: Vec<Root> = Vec::with_capacity(paths.len());
            for path in paths {
                let root = Root::open(path, access)?;
                if !roots.iter().any(|r| r.resolved == root.resolved) {
                    roots.push(root);
                }
            }
            Ok(roots)
        };
        Ok(Some(Arc::new(Self { read: open(read, Access::Read)?, write: open(write, Access::Write)? })))
    }

    /// The roots for people: `read A, B; write C` (`none` for a kind without any).
    pub fn describe(&self) -> String {
        let list = |roots: &[Root]| {
            if roots.is_empty() {
                return "none".to_string();
            }
            roots.iter().map(|r| r.shown.display().to_string()).collect::<Vec<_>>().join(", ")
        };
        format!("read {}; write {}", list(&self.read), list(&self.write))
    }

    /// May the file or folder at `path` be read (or written)? It must lie inside one of the roots
    /// of that kind. `Err` says why not.
    pub fn check(&self, path: impl AsRef<Path>, access: Access) -> Result<(), String> {
        let path = path.as_ref();
        let shown_path = path.display();
        let roots = match access {
            Access::Read => &self.read,
            Access::Write => &self.write,
        };
        if roots.is_empty() {
            return match access {
                Access::Read => Err(format!("automation filesystem access is not granted: read authority is absent: {shown_path}")),
                Access::Write => Err(format!("automation filesystem access is not granted: write authority is absent: {shown_path}")),
            };
        }
        let folders: Vec<&Path> = roots.iter().map(|r| r.resolved.as_path()).collect();
        let resolved = resolve(path, &folders)?;
        if roots.iter().any(|r| inside(&resolved, &r.resolved)) {
            return Ok(());
        }
        let shown: Vec<String> = roots.iter().map(|r| r.shown.display().to_string()).collect();
        match (shown.as_slice(), access) {
            ([root], Access::Read) => Err(format!("automation path rejected: outside the read root {root}: {shown_path}")),
            ([root], Access::Write) => Err(format!("automation path rejected: outside the write root {root}: {shown_path}")),
            (_, Access::Read) => Err(format!("automation path rejected: outside the read roots {}: {shown_path}", shown.join(", "))),
            (_, Access::Write) => Err(format!("automation path rejected: outside the write roots {}: {shown_path}", shown.join(", "))),
        }
    }
}

/// Where the roots are in force on a thread ([`confine`], [`unconfined`]).
#[derive(Clone, Debug)]
enum Scope {
    Confined(Arc<AutomationRoots>),
    Unconfined,
}

thread_local! {
    /// This thread's scope; `None`: the process's roots ([`confine_process`]).
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// The roots in force in every thread outside [`confine`] and [`unconfined`].
static PROCESS: OnceLock<Arc<AutomationRoots>> = OnceLock::new();

/// Put `roots` in force in the whole process, for good. `false` when roots were in force already
/// (they stay as they were).
pub fn confine_process(roots: Arc<AutomationRoots>) -> bool {
    PROCESS.set(roots).is_ok()
}

/// Puts the scope it replaced back when dropped (also when `f` panics).
struct Restore(Option<Scope>);

impl Drop for Restore {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPE.with(|s| *s.borrow_mut() = previous);
    }
}

fn with_scope<T>(scope: Option<Scope>, f: impl FnOnce() -> T) -> T {
    let _restore = Restore(SCOPE.with(|s| s.replace(scope)));
    f()
}

/// Run `f` with `roots` in force on this thread (`None`: as things stand).
pub fn confine<T>(roots: Option<&Arc<AutomationRoots>>, f: impl FnOnce() -> T) -> T {
    match roots {
        Some(r) => with_scope(Some(Scope::Confined(r.clone())), f),
        None => f(),
    }
}

/// Run `f` with no roots in force on this thread: what the person at the keyboard does, and the
/// app's own folders.
pub fn unconfined<T>(f: impl FnOnce() -> T) -> T {
    with_scope(Some(Scope::Unconfined), f)
}

/// The roots in force on this thread, if any.
pub fn current() -> Option<Arc<AutomationRoots>> {
    match SCOPE.with(|s| s.borrow().clone()) {
        Some(Scope::Confined(r)) => Some(r),
        Some(Scope::Unconfined) => None,
        None => PROCESS.get().cloned(),
    }
}

/// `path` without Windows' `\\?\` prefix (which `canonicalize` adds), for people to read.
fn plain(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

/// May `path` be read here? Always, without roots in force.
pub fn check_read(path: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check(path.as_ref(), Access::Read))
}

/// May `path` be written here? Always, without roots in force.
pub fn check_write(path: impl AsRef<Path>) -> Result<(), String> {
    current().map_or(Ok(()), |r| r.check(path.as_ref(), Access::Write))
}

/// `path` made absolute with its links followed; for a path that doesn't exist (yet), its nearest
/// existing folder's links followed and the rest appended.
///
/// A Windows network, device or non-disk verbatim prefix is refused before any of that, unless one
/// of `roots` (resolved folders) already stands on the same server and share, device or prefix.
fn resolve(path: &Path, roots: &[&Path]) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("automation path rejected: path is empty".into());
    }
    // Checked as given: making `…\NUL` absolute gives the device path `\\.\NUL` on Windows.
    if cfg!(windows) && path.components().any(|c| matches!(c, Component::Normal(n) if is_device_name(&n.to_string_lossy()))) {
        return Err(format!("automation path rejected: reserved device names are not allowed: {}", path.display()));
    }
    // Before `absolute` too: on Windows that call asks the system about the path.
    if let Some(why) = refused_prefix(path, roots) {
        return Err(format!("automation path rejected: {why}: {}", path.display()));
    }
    let absolute = std::path::absolute(path).map_err(|e| format!("automation path rejected: cannot resolve {}: {e}", path.display()))?;
    if let Some(why) = refused_prefix(&absolute, roots) {
        return Err(format!("automation path rejected: {why}: {}", path.display()));
    }
    // A link is judged by its own text: canonicalizing one that leads to a network path would
    // contact that host. Only Windows has such prefixes. One walk of the whole path covers every
    // shorter one the loop below tries.
    if cfg!(windows)
        && let Some(why) = link_leads_to_refused_prefix(&absolute, roots)
    {
        return Err(format!("automation path rejected: {why}: {}", path.display()));
    }
    // The names below the nearest existing folder, last first.
    let mut missing: Vec<OsString> = vec![];
    let mut at = absolute.as_path();
    loop {
        match std::fs::canonicalize(at) {
            Ok(mut found) => {
                found.extend(missing.iter().rev());
                return Ok(found);
            }
            Err(e) => match std::fs::symlink_metadata(at) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(format!("automation path rejected: a link that leads nowhere: {}", path.display()));
                }
                // There, yet its real path can't be had (no permission): nothing to compare.
                Ok(_) => return Err(format!("automation path rejected: cannot resolve {}: {e}", path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("automation path rejected: cannot resolve {}: {e}", path.display())),
            },
        }
        let parent = at.parent();
        match (at.components().next_back(), parent) {
            (Some(Component::Normal(name)), Some(parent)) => {
                missing.push(name.to_os_string());
                at = parent;
            }
            (Some(Component::ParentDir), _) => {
                return Err(format!("automation path rejected: parent traversal below a folder that doesn't exist: {}", path.display()));
            }
            // Down to the root (or drive) and it doesn't exist either.
            _ => {
                let e = std::io::Error::from(std::io::ErrorKind::NotFound);
                return Err(format!("automation path rejected: cannot resolve {}: {e}", path.display()));
            }
        }
    }
}

/// Is `path` (resolved) `root` or inside it, component by component (without regard to case on
/// Windows, whose file systems ignore it)?
fn inside(path: &Path, root: &Path) -> bool {
    let mut path = path.components();
    root.components().all(|r| path.next().is_some_and(|p| same_component(p, r)))
}

fn same_component(a: Component, b: Component) -> bool {
    if a == b {
        return true;
    }
    if !cfg!(windows) {
        return false;
    }
    match (a.as_os_str().to_str(), b.as_os_str().to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

/// A Windows device name (`CON`, `NUL`, `COM1`… also with an extension), which names no file.
fn is_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"));
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

/// A Windows path prefix that isn't a local disk: opening or even resolving a path that starts
/// with one can reach another computer.
#[derive(Debug)]
enum OffDisk {
    /// `\\server\share` or `\\?\UNC\server\share`; WebDAV's `\\host@SSL\share` is a server name too.
    Unc(OsString, OsString),
    /// `\\.\device`.
    Device(OsString),
    /// `\\?\` followed by anything but a drive or `UNC`.
    Verbatim(OsString),
}

impl OffDisk {
    /// The prefix of `path` when it isn't a local disk. A drive letter, a path without a prefix
    /// and every path off Windows give `None`.
    fn of(path: &Path) -> Option<Self> {
        use std::path::Prefix;
        let Some(Component::Prefix(prefix)) = path.components().next() else { return Self::unparsed_unc(path) };
        match prefix.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => None,
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => Some(Self::Unc(server.to_os_string(), share.to_os_string())),
            Prefix::DeviceNS(device) => Some(Self::Device(device.to_os_string())),
            Prefix::Verbatim(rest) => Some(Self::Verbatim(rest.to_os_string())),
        }
    }

    /// A path that starts with two separators on Windows but isn't a whole `\\server\share` (`\\server`,
    /// `\\\x`): Windows still reads it as the name of a computer, so it is a UNC path with no share.
    fn unparsed_unc(path: &Path) -> Option<Self> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let separator = |b: Option<&u8>| matches!(b, Some(b'\\' | b'/'));
        if cfg!(windows) && separator(bytes.first()) && separator(bytes.get(1)) { Some(Self::Unc(OsString::new(), OsString::new())) } else { None }
    }

    /// The refusal for a path with this prefix, which names the prefix.
    fn refusal(&self) -> &'static str {
        match self {
            Self::Unc(..) => "UNC paths are refused unless a root is on the same server and share",
            Self::Device(_) => "device paths are refused unless a root uses the same device",
            Self::Verbatim(_) => "verbatim paths are refused unless a root uses the same prefix",
        }
    }

    /// Whether `other` is the same server and share, device or prefix, the ASCII case aside. This
    /// decides only whether that host may be contacted; [`inside`] still decides the folder.
    fn same_as(&self, other: &Self) -> bool {
        let same = |a: &OsString, b: &OsString| a.eq_ignore_ascii_case(b);
        match (self, other) {
            (Self::Unc(server, share), Self::Unc(other_server, other_share)) => same(server, other_server) && same(share, other_share),
            (Self::Device(device), Self::Device(other)) => same(device, other),
            (Self::Verbatim(rest), Self::Verbatim(other)) => same(rest, other),
            _ => false,
        }
    }
}

/// Why `path` must not be looked up: it starts with a Windows network, device or non-disk
/// verbatim prefix and none of `roots` stands on that same prefix. A local path gives `None`.
fn refused_prefix(path: &Path, roots: &[&Path]) -> Option<&'static str> {
    let prefix = OffDisk::of(path)?;
    if roots.iter().filter_map(|root| OffDisk::of(root)).any(|root| root.same_as(&prefix)) { None } else { Some(prefix.refusal()) }
}

/// How many links in a row [`link_leads_to_refused_prefix`] follows before it gives up and refuses.
const MAX_LINK_HOPS: usize = 40;

/// How many spellings of a path one walk scans in all, so a hostile set of links can't make it
/// scan without end.
const MAX_SPELLINGS: usize = 1024;

/// The refusal for a link in `path`, taken from the link's own text.
///
/// Folders are inspected from the top down with [`std::fs::symlink_metadata`], which doesn't follow
/// a link, and a link's target is read with [`std::fs::read_link`]. A link to a network path is
/// therefore refused before [`std::fs::canonicalize`] could open that host, also when the link is
/// only an ancestor of the name. A link whose target can't be read, the one after
/// [`MAX_LINK_HOPS`] in a row, or a walk past [`MAX_SPELLINGS`] spellings, is refused rather than
/// opened.
fn link_leads_to_refused_prefix(path: &Path, roots: &[&Path]) -> Option<&'static str> {
    follow_links(path, roots, &mut std::collections::HashSet::new(), MAX_LINK_HOPS)
}

/// [`link_leads_to_refused_prefix`] for one spelling of the path. `judged` holds every spelling the
/// walk has scanned (so none is scanned twice, nor in a cycle forever) and `hops_left` the links
/// still allowed in this chain, which the other spelling of a target does not use up.
fn follow_links(path: &Path, roots: &[&Path], judged: &mut std::collections::HashSet<PathBuf>, hops_left: usize) -> Option<&'static str> {
    if !judged.insert(path.to_path_buf()) {
        return None;
    }
    if judged.len() > MAX_SPELLINGS {
        return Some("too many links");
    }
    let mut ancestors: Vec<&Path> = path.ancestors().collect();
    ancestors.reverse();
    for prefix in ancestors {
        if prefix.as_os_str().is_empty() {
            continue;
        }
        if let Some(why) = refused_prefix(prefix, roots) {
            return Some(why);
        }
        let meta = match std::fs::symlink_metadata(prefix) {
            Ok(meta) => meta,
            // Nothing is there, so no link is below it.
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => return None,
            // Anything else (no permission to look): a link below it can't be seen, so don't go on.
            Err(_) => return Some("a folder that can't be looked at"),
        };
        if !meta.file_type().is_symlink() {
            continue;
        }
        if hops_left == 0 {
            return Some("too many links");
        }
        let Ok(target) = std::fs::read_link(prefix) else { return Some("a link that leads nowhere") };
        // Judged as written: joined onto the link's folder, a bare `\\server` would keep that folder's drive.
        if let Some(why) = refused_prefix(&target, roots) {
            return Some(why);
        }
        let joined = match prefix.parent() {
            Some(parent) if !target.is_absolute() && !parent.as_os_str().is_empty() => parent.join(target),
            _ => target,
        };
        let next = match path.strip_prefix(prefix) {
            Ok(rest) if !rest.as_os_str().is_empty() => joined.join(rest),
            _ => joined,
        };
        if let Some(why) = refused_prefix(&next, roots) {
            return Some(why);
        }
        // Windows takes the `..` of a relative target by name, so `missing\..\remote` is `remote`
        // though `missing` isn't there: judge that spelling as well as the one written.
        let by_name = lexical(&next);
        if by_name != next
            && let Some(why) = follow_links(&by_name, roots, judged, hops_left - 1)
        {
            return Some(why);
        }
        return follow_links(&next, roots, judged, hops_left - 1);
    }
    None
}

/// `path` with its `.` and `..` taken by name, never above its root.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder holding `in` (read root), `out` (write root) and `outside`.
    fn folders(name: &str) -> (PathBuf, Arc<AutomationRoots>) {
        let base = std::env::temp_dir().join(format!("sc-file-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["in/sub", "out", "outside", "in2"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("in/a.scraft"), "x").unwrap();
        std::fs::write(base.join("outside/secret.scraft"), "x").unwrap();
        let roots = AutomationRoots::new(&[&base.join("in")], &[&base.join("out")]).unwrap().unwrap();
        (base, roots)
    }

    fn s(p: PathBuf) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn paths_inside_the_roots_pass_and_others_dont() {
        let (base, roots) = folders("inside");
        assert_eq!(roots.check(s(base.join("in/a.scraft")), Access::Read), Ok(()));
        assert_eq!(roots.check(s(base.join("in")), Access::Read), Ok(()), "the root itself");
        assert_eq!(roots.check(s(base.join("in/sub/new.scraft")), Access::Read), Ok(()), "missing files are checked by their folder");
        assert_eq!(roots.check(s(base.join("out/new/deeper/x.wav")), Access::Write), Ok(()), "and by their nearest folder");
        let e = roots.check(s(base.join("outside/secret.scraft")), Access::Read).unwrap_err();
        assert!(e.starts_with("automation path rejected: outside the read root"), "{e}");
        // Read and write access are separate.
        let e = roots.check(s(base.join("in/a.scraft")), Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "{e}");
        assert!(roots.check(s(base.join("out/x.wav")), Access::Read).unwrap_err().contains("outside the read root"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_sibling_with_the_same_prefix_is_outside() {
        let (base, roots) = folders("prefix");
        std::fs::write(base.join("in2/b.scraft"), "x").unwrap();
        assert!(roots.check(s(base.join("in2/b.scraft")), Access::Read).is_err(), "in2 is not inside in");
        assert!(roots.check(format!("{}2/c.scraft", s(base.join("in"))), Access::Read).is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn parent_traversal_is_resolved_or_refused() {
        let (base, roots) = folders("dotdot");
        let sep = std::path::MAIN_SEPARATOR;
        let up = format!("{}{sep}..{sep}outside{sep}secret.scraft", s(base.join("in")));
        assert!(roots.check(up, Access::Read).unwrap_err().contains("outside the read root"), "`..` out of the root");
        let back = format!("{}{sep}..{sep}a.scraft", s(base.join("in/sub")));
        assert_eq!(roots.check(back, Access::Read), Ok(()), "`..` that stays inside");
        let missing = format!("{}{sep}nope{sep}..{sep}..{sep}outside{sep}x.scraft", s(base.join("out")));
        let e = roots.check(missing, Access::Write).unwrap_err();
        // Windows resolves `..` by name first (so does opening the file there): then it's outside.
        assert!(e.contains("parent traversal below a folder that doesn't exist") || e.contains("outside the write root"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn relative_paths_resolve_against_the_working_directory() {
        let (base, roots) = folders("relative");
        // The tests' working directory is the crate's folder: not inside the roots.
        assert!(roots.check("a.scraft", Access::Read).unwrap_err().contains("outside the read root"));
        assert!(roots.check("", Access::Read).unwrap_err().contains("path is empty"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_missing_root_is_an_error_and_none_is_no_confinement() {
        let (base, _) = folders("missing");
        let e = AutomationRoots::new(&[&base.join("nope")], &[]).unwrap_err().to_string();
        assert!(e.starts_with("cannot open the automation read root"), "{e}");
        let e = AutomationRoots::new(&[], &[&base.join("in/a.scraft")]).unwrap_err().to_string();
        assert!(
            e.starts_with("cannot open the automation write root")
                && e.ends_with(&std::io::Error::from(std::io::ErrorKind::NotADirectory).to_string()),
            "{e}"
        );
        assert!(AutomationRoots::new(&[Path::new("")], &[]).unwrap_err().to_string().starts_with("cannot open the automation read root ``"));
        assert!(AutomationRoots::new(&[], &[]).unwrap().is_none());
        // One root alone grants only its own access.
        let read_only = AutomationRoots::new(&[&base.join("in")], &[]).unwrap().unwrap();
        let e = read_only.check(s(base.join("in/x.scraft")), Access::Write).unwrap_err();
        assert!(e.contains("write authority is absent"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn links_are_followed_before_comparing() {
        let (base, roots) = folders("links");
        #[cfg(unix)]
        let made = |target: &Path, link: &Path| std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        let made = |target: &Path, link: &Path| {
            let made =
                if target.is_dir() { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) };
            made.is_ok()
        };
        #[cfg(not(any(unix, windows)))]
        let made = |_: &Path, _: &Path| false;
        // Windows needs Developer Mode or elevation to make links: nothing to test without them.
        if !made(&base.join("outside/secret.scraft"), &base.join("in/leak.scraft")) {
            eprintln!("skipped: symbolic links can't be made here");
            let _ = std::fs::remove_dir_all(base);
            return;
        }
        assert!(roots.check(s(base.join("in/leak.scraft")), Access::Read).unwrap_err().contains("outside the read root"), "a file link out");
        assert!(made(&base.join("outside"), &base.join("out/away")));
        let e = roots.check(s(base.join("out/away/new.wav")), Access::Write).unwrap_err();
        assert!(e.contains("outside the write root"), "a folder link out: {e}");
        assert!(made(&base.join("outside/gone.scraft"), &base.join("out/dangling.wav")));
        let e = roots.check(s(base.join("out/dangling.wav")), Access::Write).unwrap_err();
        assert!(e.contains("a link that leads nowhere"), "writing would create the file outside: {e}");
        // A link to a folder inside the root stays inside.
        assert!(made(&base.join("in/sub"), &base.join("in/alias")));
        assert_eq!(roots.check(s(base.join("in/alias/x.scraft")), Access::Read), Ok(()));
        // A root given through a link is the folder it leads to.
        assert!(made(&base.join("in"), &base.join("in-link")));
        let linked = AutomationRoots::new(&[&base.join("in-link")], &[]).unwrap().unwrap();
        assert_eq!(linked.check(s(base.join("in/a.scraft")), Access::Read), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// #874: several roots of a kind: a path inside any of them passes, one inside none is refused
    /// naming them all, and read and write stay separate.
    #[test]
    fn several_roots_of_a_kind_each_grant_their_access() {
        let (base, _) = folders("several");
        std::fs::create_dir_all(base.join("out2")).unwrap();
        std::fs::write(base.join("in2/b.scraft"), "x").unwrap();
        std::fs::write(base.join("parent.scraft"), "x").unwrap();
        let roots = AutomationRoots::new(&[&base.join("in"), &base.join("in2")], &[&base.join("out"), &base.join("out2")]).unwrap().unwrap();
        assert_eq!(roots.check(s(base.join("in/a.scraft")), Access::Read), Ok(()));
        assert_eq!(roots.check(s(base.join("in2/b.scraft")), Access::Read), Ok(()));
        let e = roots.check(s(base.join("outside/secret.scraft")), Access::Read).unwrap_err();
        assert!(e.starts_with("automation path rejected: outside the read roots ") && e.contains(", "), "{e}");
        // Two sibling roots don't make their parent a root.
        assert!(roots.check(s(base.join("parent.scraft")), Access::Read).unwrap_err().contains("outside the read roots"));
        assert!(roots.check(s(base.clone()), Access::Read).is_err());
        assert_eq!(roots.check(s(base.join("out/x.wav")), Access::Write), Ok(()));
        assert_eq!(roots.check(s(base.join("out2/new/x.wav")), Access::Write), Ok(()));
        assert!(roots.check(s(base.join("outside/x.wav")), Access::Write).unwrap_err().contains("outside the write roots"));
        assert!(roots.check(s(base.join("x.wav")), Access::Write).is_err());
        assert!(roots.check(s(base.join("in2/b.scraft")), Access::Write).is_err());
        assert!(roots.check(s(base.join("out2/x.wav")), Access::Read).is_err());
        // `..` out of one root is outside, also when it lands next to another root.
        let sep = std::path::MAIN_SEPARATOR;
        let up = format!("{}{sep}..{sep}outside{sep}secret.scraft", s(base.join("in2")));
        assert!(roots.check(up, Access::Read).is_err());
        // `..` from one root into another stays inside.
        let across = format!("{}{sep}..{sep}in{sep}a.scraft", s(base.join("in2")));
        assert_eq!(roots.check(across, Access::Read), Ok(()));
        // A folder given twice (here once through `..`) counts once.
        let twice = AutomationRoots::new(&[&base.join("in"), &base.join("in/sub/..")], &[]).unwrap().unwrap();
        assert!(!twice.describe().contains(", ") && twice.describe().ends_with("; write none"), "{}", twice.describe());
        // A missing folder among several is an error.
        let e = AutomationRoots::new(&[&base.join("in"), &base.join("nope")], &[]).unwrap_err().to_string();
        assert!(e.starts_with("cannot open the automation read root"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A link inside one root that leads into another root of the same kind stays inside; one that
    /// leads out of all of them is outside.
    #[cfg(unix)]
    #[test]
    fn links_between_several_roots() {
        let (base, _) = folders("several-links");
        std::fs::write(base.join("in2/b.scraft"), "x").unwrap();
        std::os::unix::fs::symlink(base.join("in2/b.scraft"), base.join("in/to-in2.scraft")).unwrap();
        std::os::unix::fs::symlink(base.join("outside/secret.scraft"), base.join("in2/leak.scraft")).unwrap();
        std::os::unix::fs::symlink(&base, base.join("in2/up")).unwrap();
        let roots = AutomationRoots::new(&[&base.join("in"), &base.join("in2")], &[]).unwrap().unwrap();
        assert_eq!(roots.check(s(base.join("in/to-in2.scraft")), Access::Read), Ok(()));
        assert!(roots.check(s(base.join("in2/leak.scraft")), Access::Read).unwrap_err().contains("outside the read roots"));
        assert!(roots.check(s(base.join("in2/up/outside/secret.scraft")), Access::Read).unwrap_err().contains("outside the read roots"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_case_and_refuses_devices() {
        let (base, roots) = folders("case");
        let upper = s(base.join("IN/a.scraft"));
        assert_eq!(roots.check(&upper, Access::Read), Ok(()), "{upper}");
        assert_eq!(roots.check(s(base.join("In/Sub/New.scraft")), Access::Read), Ok(()));
        let e = roots.check(s(base.join("out/NUL")), Access::Write).unwrap_err();
        assert!(e.contains("reserved device names"), "{e}");
        assert!(roots.check(s(base.join("out/com1.txt")), Access::Write).is_err());
        assert_eq!(roots.check(s(base.join("out/console.txt")), Access::Write), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A path with no Windows prefix, a drive letter, and every path off Windows have no off-disk prefix to refuse.
    #[test]
    fn a_local_path_has_no_off_disk_prefix() {
        for p in ["/home/me/a.scraft", "a.scraft", r"C:\Projects\a.scraft", r"\\?\C:\Projects\a.scraft"] {
            assert!(OffDisk::of(Path::new(p)).is_none(), "{p}");
            assert_eq!(refused_prefix(Path::new(p), &[]), None, "{p}");
        }
    }

    /// A network, device or non-disk verbatim path is refused by its name before the file system is
    /// asked, so no host the agent names is contacted. The error names the prefix; it is not the
    /// error that opening the path would give. Nothing here touches the network.
    #[cfg(windows)]
    #[test]
    fn a_network_path_is_refused_before_it_is_opened() {
        let (base, roots) = folders("unc");
        let paths = [
            (r"\\203.0.113.5\s\x.scraft", "UNC"),
            (r"\\?\UNC\203.0.113.5\s\x.scraft", "UNC"),
            ("//203.0.113.5/s/x.scraft", "UNC"),
            (r"\\host@SSL\share\x.scraft", "UNC"),
            (r"\\host@SSL@443\share\x.scraft", "UNC"),
            (r"\\203.0.113.5", "UNC"),
            (r"\\\203.0.113.5\s\x.scraft", "UNC"),
            ("//203.0.113.5", "UNC"),
            (r"\\.\PhysicalDrive0", "device path"),
            (r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x.scraft", "verbatim path"),
        ];
        for (p, kind) in paths {
            for access in [Access::Read, Access::Write] {
                let e = roots.check(p, access).unwrap_err();
                assert!(e.starts_with("automation path rejected:"), "{p}: {e}");
                assert!(e.contains(kind), "{p}: {e}");
                assert!(!e.contains("cannot resolve") && !e.to_ascii_lowercase().contains("os error"), "{p}: {e}");
            }
        }
        let e = roots.check(s(base.join("outside/secret.scraft")), Access::Read).unwrap_err();
        assert!(e.contains("outside the read root"), "a local path is still judged by its folder: {e}");
        let verbatim = format!(r"\\?\{}", s(base.join("in").join("a.scraft")));
        assert_eq!(roots.check(&verbatim, Access::Read), Ok(()), "a verbatim disk path is a local path: {verbatim}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A root on a share lets that server and share through (either spelling, any ASCII case) and nothing else.
    #[cfg(windows)]
    #[test]
    fn a_root_on_a_share_lets_only_that_share_through() {
        let share = [Path::new(r"\\?\UNC\FileServer\Projects\work")];
        assert_eq!(refused_prefix(Path::new(r"\\fileserver\projects\a.scraft"), &share), None);
        assert_eq!(refused_prefix(Path::new(r"\\?\UNC\FILESERVER\PROJECTS\a.scraft"), &share), None);
        assert!(refused_prefix(Path::new(r"\\fileserver\secret\a.scraft"), &share).is_some(), "another share");
        assert!(refused_prefix(Path::new(r"\\203.0.113.5\Projects\a.scraft"), &share).is_some(), "another server");
        let work = [Path::new(r"\\?\UNC\work\Projects\w")];
        assert!(refused_prefix(Path::new("\\\\wor\u{212a}\\Projects\\a.scraft"), &work).is_some(), "U+212A is not k");
        assert!(refused_prefix(Path::new(r"\\.\PhysicalDrive0"), &share).is_some(), "a share is not a device");
        let device = [Path::new(r"\\.\PhysicalDrive0")];
        assert_eq!(refused_prefix(Path::new(r"\\.\physicaldrive0"), &device), None, "the same device");
        assert!(refused_prefix(Path::new(r"\\.\PhysicalDrive1"), &device).is_some());
    }

    /// A link whose text is a bare `\\server` (no share) is refused as written: joined onto the link's folder
    /// it would read as a local path and pass.
    #[cfg(windows)]
    #[test]
    fn a_link_to_a_bare_server_name_is_refused() {
        let (base, roots) = folders("unc-link-bare");
        let link = base.join("in").join("bare");
        if std::os::windows::fs::symlink_dir(r"\\203.0.113.5", &link).is_err() {
            eprintln!("skipped: symbolic links can't be made here");
            let _ = std::fs::remove_dir_all(base);
            return;
        }
        let e = roots.check(s(link.join("x.scraft")), Access::Read).unwrap_err();
        assert!(e.contains("UNC"), "{e}");
        assert!(!e.contains("cannot resolve") && !e.to_ascii_lowercase().contains("os error"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// Nine links that each go through `sub\..\` to the next are nine links, not a reason to give up: both
    /// spellings of each target are judged once.
    #[cfg(windows)]
    #[test]
    fn a_chain_of_links_with_parent_names_is_followed_in_few_steps() {
        let (base, roots) = folders("link-chain");
        let folder = base.join("in");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        for n in 1..=9 {
            let target = if n == 9 { "sub".to_string() } else { format!(r"sub\..\l{}", n + 1) };
            if std::os::windows::fs::symlink_dir(&target, folder.join(format!("l{n}"))).is_err() {
                eprintln!("skipped: symbolic links can't be made here");
                let _ = std::fs::remove_dir_all(base);
                return;
            }
        }
        assert_eq!(roots.check(s(folder.join("l1").join("x.scraft")), Access::Read), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    /// A link whose relative target goes through a missing folder and back (`missing\..\remote`) is
    /// judged with the `..` taken by name too, as Windows takes it: `remote` is a link to a network path.
    #[cfg(windows)]
    #[test]
    fn a_relative_link_through_a_missing_folder_still_reaches_the_network_check() {
        let (base, roots) = folders("unc-link-dotdot");
        let (remote, alias) = (base.join("in").join("remote"), base.join("in").join("alias"));
        if std::os::windows::fs::symlink_dir(r"\\203.0.113.5\s", &remote).is_err() {
            eprintln!("skipped: symbolic links can't be made here");
            let _ = std::fs::remove_dir_all(base);
            return;
        }
        assert!(std::os::windows::fs::symlink_dir(r"missing\..\remote", &alias).is_ok());
        let e = roots.check(s(alias.join("x.scraft")), Access::Read).unwrap_err();
        assert!(e.contains("UNC"), "{e}");
        assert!(!e.contains("cannot resolve") && !e.to_ascii_lowercase().contains("os error"), "{e}");
        let _ = std::fs::remove_dir_all(base);
    }

    /// A link whose text names a network path, or that has one as a folder on the way, is refused
    /// from that text before the link is followed: following it would contact the host.
    #[cfg(windows)]
    #[test]
    fn a_link_to_a_network_path_is_refused_before_it_is_followed() {
        let (base, roots) = folders("unc-link");
        let file_link = base.join("in/leak.scraft");
        if std::os::windows::fs::symlink_file(r"\\203.0.113.5\s\x.scraft", &file_link).is_err() {
            eprintln!("skipped: symbolic links can't be made here");
            let _ = std::fs::remove_dir_all(base);
            return;
        }
        let folder_link = base.join("in/folder-leak");
        assert!(std::os::windows::fs::symlink_dir(r"\\203.0.113.5\s", &folder_link).is_ok());
        for path in [file_link, folder_link.join("x.scraft")] {
            let e = roots.check(s(path.clone()), Access::Read).unwrap_err();
            assert!(e.contains("UNC"), "{}: {e}", path.display());
            assert!(!e.contains("cannot resolve") && !e.to_ascii_lowercase().contains("os error"), "{}: {e}", path.display());
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn scopes_nest_and_stay_on_their_thread() {
        let (base, roots) = folders("scopes");
        let outside = s(base.join("outside/secret.scraft"));
        assert_eq!(check_read(&outside), Ok(()), "nothing is confined by default");
        confine(Some(&roots), || {
            assert!(check_read(&outside).is_err());
            assert_eq!(unconfined(|| check_read(&outside)), Ok(()));
            assert!(check_read(&outside).is_err(), "restored after unconfined");
            let o = outside.clone();
            let there = std::thread::spawn(move || check_read(&o).is_ok()).join().unwrap();
            assert!(there, "another thread is not confined by this thread's scope");
        });
        assert_eq!(check_read(&outside), Ok(()), "restored after confine");
        let r = std::panic::catch_unwind(|| confine(Some(&roots), || panic!("boom")));
        assert!(r.is_err());
        assert_eq!(check_read(&outside), Ok(()), "restored after a panic");
        assert_eq!(confine(None, || check_read(&outside)), Ok(()));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn device_names() {
        for d in ["CON", "nul", "Com1", "LPT9.txt", "aux.scraft", "NUL "] {
            assert!(is_device_name(d), "{d}");
        }
        for f in ["console", "com10", "COM0", "lpt", "a.scraft", "nul-x"] {
            assert!(!is_device_name(f), "{f}");
        }
    }

    /// Every file a command reads or writes by path goes through `io` (`read_file`, `write_file`
    /// and the other checked helpers there), which check the roots. Engine code that reaches for
    /// the file system itself would bypass them, so none may: every file under `src/` except
    /// `io.rs`, this module and the test files. Audio is decoded into the session when it enters
    /// it (import, open, sampler load), so playback and bounce read nothing from disk and need no
    /// check of their own. Plug-in scans live in the host crates. The one engine module that reads
    /// and writes a fixed folder is `autosave.rs`: crash-recovery copies in the app's own data
    /// folder, which no command names, so it is skipped too.
    #[test]
    fn commands_do_not_touch_the_file_system_themselves() {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, files);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![];
        walk(&src, &mut files);
        assert!(files.iter().any(|f| f.ends_with("lib.rs")) && files.iter().any(|f| f.ends_with("cmd/file.rs")), "{files:?}");
        let mut offenders = vec![];
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // `io` and this module are the checked gateway; `autosave` writes the app's own
            // recovery folder; `tests*.rs` are test-only modules.
            if path.parent() == Some(src.as_path()) && matches!(name, "io.rs" | "file_access.rs" | "autosave.rs") || name.starts_with("tests") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Inline test modules (which build their own fixtures) come last in each file. Cut only
            // there: `#[cfg(test)]` also marks `use` lines and `mod tests;` declarations above code.
            let lines: Vec<&str> = text.lines().collect();
            let test_module =
                (0..lines.len()).find(|&i| {
                    lines[i].trim() == "#[cfg(test)]"
                        && lines[i + 1..].iter().map(|l| l.trim()).find(|l| !l.starts_with("#[")).is_some_and(|l| {
                            (l.starts_with("mod ") || l.starts_with("pub mod ") || l.starts_with("pub(crate) mod ")) && l.ends_with('{')
                        })
                });
            let code = lines[..test_module.unwrap_or(lines.len())].join("\n");
            for (n, line) in code.lines().enumerate() {
                let line = line.trim_start();
                let touches =
                    ["std::fs", "fs::", "File::open", "File::create", "OpenOptions", ".is_file()", ".exists()", ".is_dir()", "canonicalize"]
                        .iter()
                        .any(|w| line.contains(w));
                if !line.starts_with("//") && touches {
                    offenders.push(format!("{}:{}: {line}", path.display(), n + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "use the checked helpers in crate::io (read_file, write_file, ...): {offenders:#?}");
    }
}
