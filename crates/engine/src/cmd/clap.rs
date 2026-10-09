//! Third-party CLAP plugins: discovery (`engine.clap_plugins`) and id lookup shared by the mixer
//! commands. Hosting itself lives in `soundcraft-clap-host`.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::PluginInfo;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "engine.clap_plugins",
        "List CLAP Plugins",
        [],
        None,
        "{rescan?: bool} → [{id: \"clap:…\", plugin_id, name, vendor, version, description, features, category, is_instrument, path}]",
        always,
        |_, p| {
            let list = if p.get("rescan").and_then(Value::as_bool).unwrap_or(false) {
                soundcraft_clap_host::rescan()
            } else {
                soundcraft_clap_host::scan()
            };
            Ok(json!(list))
        }
    )]
}

/// A built-in plugin's description, else a hosted CLAP (`clap:<id>`), VST3 (`vst3:<class id>`) or
/// Audio Units (`au:<type>:<subtype>:<manufacturer>`) plugin's.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    soundcraft_dsp::plugin_info(id)
        .or_else(|| soundcraft_clap_host::plugin_info(id))
        .or_else(|| soundcraft_vst3_host::plugin_info(id))
        .or_else(|| soundcraft_au_host::plugin_info(id))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn clap_plugins_query_returns_a_list() {
        let mut e = Engine::default();
        let v = e.execute("engine.clap_plugins", &json!({})).unwrap();
        assert!(v.is_array());
    }

    #[test]
    fn unknown_clap_insert_is_an_error() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let r = e.execute("mix.insert", &json!({"track": t, "plugin": "clap:no.such.plugin"}));
        assert!(r.is_err());
        assert!(super::plugin_info("eq_7band").is_some());
        assert!(super::plugin_info("clap:no.such.plugin").is_none());
    }

    /// Apple's own Audio Units ship with every Mac, so `mix.insert` can be checked against one.
    #[cfg(target_os = "macos")]
    #[test]
    fn audio_units_insert_by_id() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let r = e.execute("mix.insert", &json!({"track": t, "plugin": "au:aufx:bpas:appl"}));
        assert_eq!(r.map(|v| v["plugin"].clone()), Ok(json!("au:aufx:bpas:appl")), "Apple AUBandpass");
    }
}
