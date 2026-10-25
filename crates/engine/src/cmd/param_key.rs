//! Resolve a plugin parameter key to the parameter the session stores.
//!
//! A key is an id, a whole number written as text, or a name. Ids win, so a script that
//! already passes ids keeps its meaning when some other parameter is named like that id.
//! The session always stores [`soundcraft_dsp::ParamInfo::id`], never the name.

use soundcraft_dsp::{ParamInfo, PluginInfo};
use std::fmt;

/// Reject longer keys before scanning a plugin's parameter list.
pub const MAX_PARAM_KEY_CHARS: usize = 200;

const MAX_SUGGESTIONS: usize = 8;

/// Why [`resolve_param`] refused a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// `key` is longer than [`MAX_PARAM_KEY_CHARS`].
    TooLong,
    /// No id or name matched. `suggestions` are `Name (id)`, at most eight.
    Unknown { key: String, plugin: String, suggestions: Vec<String> },
    /// Several parameters share the normalized name, or (automation) the folded id.
    Ambiguous { key: String, plugin: String, hits: Vec<String> },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::TooLong => write!(f, "a parameter key must be at most {MAX_PARAM_KEY_CHARS} characters"),
            ResolveError::Unknown { key, plugin, suggestions } => {
                if suggestions.is_empty() {
                    write!(f, "{key} is not a parameter of {plugin}")
                } else {
                    write!(f, "{key} is not a parameter of {plugin}; did you mean: {}", suggestions.join(", "))
                }
            }
            ResolveError::Ambiguous { key, plugin, hits } => {
                write!(f, "{key} matches several parameters of {plugin}: {} - use the id", hits.join(", "))
            }
        }
    }
}

/// Two keys of one call resolved to the same stored id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamKeysError {
    Resolve(ResolveError),
    Duplicate { first_key: String, second_key: String, name: String, id: String },
}

impl From<ResolveError> for ParamKeysError {
    fn from(err: ResolveError) -> Self {
        ParamKeysError::Resolve(err)
    }
}

impl fmt::Display for ParamKeysError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamKeysError::Resolve(err) => write!(f, "{err}"),
            ParamKeysError::Duplicate { first_key, second_key, name, id } => {
                write!(f, "{first_key} and {second_key} both set {name} ({id}); pass it once")
            }
        }
    }
}

/// Resolve `key` against `info`.
///
/// 1. Exact id match. Ids win over names.
/// 2. Exact name: Unicode-lowercased, trimmed, inner whitespace collapsed.
/// 3. `Group/Name`. CLAP names are already stored as `Module/Name`, so this is step 2.
///    AU and VST3 expose no group separate from the name, so a key is not split on `/`.
/// 4. Otherwise [`ResolveError`]. One normalized name shared by several parameters is
///    ambiguous.
///
/// A key longer than [`MAX_PARAM_KEY_CHARS`] is rejected before any of that work.
pub fn resolve_param<'a>(info: &'a PluginInfo, key: &str) -> Result<&'a ParamInfo, ResolveError> {
    resolve(info, key, false)
}

/// [`resolve_param`] for an automation lane.
///
/// [`soundcraft_model::AutoParam::parse`] ASCII-lowercases the whole lane string, so the
/// key arrives folded. Ids are matched case-insensitively as well, still before names, and
/// the caller stores the original [`ParamInfo::id`].
pub fn resolve_automation_param<'a>(info: &'a PluginInfo, key: &str) -> Result<&'a ParamInfo, ResolveError> {
    resolve(info, key, true)
}

/// Every key in one object, in order. Two keys that select the same id are an error, and
/// nothing is returned for the caller to store.
pub fn resolve_unique_params<'info, 'key, I>(info: &'info PluginInfo, keys: I) -> Result<Vec<(&'key str, &'info ParamInfo)>, ParamKeysError>
where
    I: IntoIterator<Item = &'key str>,
{
    let mut bound: Vec<(&'key str, &'info ParamInfo)> = Vec::new();
    for key in keys {
        let param = resolve_param(info, key)?;
        if let Some((first_key, _)) = bound.iter().find(|entry| entry.1.id == param.id) {
            return Err(ParamKeysError::Duplicate {
                first_key: (*first_key).to_string(),
                second_key: key.to_string(),
                name: param.name.to_string(),
                id: param.id.to_string(),
            });
        }
        bound.push((key, param));
    }
    Ok(bound)
}

/// Trim, Unicode-lowercase, and collapse inner whitespace to one space.
pub(super) fn normalize_key(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut started = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if started {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
        started = true;
    }
    out
}

fn resolve<'a>(info: &'a PluginInfo, key: &str, ids_ignore_case: bool) -> Result<&'a ParamInfo, ResolveError> {
    if key.chars().nth(MAX_PARAM_KEY_CHARS).is_some() {
        return Err(ResolveError::TooLong);
    }
    let trimmed = key.trim();
    if let Some(param) = info.params.iter().find(|param| param.id == trimmed) {
        return Ok(param);
    }
    if ids_ignore_case {
        let folded = fold_case(trimmed);
        let hits: Vec<&ParamInfo> = info.params.iter().filter(|param| fold_case(param.id) == folded).collect();
        match hits.as_slice() {
            [one] => return Ok(*one),
            [] => {}
            many => return Err(ambiguous(info, trimmed, many)),
        }
    }
    let needle = normalize_key(trimmed);
    // Step 3 has nothing to add: a CLAP `Module/Name` is the parameter's name, so step 2
    // matches it. AU and VST3 have no group beside that name, so `Group/Name` is not split.
    let named: Vec<&ParamInfo> = info.params.iter().filter(|param| normalize_key(param.name) == needle).collect();
    match named.as_slice() {
        [one] => Ok(*one),
        [] => Err(ResolveError::Unknown { key: shown_key(trimmed), plugin: info.name.to_string(), suggestions: suggestions(info, trimmed) }),
        many => Err(ambiguous(info, trimmed, many)),
    }
}

fn ambiguous(info: &PluginInfo, trimmed: &str, hits: &[&ParamInfo]) -> ResolveError {
    ResolveError::Ambiguous { key: shown_key(trimmed), plugin: info.name.to_string(), hits: hits.iter().copied().map(label).collect() }
}

fn shown_key(trimmed: &str) -> String {
    if trimmed.is_empty() { "\"\"".to_string() } else { trimmed.to_string() }
}

fn fold_case(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

fn label(param: &ParamInfo) -> String {
    format!("{} ({})", param.name, param.id)
}

fn words(normalized: &str) -> Vec<&str> {
    normalized.split(' ').filter(|word| !word.is_empty()).collect()
}

/// Names containing the key, else the names that share the most words with it, else the first few.
fn suggestions(info: &PluginInfo, key: &str) -> Vec<String> {
    let needle = normalize_key(key);
    if !needle.is_empty() {
        let mut containing = Vec::new();
        for param in info.params {
            if normalize_key(param.name).contains(&needle) {
                containing.push(label(param));
                if containing.len() == MAX_SUGGESTIONS {
                    break;
                }
            }
        }
        if !containing.is_empty() {
            return containing;
        }
        let key_words = words(&needle);
        if !key_words.is_empty() {
            let mut best = 0usize;
            let mut ranked: Vec<(usize, &ParamInfo)> = Vec::new();
            for param in info.params {
                let name = normalize_key(param.name);
                let name_words = words(&name);
                let shared = key_words.iter().filter(|word| name_words.contains(word)).count();
                if shared > best {
                    best = shared;
                }
                if shared > 0 {
                    ranked.push((shared, param));
                }
            }
            if best > 0 {
                let mut out = Vec::new();
                for (shared, param) in ranked {
                    if shared == best {
                        out.push(label(param));
                        if out.len() == MAX_SUGGESTIONS {
                            break;
                        }
                    }
                }
                return out;
            }
        }
    }
    info.params.iter().take(MAX_SUGGESTIONS).map(label).collect()
}

#[cfg(test)]
mod tests {
    use super::{ResolveError, normalize_key, resolve_automation_param, resolve_param, resolve_unique_params};
    use soundcraft_dsp::{Category, ParamInfo, PluginInfo, Taper, Unit};

    const fn param(id: &'static str, name: &'static str) -> ParamInfo {
        ParamInfo { id, name, min: 0.0, max: 1.0, default: 0.0, unit: Unit::None, taper: Taper::Linear, choices: &[] }
    }

    fn plugin(params: &'static [ParamInfo]) -> PluginInfo {
        PluginInfo { id: "test", name: "Test Plugin", short_name: "Test", category: Category::Other, params, is_instrument: false, audiosuite: false }
    }

    static ID_WINS: [ParamInfo; 2] = [param("other", "1598"), param("1598", "S-Strum Time")];
    static CLAP: [ParamInfo; 2] = [param("3", "Filter/Cutoff"), param("4", "Cutoff")];
    static STRUM: [ParamInfo; 3] = [param("1598", "S-Strum Time"), param("1575", "S-Strum Toggle"), param("vol", "Volume")];
    static AMBIGUOUS: [ParamInfo; 2] = [param("a", "Gain"), param("b", "GAIN")];
    static MIXED_CASE_ID: [ParamInfo; 2] = [param("Gain", "Volume"), param("other", "gain")];
    static MANY: [ParamInfo; 10] = [
        param("p0", "N0"),
        param("p1", "N1"),
        param("p2", "N2"),
        param("p3", "N3"),
        param("p4", "N4"),
        param("p5", "N5"),
        param("p6", "N6"),
        param("p7", "N7"),
        param("p8", "N8"),
        param("p9", "N9"),
    ];

    #[test]
    fn exact_id_beats_a_name_that_equals_another_parameters_id() {
        let info = plugin(&ID_WINS);
        let param = resolve_param(&info, "1598").unwrap();
        assert_eq!(param.id, "1598");
        assert_eq!(param.name, "S-Strum Time");
    }

    #[test]
    fn names_match_case_insensitively_with_collapsed_whitespace() {
        let info = plugin(&ID_WINS);
        assert_eq!(resolve_param(&info, "s-strum time").unwrap().id, "1598");
        assert_eq!(resolve_param(&info, "  S-Strum   Time ").unwrap().id, "1598");
        assert_eq!(resolve_param(&info, "S-STRUM\tTIME").unwrap().id, "1598");
        assert_eq!(normalize_key("  S-Strum   Time "), "s-strum time");
    }

    #[test]
    fn clap_style_module_name_matches_the_stored_name_and_is_not_split() {
        let info = plugin(&CLAP);
        assert_eq!(resolve_param(&info, "Filter/Cutoff").unwrap().id, "3");
        assert_eq!(resolve_param(&info, "filter/cutoff").unwrap().id, "3");
        // The bare name is a different parameter. A slash is not a group lookup.
        assert_eq!(resolve_param(&info, "Cutoff").unwrap().id, "4");
        let split = resolve_param(&info, "Filter / Cutoff").unwrap_err().to_string();
        assert!(split.contains("Filter / Cutoff is not a parameter of Test Plugin"), "{split}");
        let bare = plugin(&ID_WINS);
        let grouped = resolve_param(&bare, "Group/S-Strum Time").unwrap_err().to_string();
        assert!(grouped.contains("is not a parameter of Test Plugin"), "{grouped}");
    }

    #[test]
    fn an_unknown_key_suggests_containing_names_then_shared_words_then_the_first_few() {
        let info = plugin(&STRUM);
        let contained = resolve_param(&info, "strum").unwrap_err().to_string();
        assert_eq!(contained, "strum is not a parameter of Test Plugin; did you mean: S-Strum Time (1598), S-Strum Toggle (1575)");

        let shared = resolve_param(&info, "toggle zebra").unwrap_err().to_string();
        assert!(shared.contains("did you mean: S-Strum Toggle (1575)"), "{shared}");
        assert!(!shared.contains("S-Strum Time"), "{shared}");

        let first = resolve_param(&plugin(&MANY), "zzzz-not-a-word").unwrap_err().to_string();
        assert!(first.contains("N0 (p0)") && first.contains("N7 (p7)"), "{first}");
        assert!(!first.contains("N8 (p8)"), "{first}");
    }

    #[test]
    fn an_ambiguous_name_lists_ids() {
        let err = resolve_param(&plugin(&AMBIGUOUS), "gain").unwrap_err().to_string();
        assert_eq!(err, "gain matches several parameters of Test Plugin: Gain (a), GAIN (b) - use the id");
    }

    #[test]
    fn a_long_key_is_rejected_before_matching() {
        let info = plugin(&AMBIGUOUS);
        let long = "x".repeat(201);
        assert_eq!(resolve_param(&info, &long).unwrap_err(), ResolveError::TooLong);
        assert!(!resolve_param(&info, &long).unwrap_err().to_string().contains(&long));
        let edge = "y".repeat(200);
        assert!(matches!(resolve_param(&info, &edge).unwrap_err(), ResolveError::Unknown { .. }));
    }

    #[test]
    fn automation_matches_ids_case_insensitively_and_ids_still_win() {
        let info = plugin(&MIXED_CASE_ID);
        // The ordinary resolver does not fold ids: "gain" is the other parameter's name.
        assert_eq!(resolve_param(&info, "gain").unwrap().id, "other");
        assert_eq!(resolve_param(&info, "Gain").unwrap().id, "Gain");
        // The lane key was lowercased by AutoParam::parse, so the id "Gain" still wins.
        assert_eq!(resolve_automation_param(&info, "gain").unwrap().id, "Gain");
    }

    #[test]
    fn two_keys_for_one_id_are_refused() {
        let info = plugin(&ID_WINS);
        let err = resolve_unique_params(&info, ["1598", "S-Strum Time"]).unwrap_err().to_string();
        assert_eq!(err, "1598 and S-Strum Time both set S-Strum Time (1598); pass it once");
        let bound = resolve_unique_params(&info, ["S-Strum Time"]).unwrap();
        assert_eq!(bound[0].1.id, "1598");
    }
}
