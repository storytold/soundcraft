//! Interface preferences: shell-owned settings that are not part of the session document.
//!
//! The engine stores and validates the values; it never touches the filesystem (the web build has
//! none). Each frontend persists [`Preferences::to_json`] and restores [`Preferences::from_json`].
//! A missing value keeps its default, and a file written by a newer build loads with its unknown
//! keys ignored, so one bad preference never costs the user the rest.
//!
//! The values are sent to the shell as a lowercase code (or `auto`); the engine does not know which
//! languages the shell ships. An unregistered code is accepted here and resolves to `auto` there.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Dotted path of the interface language preference.
pub const LANGUAGE: &str = "interface.language";

/// Longest accepted language value: longer than any registered code (`zh-hans`, `pt-br`, …).
pub const MAX_LANGUAGE_BYTES: usize = 16;

/// Largest preferences document a frontend reads or writes.
pub const MAX_BYTES: u64 = 64 * 1024;

/// Preferences shared by every frontend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub interface: Interface,
}

/// The interface section of the preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Interface {
    /// UI language: `auto` (follow the system) or a language code such as `en` or `zh-hans`.
    /// Only display text changes; command ids, menu paths and the control protocol stay English.
    /// A code the shell does not know falls back to `auto`.
    pub language: String,
}

impl Default for Interface {
    fn default() -> Self {
        Self { language: "auto".to_owned() }
    }
}

impl Preferences {
    /// The value at `path` as JSON: everything for `""`, the section object for `"interface"`, the
    /// leaf for `"interface.language"`. `None` for an unknown path.
    pub fn get(&self, path: &str) -> Option<Value> {
        match path {
            "" => Some(json!({"interface": {"language": self.interface.language}})),
            "interface" => Some(json!({"language": self.interface.language})),
            LANGUAGE => Some(Value::String(self.interface.language.clone())),
            _ => None,
        }
    }

    /// Validate and set the leaf at `path`. An unknown path or an invalid value changes nothing.
    pub fn set(&mut self, path: &str, value: Value) -> std::result::Result<(), String> {
        check_value(path, &value)?;
        if path != LANGUAGE {
            return Err(format!("unknown preference `{path}`"));
        }
        let language = value.as_str().ok_or_else(|| format!("`{LANGUAGE}` must be `auto` or a language code such as `en` or `zh-hans`"))?;
        self.interface.language.clear();
        self.interface.language.push_str(language);
        Ok(())
    }

    /// Reset everything (`None` or `""`), one section (`"interface"`), or one value.
    pub fn reset(&mut self, path: Option<&str>) -> std::result::Result<(), String> {
        match path.map(str::trim).filter(|path| !path.is_empty()) {
            None => {
                *self = Self::default();
                Ok(())
            }
            Some("interface") => {
                self.interface = Interface::default();
                Ok(())
            }
            Some(LANGUAGE) => {
                self.interface.language = Interface::default().language;
                Ok(())
            }
            Some(other) => Err(format!("unknown preference `{other}`")),
        }
    }

    /// Pretty JSON for the frontend's preferences file.
    pub fn to_json(&self) -> std::result::Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    /// Leniently load a frontend's file. Malformed JSON falls back to the defaults and each value is
    /// recovered independently, so one bad preference cannot discard the others; unknown keys are
    /// ignored so a file written by a newer build still loads. Returns the load warnings for the log.
    pub fn from_json(bytes: &[u8]) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        if bytes.len() as u64 > MAX_BYTES {
            warnings.push(format!("Preferences exceed the {} KiB limit; using defaults", MAX_BYTES / 1024));
            return (Self::default(), warnings);
        }
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            // A fresh profile writes nothing yet; an empty file is not a broken one.
            return (Self::default(), warnings);
        }
        let value: Value = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("Preferences could not be read ({error}); using defaults"));
                return (Self::default(), warnings);
            }
        };
        let Some(root) = value.as_object() else {
            warnings.push("Preferences must be a JSON object; using defaults".to_owned());
            return (Self::default(), warnings);
        };
        let mut prefs = Self::default();
        match root.get("interface") {
            None => {}
            Some(section) => match section.as_object() {
                Some(section) => {
                    if let Some(value) = section.get("language") {
                        match prefs.set(LANGUAGE, value.clone()) {
                            Ok(()) => {}
                            Err(error) => {
                                warnings.push(format!("{error}; using `{}`", prefs.interface.language));
                            }
                        }
                    }
                }
                None => warnings.push("`interface` must be an object; using its defaults".to_owned()),
            },
        }
        (prefs, warnings)
    }
}

/// Whether `language` is a value the preference accepts: `auto` or a non-empty code of at most
/// [`MAX_LANGUAGE_BYTES`] ASCII letters, digits or `-`.
pub fn valid_language(language: &str) -> bool {
    !language.is_empty() && language.len() <= MAX_LANGUAGE_BYTES && language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Validate `value` for `path` without changing anything.
pub fn check_value(path: &str, value: &Value) -> std::result::Result<(), String> {
    match path {
        LANGUAGE => {
            if value.as_str().is_some_and(valid_language) {
                Ok(())
            } else {
                Err(format!("`{LANGUAGE}` must be `auto` or a language code such as `en` or `zh-hans`"))
            }
        }
        "" => Err("`path` must name a preference such as `interface.language`".to_owned()),
        "interface" => Err(format!("`interface` is a section; set a preference such as `{LANGUAGE}`")),
        other => Err(format!("unknown preference `{other}`")),
    }
}

/// The engine's preferences plus a change counter the apps key their saves on.
#[derive(Debug, Default)]
pub struct Store {
    values: Preferences,
    revision: u64,
}

impl Store {
    pub fn new(values: Preferences) -> Self {
        Self { values, revision: 0 }
    }

    pub fn get(&self) -> &Preferences {
        &self.values
    }

    /// Bumps on every change, so a frontend saves once per edit and compares revisions at startup.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Run `edit` and bump the revision once; a gesture that sets several values is one save.
    pub fn edit<R>(&mut self, edit: impl FnOnce(&mut Preferences) -> R) -> R {
        let out = edit(&mut self.values);
        self.revision = self.revision.wrapping_add(1);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_language_is_auto() {
        let prefs = Preferences::default();
        assert_eq!(prefs.interface.language, "auto");
        assert_eq!(prefs.get(LANGUAGE), Some(json!("auto")));
        assert_eq!(prefs.get("interface"), Some(json!({"language": "auto"})));
        assert_eq!(prefs.get(""), Some(json!({"interface": {"language": "auto"}})));
    }

    #[test]
    fn accepts_auto_and_language_codes() {
        for code in ["auto", "en", "zh-hans", "ja", "pt-br", "zh-Hans"] {
            assert_eq!(check_value(LANGUAGE, &json!(code)), Ok(()), "{code}");
        }
        assert_eq!(check_value(LANGUAGE, &json!("a".repeat(MAX_LANGUAGE_BYTES))), Ok(()));
    }

    #[test]
    fn rejects_bad_language_values() {
        let expected = format!("`{LANGUAGE}` must be `auto` or a language code such as `en` or `zh-hans`");
        for value in [
            json!(""),
            json!(null),
            json!(42),
            json!(true),
            json!(["en"]),
            json!("zh hans"),
            json!("zh_hans"),
            json!("zh.hans"),
            json!("中文"),
            json!("a".repeat(MAX_LANGUAGE_BYTES + 1)),
        ] {
            assert_eq!(check_value(LANGUAGE, &value), Err(expected.clone()), "{value}");
        }
    }

    #[test]
    fn valid_language_is_the_single_source_of_the_rule() {
        for language in ["auto", "en", "zh-hans", "zh-Hans", "pt-br", "a"] {
            assert!(valid_language(language), "{language}");
        }
        for language in ["", "zh hans", "zh_hans", "中文"] {
            assert!(!valid_language(language), "{language}");
        }
        assert!(!valid_language(&"a".repeat(MAX_LANGUAGE_BYTES + 1)));
    }

    #[test]
    fn set_validates_before_changing_anything() {
        let mut prefs = Preferences::default();
        assert!(prefs.set(LANGUAGE, json!("zh-hans")).is_ok());
        assert_eq!(prefs.interface.language, "zh-hans");
        assert!(prefs.set(LANGUAGE, json!("zh hans")).is_err());
        assert_eq!(prefs.interface.language, "zh-hans");
        assert!(prefs.set("interface", json!({"language": "en"})).is_err());
        assert!(prefs.set("nope", json!("en")).is_err());
        assert!(prefs.set("", json!({})).is_err());
        assert_eq!(prefs.interface.language, "zh-hans");
    }

    #[test]
    fn get_reports_unknown_paths() {
        let prefs = Preferences::default();
        assert_eq!(prefs.get("nope"), None);
        assert_eq!(prefs.get("interface.nope"), None);
        assert_eq!(prefs.get("interface.language.extra"), None);
    }

    #[test]
    fn reset_handles_everything_sections_and_values() {
        let mut prefs = Preferences::default();
        prefs.interface.language = "zh-hans".to_owned();
        assert!(prefs.reset(Some(LANGUAGE)).is_ok());
        assert_eq!(prefs.interface.language, "auto");
        prefs.interface.language = "ja".to_owned();
        assert!(prefs.reset(Some("interface")).is_ok());
        assert_eq!(prefs.interface.language, "auto");
        prefs.interface.language = "ja".to_owned();
        assert!(prefs.reset(None).is_ok());
        assert_eq!(prefs.interface.language, "auto");
        prefs.interface.language = "ja".to_owned();
        assert!(prefs.reset(Some("")).is_ok());
        assert_eq!(prefs.interface.language, "auto");
        prefs.interface.language = "ja".to_owned();
        assert_eq!(prefs.reset(Some("nope")), Err("unknown preference `nope`".to_owned()));
        assert_eq!(prefs.interface.language, "ja");
    }

    #[test]
    fn json_round_trips() {
        let mut prefs = Preferences::default();
        prefs.interface.language = "zh-hans".to_owned();
        let json = prefs.to_json().unwrap();
        let (loaded, warnings) = Preferences::from_json(json.as_bytes());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded, prefs);
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), json!({"interface": {"language": "zh-hans"}}));
    }

    #[test]
    fn load_recovers_each_value_and_ignores_unknown_keys() {
        // A newer build's file: unknown sections and keys load without warnings.
        let (prefs, warnings) = Preferences::from_json(br#"{"interface": {"language": "zh-hans", "future": true}, "future": {"x": 1}}"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(prefs.interface.language, "zh-hans");
        // One bad value keeps the rest.
        let (prefs, warnings) = Preferences::from_json(br#"{"interface": {"language": ""}, "future": 1}"#);
        assert_eq!(prefs.interface.language, "auto");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // A missing key keeps its default.
        let (prefs, warnings) = Preferences::from_json(br#"{"interface": {}}"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(prefs, Preferences::default());
        // A non-object section falls back to the section defaults.
        let (prefs, warnings) = Preferences::from_json(br#"{"interface": "zh-hans"}"#);
        assert_eq!(prefs, Preferences::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // Malformed JSON and non-object documents fall back to everything default.
        for bytes in [&b"{oops"[..], b"[]", b"\"auto\"", b"42"] {
            let (prefs, warnings) = Preferences::from_json(bytes);
            assert_eq!(prefs, Preferences::default(), "{}", String::from_utf8_lossy(bytes));
            assert_eq!(warnings.len(), 1, "{}", String::from_utf8_lossy(bytes));
        }
        // An empty file is a fresh profile, not an error.
        let (prefs, warnings) = Preferences::from_json(b" \n\t");
        assert_eq!(prefs, Preferences::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn oversized_documents_fall_back_to_defaults() {
        let bytes = vec![b' '; MAX_BYTES as usize + 1];
        let (prefs, warnings) = Preferences::from_json(&bytes);
        assert_eq!(prefs, Preferences::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("64 KiB"), "{warnings:?}");
    }

    #[test]
    fn store_revision_increments_once_per_edit() {
        let mut store = Store::new(Preferences::default());
        assert_eq!(store.revision(), 0);
        assert_eq!(store.get().interface.language, "auto");
        store.edit(|prefs| prefs.set(LANGUAGE, json!("zh-hans")).unwrap());
        assert_eq!(store.revision(), 1);
        assert_eq!(store.get().interface.language, "zh-hans");
        store.edit(|prefs| prefs.reset(Some(LANGUAGE)).unwrap());
        assert_eq!(store.revision(), 2);
        assert_eq!(store.get().interface.language, "auto");
        store.edit(|_| ());
        assert_eq!(store.revision(), 3);
    }
}
