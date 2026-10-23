//! Preferences commands: the shell-owned settings such as `interface.language`.

use super::*;
use crate::cmd;
use serde_json::{Map, Value};

fn prefs_get(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").unwrap_or("");
    e.prefs().get(path).ok_or_else(|| bad("prefs.get", format!("unknown preference `{path}`")))
}

fn prefs_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let changes = prefs_set_changes(p)?;
    // Validate every value against a copy first: one bad value leaves the store untouched.
    let mut next = e.prefs().clone();
    for (path, value) in &changes {
        next.set(path, value.clone()).map_err(|error| bad("prefs.set", error))?;
    }
    e.edit_prefs(|prefs| *prefs = next);
    let mut out = Map::new();
    for (path, _) in changes {
        if let Some(value) = e.prefs().get(&path) {
            out.insert(path, value);
        }
    }
    Ok(Value::Object(out))
}

/// The `(path, value)` pairs of a `prefs.set` call: `path` + `value`, and/or a `values` object.
fn prefs_set_changes(p: &Value) -> Result<Vec<(String, Value)>> {
    let mut changes = Vec::new();
    match (str_param(p, "path"), p.get("value")) {
        (Some(path), Some(value)) => changes.push((path.to_owned(), value.clone())),
        (Some(_), None) => return Err(bad("prefs.set", "missing `value`")),
        (None, Some(_)) => return Err(bad("prefs.set", "`value` needs a `path`")),
        (None, None) => {}
    }
    if let Some(values) = p.get("values") {
        let values = values.as_object().ok_or_else(|| bad("prefs.set", "`values` must be an object of path: value"))?;
        for (path, value) in values {
            changes.push((path.clone(), value.clone()));
        }
    }
    if changes.is_empty() {
        return Err(bad("prefs.set", "give `path` and `value`, or `values: {path: value, …}`"));
    }
    Ok(changes)
}

fn prefs_reset(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").or_else(|| str_param(p, "section"));
    let mut next = e.prefs().clone();
    next.reset(path).map_err(|error| bad("prefs.reset", error))?;
    e.edit_prefs(|prefs| *prefs = next);
    let path = path.unwrap_or("");
    e.prefs().get(path).ok_or_else(|| bad("prefs.reset", format!("unknown preference `{path}`")))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "prefs.get", "Get Preferences", [], None, r##"{"path":"section|section.key"?=everything (e.g. "interface.language")}"##, always, prefs_get),
        cmd!(
            query "prefs.set",
            "Set Preferences",
            [],
            None,
            r##"{"path":"section.key","value":json} or {"values":{"section.key":json,…}} (validated; all or nothing)"##,
            always,
            prefs_set
        ),
        cmd!(query "prefs.reset", "Reset Preferences", [], None, r##"{"path":"section|section.key"?=everything}"##, always, prefs_reset),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EngineError;
    use serde_json::json;

    fn run(e: &mut Engine, id: &str, params: Value) -> Result<Value> {
        e.execute(id, &params)
    }

    #[test]
    fn default_language_is_auto_and_get_reads_every_level() {
        let mut e = Engine::default();
        assert_eq!(run(&mut e, "prefs.get", json!({})).unwrap(), json!({"interface": {"language": "auto"}}));
        assert_eq!(run(&mut e, "prefs.get", json!({"path": ""})).unwrap(), json!({"interface": {"language": "auto"}}));
        assert_eq!(run(&mut e, "prefs.get", json!({"path": "interface"})).unwrap(), json!({"language": "auto"}));
        assert_eq!(run(&mut e, "prefs.get", json!({"path": "interface.language"})).unwrap(), json!("auto"));
        assert!(matches!(run(&mut e, "prefs.get", json!({"path": "interface.nope"})), Err(EngineError::BadParams(..))));
    }

    #[test]
    fn set_stores_the_value_and_bumps_the_revision() {
        let mut e = Engine::default();
        assert_eq!(e.prefs_revision(), 0);
        let out = run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "zh-hans"})).unwrap();
        assert_eq!(out, json!({"interface.language": "zh-hans"}));
        assert_eq!(e.prefs().interface.language, "zh-hans");
        assert_eq!(e.prefs_revision(), 1);
        let out = run(&mut e, "prefs.set", json!({"values": {"interface.language": "en"}})).unwrap();
        assert_eq!(out, json!({"interface.language": "en"}));
        assert_eq!(e.prefs_revision(), 2);
    }

    #[test]
    fn set_is_all_or_nothing() {
        let mut e = Engine::default();
        let error = run(&mut e, "prefs.set", json!({"values": {"interface.language": "zh-hans", "nope": 1}}));
        assert!(
            matches!(error, Err(EngineError::BadParams(ref cmd, ref msg)) if cmd == "prefs.set" && msg.contains("unknown preference `nope`")),
            "{error:?}"
        );
        assert_eq!(e.prefs().interface.language, "auto");
        assert_eq!(e.prefs_revision(), 0);
        let error = run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "zh hans"}));
        assert!(matches!(error, Err(EngineError::BadParams(ref cmd, _)) if cmd == "prefs.set"));
        assert_eq!(e.prefs().interface.language, "auto");
        assert_eq!(e.prefs_revision(), 0);
    }

    #[test]
    fn set_rejects_invalid_language_values() {
        let mut e = Engine::default();
        for value in [json!(""), json!(42), json!(null), json!("zh hans"), json!("a".repeat(17))] {
            let error = run(&mut e, "prefs.set", json!({"path": "interface.language", "value": value}));
            match error {
                Err(EngineError::BadParams(_, msg)) => assert!(msg.contains("must be `auto` or a language code"), "{msg}"),
                other => panic!("expected a rejected value, got {other:?}"),
            }
        }
        assert_eq!(e.prefs().interface.language, "auto");
    }

    #[test]
    fn set_validates_its_parameter_shape() {
        let mut e = Engine::default();
        let error = run(&mut e, "prefs.set", json!({}));
        assert!(matches!(error, Err(EngineError::BadParams(_, ref msg)) if msg.contains("give `path` and `value`")), "{error:?}");
        let error = run(&mut e, "prefs.set", json!({"path": "interface.language"}));
        assert!(matches!(error, Err(EngineError::BadParams(_, ref msg)) if msg.contains("missing `value`")), "{error:?}");
        let error = run(&mut e, "prefs.set", json!({"value": "en"}));
        assert!(matches!(error, Err(EngineError::BadParams(_, ref msg)) if msg.contains("needs a `path`")), "{error:?}");
        let error = run(&mut e, "prefs.set", json!({"values": ["en"]}));
        assert!(matches!(error, Err(EngineError::BadParams(..))), "{error:?}");
    }

    #[test]
    fn reset_handles_a_value_a_section_and_everything() {
        let mut e = Engine::default();
        run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "zh-hans"})).unwrap();
        assert_eq!(run(&mut e, "prefs.reset", json!({"path": "interface.language"})).unwrap(), json!("auto"));
        assert_eq!(e.prefs().interface.language, "auto");
        run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "ja"})).unwrap();
        assert_eq!(run(&mut e, "prefs.reset", json!({"section": "interface"})).unwrap(), json!({"language": "auto"}));
        run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "ja"})).unwrap();
        assert_eq!(run(&mut e, "prefs.reset", json!({})).unwrap(), json!({"interface": {"language": "auto"}}));
        assert_eq!(e.prefs().interface.language, "auto");
        let revision = e.prefs_revision();
        assert!(matches!(run(&mut e, "prefs.reset", json!({"path": "nope"})), Err(EngineError::BadParams(..))));
        assert_eq!(e.prefs_revision(), revision);
    }

    #[test]
    fn preferences_are_outside_the_document_undo_and_journal() {
        let mut e = Engine::default();
        let revision = e.revision;
        let history = e.undo_history();
        run(&mut e, "prefs.get", json!({})).unwrap();
        run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "zh-hans"})).unwrap();
        assert_eq!(e.revision, revision, "the document revision must not change");
        assert_eq!(e.undo_history(), history, "preferences are not undoable");
        assert!(!e.is_dirty(), "preferences must not dirty the session");
        assert!(e.journal.is_empty(), "queries and preference changes stay out of the journal");
    }

    #[test]
    fn preferences_json_round_trips_through_the_engine() {
        let mut e = Engine::default();
        run(&mut e, "prefs.set", json!({"path": "interface.language", "value": "zh-hans"})).unwrap();
        let json = e.prefs_json().unwrap();
        let mut loaded = Engine::default();
        assert!(loaded.load_prefs_json(json.as_bytes()).is_empty());
        assert_eq!(loaded.prefs(), e.prefs());
        // Lenient load: a damaged file keeps the defaults instead of rejecting the app start.
        assert_eq!(loaded.load_prefs_json(b"{damaged").len(), 1);
        assert_eq!(loaded.prefs().interface.language, "auto");
    }
}
