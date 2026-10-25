//! Read-only plugin parameter metadata for agents (`mix.plugin_params`).
use super::mix::{PluginSlot, plugin_slot_index, track_plugin_slot};
use super::{CommandSpec, bad, has_selection};

use crate::{Engine, LivePlugins, Result, cmd};
use serde_json::{Value, json};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Unit};
use soundcraft_model::{Insert, TrackId};

const CMD: &str = "mix.plugin_params";
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "mix.plugin_params",
        "List Plugin Parameters",
        [],
        None,
        "{track?, slot: 0..9 | a..j | \"instrument\", filter?: string (max 200 characters), offset?: 0, limit?: 200 (1..1000), probe?: false} → {plugin, name, total, offset, limit, has_more, next_offset (null on the last page), params: [{id, name, min, max, default, unit, taper, steps, values, value, value_source, display, display_min?, display_max?}]}. Filter is a Unicode-lowercase substring of id or name; total is the filtered count before pagination. Unit is None|Db|Hz|Ms|Percent|Ratio|Semitones|Cents|Seconds|Toggle|Choice; taper is Linear|Log. Toggle/Choice value is a numeric index into values; Toggle values are always [\"Off\",\"On\"] (host-provided toggle strings are not yet read), and continuous values is []. Non-finite min/max/default/value is null. Without probe, value/value_source come from the session map or default; display uses SoundCraft formatting for built-ins and is null for hosted plugins. With probe, a parameter stored in the session is authoritative: value is that stored value and value_source is \"session\", because the live instance only receives the new value on the next UI frame or audio block. Its display is the plugin's own text for that stored value — the live instance's value_text when the app has the same plugin loaded in the slot (no temporary instance is created), otherwise a temporary instance's value_text, otherwise SoundCraft formatting. A parameter that is not stored is read from that same instance with value_source \"plugin\". display_min and display_max are the plugin's text at the endpoints. Probing is read-only with no session side effects. A temporary instance of a large hosted instrument can take seconds; the live instance does not.",
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

/// A source of live parameter values for one probe. The temporary plugin and the desktop player's
/// live instance both answer through this, so the JSON shape stays one function.
trait Probe {
    fn value(&mut self, id: &str) -> Option<f32>;
    fn value_text(&mut self, id: &str, value: f32) -> Option<String>;
}

struct PluginProbe<'a>(&'a mut dyn Plugin);

impl Probe for PluginProbe<'_> {
    fn value(&mut self, id: &str) -> Option<f32> {
        self.0.param(id)
    }

    fn value_text(&mut self, id: &str, value: f32) -> Option<String> {
        self.0.value_text(id, value)
    }
}

struct LiveProbe<'a> {
    live: &'a mut dyn LivePlugins,
    track: TrackId,
    slot: usize,
}

impl Probe for LiveProbe<'_> {
    fn value(&mut self, id: &str) -> Option<f32> {
        self.live.value(self.track, self.slot, id).filter(|value| value.is_finite())
    }

    fn value_text(&mut self, id: &str, value: f32) -> Option<String> {
        self.live.value_text(self.track, self.slot, id, value)
    }
}

fn parameter_json(param: &ParamInfo, insert: &Insert, hosted: bool, mut probe: Option<&mut dyn Probe>) -> Value {
    let (steps, values): (Option<usize>, Vec<&str>) = match param.unit {
        Unit::Toggle => (Some(2), vec!["Off", "On"]),
        Unit::Choice => (Some(param.choices.len()), param.choices.to_vec()),
        _ => (None, Vec::new()),
    };
    // A stored key is the value the command just wrote. The live instance, and a temporary one,
    // can still be on the previous block, so the session wins and the plugin only renders that value.
    let (value, value_source) = if let Some(stored) = insert.params.get(param.id).copied() {
        (stored, "session")
    } else if let Some(live) = probe.as_mut().and_then(|source| source.value(param.id)) {
        (live, "plugin")
    } else {
        (param.default, "default")
    };
    let mut result = json!({
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
        "display": if probe.is_some() || !hosted { Some(param.format(value)) } else { None },
    });
    if let Some(probe) = probe {
        let shown = |text: Option<String>, value: f32| text.map_or_else(|| param.format(value), |text| super::mix::shown_text(&text));
        let display = shown(probe.value_text(param.id, value), value);
        let display_min = shown(probe.value_text(param.id, param.min), param.min);
        let display_max = shown(probe.value_text(param.id, param.max), param.max);
        if let Some(object) = result.as_object_mut() {
            object.insert("display".into(), json!(display));
            object.insert("display_min".into(), json!(display_min));
            object.insert("display_max".into(), json!(display_max));
        }
    }
    result
}

fn response(info: &PluginInfo, insert: &Insert, filter: Option<&str>, offset: usize, limit: usize, probe: Option<&mut dyn Probe>) -> Value {
    let matching: Vec<_> = info.params.iter().filter(|param| filter.is_none_or(|needle| matches_filter(param, needle))).collect();
    let total = matching.len();
    let hosted = soundcraft_mix::is_third_party(&insert.plugin);
    let page = matching.into_iter().skip(offset).take(limit);
    let params: Vec<_> = match probe {
        Some(probe) => page.map(|param| parameter_json(param, insert, hosted, Some(&mut *probe))).collect(),
        None => page.map(|param| parameter_json(param, insert, hosted, None)).collect(),
    };
    let end = offset.saturating_add(params.len());
    let next_offset = (end < total).then_some(end);
    json!({
        "plugin": info.id,
        "name": info.name,
        "total": total,
        "offset": offset,
        "limit": limit,
        "has_more": next_offset.is_some(),
        "next_offset": next_offset,
        "params": params,
    })
}

/// Whether the requested page lists any parameter. A probe of an empty page (a filter that
/// matches nothing, or an offset past the end) has nothing to read, so it loads no plugin.
fn page_has_params(info: &PluginInfo, filter: Option<&str>, offset: usize) -> bool {
    info.params.iter().filter(|param| filter.is_none_or(|needle| matches_filter(param, needle))).nth(offset).is_some()
}

fn plugin_params(e: &mut Engine, p: &Value) -> Result<Value> {
    let (track, slot) = track_plugin_slot(e, p, CMD)?;
    let index = plugin_slot_index(slot);
    let insert = {
        let tr = e.session().track(track).ok_or_else(|| bad(CMD, "no track"))?;
        match slot {
            PluginSlot::Insert(index) => tr.mixer.inserts.get(index).and_then(Option::as_ref).cloned().ok_or_else(|| bad(CMD, "empty slot")),
            PluginSlot::Instrument => tr.instrument.clone().ok_or_else(|| bad(CMD, "the track has no instrument")),
        }?
    };
    let filter = match p.get("filter") {
        None => None,
        Some(value) => {
            let value = value.as_str().ok_or_else(|| bad(CMD, "`filter` must be a string"))?;
            if value.chars().nth(200).is_some() {
                return Err(bad(CMD, "`filter` must be at most 200 characters"));
            }
            Some(value.to_lowercase())
        }
    };
    let (offset, limit) = pagination(p)?;
    let probe = match p.get("probe") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| bad(CMD, "`probe` must be a boolean"))?,
    };
    let Some(info) = super::clap::plugin_info(&insert.plugin) else {
        return Err(bad(CMD, format!("unknown plugin `{}`", insert.plugin)));
    };
    // A matching live instance supplies display text, and the value of parameters the session has
    // not stored. A stored parameter keeps the session value. An absent live value is not a reason
    // to create a temporary instance.
    if probe
        && let Some(live) = e.live_plugins_mut()
        && live.plugin_id(track, index).as_deref() == Some(insert.plugin.as_str())
    {
        let mut source = LiveProbe { live, track, slot: index };
        return Ok(response(info, &insert, filter.as_deref(), offset, limit, Some(&mut source)));
    }
    let mut instance = if probe && page_has_params(info, filter.as_deref(), offset) {
        let state = insert.state_bytes();
        let sample_rate = e.session().sample_rate.as_f64() as f32;
        let mut instance = soundcraft_mix::create_instance(&insert.plugin, state.as_deref(), sample_rate, 512, 2)
            .ok_or_else(|| bad(CMD, format!("could not instantiate plugin `{}`", insert.plugin)))?;
        for (id, value) in &insert.params {
            instance.plugin.set_param(id, *value);
        }
        Some(instance)
    } else {
        None
    };
    match instance.as_mut() {
        Some(instance) => {
            let mut source = PluginProbe(&mut *instance.plugin);
            Ok(response(info, &insert, filter.as_deref(), offset, limit, Some(&mut source)))
        }
        None => Ok(response(info, &insert, filter.as_deref(), offset, limit, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::{CMD, DEFAULT_LIMIT, matches_filter, page_has_params, pagination, parameter_json, response};
    use crate::Engine;
    use serde_json::{Value, json};
    use soundcraft_dsp::{Category, ParamInfo, PluginInfo, Taper, Unit};
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
        assert_eq!(changed.get("display"), Some(&json!(info.param("mid_gain").unwrap().format(7.5))));
        assert!(changed.get("display_min").is_none());
        assert!(changed.get("display_max").is_none());
    }

    #[test]
    fn built_in_probe_reads_live_values_and_range_displays() {
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        e.execute("mix.insert_param", &json!({"track": track, "slot": 0, "param": "mid_gain", "value": 7.5})).unwrap();
        let result = e.execute(CMD, &json!({"track": track, "slot": 0, "filter": "mid_gain", "probe": true})).unwrap();
        let param = listed_params(&result).iter().find(|param| param.get("id") == Some(&json!("mid_gain"))).unwrap();
        let info = soundcraft_dsp::plugin_info("eq_7band").unwrap().param("mid_gain").unwrap();
        assert_eq!(param.get("value"), Some(&json!(7.5)));
        assert_eq!(param.get("value_source"), Some(&json!("session")));
        assert_eq!(param.get("display"), Some(&json!(info.format(7.5))));
        assert_eq!(param.get("display_min"), Some(&json!(info.format(info.min))));
        assert_eq!(param.get("display_max"), Some(&json!(info.format(info.max))));
    }

    #[test]
    fn a_probe_of_an_empty_page_loads_nothing() {
        let info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
        let count = info.params.len();
        assert!(page_has_params(info, None, 0));
        assert!(page_has_params(info, None, count - 1));
        assert!(!page_has_params(info, None, count), "an offset past the end");
        assert!(page_has_params(info, Some("mid_gain"), 0));
        assert!(!page_has_params(info, Some("no such parameter"), 0), "a filter that matches nothing");
        let mut e = Engine::default();
        let track = insert_eq(&mut e);
        let past = e.execute(CMD, &json!({"track": track, "slot": 0, "offset": count, "probe": true})).unwrap();
        assert!(listed_params(&past).is_empty());
        assert_eq!(past.get("total"), Some(&json!(count)));
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
        let value = parameter_json(&param, &Insert::new("test"), false, None);
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
            json!({"track": track, "slot": 0, "probe": null}),
            json!({"track": track, "slot": 0, "probe": 1}),
            json!({"track": track, "slot": 0, "probe": "true"}),
        ] {
            assert!(e.execute(CMD, &params).is_err(), "accepted {params}");
        }
    }

    #[test]
    fn empty_instrument_unknown_plugin_and_unknown_slot_are_errors() {
        let mut e = Engine::default();
        let audio = new_track(&mut e, None);
        assert!(e.execute(CMD, &json!({"track": audio, "slot": 0})).unwrap_err().to_string().contains("empty slot"));
        assert!(e.execute(CMD, &json!({"track": audio, "slot": 0, "probe": true})).unwrap_err().to_string().contains("empty slot"));
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
        for probe in [false, true] {
            e.execute(CMD, &json!({"track": track, "slot": 0, "probe": probe})).unwrap();
            assert_eq!(e.revision, revision);
            assert_eq!(e.undo_history(), undo);
            assert_eq!(e.journal, journal);
            assert_eq!(e.is_dirty(), dirty);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_dls_instrument_lists_numeric_parameter_ids() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "instrument", "instrument": "au:aumu:dls :appl"})).unwrap();
        let track = e.session().tracks[0].id.0;
        let result = e.execute(CMD, &json!({"track": track, "slot": "instrument", "limit": 1000})).unwrap();
        let params = listed_params(&result);
        if params.is_empty() {
            return; // no parameters reported on this system
        }
        assert!(params.iter().all(|param| param.get("id").and_then(Value::as_str).and_then(|id| id.parse::<u32>().ok()).is_some()));
        assert!(params.iter().all(|param| param.get("display") == Some(&Value::Null)));

        let probed = e.execute(CMD, &json!({"track": track, "slot": "instrument", "limit": 1000, "probe": true})).unwrap();
        let params = listed_params(&probed);
        assert!(!params.is_empty());
        assert!(params.iter().all(|param| param.get("value_source") == Some(&json!("plugin"))));
        assert!(params.iter().all(|param| param.get("display").and_then(Value::as_str).is_some()));
        assert!(params.iter().all(|param| param.get("display_min").and_then(Value::as_str).is_some()));
        assert!(params.iter().all(|param| param.get("display_max").and_then(Value::as_str).is_some()));
    }

    /// Surge XT reports 775 parameters: more than one default page.
    #[test]
    fn a_large_plugin_pages_by_the_default_limit() {
        let ids: Vec<&'static str> = (0..775).map(|i| &*format!("p{i:03}").leak()).collect();
        let params: &'static [ParamInfo] = ids
            .iter()
            .map(|&id| ParamInfo { id, name: id, min: 0.0, max: 1.0, default: 0.5, unit: Unit::None, taper: Taper::Linear, choices: &[] })
            .collect::<Vec<_>>()
            .leak();
        let info = PluginInfo { id: "big", name: "Big", short_name: "Big", category: Category::Eq, params, is_instrument: false, audiosuite: false };
        let insert = Insert::new("big");
        let mut seen = Vec::new();
        let mut offset = 0;
        loop {
            let page = response(&info, &insert, None, offset, DEFAULT_LIMIT, None);
            assert_eq!(page["total"], 775);
            assert_eq!(page["offset"], offset);
            assert_eq!(page["limit"], DEFAULT_LIMIT);
            let listed = listed_params(&page);
            assert_eq!(listed.len(), DEFAULT_LIMIT.min(775 - offset));
            seen.extend(listed.iter().filter_map(|p| p["id"].as_str().map(str::to_string)));
            match page["next_offset"].as_u64() {
                Some(next) => {
                    assert_eq!(page["has_more"], true);
                    offset = usize::try_from(next).unwrap();
                }
                None => {
                    assert_eq!(page["has_more"], false);
                    assert!(page["next_offset"].is_null());
                    break;
                }
            }
        }
        assert_eq!(seen, ids);
        let past_end = response(&info, &insert, None, 10_000, DEFAULT_LIMIT, None);
        assert_eq!(past_end["has_more"], false);
        assert!(listed_params(&past_end).is_empty());
    }
}
