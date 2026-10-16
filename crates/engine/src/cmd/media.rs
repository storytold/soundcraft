//! Explicit audio media recovery commands; programmatic calls never open a dialog.

use super::*;
use crate::cmd;
use soundcraft_model::SourceId;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "media.relink",
            "Relink Audio",
            [],
            None,
            "{source: id, path, allow_unverified?: false} — restore missing audio after metadata/content verification",
            always,
            relink
        ),
        cmd!("source.relink", "Relink Audio", [], None, "{source: id, path, allow_unverified?: false} — alias of media.relink", always, relink),
    ]
}

fn relink(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = SourceId(p.get("source").and_then(Value::as_u64).ok_or_else(|| bad("media.relink", "`source` id required"))?);
    let path = str_param(p, "path").filter(|path| !path.trim().is_empty()).ok_or_else(|| bad("media.relink", "`path` required"))?;
    if p.get("allow_unverified").is_some_and(|value| !value.is_boolean()) {
        return Err(bad("media.relink", "`allow_unverified` must be a boolean"));
    }
    crate::media::relink(e, id, path, bool_or(p, "allow_unverified", false))
}
