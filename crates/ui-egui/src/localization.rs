//! Language selection and source-language fallback for UI strings.

#[path = "../locales/pt-BR/mod.rs"]
mod pt_br;

use std::cell::Cell;

thread_local! {
    static CURRENT_LANGUAGE: Cell<Language> = const { Cell::new(Language::English) };
}

/// A language supported by the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt-BR")]
    PortugueseBrazil,
}

impl Language {
    /// All supported languages, in the order they are shown in the language menu.
    pub const ALL: [Self; 2] = [Self::English, Self::PortugueseBrazil];

    /// Stable language tag used in preferences and control commands.
    pub const fn id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::PortugueseBrazil => "pt-BR",
        }
    }

    /// Language name displayed in the language's own form.
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::PortugueseBrazil => "Português (Brasil)",
        }
    }

    pub fn parse(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.id().eq_ignore_ascii_case(tag.trim()))
    }

    /// Rewrites the decimal point of a rendered number for this language.
    pub fn localize_decimal(self, number: &str) -> String {
        match self {
            Self::English => number.to_owned(),
            Self::PortugueseBrazil => number.replace('.', ","),
        }
    }

    /// Translates a UI string, falling back to its English source when no translation exists.
    pub fn text(self, source: &str) -> &str {
        match self {
            Self::English => source,
            Self::PortugueseBrazil => pt_br::translate(source).unwrap_or(source),
        }
    }
}

/// Sets the language used by text widgets on this thread for the duration of a UI frame.
pub fn enter(language: Language) -> LanguageScope {
    let previous = CURRENT_LANGUAGE.with(|current| current.replace(language));
    LanguageScope(previous)
}

/// The language selected for the current UI frame.
pub fn current_language() -> Language {
    CURRENT_LANGUAGE.with(Cell::get)
}

/// Translates a source string using the language selected for the current UI frame.
pub fn text(source: &str) -> &str {
    current_language().text(source)
}

/// Whether the Brazilian Portuguese catalog has an entry for `source`, even an identical one.
#[cfg(test)]
pub(crate) fn has_pt_br_entry(source: &str) -> bool {
    pt_br::translate(source).is_some()
}

/// Guard that restores the previously active language when dropped.
pub struct LanguageScope(Language);

impl Drop for LanguageScope {
    /// Restores the language that was active when the scope was created.
    fn drop(&mut self) {
        CURRENT_LANGUAGE.with(|language| language.set(self.0));
    }
}

/// One formatting argument, kept as a reference and rendered only when the template asks for it.
///
/// The template is only known at run time, so the format specifier is too: the formatter reads a
/// `:.N` precision from it and renders the value with that precision.
pub struct Arg<'a> {
    value: &'a dyn std::fmt::Display,
}

impl<'a> Arg<'a> {
    /// Wraps the argument without rendering it.
    pub fn new<T: std::fmt::Display>(value: &'a T) -> Self {
        Self { value }
    }

    /// Renders the argument for the given precision specifier (`.0` to `.3`), if any. A number
    /// given a precision uses the active language's decimal separator.
    fn render(&self, spec: Option<&str>) -> String {
        let precision = spec.and_then(|spec| spec.strip_prefix('.')).and_then(|digits| digits.parse::<usize>().ok()).filter(|n| *n <= 3);
        match precision {
            Some(n) => {
                let text = format!("{:.*}", n, self.value);
                if text.parse::<f64>().is_ok() { current_language().localize_decimal(&text) } else { text }
            }
            None => self.value.to_string(),
        }
    }
}

/// Formats the translated `source` with positional arguments. Placeholders may carry a `:.N`
/// precision, as in `format!`; other specifiers are ignored.
pub fn format_positional(source: &str, args: &[Arg<'_>]) -> String {
    format_template(source, args, &[], true)
}

/// Formats the translated `source` with named arguments (`name = value`).
pub fn format_named(source: &str, args: &[(&str, Arg<'_>)]) -> String {
    format_template(source, &[], args, false)
}

/// Shared implementation behind `format_positional` and `format_named`.
fn format_template(source: &str, positional: &[Arg<'_>], named: &[(&str, Arg<'_>)], positional_mode: bool) -> String {
    let template = text(source);
    let mut out = String::with_capacity(template.len());
    let mut next = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut field = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    field.push(ch);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&field);
                    break;
                }
                let (key, spec) = match field.split_once(':') {
                    Some((key, spec)) => (key, Some(spec)),
                    None => (field.as_str(), None),
                };
                let value = if positional_mode {
                    let value = positional.get(next);
                    next += 1;
                    value
                } else {
                    named.iter().find(|(name, _)| *name == key).map(|(_, value)| value)
                };
                match value {
                    Some(value) => out.push_str(&value.render(spec)),
                    None => {
                        out.push('{');
                        out.push_str(&field);
                        out.push('}');
                    }
                }
            }
            '}' => out.push('}'),
            ch => out.push(ch),
        }
    }
    out
}

#[macro_export]
macro_rules! localized_format {
    ($source:literal) => {
        $crate::localization::text($source).to_owned()
    };
    ($source:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        $crate::localization::format_named(
            $source,
            &[$( (stringify!($name), $crate::localization::Arg::new(&$value)) ),+],
        )
    }};
    ($source:literal, $($value:expr),+ $(,)?) => {{
        $crate::localization::format_positional(
            $source,
            &[$($crate::localization::Arg::new(&$value)),+],
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::{Arg, Language, enter, format_named, format_positional, text};

    #[test]
    fn language_tags_are_stable_and_round_trip() {
        for language in Language::ALL {
            assert_eq!(Language::parse(language.id()), Some(language));
            let encoded = serde_json::to_value(language).unwrap_or_default();
            let decoded: Language = serde_json::from_value(encoded).unwrap_or_default();
            assert_eq!(decoded, language);
        }
        assert_eq!(Language::parse(" PT-br "), Some(Language::PortugueseBrazil));
        assert_eq!(Language::parse("fr"), None);
    }

    #[test]
    fn language_translations_fall_back_to_the_english_source() {
        assert_eq!(Language::PortugueseBrazil.text("Appearance"), "Aparência");
        assert_eq!(Language::PortugueseBrazil.text("A source string with no catalog entry"), "A source string with no catalog entry");
        assert_eq!(Language::English.text("Appearance"), "Appearance");
    }

    #[test]
    fn language_scope_restores_the_previous_language() {
        assert_eq!(text("Appearance"), "Appearance");
        {
            let _scope = enter(Language::PortugueseBrazil);
            assert_eq!(text("Appearance"), "Aparência");
            {
                let _inner = enter(Language::English);
                assert_eq!(text("Appearance"), "Appearance");
            }
            assert_eq!(text("Appearance"), "Aparência");
        }
        assert_eq!(text("Appearance"), "Appearance");
    }

    #[test]
    fn portuguese_catalog_covers_every_menu_label() {
        for path in include_str!("../../engine/catalog/menus.txt").lines() {
            for label in path.split(" > ").map(str::trim).filter(|label| !label.is_empty()) {
                assert!(super::pt_br::translate(label).is_some(), "missing pt-BR menu label: {label}");
            }
        }
    }

    #[test]
    fn positional_templates_fill_in_order() {
        assert_eq!(format_positional("{} of {}", &[Arg::new(&1), Arg::new(&"two")]), "1 of two");
        assert_eq!(format_positional("no fields", &[Arg::new(&1)]), "no fields");
    }

    #[test]
    fn named_templates_fill_by_name_in_any_order() {
        assert_eq!(format_named("{b}-{a}-{b}", &[("a", Arg::new(&1)), ("b", Arg::new(&2))]), "2-1-2");
    }

    #[test]
    fn precision_specifiers_are_honoured() {
        let value = 3.14259_f64;
        for (template, expected) in [("{:.0}", "3"), ("{:.1}", "3.1"), ("{:.2}", "3.14"), ("{:.3}", "3.143"), ("{}", "3.14259")] {
            assert_eq!(format_positional(template, &[Arg::new(&value)]), expected, "{template}");
        }
    }

    #[test]
    fn braces_can_be_escaped() {
        assert_eq!(format_positional("{{{}}}", &[Arg::new(&5)]), "{5}");
    }

    #[test]
    fn missing_unknown_and_unclosed_fields_are_kept_verbatim() {
        assert_eq!(format_positional("{} {}", &[Arg::new(&1)]), "1 {}");
        assert_eq!(format_named("{nope}", &[("a", Arg::new(&1))]), "{nope}");
        assert_eq!(format_positional("open {", &[]), "open {");
        assert_eq!(format_positional("tail {abc", &[]), "tail {abc");
    }

    #[test]
    fn portuguese_numbers_use_a_decimal_comma_only_with_a_precision() {
        let _scope = enter(Language::PortugueseBrazil);
        assert_eq!(format_positional("{:.1} dB", &[Arg::new(&-6.04_f64)]), "-6,0 dB");
        assert_eq!(format_positional("{}", &[Arg::new(&"v1.2")]), "v1.2");
        let _english = enter(Language::English);
        assert_eq!(format_positional("{:.1}", &[Arg::new(&2.5_f64)]), "2.5");
    }

    /// Parses a Rust string literal at the start of `s`; returns its value and the rest.
    fn literal(s: &str) -> Option<(String, &str)> {
        let mut chars = s.strip_prefix('"')?.char_indices();
        let mut out = String::new();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &s[i + 2..])),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Every `"source" => "translation"` pair of the catalog, read from its source file.
    fn catalog_entries() -> Vec<(String, String)> {
        let text = include_str!("../locales/pt-BR/mod.rs");
        let mut lines = text.lines().map(str::trim);
        let mut entries = Vec::new();
        while let Some(line) = lines.next() {
            let Some((key, rest)) = literal(line) else { continue };
            let Some(rest) = rest.trim_start().strip_prefix("=>") else { continue };
            let rest = rest.trim();
            let value =
                if rest.starts_with('{') { lines.next().and_then(|next| literal(next).map(|(v, _)| v)) } else { literal(rest).map(|(v, _)| v) };
            if let Some(value) = value {
                entries.push((key, value));
            }
        }
        entries
    }

    fn placeholders(template: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'{') {
                chars.next();
            } else if c == '{' {
                let field: String = chars.by_ref().take_while(|c| *c != '}').collect();
                found.push(field);
            }
        }
        found.sort();
        found
    }

    #[test]
    fn catalog_entries_are_parsed_and_unique() {
        let entries = catalog_entries();
        assert!(entries.len() > 1000, "parsed only {} catalog entries", entries.len());
        let mut keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "duplicate catalog keys");
    }

    #[test]
    fn catalog_translations_keep_the_english_placeholders() {
        for (source, translation) in catalog_entries() {
            assert_eq!(placeholders(&source), placeholders(&translation), "placeholders differ for {source:?}");
            assert_eq!(super::pt_br::translate(&source), Some(translation.as_str()), "lookup differs for {source:?}");
        }
    }

    #[test]
    fn every_localized_source_string_has_a_catalog_entry() {
        fn visit(dir: &std::path::Path, missing: &mut Vec<String>) {
            let Ok(read) = std::fs::read_dir(dir) else { return };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, missing);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("localization.rs") {
                    let Ok(text) = std::fs::read_to_string(&path) else { continue };
                    for marker in ["localization::text(\"", "localized_format!(\""] {
                        for (at, _) in text.match_indices(marker) {
                            let start = at + marker.len() - 1;
                            if let Some((source, _)) = literal(&text[start..])
                                && super::pt_br::translate(&source).is_none()
                            {
                                missing.push(format!("{}: {source:?}", path.display()));
                            }
                        }
                    }
                }
            }
        }
        let mut missing = Vec::new();
        visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut missing);
        assert!(missing.is_empty(), "strings without a pt-BR entry:\n{}", missing.join("\n"));
    }
}
