//! A take being recorded is never replaced or quit away from.
use serde_json::json;
use soundcraft_engine::{Engine, EngineError};

#[test]
fn nothing_replaces_or_quits_a_session_while_it_records() {
    let mut e = Engine::default();
    e.transport.recording = true;
    for (id, params) in [
        ("session.new", json!({})),
        ("session.open", json!({"path": "Song.scraft"})),
        ("session.close", json!({})),
        ("session.revert", json!({})),
        ("app.quit", json!({})),
    ] {
        assert!(matches!(e.execute(id, &params), Err(EngineError::Disabled(_, why)) if why == "stop recording first"), "{id}");
    }
    assert!(e.execute("track.new", &json!({})).is_ok(), "editing goes on");
    e.transport.recording = false;
    assert!(e.execute("session.close", &json!({})).is_ok());
}
