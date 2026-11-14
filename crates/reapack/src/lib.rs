#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! SoundCraft's ReaPack client.
//!
//! ReaPack is the package manager REAPER users get JSFX effects from. A repository is a public
//! `index.xml` (format version 1) listing packages, their versions and the files to download. This
//! crate reads those indexes, searches them, and installs an effect's files into SoundCraft's own
//! Effects folder (never REAPER's), where `soundcraft-jsfx-host` finds them.
//!
//! Downloaded files are untrusted input, so:
//! - only `https://` URLs are used, with size caps and timeouts;
//! - a `hash` in the index (multihash SHA-256) is verified, and an unknown hash algorithm refuses
//!   the install;
//! - install paths are built from the index name, category and file name, and anything that could
//!   leave the Effects folder (`..`, absolute paths, drive letters, backslashes) is refused;
//! - files are written next to their destination first and renamed into place only when every
//!   download succeeded, so a failed install leaves nothing half-written;
//! - an installed effect can read and write nothing but its audio: the JSFX interpreter has no
//!   file, network or process access.
//!
//! Downloading goes through the [`Fetch`] trait; the default runs the system `curl` (see `fetch`).

mod fetch;
mod index;
mod manager;
mod sha256;
mod xml;

pub use fetch::{CurlFetch, Fetch};
pub use index::{Index, Package, Source, Version, compare_versions, parse as parse_index};
pub use manager::{DEFAULT_REPO_URL, Hit, Installed, Manager, RefreshOutcome, Repo, RepoInfo, system};

/// Why a ReaPack operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReapackError {
    #[error("network error: {0}")]
    Network(String),
    #[error("cannot read the index: {0}")]
    Parse(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("refused: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("checksum mismatch for {url}: expected {expected}, got {got}")]
    Hash { url: String, expected: String, got: String },
    #[error("i/o error: {0}")]
    Io(String),
}

/// An `https://` URL with a host and no spaces or control characters.
pub(crate) fn valid_url(u: &str) -> bool {
    let Some(rest) = u.strip_prefix("https://") else { return false };
    u.len() <= 2048 && !rest.is_empty() && !rest.starts_with('/') && !u.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\\')
}

/// The components of a relative path that stays inside the folder it is joined to; `None` for
/// anything else (empty parts, `.`/`..`, drive letters, backslashes, NUL, control characters,
/// Windows device names).
pub(crate) fn safe_rel(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.len() > 400 || path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let ok = parts.len() <= 12
        && parts.iter().all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.trim() == *p
                && !p.ends_with('.')
                && !p.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
                && !windows_device(p)
        });
    ok.then_some(parts)
}

/// A name Windows maps to a device whatever its extension (`CON`, `nul.jsfx`, `COM1.txt`, `LPT²`):
/// writing to it would talk to the device instead of creating a file.
fn windows_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        s => {
            (s.starts_with("COM") || s.starts_with("LPT"))
                && s.get(3..).is_some_and(|d| matches!(d, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
        }
    }
}

/// A repository name turned into one safe folder name.
pub(crate) fn folder_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let s = s.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if s.is_empty() {
        "repository".to_string()
    } else if windows_device(&s) {
        format!("_{s}")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_must_be_plain_https() {
        assert!(valid_url("https://example.com/a/index.xml?x=1"));
        for bad in [
            "http://example.com/x",
            "ftp://x/y",
            "file:///etc/passwd",
            "https://",
            "https:///x",
            "https://a b/c",
            "https://a/\"b",
            "https://a\\b",
            "",
        ] {
            assert!(!valid_url(bad), "{bad}");
        }
        assert!(!valid_url(&format!("https://x/{}", "a".repeat(3000))));
    }

    #[test]
    fn relative_paths_cannot_escape() {
        assert_eq!(safe_rel("Repo/Dynamics/comp.jsfx").map(|p| p.len()), Some(3));
        for bad in ["", "/etc/x", "../x", "a/../x", "a/./x", "a//x", "a\\b", "C:/x", "a/b:c", "a/b\0", "a/ b", "a/b.", "a/*", &"a/".repeat(20)] {
            assert!(safe_rel(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn windows_device_names_are_refused_on_every_platform() {
        for bad in ["CON", "a/nul", "a/NUL.jsfx", "Aux.txt/x", "a/com1", "a/LPT9.jsfx-inc", "a/COM¹", "a/con .txt", "a/CONOUT$"] {
            assert!(safe_rel(bad).is_none(), "{bad:?}");
        }
        for ok in ["a/console.jsfx", "a/nullify", "a/COM10", "a/comp.jsfx", "a/LPT", "a/auxiliary.jsfx"] {
            assert!(safe_rel(ok).is_some(), "{ok:?}");
        }
        assert_eq!(folder_name("NUL"), "_NUL");
        assert!(safe_rel(&folder_name("com3.repo")).is_some());
    }

    #[test]
    fn folder_names_are_flattened() {
        assert_eq!(folder_name("ReaTeam JSFX"), "ReaTeam JSFX");
        assert_eq!(folder_name("../../x"), "_.._x");
        assert!(safe_rel(&folder_name("..")).is_some());
        assert_eq!(folder_name("  "), "repository");
        assert_eq!(folder_name("a/b\\c"), "a_b_c");
    }
}
