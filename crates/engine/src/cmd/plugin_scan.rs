//! Why plugins are missing: `engine.plugin_scan_failures`.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "engine.plugin_scan_failures",
        "Plugin Scan Failures",
        [],
        None,
        "{} → [{format: \"clap\" | \"vst3\", path, reason}] from the scans so far (engine.clap_plugins, engine.vst3_plugins)",
        always,
        |_, _| Ok(soundcraft_plugin_scan::failures()
            .into_iter()
            .map(|(format, path, reason)| json!({"format": format, "path": path.to_string_lossy(), "reason": reason}))
            .collect())
    )]
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn failures_are_a_list() {
        let mut e = Engine::default();
        assert!(e.execute("engine.plugin_scan_failures", &json!({})).unwrap().is_array());
    }
}
