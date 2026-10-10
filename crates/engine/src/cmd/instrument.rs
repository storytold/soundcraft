//! An instrument track's instrument (`mix.instrument`): the plugin that turns the track's MIDI
//! into audio, built in or hosted (CLAP, VST3, Audio Units). The mixer plays `Track::instrument`;
//! the instrument is otherwise only set when the track is created (`track.new`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Insert, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.instrument",
            "Set Instrument",
            [],
            None,
            "{track?, plugin: id} — an instrument track's instrument: built-in, `clap:`, `vst3:` or `au:` id",
            has_selection,
            set_instrument
        ),
        cmd!(
            "mix.instrument_param",
            "Set Instrument Parameter",
            [],
            None,
            "{track?, param, value}: one parameter of an instrument track's built-in instrument (clamped into range)",
            has_selection,
            set_instrument_param
        ),
    ]
}

fn set_instrument(e: &mut Engine, p: &Value) -> Result<Value> {
    let plugin = str_param(p, "plugin").ok_or_else(|| bad("mix.instrument", "`plugin` required"))?;
    let info = super::clap::plugin_info(plugin).ok_or_else(|| bad("mix.instrument", format!("unknown plugin `{plugin}`")))?;
    if !info.is_instrument {
        return Err(bad("mix.instrument", format!("`{plugin}` is not an instrument")));
    }
    let t = tracks_required(e, "mix.instrument", p)?.first().copied().ok_or_else(|| bad("mix.instrument", "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad("mix.instrument", "only instrument tracks have an instrument"));
    }
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = Some(ins);
    }
    Ok(json!({"track": t, "plugin": info.id}))
}

fn set_instrument_param(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.instrument_param";
    let param = str_param(p, "param").ok_or_else(|| bad(id, "`param` required"))?.to_string();
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`value` must be a number"))? as f32;
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let ins = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(id, "the track has no instrument"))?;
    // Hosted instruments keep their parameters in their own state.
    let info = soundcraft_dsp::plugin_info(&ins.plugin).ok_or_else(|| bad(id, format!("`{}` is not a built-in instrument", ins.plugin)))?;
    let pi = info.param(&param).ok_or_else(|| bad(id, format!("`{}` has no parameter `{param}`", info.id)))?;
    let v = pi.clamp(value);
    ins.params.insert(param.clone(), v);
    Ok(json!({"track": t, "param": param, "value": v}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::TrackId;

    fn track(e: &mut Engine, kind: &str) -> TrackId {
        e.execute("track.new", &json!({"kind": kind})).unwrap();
        e.session().tracks.last().unwrap().id
    }

    fn instrument_of(e: &Engine, t: TrackId) -> Option<String> {
        e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.plugin.clone())
    }

    #[test]
    fn sets_an_instrument_tracks_instrument_with_defaults_and_undoes() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        assert_eq!(instrument_of(&e, t).as_deref(), Some("subtractive_synth"));
        let r = e.execute("mix.instrument", &json!({"track": t.0, "plugin": "drum_synth"})).unwrap();
        assert_eq!(r["plugin"], "drum_synth");
        assert_eq!(instrument_of(&e, t).as_deref(), Some("drum_synth"));
        let params = e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.params.len());
        assert_eq!(params, soundcraft_dsp::plugin_info("drum_synth").map(|p| p.params.len()), "every parameter at its default");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(instrument_of(&e, t).as_deref(), Some("subtractive_synth"));
    }

    #[test]
    fn sets_a_built_in_instruments_parameters() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        let r = e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 5000})).unwrap();
        assert_eq!(r["value"], 5000.0);
        let r = e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 1.0e9})).unwrap();
        assert_eq!(r["value"], 20000.0, "clamped");
        let v = e.session().track(t).and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get("cutoff").copied());
        assert_eq!(v, Some(20000.0));
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "nope", "value": 1})).is_err());
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff"})).is_err());
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": "loud"})).is_err());
        let audio = track(&mut e, "audio");
        assert!(e.execute("mix.instrument_param", &json!({"track": audio.0, "param": "cutoff", "value": 1})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
    }

    #[test]
    fn refuses_effects_unknown_plugins_and_other_track_kinds() {
        let mut e = Engine::default();
        let inst = track(&mut e, "instrument");
        let audio = track(&mut e, "audio");
        assert!(e.execute("mix.instrument", &json!({"track": inst.0, "plugin": "eq_7band"})).is_err(), "an effect");
        assert!(e.execute("mix.instrument", &json!({"track": inst.0, "plugin": "vst3:00000000000000000000000000000000"})).is_err());
        assert!(e.execute("mix.instrument", &json!({"track": inst.0})).is_err(), "no plugin");
        assert!(e.execute("mix.instrument", &json!({"track": audio.0, "plugin": "drum_synth"})).is_err(), "an audio track");
        assert_eq!(instrument_of(&e, inst).as_deref(), Some("subtractive_synth"));
        assert_eq!(instrument_of(&e, audio), None);
    }
}
