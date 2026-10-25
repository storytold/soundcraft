//! Every command survives hostile parameters (what an agent or a buggy script might send).

use proptest::prelude::*;
use serde_json::{Value, json};

const KEYS: &[&str] = &[
    "track",
    "tracks",
    "start",
    "end",
    "at",
    "length",
    "clip",
    "clips",
    "db",
    "delta_db",
    "value",
    "pan",
    "count",
    "ratio",
    "bpm",
    "format",
    "slot",
    "param",
    "plugin",
    "mode",
    "tool",
    "kind",
    "name",
    "index",
    "number",
    "semitones",
    "by",
    "to",
    "grid",
    "height",
    "view",
    "color",
    "params",
    "process",
    "group",
    "bus",
    "input",
    "output",
    "numerator",
    "denominator",
    "factor",
    "x",
    "by_px",
    "samples_per_px",
    "sensitivity",
    "rating",
    "direction",
    "seconds",
    "frames",
    "samples",
    "notes",
    "pitch",
    "velocity",
    "flag",
];

fn hostile_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(json!(null)),
        Just(json!(-1)),
        Just(json!(0)),
        Just(json!(i64::MAX)),
        Just(json!(i64::MIN)),
        Just(json!(1.0e308)),
        Just(json!(-1.0e308)),
        Just(json!(0.5)),
        Just(json!("")),
        Just(json!("Kick")),
        Just(json!("bars_beats:9999999|99|9999")),
        Just(json!("\u{0}\u{fffd}")),
        Just(json!([])),
        Just(json!([1, "Kick", null, -5])),
        Just(json!({})),
        Just(json!({"seconds": -1e12})),
        Just(json!({"a": 1})),
        Just(json!(true)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 6, .. ProptestConfig::default() })]
    #[test]
    fn no_command_panics_on_hostile_params(fields in proptest::collection::vec((0usize..KEYS.len(), hostile_value()), 0..6)) {
        let mut params = serde_json::Map::new();
        for (k, v) in fields {
            params.insert(KEYS[k].to_string(), v);
        }
        let params = Value::Object(params);
        for spec in soundcraft_engine::command_specs() {
            // Commands that write files take paths; keep the test hermetic.
            if spec.params.contains("path") || spec.params.contains("dir") || spec.id == "app.quit" {
                continue;
            }
            let mut e = soundcraft_engine::demo::demo_engine();
            let _ = e.execute("edit.select", &json!({"tracks": ["Kick", "Keys"], "start": 100_000, "end": 600_000}));
            if let Err(soundcraft_engine::EngineError::Internal(id, msg)) = e.execute(spec.id, &params) {
                prop_assert!(false, "{id} panicked with {params}: {msg}");
            }
        }
    }
}
