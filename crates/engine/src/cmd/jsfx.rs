//! REAPER JS effects: discovery (`engine.jsfx_plugins`). Hosting itself lives in
//! `soundcraft-jsfx-host`; id lookup for the mixer commands is shared with CLAP in
//! `cmd::clap::plugin_info`.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "engine.jsfx_plugins",
        "List JSFX Effects",
        [],
        None,
        "{rescan?: bool} → [{id: \"jsfx:<path in the Effects folder>\", name, vendor (top-level folder), is_instrument, path}]",
        always,
        |_, p| {
            let list = if p.get("rescan").and_then(Value::as_bool).unwrap_or(false) {
                soundcraft_jsfx_host::rescan()
            } else {
                soundcraft_jsfx_host::scan()
            };
            Ok(json!(list))
        }
    )]
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn jsfx_plugins_query_returns_a_list() {
        let mut e = Engine::default();
        let v = e.execute("engine.jsfx_plugins", &json!({})).unwrap();
        assert!(v.is_array());
    }

    #[test]
    fn unknown_jsfx_insert_is_an_error() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        assert!(e.execute("mix.insert", &json!({"track": t, "plugin": "jsfx:no/such/effect"})).is_err());
        assert!(super::super::clap::plugin_info("jsfx:no/such/effect").is_none());
    }
}
