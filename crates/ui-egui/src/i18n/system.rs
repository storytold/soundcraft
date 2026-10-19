//! The desktop's UI-language preference, read from the standard locale environment once.
//!
//! SoundCraft takes no dependency on a platform locale API: the POSIX variables below are what
//! Linux, the BSDs and a macOS terminal session set, and [`LOCALE_OVERRIDE`] covers any other
//! case (Windows, a locked-down desktop, scripted runs). Nothing here runs a subprocess, so
//! startup cost and failure modes stay nil; a missing or unsupported preference means English.

use super::{Lang, lang_from_tag};

/// A tag to trust above the environment (`zh_CN.UTF-8`, `de-DE`). Handy for scripts and tests.
pub const LOCALE_OVERRIDE: &str = "SOUNDCRAFT_LOCALE";

/// At most this many tags are considered, each at most this long (an absurd environment must not
/// grow the candidate list without bound).
const MAX_SYSTEM_TAGS: usize = 64;
const MAX_SYSTEM_TAG_BYTES: usize = 128;

#[cfg(not(test))]
static SYSTEM: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Read the desktop's preference once, at startup. The platform entry point calls this before the
/// first frame; anything that does not (library tests, the offscreen renderer) draws English under
/// Auto, so no test depends on the machine's locale.
pub fn init_system_locale() {
    #[cfg(not(test))]
    {
        // A second call (two windows, a reloaded page) keeps the first answer.
        let _ = SYSTEM.set(detect_system_tags());
    }
}

/// Resolve Auto against the desktop's preference. Missing or unsupported values give English.
pub fn system_lang() -> Lang {
    #[cfg(test)]
    {
        TEST_SYSTEM_TAGS.with(|tags| resolve(&tags.borrow()))
    }
    #[cfg(not(test))]
    {
        resolve(SYSTEM.get().map_or(&[][..], Vec::as_slice))
    }
}

fn resolve(tags: &[String]) -> Lang {
    tags.iter().find_map(|tag| lang_from_tag(tag)).unwrap_or(Lang::EN)
}

/// Keep only plausible locale tags: no empties, no absurd lengths, no control characters or
/// paths (`LANG` is user-controlled input like any other).
fn bounded_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    tags.into_iter()
        .take(MAX_SYSTEM_TAGS)
        .filter(|tag| {
            !tag.trim().is_empty()
                && tag.len() <= MAX_SYSTEM_TAG_BYTES
                && tag.trim().bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'@'))
        })
        .map(|tag| tag.trim().to_owned())
        .collect()
}

/// The override, then `LANGUAGE` (a colon-separated priority list), then the POSIX precedence
/// `LC_ALL` › `LC_MESSAGES` › `LANG`.
#[cfg(not(test))]
fn detect_system_tags() -> Vec<String> {
    let mut tags = Vec::new();
    if let Ok(tag) = std::env::var(LOCALE_OVERRIDE)
        && !tag.trim().is_empty()
    {
        tags.extend(bounded_tags([tag]));
    }
    for name in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(name) {
            tags.extend(bounded_tags(value.split(':').map(str::to_owned)));
        }
    }
    tags
}

#[cfg(test)]
thread_local! {
    // Ordinary tests stay independent of the developer's locale without mutating the process
    // environment: startup tests supply the desktop's answer on their own thread.
    static TEST_SYSTEM_TAGS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn with_system_tags<R>(tags: &[&str], run: impl FnOnce() -> R) -> R {
    struct Restore(Vec<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_SYSTEM_TAGS.set(std::mem::take(&mut self.0));
        }
    }
    let _restore = Restore(TEST_SYSTEM_TAGS.replace(tags.iter().map(|tag| (*tag).to_owned()).collect()));
    run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_preferences_are_negotiated_in_order() {
        for (tags, expected) in [
            (vec!["zh_CN.UTF-8", "en_US.UTF-8"], "zh-Hans"),
            (vec!["zh-CN", "en-US"], "zh-Hans"),
            (vec!["zh_SG.UTF-8"], "zh-Hans"),
            (vec!["en_US.UTF-8"], "en"),
            (vec!["de_DE.UTF-8", "en_US.UTF-8"], "en"),
            (vec!["C"], "en"),
            (vec![], "en"),
        ] {
            with_system_tags(&tags, || assert_eq!(system_lang().code(), expected, "{tags:?}"));
        }
    }

    #[test]
    fn test_preferences_restore_and_stay_thread_local() {
        with_system_tags(&["zh_CN.UTF-8"], || {
            with_system_tags(&["en_GB.UTF-8"], || assert_eq!(system_lang(), Lang::EN));
            assert_eq!(system_lang().code(), "zh-Hans");
            std::thread::spawn(|| assert_eq!(system_lang(), Lang::EN)).join().unwrap();
            assert!(std::panic::catch_unwind(|| with_system_tags(&["zh_CN"], || panic!("unwind"))).is_err());
            assert_eq!(system_lang().code(), "zh-Hans");
        });
        assert_eq!(system_lang(), Lang::EN);
    }

    #[test]
    fn locale_tags_are_bounded_and_trimmed() {
        assert_eq!(bounded_tags(["  zh_CN.UTF-8  ".into(), "".into(), " ".into(), "zh-\0".into(), "x".repeat(129)]), ["zh_CN.UTF-8"]);
        assert_eq!(bounded_tags(std::iter::repeat_n("en_US".into(), 100)).len(), MAX_SYSTEM_TAGS);
    }
}
