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

    /// Translates a UI string, falling back to its English source when no translation exists.
    pub fn text<'a>(self, source: &'a str) -> &'a str {
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

/// Translates a source string using the language selected for the current UI frame.
pub fn text(source: &str) -> &str {
    CURRENT_LANGUAGE.with(|language| language.get().text(source))
}

pub struct LanguageScope(Language);

impl Drop for LanguageScope {
    fn drop(&mut self) {
        CURRENT_LANGUAGE.with(|language| language.set(self.0));
    }
}

/// One formatting argument: its default rendering plus fixed-precision variants.
///
/// The template is only known at run time, so the format specifier is too. Building the precision
/// variants up front lets the formatter honour `{:.0}`–`{:.3}` while evaluating the call site's
/// arguments exactly once.
pub struct Arg {
    text: String,
    fixed: [String; 4],
}

impl Arg {
    pub fn new<T: std::fmt::Display + ?Sized>(value: &T) -> Self {
        Self { text: format!("{value}"), fixed: [format!("{value:.0}"), format!("{value:.1}"), format!("{value:.2}"), format!("{value:.3}")] }
    }

    fn render(&self, spec: Option<&str>) -> &str {
        match spec.and_then(|spec| spec.strip_prefix('.')) {
            Some(digits) => digits.parse::<usize>().ok().and_then(|n| self.fixed.get(n)).map_or(self.text.as_str(), String::as_str),
            None => &self.text,
        }
    }
}

/// Formats the translated `source` with positional arguments. Placeholders may carry a `:.N`
/// precision, as in `format!`; other specifiers are ignored.
pub fn format_positional(source: &str, args: &[Arg]) -> String {
    format_template(source, args, &[], true)
}

/// Formats the translated `source` with named arguments (`name = value`).
pub fn format_named(source: &str, args: &[(&str, Arg)]) -> String {
    format_template(source, &[], args, false)
}

fn format_template(source: &str, positional: &[Arg], named: &[(&str, Arg)], positional_mode: bool) -> String {
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
                    Some(value) => out.push_str(value.render(spec)),
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
    use super::Language;

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
}
