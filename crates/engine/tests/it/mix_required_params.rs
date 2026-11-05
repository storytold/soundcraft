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
    assert_eq!(kick_pan(&e)[0], -1.0);

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
