//! Commands consult [`soundcraft_model::LivePlugins`] before they create a temporary plugin.

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{Insert, LivePlugins, TrackId};

    struct Answer {
        plugin: &'static str,
        track: TrackId,
        slot: usize,
        from_text: Option<f32>,
        value: Option<f32>,
        display: &'static str,
    }

    struct Fake {
        answers: Vec<Answer>,
    }

    impl Fake {
        fn answer(&self, track: TrackId, slot: usize) -> Option<&Answer> {
            self.answers.iter().find(|a| a.track == track && a.slot == slot)
        }
    }

    impl LivePlugins for Fake {
        fn plugin_id(&mut self, track: TrackId, slot: usize) -> Option<String> {
            self.answer(track, slot).map(|a| a.plugin.to_string())
        }

        fn value_text(&mut self, track: TrackId, slot: usize, _id: &str, _value: f32) -> Option<String> {
            self.answer(track, slot).map(|a| a.display.to_string())
        }

        fn value_from_text(&mut self, track: TrackId, slot: usize, _id: &str, _text: &str) -> Option<f32> {
            self.answer(track, slot).and_then(|a| a.from_text)
        }

        fn value(&mut self, track: TrackId, slot: usize, _id: &str) -> Option<f32> {
            self.answer(track, slot).and_then(|a| a.value)
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
    fn an_unlisted_plugin_resolves_text_and_probes_through_the_live_instance() {
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
                Answer { plugin: "clap:does.not.exist", track: audio, slot: 0, from_text: Some(0.5), value: Some(0.5), display: "LIVE" },
                Answer {
                    plugin: "clap:does.not.exist",
                    track: instrument,
                    slot: soundcraft_mix::INSTRUMENT_SLOT,
                    from_text: Some(0.25),
                    value: Some(0.25),
                    display: "SOFT",
                },
            ],
        })));

        // No registry entry and no probe: still the historical error. A temporary instance cannot
        // exist for this id either.
        let err = e.execute("mix.plugin_params", &json!({"track": audio.0, "slot": 0})).unwrap_err();
        assert!(err.to_string().contains("unknown plugin"), "{err}");

        let set = e.execute("mix.insert_param", &json!({"track": audio.0, "slot": 0, "param": "gain", "value": "warm"})).unwrap();
        assert_eq!(set["value"], 0.5);
        assert_eq!(stored_insert(&e, audio, "gain"), Some(0.5));

        let probed = e.execute("mix.plugin_params", &json!({"track": audio.0, "slot": 0, "probe": true})).unwrap();
        let gain = param_row(&probed, "gain");
        assert_eq!(gain["value"], 0.5);
        assert_eq!(gain["value_source"], "plugin");
        assert_eq!(gain["display"], "LIVE");
        assert_eq!(gain["name"], "gain");
        assert_eq!(gain["min"], serde_json::Value::Null);
        assert_eq!(gain["max"], serde_json::Value::Null);
        assert_eq!(gain["default"], serde_json::Value::Null);
        assert_eq!(gain["unit"], "None");
        assert_eq!(gain["taper"], "Linear");
        assert_eq!(gain["steps"], serde_json::Value::Null);
        assert_eq!(gain["values"], json!([]));
        assert!(gain.get("display_min").is_none());
        assert!(gain.get("display_max").is_none());

        let set = e.execute("mix.instrument_param", &json!({"track": instrument.0, "param": "tone", "value": "soft"})).unwrap();
        assert_eq!(set["value"], 0.25);
        let probed = e.execute("mix.plugin_params", &json!({"track": instrument.0, "slot": "instrument", "probe": true})).unwrap();
        let tone = param_row(&probed, "tone");
        assert_eq!(tone["value_source"], "plugin");
        assert_eq!(tone["value"], 0.25);
        assert_eq!(tone["display"], "SOFT");
    }

    #[test]
    fn live_none_falls_back_to_a_temporary_instance_for_text_but_not_for_a_probe() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![Answer { plugin: "eq_7band", track, slot: 0, from_text: None, value: None, display: "LIVE" }],
        })));
        // "-3" is not a label. The live parser declines, so the temporary built-in parses it.
        // That parse runs only after the temporary instance is created.
        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "-3"})).unwrap();
        assert_eq!(set["value"], -3.0);
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(-3.0));

        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        let probed = e.execute("mix.plugin_params", &json!({"track": track.0, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let row = param_row(&probed, "mid_gain");
        // A temporary probe would set the stored 7.5 and report value_source "plugin".
        assert_eq!(row["value"], 7.5);
        assert_eq!(row["value_source"], "session");
        assert_eq!(row["display"], "LIVE");
    }

    #[test]
    fn a_plugin_id_mismatch_falls_back_to_the_temporary_instance() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![Answer { plugin: "clap:wrong", track, slot: 0, from_text: Some(99.0), value: Some(99.0), display: "WRONG" }],
        })));
        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "-3"})).unwrap();
        assert_eq!(set["value"], -3.0);
        assert_ne!(stored_insert(&e, track, "mid_gain"), Some(99.0));

        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        let probed = e.execute("mix.plugin_params", &json!({"track": track.0, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let row = param_row(&probed, "mid_gain");
        assert_eq!(row["value"], 7.5);
        assert_eq!(row["value_source"], "plugin");
        assert_ne!(row["display"], "WRONG");
    }

    #[test]
    fn live_text_that_parses_as_zero_without_a_matching_display_is_rejected() {
        let mut e = Engine::default();
        let track = track(&mut e, None);
        e.execute("mix.insert", &json!({"track": track.0, "slot": 0, "plugin": "eq_7band"})).unwrap();
        e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        // Every string parses as 0, and the display is always "0.0 dB". "twintig" does not
        // round-trip, so it is not stored. "0.0 dB" does.
        e.set_live_plugins(Some(Box::new(Fake {
            answers: vec![Answer { plugin: "eq_7band", track, slot: 0, from_text: Some(0.0), value: Some(0.0), display: "0.0 dB" }],
        })));
        let err =
            e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "twintig"})).unwrap_err().to_string();
        assert!(err.contains("`twintig` is not a value of `mid_gain`"), "{err}");
        assert!(err.contains("0.0 dB"), "{err}");
        assert!(!err.contains("out of range"), "{err}");
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(7.5));

        let set = e.execute("mix.insert_param", &json!({"track": track.0, "slot": 0, "param": "mid_gain", "value": "0.0 dB"})).unwrap();
        assert_eq!(set["value"], 0.0);
        assert_eq!(stored_insert(&e, track, "mid_gain"), Some(0.0));
    }
}
