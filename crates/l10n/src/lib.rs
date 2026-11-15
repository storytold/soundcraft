//! Interface languages: the language list, the current-language global and the translation
//! catalogs. The English source string is the catalog key, so a string a language has no entry
//! for falls back to itself — features keep working in English until they are translated.
//!
//! Translation happens where strings are *displayed* (menus, toolbar, dialogs); command ids,
//! menu paths and the labels agents read stay English, because they are also match keys.
//!
//! L0: no dependencies, usable from the engine, the UI and the web app.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

mod zh_hans;

/// A supported interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    /// English, the source language every catalog key is written in.
    #[default]
    English,
    /// 简体中文 (Simplified Chinese).
    SimplifiedChinese,
}

impl Language {
    /// Every supported language, in menu order.
    pub const ALL: [Language; 2] = [Language::English, Language::SimplifiedChinese];

    /// BCP 47 id, as stored in preferences and taken by `ui.language`.
    pub fn id(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::SimplifiedChinese => "zh-Hans",
        }
    }

    /// The language's name in its own language, for the Language menu.
    pub fn native_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::SimplifiedChinese => "简体中文",
        }
    }

    /// Parses an id from preferences or a control request; `zh`, `zh-CN` and `zh-Hans-CN` mean
    /// zh-Hans, the only Chinese the interface ships.
    pub fn parse(id: &str) -> Option<Language> {
        match id.trim().to_ascii_lowercase().as_str() {
            "en" | "en-us" => Some(Language::English),
            "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" => Some(Language::SimplifiedChinese),
            _ => None,
        }
    }

    /// The translation of the English source string `s`, or `s` itself when this language has no
    /// entry for it. `s` must be a source literal so English can return it directly.
    pub fn translate(self, s: &'static str) -> &'static str {
        match self {
            Language::English => s,
            Language::SimplifiedChinese => zh_hans::get(s).unwrap_or(s),
        }
    }

    /// [`Language::translate`] for strings built at run time (e.g. "Undo {step}"); never
    /// allocates — translations are `&'static str`, and the fallback borrows the input.
    pub fn translate_dyn<'a>(self, s: &'a str) -> Cow<'a, str> {
        match self {
            Language::English => Cow::Borrowed(s),
            Language::SimplifiedChinese => zh_hans::get(s).map(Cow::Borrowed).unwrap_or_else(|| Cow::Borrowed(s)),
        }
    }

    /// Whether this language has a translation for `s` (English always has: it is the source).
    /// Lets tests require catalog coverage.
    pub fn covers(self, s: &str) -> bool {
        match self {
            Language::English => true,
            Language::SimplifiedChinese => zh_hans::get(s).is_some(),
        }
    }
}

/// The interface language as a discriminant, for the atomic.
fn rank(lang: Language) -> u8 {
    match lang {
        Language::English => 0,
        Language::SimplifiedChinese => 1,
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The language the interface renders in. English until the app sets it from its saved
/// preferences; `ui.language` changes it live, including from the control channel.
pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::SimplifiedChinese,
        _ => Language::English,
    }
}

/// Sets the language the interface renders in (see [`current`]).
pub fn set_current(lang: Language) {
    CURRENT.store(rank(lang), Ordering::Relaxed);
}

/// Translates `s` (an English source literal) into the current language.
pub fn t(s: &'static str) -> &'static str {
    current().translate(s)
}

/// Translates a run-time string (e.g. "Undo {step}") into the current language.
pub fn td(s: &str) -> Cow<'_, str> {
    current().translate_dyn(s)
}

/// Translates a template containing `{name}` placeholders (e.g. `"Undo {step}"`) and fills them
/// in. The values themselves are not translated. Allocates only when the template has args.
pub fn tf(s: &'static str, args: &[(&str, &str)]) -> Cow<'static, str> {
    let mut out = Cow::Borrowed(t(s));
    for (name, value) in args {
        let pattern = format!("{{{name}}}");
        out = Cow::Owned(out.replace(&pattern, value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_round_trip() {
        for lang in Language::ALL {
            assert_eq!(Language::parse(lang.id()), Some(lang));
            assert_eq!(Language::parse(&lang.id().to_uppercase()), Some(lang), "ids parse case-insensitively");
            assert!(!lang.native_name().is_empty());
        }
        assert_eq!(Language::parse("zh"), Some(Language::SimplifiedChinese));
        assert_eq!(Language::parse("zh-CN"), Some(Language::SimplifiedChinese));
        assert_eq!(Language::parse("zh-Hant"), None, "we ship no Traditional Chinese");
        assert_eq!(Language::parse("de"), None);
        assert_eq!(Language::parse(""), None);
    }

    #[test]
    fn ids_are_unique() {
        let ids: Vec<_> = Language::ALL.iter().map(|l| l.id()).collect();
        assert_eq!(ids.len(), std::collections::HashSet::<_>::from_iter(ids.iter().copied()).len());
    }

    #[test]
    fn english_is_the_identity() {
        for s in ["File", "Bounce Mix...", ""] {
            assert_eq!(Language::English.translate(s), s);
            assert_eq!(Language::English.translate_dyn(s).as_ref(), s);
        }
    }

    /// Every zh-Hans entry is a real translation: non-empty, no stray whitespace. An entry
    /// identical to its key is allowed and means "stays English on purpose" (product names,
    /// channel formats, Ambisonics orders); the menu-coverage test relies on that marking.
    #[test]
    fn zh_hans_entries_are_well_formed() {
        for (en, zh) in zh_hans::ENTRIES {
            assert!(!zh.is_empty(), "{en}: empty translation");
            assert_eq!(zh.trim(), *zh, "{en}: translation has stray whitespace: {zh:?}");
            assert!(!en.starts_with(' ') && !en.ends_with(' '), "key has stray whitespace: {en:?}");
        }
    }

    /// Keys stay in sync with the catalog match: ENTRIES and `get` must cover the same strings.
    #[test]
    fn zh_hans_lookup_agrees_with_entries() {
        for (en, zh) in zh_hans::ENTRIES {
            assert_eq!(zh_hans::get(en), Some(*zh), "get() disagrees with ENTRIES for {en:?}");
        }
        assert_eq!(zh_hans::get("no such key"), None);
    }

    /// `get` builds a map, so a duplicate key would silently replace its twin.
    #[test]
    fn zh_hans_has_no_duplicate_keys() {
        assert_eq!(zh_hans::ENTRIES.len(), zh_hans::ENTRIES.iter().map(|(en, _)| *en).collect::<std::collections::HashSet<_>>().len());
    }

    /// Templates fill their placeholders. (Sets the global language; other tests pass explicit
    /// `Language` values, so the global doesn't affect them.)
    #[test]
    fn templates_fill_placeholders() {
        set_current(Language::SimplifiedChinese);
        assert_eq!(tf("Undo {step}", &[("step", "剪切")]), "撤销 剪切");
        assert_eq!(tf("No template here", &[]), "No template here");
        // An unknown template falls back to English and still fills in.
        assert_eq!(tf("No {x} entry", &[("x", "zh")]), "No zh entry");
    }
}
