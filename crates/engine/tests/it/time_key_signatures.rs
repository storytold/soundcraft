use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Session, TrackKind};

fn engine_with_keys() -> Engine {
    let mut session = Session::default();
    session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut engine = Engine::new(session);
    for (at, key) in [(10, "C major"), (48000, "D major"), (100000, "E major")] {
        engine.execute("event.add_key_change", &json!({"key": key, "at": at})).unwrap();
    }
    engine
}

fn keys(engine: &Engine) -> Vec<(u64, String)> {
    engine.session().key_signatures.iter().map(|(at, k)| (*at as u64, k.clone())).collect()
}

fn k(at: u64, key: &str) -> (u64, String) {
    (at, key.to_string())
}

#[test]
fn insert_time_moves_key_signatures() {
    let mut engine = engine_with_keys();
    let original = keys(&engine);
    engine.execute("event.insert_time", &json!({"start": 48000, "length": 1000})).unwrap();
    assert_eq!(keys(&engine), vec![k(10, "C major"), k(49000, "D major"), k(101000, "E major")]);
    engine.undo();
    assert_eq!(keys(&engine), original);
}

#[test]
fn cut_time_removes_and_moves_key_signatures() {
    let mut engine = engine_with_keys();
    let original = keys(&engine);
    engine.execute("event.cut_time", &json!({"start": 20, "end": 60000})).unwrap();
    assert_eq!(keys(&engine), vec![k(10, "C major"), k(40020, "E major")]);
    engine.undo();
    assert_eq!(keys(&engine), original);
}
