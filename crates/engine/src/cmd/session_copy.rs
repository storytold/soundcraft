//! Explicit copy and read-only completeness preview; exports are never replayed by recovery.
use super::*;
use crate::cmd;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "session.save_copy", "Save Session Copy In...", ["File"], None, "{path, replace?: false, expected_revision?: u64} — packages all declared decoded audio; missing audio blocks the copy; video stays external", always, save),
        cmd!(query "session.save_copy_preview", "Preview Session Copy", [], None, "{path?, expected_revision?: u64} — read-only audio completeness and external-video manifest; no writes or downloads", always, preview),
    ]
}

fn validate<'a>(e: &Engine, p: &'a Value, preview: bool) -> Result<Option<&'a str>> {
    let id = if preview { "session.save_copy_preview" } else { "session.save_copy" };
    let object = p.as_object().ok_or_else(|| bad(id, "parameters must be an object"))?;
    for (key, value) in object {
        match key.as_str() {
            "path" if value.is_string() => {}
            "replace" if !preview && value.is_boolean() => {}
            "expected_revision" if value.as_u64().is_some() => {}
            "path" | "replace" | "expected_revision" => return Err(bad(id, format!("invalid `{key}` type or option"))),
            _ => return Err(bad(id, format!("unknown parameter `{key}`"))),
        }
    }
    crate::session_copy::check_revision(e, p.get("expected_revision"))?;
    let path = str_param(p, "path");
    if !preview && path.is_none() {
        return Err(bad(id, "`path` required"));
    }
    Ok(path)
}

fn save(e: &mut Engine, p: &Value) -> Result<Value> {
    let object = p.as_object().ok_or_else(|| bad("session.save_copy", "parameters must be an object"))?;
    let path = object.get("path").and_then(Value::as_str).ok_or_else(|| bad("session.save_copy", "`path` required"))?;
    let destination = crate::io::session_file(path);
    if !cfg!(target_arch = "wasm32") {
        crate::file_access::check_write(&destination).map_err(EngineError::Io)?;
    }
    if !cfg!(target_arch = "wasm32") && crate::io::replaces(e, "session.save_copy", &destination) && !bool_or(p, "replace", false) {
        return Err(bad("session.save_copy", "destination already exists; pass `replace: true` to replace its session file"));
    }
    let _ = validate(e, p, false)?;
    serde_json::to_value(crate::session_copy::save(e, path)?).map_err(|err| EngineError::Io(err.to_string()))
}

fn preview(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = validate(e, p, true)?;
    serde_json::to_value(crate::session_copy::preview(e, path)).map_err(|err| EngineError::Io(err.to_string()))
}
