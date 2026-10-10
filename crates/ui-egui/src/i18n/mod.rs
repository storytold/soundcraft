//! Interface language. English is the source language: every visible string is written in English
//! in the code and passed through [`tr`], which looks it up in the [`strings`] table for the chosen
//! language and falls back to English when a string has no translation.
//!
//! Only presentation is translated. Command ids, menu paths used for dispatch, saved sessions and
//! the control channel stay in English, so scripts and agents behave the same in every language.

mod strings;

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

/// The Language setting (`ui.language {lang}`). English by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Fr,
    Es,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::En, Self::Fr, Self::Es];

    /// The language's own name, shown untranslated so anyone can find theirs.
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Fr => "Français",
            Self::Es => "Español",
        }
    }

    /// The id used in `ui.language {"lang": …}` and in the saved UI prefs.
    pub const fn id(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
            Self::Es => "es",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL.into_iter().find(|l| l.id().eq_ignore_ascii_case(s) || l.native_name().eq_ignore_ascii_case(s))
    }
}

thread_local! {
    // Per thread: the UI draws on one thread, and parallel tests don't see each other's language.
    static CURRENT: Cell<Language> = const { Cell::new(Language::En) };
}

/// Switch the interface language for this (the UI) thread.
pub fn set(lang: Language) {
    CURRENT.with(|c| c.set(lang));
}

pub fn current() -> Language {
    CURRENT.with(Cell::get)
}

fn table(lang: Language) -> Option<&'static HashMap<&'static str, &'static str>> {
    static FR: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static ES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    let (cell, pick): (_, fn(&(&'static str, &'static str, &'static str)) -> &'static str) = match lang {
        Language::En => return None,
        Language::Fr => (&FR, |r| r.1),
        Language::Es => (&ES, |r| r.2),
    };
    Some(cell.get_or_init(|| strings::STRINGS.iter().map(|r| (r.0, pick(r))).collect()))
}

/// `s` (English) in the current language, or `s` itself when it has no translation.
pub fn tr(s: &str) -> &str {
    tr_in(current(), s)
}

/// `s` (English) in `lang`.
pub fn tr_in(lang: Language, s: &str) -> &str {
    match table(lang).and_then(|t| t.get(s)) {
        Some(t) if !t.is_empty() => t,
        _ => s,
    }
}

/// Translate a template, then fill its `{}` placeholders in order (`{{` and `}}` are literal braces).
pub fn trf(template: &str, args: &[&dyn Display]) -> String {
    fill(tr(template), args)
}

fn fill(template: &str, args: &[&dyn Display]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(template.len() + 8 * args.len());
    let mut args = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('{', Some('}')) => {
                chars.next();
                if let Some(a) = args.next() {
                    let _ = write!(out, "{a}");
                }
            }
            ('{', Some('{')) | ('}', Some('}')) => {
                chars.next();
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
