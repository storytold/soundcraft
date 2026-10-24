//! Interface translations for the menu bar and dialog titles. Menu labels are the lookup keys;
//! command ids, drawing data, file names and automation stay unchanged. Untranslated labels fall
//! back to English, so coverage can grow incrementally.
//!
//! The interface language is a preference (`UiState::language`, default "auto"): "auto" follows
//! the operating system's language, otherwise it is a registered catalog code.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::OnceLock;

mod de;
mod es;
mod fr;
mod pt;
mod ru;

/// Every language with a catalog, in the order the language picker lists them.
pub const CATALOG_CODES: [&str; 6] = ["en", "ru", "fr", "de", "es", "pt"];

thread_local! {
    // Scoped to a single app's render. Headless calls and parallel tests default to English.
    static CURRENT: Cell<&'static str> = const { Cell::new("en") };
}

/// The language the next frame draws in; set from the stored preference each frame.
pub fn set_current(language: &'static str) {
    CURRENT.set(language);
}

/// The language `tr` is using.
pub fn current() -> &'static str {
    CURRENT.with(Cell::get)
}

/// Resolve the `language` setting: a catalog code, or "auto" (and anything unknown, e.g. a code
/// from a newer version) to follow the system.
pub fn resolve(stored: &str) -> &'static str {
    match stored {
        "auto" | "" => system_language(),
        code => code_from(code).unwrap_or("en"),
    }
}

fn code_from(stored: &str) -> Option<&'static str> {
    CATALOG_CODES.iter().copied().find(|c| *c == stored)
}

/// The system's preferred interface language where there is a catalog, else English.
fn system_language() -> &'static str {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static LANGUAGE: OnceLock<&'static str> = OnceLock::new();
        *LANGUAGE.get_or_init(|| {
            sys_locale::get_locale()
                .as_deref()
                .and_then(|tag| tag.split(['-', '_']).next())
                .and_then(code_from)
                .unwrap_or("en")
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        "en"
    }
}

type Catalog = HashMap<&'static str, &'static str>;

fn catalog(language: &str) -> Option<&'static Catalog> {
    static RU: OnceLock<Catalog> = OnceLock::new();
    static FR: OnceLock<Catalog> = OnceLock::new();
    static DE: OnceLock<Catalog> = OnceLock::new();
    static ES: OnceLock<Catalog> = OnceLock::new();
    static PT: OnceLock<Catalog> = OnceLock::new();
    match language {
        "ru" => Some(RU.get_or_init(|| ru::TABLE.iter().copied().collect())),
        "fr" => Some(FR.get_or_init(|| fr::TABLE.iter().copied().collect())),
        "de" => Some(DE.get_or_init(|| de::TABLE.iter().copied().collect())),
        "es" => Some(ES.get_or_init(|| es::TABLE.iter().copied().collect())),
        "pt" => Some(PT.get_or_init(|| pt::TABLE.iter().copied().collect())),
        _ => None,
    }
}

/// Translate a menu label; the label itself when there is no catalog entry.
pub fn tr(text: &str) -> &str {
    catalog(current()).and_then(|c| c.get(text)).copied().unwrap_or(text)
}
