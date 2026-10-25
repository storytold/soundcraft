//! Read-only plugin parameter metadata for agents (`mix.plugin_params`).
use super::mix::{PluginSlot, track_plugin_slot};
use super::{CommandSpec, bad, has_selection};

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_dsp::Unit;

const CMD: &str = "mix.plugin_params";
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "mix.plugin_params",
        "List Plugin Parameters",
        [],
        None,
        "{track?, slot: 0..9 | a..j | \"instrument\", filter?: string, offset?: 0, limit?: 200 (1..1000)} → {plugin, name, total, offset, params: [{id, name, min, max, default, unit, taper, steps, values, value, value_source}]}; filter is a case-insensitive substring of id or name and total is before pagination. A hosted plugin's default may differ from its actual internal value stored in its state blob; reading live values is a later addition.",
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
        Some(value) => Some(value.as_str().ok_or_else(|| bad(CMD, "`filter` must be a string"))?.to_ascii_lowercase()),
    };
    let offset = nonnegative_integer(p, "offset", 0)?;
    let limit = nonnegative_integer(p, "limit", DEFAULT_LIMIT)?;
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(bad(CMD, "`limit` must be an integer from 1 through 1000"));
    }

    let matching: Vec<_> = info
        .params
        .iter()
        .filter(|param| {
            filter.as_ref().is_none_or(|needle| param.id.to_ascii_lowercase().contains(needle) || param.name.to_ascii_lowercase().contains(needle))
        })
        .collect();
    let total = matching.len();
    let params: Vec<_> = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|param| {
            let (steps, values): (Option<usize>, Option<Vec<&str>>) = match param.unit {
                Unit::Toggle => (Some(2), Some(vec!["Off", "On"])),
                Unit::Choice => (Some(param.choices.len()), Some(param.choices.to_vec())),
                _ => (None, None),
            };
            let (value, value_source) = insert.params.get(param.id).map_or((param.default, "default"), |value| (*value, "session"));
            json!({
                "id": param.id,
                "name": param.name,
                "min": param.min,
                "max": param.max,
                "default": param.default,
                "unit": param.unit,
                "taper": param.taper,
                "steps": steps,
                "values": values,
                "value": value,
                "value_source": value_source,
            })
        })
        .collect();

    Ok(json!({"plugin": info.id, "name": info.name, "total": total, "offset": offset, "params": params}))
}

#[cfg(test)]
mod tests {
    use super::{CMD, DEFAULT_LIMIT};
    use crate::Engine;
    use serde_json::{Value, json};
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
        e.execute(CMD, &json!({"track": track, "slot": 0})).unwrap();
        assert_eq!(e.revision, revision);
        assert_eq!(e.undo_history(), undo);
        assert_eq!(e.journal, journal);
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
