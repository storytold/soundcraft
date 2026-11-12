//! Commands consult [`soundcraft_model::LivePlugins`] before they create a temporary plugin.

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{Insert, LivePlugins, TrackId};

    /// One live slot. It knows one parameter, `param`: other ids get no answer. Its display
    /// for a value is `LIVE <value>`, so the text depends on the value it was asked to render, and
    /// when `parses` it reads that text back. A resolver passing the wrong id, the wrong value or
    /// the wrong text gets a different answer, or none.
    struct Answer {
        plugin: &'static str,
        track: TrackId,
        slot: usize,
        param: &'static str,
        parses: bool,
        value: Option<f32>,
    }

    struct Fake {
        answers: Vec<Answer>,
    }

    impl Fake {
        fn answer(&self, track: TrackId, slot: usize) -> Option<&Answer> {
            self.answers.iter().find(|a| a.track == track && a.slot == slot)
        }

        /// As the player: an answer only when the slot holds the plugin the caller expects.
        fn answer_for(&self, at: crate::LiveSlot<'_>) -> Option<&Answer> {
            self.answer(at.track, at.slot).filter(|a| a.plugin == at.plugin)
        }
    }

    impl LivePlugins for Fake {
        fn plugin_id(&mut self, track: TrackId, slot: usize) -> Option<String> {
            self.answer(track, slot).map(|a| a.plugin.to_string())
        }

        fn value_text(&mut self, at: crate::LiveSlot<'_>, id: &str, value: f32) -> Option<String> {
            self.answer_for(at).filter(|a| a.param == id).map(|_| format!("LIVE {value}"))
        }

        fn value_from_text(&mut self, at: crate::LiveSlot<'_>, id: &str, text: &str) -> Option<f32> {
            self.answer_for(at).filter(|a| a.param == id && a.parses)?;
            text.strip_prefix("LIVE ")?.parse().ok()
        }

        fn value(&mut self, at: crate::LiveSlot<'_>, id: &str) -> Option<f32> {
            self.answer_for(at).filter(|a| a.param == id).and_then(|a| a.value)
        }
    }

    fn track(e: &mut Engine, kind: Option<&str>) -> TrackId {
        let params = kind.map_or_else(|| json!({}), |kind| json!({"kind": kind}));
        e.execute("track.new", &params).unwrap();
        e.session().tracks.last().unwrap().id
    }

    fn plant_insert(e: &mut Engine, track: TrackId, plugin: &str, param: &str, value: f32) {
        let tr = e.session_mut().track_mut(track).unwrap();
        let mut insert = Insert::new(plugin);
        insert.params.insert(param.to_string(), value);
        tr.mixer.inserts[0] = Some(insert);
    }

    fn stored_insert(e: &Engine, track: TrackId, param: &str) -> Option<f32> {
        e.session().track(track).and_then(|tr| tr.mixer.inserts[0].as_ref()).and_then(|insert| insert.params.get(param).copied())
    }

    fn param_row<'a>(result: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        result["params"].as_array().unwrap().iter().find(|row| row["id"] == id).unwrap()
    }

    #[test]
    fn a_known_plugin_resolves_text_and_probes_through_the_live_instance() {
        let mut e = Engine::default();
        let audio = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": audio.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        let instrument = track(&mut e, Some("instrument"));
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![
                Answer { plugin: "eq_7band", track: audio, slot: 0, param: "mid_gain", parses: true, value: Some(-3.0) },
                Answer {
                    plugin: "subtractive_synth",
                    track: instrument,
                    slot: soundcraft_mix::INSTRUMENT_SLOT,
                    param: "cutoff",
                    parses: true,
                    value: Some(440.0),
                },
            ],
        })));

        // "LIVE -3" is not text a built-in parses. Storing it means the live instance answered
        // for this parameter and no temporary instance was created.
        let set = e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "mid_gain", "value": "LIVE -3"})).unwrap();
        assert_eq!(set["value"], -3.0);
        assert_eq!(stored_insert(&e, audio, "mid_gain"), Some(-3.0));
        // The live instance knows only mid_gain: asked about another parameter, it has no answer.
        let err = e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "low_shelf_gain", "value": "LIVE -3"})).unwrap_err();
        assert!(err.to_string().contains("is not a value of"), "{err}");

        let probed = e.execute("mix.plugin_params", &json!({"track": audio.0, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let mid = param_row(&probed, "mid_gain");
        assert_eq!(mid["value"], -3.0);
        assert_eq!(mid["value_source"], "session");
        assert_eq!(mid["display"], "LIVE -3", "the live text for the stored value");
        assert_eq!(mid["name"], "MF Gain");
        assert!(mid["min"].is_number());

        let set = e.execute("mix.instrument_param", &json!({"track": instrument.0, "param": "cutoff", "value": "LIVE 440"})).unwrap();
        assert_eq!(set["value"], 440.0);
        let probed =
            e.execute("mix.plugin_params", &json!({"track": instrument.0, "slot": "instrument", "filter": "cutoff", "probe": true})).unwrap();
        let cutoff = param_row(&probed, "cutoff");
        assert_eq!(cutoff["value_source"], "session");
        assert_eq!(cutoff["value"], 440.0);
        assert_eq!(cutoff["display"], "LIVE 440");
        assert_eq!(cutoff["name"], "Cutoff");
    }

    #[test]
    fn an_unknown_plugin_stays_unknown_even_when_the_live_instance_parses_text() {
        let mut e = Engine::default();
        let audio = track(&mut e, None);
        plant_insert(&mut e, audio, "clap:does.not.exist", "gain", 0.1);
        let instrument = track(&mut e, Some("instrument"));
        {
            let mut insert = Insert::new("clap:does.not.exist");
            insert.params.insert("tone".into(), 1.0);
            e.session_mut().track_mut(instrument).unwrap().instrument = Some(insert);
        }
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![
                Answer { plugin: "clap:does.not.exist", track: audio, slot: 0, param: "gain", parses: true, value: Some(0.5) },
                Answer {
                    plugin: "clap:does.not.exist",
                    track: instrument,
                    slot: soundcraft_mix::INSTRUMENT_SLOT,
                    param: "tone",
                    parses: true,
                    value: Some(0.25),
                },
            ],
        })));

        for params in [json!({"track": audio.0, "slot": 0}), json!({"track": audio.0, "slot": 0, "probe": true})] {
            let err = e.execute("mix.plugin_params", &params).unwrap_err().to_string();
            assert!(err.contains("unknown plugin"), "{err}");
        }
        let err = e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "gain", "value": "LIVE-29"})).unwrap_err().to_string();
        assert!(err.contains("unknown plugin"), "{err}");
        let err = e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "gain", "value": 0.5})).unwrap_err().to_string();
        assert!(err.contains("unknown plugin"), "{err}");
        let err = e.execute("mix.insert_params", &json!({"track": audio.0, "slot": 0, "params": {"gain": "LIVE-29"}})).unwrap_err().to_string();
        assert!(err.contains("unknown plugin"), "{err}");
        assert_eq!(stored_insert(&e, audio, "gain"), Some(0.1));

        let err = e.execute("mix.instrument_param", &json!({"track": instrument.0, "param": "tone", "value": "LIVE-29"})).unwrap_err().to_string();
        assert!(err.contains("unknown plugin `clap:does.not.exist`"), "{err}");
        let err = e.execute("mix.plugin_params", &json!({"track": instrument.0, "slot": "instrument", "probe": true})).unwrap_err().to_string();
        assert!(err.contains("unknown plugin"), "{err}");
        let stored = e.session().track(instrument).and_then(|tr| tr.instrument.as_ref()).and_then(|insert| insert.params.get("tone").copied());
        assert_eq!(stored, Some(1.0));
    }

    #[test]
    fn text_the_live_instance_refuses_falls_back_to_the_built_in_parse_but_a_probe_keeps_live_display() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![Answer { plugin: "eq_7band", track, slot: 0, param: "mid_gain", parses: false, value: None }],
        })));
        // "-3" is not a label. The live instance renders mid_gain but refuses the text, and a
        // built-in reads its own display without a temporary instance: "-3" is -3 dB.
        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "-3"})).unwrap();
        assert_eq!(set["value"], -3.0);
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(-3.0));

        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        let probed = e.execute("mix.plugin_params", &json!({"track": track.0, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let row = param_row(&probed, "mid_gain");
        // mid_gain is stored, so the probe keeps 7.5. Its display is the live text for that
        // stored value, which the built-in's own formatting would not produce.
        assert_eq!(row["value"], 7.5);
        assert_eq!(row["value_source"], "session");
        assert_eq!(row["display"], "LIVE 7.5");
    }

    #[test]
    fn a_plugin_id_mismatch_falls_back_to_the_temporary_instance() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![Answer { plugin: "clap:wrong", track, slot: 0, param: "mid_gain", parses: true, value: Some(99.0) }],
        })));
        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "-3"})).unwrap();
        assert_eq!(set["value"], -3.0);
        assert_ne!(stored_insert(&e, track, "mid_gain"), Some(99.0));

        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        let probed = e.execute("mix.plugin_params", &json!({"track": track.0, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let row = param_row(&probed, "mid_gain");
        assert_eq!(row["value"], 7.5);
        assert_eq!(row["value_source"], "session");
        assert_ne!(row["display"], "LIVE 7.5", "not the other plugin's display");
    }

    #[test]
    fn live_text_that_parses_as_zero_without_a_matching_display_is_rejected() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        // Every string parses as 0, and the display is always "0.0 dB". "twintig" does not
        // round-trip, so it is not stored. "0.0 dB" does.
        e.set_live_plugins(Some(Box::new(ZeroLive)));
        let err =
            e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "twintig"})).unwrap_err().to_string();
        assert!(err.contains("`twintig` is not a value of MF Gain (mid_gain)"), "{err}");
        assert!(err.contains("0.0 dB"), "{err}");
        assert!(!err.contains("out of range"), "{err}");
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(7.5));

        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "0.0 dB"})).unwrap();
        assert_eq!(set["value"], 0.0);
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(0.0));
    }

    /// A live eq_7band whose parser answers 0 for any text and whose display is always "0.0 dB",
    /// like units that report success for garbage.
    struct ZeroLive;

    impl LivePlugins for ZeroLive {
        fn plugin_id(&mut self, _track: TrackId, _slot: usize) -> Option<String> {
            Some("eq_7band".into())
        }

        fn value_text(&mut self, at: crate::LiveSlot<'_>, _id: &str, _value: f32) -> Option<String> {
            (at.plugin == "eq_7band").then(|| "0.0 dB".to_string())
        }

        fn value_from_text(&mut self, at: crate::LiveSlot<'_>, _id: &str, _text: &str) -> Option<f32> {
            (at.plugin == "eq_7band").then_some(0.0)
        }

        fn value(&mut self, at: crate::LiveSlot<'_>, _id: &str) -> Option<f32> {
            (at.plugin == "eq_7band").then_some(0.0)
        }
    }

    /// The live instance still holds the previous block after `mix.insert_param`. A stored
    /// parameter keeps the session value; display is the live text for that stored value.
    /// A parameter the session does not store is read from the live instance.
    struct StaleLive;

    impl LivePlugins for StaleLive {
        fn plugin_id(&mut self, _track: TrackId, _slot: usize) -> Option<String> {
            Some("eq_7band".into())
        }

        fn value_text(&mut self, _at: crate::LiveSlot<'_>, id: &str, value: f32) -> Option<String> {
            Some(format!("{id}={value}"))
        }

        fn value_from_text(&mut self, _at: crate::LiveSlot<'_>, _id: &str, _text: &str) -> Option<f32> {
            None
        }

        fn value(&mut self, _at: crate::LiveSlot<'_>, id: &str) -> Option<f32> {
            match id {
                // Stale: the session already holds -3.
                "mid_gain" => Some(0.0),
                "input_gain" => Some(1.5),
                _ => None,
            }
        }
    }

    #[test]
    fn a_probe_keeps_a_stored_value_when_the_live_instance_is_stale() {
        let mut e = Engine::default();
        let audio = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": audio.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "mid_gain", "value": -3.0})).unwrap();
        e.session_mut().track_mut(audio).unwrap().mixer.inserts[0].as_mut().unwrap().params.remove("input_gain");
        e.set_live_plugins(Some(Box::new(StaleLive)));

        let probed = e.execute("mix.plugin_params", &json!({"track": audio.0, "slot": 0, "probe": true, "limit": 1000})).unwrap();
        let mid = param_row(&probed, "mid_gain");
        assert_eq!(mid["value"], -3.0);
        assert_eq!(mid["value_source"], "session");
        assert_eq!(mid["display"], "mid_gain=-3");

        let input = param_row(&probed, "input_gain");
        assert_eq!(input["value"], 1.5);
        assert_eq!(input["value_source"], "plugin");
        assert_eq!(input["display"], "input_gain=1.5");
    }
}
