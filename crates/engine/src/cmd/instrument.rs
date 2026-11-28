//! An instrument track's instrument (`mix.instrument`): the plugin that turns the track's MIDI
//! into audio, built in or hosted (CLAP, VST3, Audio Units). The mixer plays `Track::instrument`;
//! the instrument is otherwise only set when the track is created (`track.new`).
use super::{CommandSpec, bad, has_selection, str_param, tracks_required};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{Insert, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.instrument",
            "Set Instrument",
            [],
            None,
            "{track?, plugin: id, params?: {id: value}} — an instrument track's instrument: built-in, `clap:`, `vst3:` or `au:` id; `params` sets built-in parameters (clamped into range; unknown ids and other fields are errors)",
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

/// Fields `mix.instrument` reads. Anything else (a parameter written beside `plugin`, say) used to
/// be accepted and ignored, so a synth's settings looked set while the render never changed.
const FIELDS: [&str; 4] = ["track", "tracks", "plugin", "params"];

fn set_instrument(e: &mut Engine, p: &Value) -> Result<Value> {
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !FIELDS.contains(&k.as_str()))) {
        return Err(bad(
            "mix.instrument",
            format!("unknown field `{k}`: an instrument's parameters go in `params` ({{id: value}}), or one at a time in mix.instrument_param"),
        ));
    }
    let plugin = str_param(p, "plugin").ok_or_else(|| bad("mix.instrument", "`plugin` required"))?;
    let info = super::clap::plugin_info(plugin).ok_or_else(|| bad("mix.instrument", format!("unknown plugin `{plugin}`")))?;
    if !info.is_instrument {
        return Err(bad("mix.instrument", format!("`{plugin}` is not an instrument")));
    }
    let t = tracks_required(e, "mix.instrument", p)?.first().copied().ok_or_else(|| bad("mix.instrument", "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad("mix.instrument", "only instrument tracks have an instrument"));
    }
    // Validate the parameters before touching the session.
    let mut values = Vec::new();
    if let Some(v) = p.get("params") {
        let obj = v.as_object().ok_or_else(|| bad("mix.instrument", "`params` must be an object"))?;
        for (k, v) in obj {
            let pi = info.params.iter().find(|pi| pi.id == k).ok_or_else(|| bad("mix.instrument", format!("`{plugin}` has no parameter `{k}`")))?;
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad("mix.instrument", format!("`{k}` must be a number")))?;
            values.push((k.clone(), pi.clamp(x as f32)));
        }
    }
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    for (k, v) in values {
        ins.params.insert(k, v);
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
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{Clip, TrackId};
    use soundcraft_time::{Range, TICKS_PER_QUARTER};

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

    /// One note of 440 Hz (A4) on the track, `secs` of render, and the RMS of the result.
    fn rms_of_a_note(e: &mut Engine, t: TrackId, secs: i64) -> f32 {
        let q = TICKS_PER_QUARTER;
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 93, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 2 * q });
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "note", 0, 48_000 * 4, seq));
        let out = soundcraft_mix::render_range(e.session(), Range::new(0, 48_000 * secs), 512);
        let n = out[0].len().max(1) as f32;
        (out[0].iter().map(|x| x * x).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn parameters_given_with_the_instrument_reach_the_render_and_unknown_fields_are_refused() {
        // A high note through a closed and an open filter: the cutoff has to change the render.
        let mut closed = Engine::default();
        let t = track(&mut closed, "instrument");
        let r = closed.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "params": {"cutoff": 200}})).unwrap();
        assert_eq!(r["plugin"], "subtractive_synth");
        let stored = closed.session().track(t).and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get("cutoff").copied());
        assert_eq!(stored, Some(200.0));
        let mut open = Engine::default();
        let o = track(&mut open, "instrument");
        open.execute("mix.instrument", &json!({"track": o.0, "plugin": "subtractive_synth", "params": {"cutoff": 12_000}})).unwrap();
        let (quiet, loud) = (rms_of_a_note(&mut closed, t, 2), rms_of_a_note(&mut open, o, 2));
        assert!(loud > quiet * 3.0, "cutoff 12000 plays a high note much louder than 200: {loud} vs {quiet}");
        // Values are clamped into range; the instrument starts from its defaults for the rest.
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "params": {"cutoff": 1.0e9}})).unwrap();
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        assert_eq!(ins.params.get("cutoff"), Some(&20_000.0));
        assert_eq!(ins.params.len(), soundcraft_dsp::plugin_info("subtractive_synth").map(|p| p.params.len()).unwrap());

        // Nothing is accepted and then ignored: a parameter beside `plugin`, an unknown id, a value
        // that is not a number and a `params` that is not an object are errors, and change nothing.
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 777})).unwrap();
        let before = e.session().track(t).and_then(|tr| tr.instrument.clone());
        let depth = e.revision;
        for bad in [
            json!({"track": t.0, "plugin": "drum_synth", "cutoff": 200}),
            json!({"track": t.0, "plugin": "drum_synth", "params": {"nope": 1}}),
            json!({"track": t.0, "plugin": "drum_synth", "params": {"decay": "long"}}),
            json!({"track": t.0, "plugin": "drum_synth", "params": 3}),
        ] {
            let err = e.execute("mix.instrument", &bad).unwrap_err().to_string();
            assert!(!err.is_empty(), "{bad}");
            assert_eq!(e.session().track(t).and_then(|tr| tr.instrument.clone()), before, "{bad}: the instrument is untouched");
            assert_eq!(e.revision, depth, "{bad}: nothing was recorded");
        }
        let err = e.execute("mix.instrument", &json!({"track": t.0, "plugin": "drum_synth", "cutoff": 200})).unwrap_err().to_string();
        assert!(err.contains("cutoff") && err.contains("params"), "{err}");
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
