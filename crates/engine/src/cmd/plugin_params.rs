//! Read-only plugin parameter metadata for agents (`mix.plugin_params`).
use super::mix::{PluginSlot, track_plugin_slot};
use super::{CommandSpec, bad, has_selection};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_dsp::{ParamInfo, PluginInfo, Unit};
use soundcraft_model::Insert;

const CMD: &str = "mix.plugin_params";
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "mix.plugin_params",
        "List Plugin Parameters",
        [],
        None,
        "{track?, slot: 0..9 | a..j | \"instrument\", filter?: string (max 200 characters), offset?: 0, limit?: 200 (1..1000)} → {plugin, name, total, offset, params: [{id, name, min, max, default, unit, taper, steps, values, value, value_source}]}. Filter is a Unicode-lowercase substring of id or name; total is the filtered count before pagination. Unit is None|Db|Hz|Ms|Percent|Ratio|Semitones|Cents|Seconds|Toggle|Choice; taper is Linear|Log. Toggle/Choice value is a numeric index into values; Toggle values are always [\"Off\",\"On\"] (host-provided toggle strings are not yet read), and continuous values is []. Non-finite min/max/default/value is null. value_source \"session\" means the insert's stored parameter map holds the key (mix.insert stores every default; track.new's instrument starts empty); \"default\" means it does not. For hosted plugins the real value may instead live in the plugin state blob or automation; reading it live is a later addition.",
        has_selection,
        plugin_params
    )]
}

fn nonnegative_integer(p: &Value, key: &str, default: usize) -> Result<usize> {
    match p.get(key) {
        None => Ok(default),
        Some(value) => {
            value.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(CMD, format!("`{key}` must be a non-negative integer")))
        }
    }
}

fn pagination(p: &Value) -> Result<(usize, usize)> {
    let offset = nonnegative_integer(p, "offset", 0)?;
    let limit = nonnegative_integer(p, "limit", DEFAULT_LIMIT)?;
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(bad(CMD, "`limit` must be an integer from 1 through 1000"));
    }
    Ok((offset, limit))
}

fn matches_filter(param: &ParamInfo, needle: &str) -> bool {
    param.id.to_lowercase().contains(needle) || param.name.to_lowercase().contains(needle)
}

fn finite(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

fn parameter_json(param: &ParamInfo, insert: &Insert) -> Value {
    let (steps, values): (Option<usize>, Vec<&str>) = match param.unit {
        Unit::Toggle => (Some(2), vec!["Off", "On"]),
        Unit::Choice => (Some(param.choices.len()), param.choices.to_vec()),
        _ => (None, Vec::new()),
    };
    let (value, value_source) = insert.params.get(param.id).map_or((param.default, "default"), |value| (*value, "session"));
    json!({
        "id": param.id,
        "name": param.name,
        "min": finite(param.min),
        "max": finite(param.max),
        "default": finite(param.default),
        "unit": param.unit,
        "taper": param.taper,
        "steps": steps,
        "values": values,
        "value": finite(value),
        "value_source": value_source,
    })
}

fn response(info: &PluginInfo, insert: &Insert, filter: Option<&str>, offset: usize, limit: usize) -> Value {
    let matching: Vec<_> = info.params.iter().filter(|param| filter.is_none_or(|needle| matches_filter(param, needle))).collect();
    let total = matching.len();
    let params: Vec<_> = matching.into_iter().skip(offset).take(limit).map(|param| parameter_json(param, insert)).collect();
    json!({"plugin": info.id, "name": info.name, "total": total, "offset": offset, "params": params})
}

fn plugin_params(e: &mut Engine, p: &Value) -> Result<Value> {
    let (track, slot) = track_plugin_slot(e, p, CMD)?;
    let tr = e.session().track(track).ok_or_else(|| bad(CMD, "no track"))?;
    let insert = match slot {
        PluginSlot::Insert(index) => tr.mixer.inserts.get(index).and_then(Option::as_ref).ok_or_else(|| bad(CMD, "empty slot"))?,
        PluginSlot::Instrument => tr.instrument.as_ref().ok_or_else(|| bad(CMD, "the track has no instrument"))?,
    };
    let info = super::clap::plugin_info(&insert.plugin).ok_or_else(|| bad(CMD, "unknown plugin"))?;
    let filter = match p.get("filter") {
        None => None,
        Some(value) => {
            let value = value.as_str().ok_or_else(|| bad(CMD, "`filter` must be a string"))?;
            if value.chars().count() > 200 {
                return Err(bad(CMD, "`filter` must be at most 200 characters"));
            }
            Some(value.to_lowercase())
        }
    };
    let (offset, limit) = pagination(p)?;
    Ok(response(info, insert, filter.as_deref(), offset, limit))
}

#[cfg(test)]
mod tests {
    use super::{CMD, DEFAULT_LIMIT, matches_filter, pagination, parameter_json};
    use crate::Engine;
    use serde_json::{Value, json};
    use soundcraft_dsp::{ParamInfo, Taper, Unit};
    use soundcraft_model::Insert;

    fn new_track(e: &mut Engine, kind: Option<&str>) -> u64 {
        let params = kind.map_or_else(|| json!({}), |kind| json!({"kind": kind}));
        e.execute("track.new", &params).unwrap();
        e.session().tracks.last().unwrap().id.0
    }

    fn insert_eq(e: &mut Engine) -> u64 {
        let track = new_track(e, None);
        e.execute("mix.insert", &json!({"track": track, "slot": 0, "plugin": "eq_7band"})).unwrap();
        track
    }

    fn listed_params(value: &Value) -> &[Value] {
        value.get("params").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    #[test]
    fn built_in_insert_lists_metadata_and_stored_values() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        e.execute("mix.insert_param", &json!({"track": track, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        if let Some(insert) =
            e.session_mut().track_mut(soundcraft_model::TrackId(track)).and_then(|track| track.mixer.inserts.first_mut()).and_then(Option::as_mut)
        {
            insert.params.remove("input_gain");
        }
        let result = e.execute(CMD, &json!({"track": track, "slot": "a", "limit": 1000})).unwrap();
        let info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
        let params = listed_params(&result);

        assert_eq!(result.get("plugin"), Some(&json!(info.id)));
        assert_eq!(result.get("name"), Some(&json!(info.name)));
        assert_eq!(result.get("total"), Some(&json!(info.params.len())));
        assert_eq!(params.len(), info.params.len());
        for (actual, expected) in params.iter().zip(info.params) {
            assert_eq!(actual.get("id"), Some(&json!(expected.id)));
            assert_eq!(actual.get("name"), Some(&json!(expected.name)));
            assert_eq!(actual.get("min"), Some(&json!(expected.min)));
            assert_eq!(actual.get("max"), Some(&json!(expected.max)));
            assert_eq!(actual.get("default"), Some(&json!(expected.default)));
        }
        let changed = params.iter().find(|param| param.get("id") == Some(&json!("mid_gain"))).unwrap();
        assert_eq!(changed.get("value"), Some(&json!(7.5)));
        assert_eq!(changed.get("value_source"), Some(&json!("session")));
        let defaulted = params.iter().find(|param| param.get("id") == Some(&json!("input_gain"))).unwrap();
        assert_eq!(defaulted.get("value"), defaulted.get("default"));
        assert_eq!(defaulted.get("value_source"), Some(&json!("default")));
        assert_eq!(defaulted.get("steps"), Some(&Value::Null));
        assert_eq!(defaulted.get("values"), Some(&json!([])));
    }

    #[test]
    fn built_in_instrument_is_listed_through_the_instrument_slot() {
        let mut e = Engine::default();
        let track = new_track(&mut e, Some("instrument"));
        let result = e.execute(CMD, &json!({"track": track, "slot": "instrument"})).unwrap();
        let info = soundcraft_dsp::plugin_info("subtractive_synth").unwrap();
        assert_eq!(result.get("plugin"), Some(&json!("subtractive_synth")));
        assert_eq!(result.get("total"), Some(&json!(info.params.len())));
        assert_eq!(listed_params(&result).len(), info.params.len().min(DEFAULT_LIMIT));
    }

    #[test]
    fn filter_and_pagination_report_the_filtered_total() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        let all = e.execute(CMD, &json!({"track": track, "slot": 0, "filter": "FrEq", "limit": 1000})).unwrap();
        let expected: Vec<_> = soundcraft_dsp::plugin_info("eq_7band")
            .unwrap()
            .params
            .iter()
            .filter(|param| param.id.to_ascii_lowercase().contains("freq") || param.name.to_ascii_lowercase().contains("freq"))
            .collect();
        assert_eq!(all.get("total"), Some(&json!(expected.len())));

        let page = e.execute(CMD, &json!({"track": track, "slot": 0, "filter": "fReQ", "offset": 1, "limit": 2})).unwrap();
        assert_eq!(page.get("total"), Some(&json!(expected.len())));
        assert_eq!(page.get("offset"), Some(&json!(1)));
        assert_eq!(listed_params(&page).len(), expected.len().saturating_sub(1).min(2));
        assert_eq!(listed_params(&page).first().and_then(|param| param.get("id")).and_then(Value::as_str), expected.get(1).map(|param| param.id));

        let past_end = e.execute(CMD, &json!({"track": track, "slot": 0, "filter": "freq", "offset": 10_000})).unwrap();
        assert_eq!(past_end.get("total"), Some(&json!(expected.len())));
        assert!(listed_params(&past_end).is_empty());
    }

    #[test]
    fn filter_uses_unicode_lowercase_and_rejects_more_than_200_characters() {
        let param =
            ParamInfo {
                id: "größe", name: "Résonance Über", min: 0.0, max: 1.0, default: 0.5, unit: Unit::None, taper: Taper::Linear, choices: &[]
            };
        assert!(matches_filter(&param, &"RÉSONANCE".to_lowercase()));
        assert!(matches_filter(&param, &"ÜBER".to_lowercase()));

        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        let too_long = "é".repeat(201);
        let err = e.execute(CMD, &json!({"track": track, "slot": 0, "filter": too_long})).unwrap_err();
        assert!(err.to_string().contains("at most 200 characters"), "{err}");
    }

    #[test]
    fn pagination_defaults_to_200_and_rejects_null_and_numeric_strings() {
        assert_eq!(pagination(&json!({})), Ok((0, DEFAULT_LIMIT)));
        for params in [json!({"offset": null}), json!({"offset": "2"}), json!({"limit": null}), json!({"limit": "200"})] {
            assert!(pagination(&params).is_err(), "accepted {params}");
        }
    }

    #[test]
    fn non_finite_metadata_and_values_are_null_without_dropping_the_parameter() {
        let param = ParamInfo {
            id: "hostile",
            name: "Hostile",
            min: f32::NEG_INFINITY,
            max: f32::INFINITY,
            default: f32::NAN,
            unit: Unit::None,
            taper: Taper::Linear,
            choices: &[],
        };
        let value = parameter_json(&param, &Insert::new("test"));
        assert_eq!(value.get("id"), Some(&json!("hostile")));
        for field in ["min", "max", "default", "value"] {
            assert_eq!(value.get(field), Some(&Value::Null), "{field}");
        }
    }

    #[test]
    fn hostile_pagination_and_filter_values_are_rejected() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        for params in [
            json!({"track": track, "slot": 0, "offset": -1}),
            json!({"track": track, "slot": 0, "offset": 1.5}),
            json!({"track": track, "slot": 0, "limit": -1}),
            json!({"track": track, "slot": 0, "limit": 1.5}),
            json!({"track": track, "slot": 0, "limit": 0}),
            json!({"track": track, "slot": 0, "limit": 1001}),
            json!({"track": track, "slot": 0, "filter": 7}),
            json!({"track": track, "slot": 0, "offset": null}),
            json!({"track": track, "slot": 0, "offset": "1"}),
            json!({"track": track, "slot": 0, "limit": null}),
            json!({"track": track, "slot": 0, "limit": "200"}),
        ] {
            assert!(e.execute(CMD, &params).is_err(), "accepted {params}");
        }
    }

    #[test]
    fn empty_instrument_unknown_plugin_and_unknown_slot_are_errors() {
        let mut e = Engine::default();
        let audio = new_track(&mut e, None);
        assert!(e.execute(CMD, &json!({"track": audio, "slot": 0})).unwrap_err().to_string().contains("empty slot"));
        assert!(e.execute(CMD, &json!({"track": audio, "slot": "instrument"})).unwrap_err().to_string().contains("the track has no instrument"));
        assert!(e.execute(CMD, &json!({"track": audio, "slot": "unknown"})).is_err());

        if let Some(track) = e.session_mut().track_mut(soundcraft_model::TrackId(audio)) {
            track.mixer.inserts[0] = Some(Insert::new("not_a_plugin"));
        }
        assert!(e.execute(CMD, &json!({"track": audio, "slot": 0})).unwrap_err().to_string().contains("unknown plugin"));
    }

    #[test]
    fn choice_and_toggle_parameters_include_steps_and_values() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        let result = e.execute(CMD, &json!({"track": track, "slot": 0, "limit": 1000})).unwrap();
        let params = listed_params(&result);
        let toggle = params.iter().find(|param| param.get("id") == Some(&json!("hpf_on"))).unwrap();
        assert_eq!(toggle.get("steps"), Some(&json!(2)));
        assert_eq!(toggle.get("values"), Some(&json!(["Off", "On"])));
        let choice = params.iter().find(|param| param.get("id") == Some(&json!("hpf_slope"))).unwrap();
        assert_eq!(choice.get("steps"), Some(&json!(4)));
        assert_eq!(choice.get("values"), Some(&json!(["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct"])));
    }

    #[test]
    fn query_does_not_change_revision_undo_history_or_journal() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        let revision = e.revision;
        let undo = e.undo_history();
        let journal = e.journal.clone();
        e.mark_clean();
        let dirty = e.is_dirty();
        e.execute(CMD, &json!({"track": track, "slot": 0})).unwrap();
        assert_eq!(e.revision, revision);
        assert_eq!(e.undo_history(), undo);
        assert_eq!(e.journal, journal);
        assert_eq!(e.is_dirty(), dirty);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_dls_instrument_lists_numeric_parameter_ids() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument", "instrument": "au:aumu:dls :appl"})).unwrap();
        let track = e.session().tracks[0].id.0;
        let result = e.execute(CMD, &json!({"track": track, "slot": "instrument", "limit": 1000})).unwrap();
        let params = listed_params(&result);
        assert!(!params.is_empty());
        assert!(params.iter().all(|param| param.get("id").and_then(Value::as_str).and_then(|id| id.parse::<u32>().ok()).is_some()));
    }
}
