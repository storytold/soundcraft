use serde_json::json;
use soundcraft_engine::{Engine, EngineError};

fn engine_with_kick() -> Engine {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Kick", "format": "stereo"})).unwrap();
    e
}

fn kick_pan(e: &Engine) -> Vec<f32> {
    e.session().tracks.iter().find(|t| t.name == "Kick").unwrap().mixer.pan.to_vec()
}

fn kick_trim(e: &Engine) -> f32 {
    e.session().tracks.iter().find(|t| t.name == "Kick").unwrap().mixer.trim_db
}

#[test]
fn mix_pan_rejects_a_missing_or_non_numeric_pan_and_keeps_the_pan() {
    let mut e = engine_with_kick();
    e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": -1.0})).unwrap();
    assert_eq!(kick_pan(&e)[0], -1.0, "left panner set");
    assert_eq!(kick_pan(&e)[1], -1.0, "right panner also set (both moved together without index)");

    for params in [json!({"tracks": ["Kick"]}), json!({"tracks": ["Kick"], "pan": "left"}), json!({"tracks": ["Kick"], "pan": null})] {
        let r = e.execute("mix.pan", &params);
        assert!(matches!(r, Err(EngineError::BadParams(..))), "{params}: expected BadParams, got {r:?}");
        assert_eq!(kick_pan(&e)[0], -1.0, "{params}: the pan must be untouched");
    }
}

#[test]
fn mix_trim_rejects_a_missing_or_non_numeric_db_and_keeps_the_trim() {
    let mut e = engine_with_kick();
    e.execute("mix.trim", &json!({"tracks": ["Kick"], "db": -6.0})).unwrap();
    assert_eq!(kick_trim(&e), -6.0);

    for params in [json!({"tracks": ["Kick"]}), json!({"tracks": ["Kick"], "db": "loud"}), json!({"tracks": ["Kick"], "db": null})] {
        let r = e.execute("mix.trim", &params);
        assert!(matches!(r, Err(EngineError::BadParams(..))), "{params}: expected BadParams, got {r:?}");
        assert_eq!(kick_trim(&e), -6.0, "{params}: the trim must be untouched");
    }
}

#[test]
fn mix_pan_without_index_moves_all_panners() {
    let mut e = engine_with_kick();
    e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": -0.3})).unwrap();
    let pan = kick_pan(&e);
    assert_eq!(pan.len(), 2, "stereo track has 2 panners");
    assert_eq!(pan[0], -0.3, "left panner changed");
    assert_eq!(pan[1], -0.3, "right panner also changed (both moved together)");
}

#[test]
fn mix_pan_with_explicit_index_moves_only_that_panner() {
    let mut e = engine_with_kick();
    e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": -0.5, "index": 0})).unwrap();
    let pan = kick_pan(&e);
    assert_eq!(pan[0], -0.5, "left panner changed");
    assert_eq!(pan[1], 1.0, "right panner unchanged");
}

#[test]
fn mix_pan_index_1_on_mono_track_is_an_error() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Mono", "format": "mono"})).unwrap();
    let r = e.execute("mix.pan", &json!({"track": "Mono", "pan": 0.5, "index": 1}));
    assert!(matches!(r, Err(EngineError::BadParams(..))), "index out of range should error");
}

#[test]
fn mix_pan_non_integer_index_is_an_error() {
    let mut e = engine_with_kick();
    let r = e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": 0.5, "index": "0"}));
    assert!(matches!(r, Err(EngineError::BadParams(..))), "string index should error");
}

#[test]
fn mix_pan_index_out_of_range_is_an_error() {
    let mut e = engine_with_kick();
    // Stereo track has 2 panners (indices 0 and 1); index 2 is out of range.
    let r = e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": 0.5, "index": 2}));
    assert!(matches!(r, Err(EngineError::BadParams(..))), "index beyond len should error");
}

#[test]
fn mix_pan_negative_index_is_an_error() {
    let mut e = engine_with_kick();
    let r = e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": 0.5, "index": -1}));
    assert!(matches!(r, Err(EngineError::BadParams(..))), "negative index should error");
}

#[test]
fn mix_pan_fractional_index_is_an_error() {
    let mut e = engine_with_kick();
    let r = e.execute("mix.pan", &json!({"tracks": ["Kick"], "pan": 0.5, "index": 0.5}));
    assert!(matches!(r, Err(EngineError::BadParams(..))), "fractional index should error");
}
