use serde_json::json;
use soundcraft_engine::{Engine, model::Session};
use soundcraft_time::{FrameRate, SampleRate};

fn feet_frames(engine: &mut Engine, at: i64) -> serde_json::Value {
    engine.execute("session.format_time", &json!({"at": at, "format": "feet_frames"})).unwrap()["text"].clone()
}

#[test]
fn current_feet_frames_sets_the_reading_at_the_insertion_point() {
    let mut session = Session::new("Feet frames", SampleRate::HZ_48000);
    session.frame_rate = FrameRate::Fps30;
    let mut engine = Engine::new(session);
    engine.execute("edit.select", &json!({"start": 48_000})).unwrap();
    assert_eq!(feet_frames(&mut engine, 48_000), "1+14");

    let response = engine.execute("setup.current_feet_frames", &json!({"feet_frames": "12+08"})).unwrap();
    assert_eq!(response, json!({"feet_frames_start": 170}));

    assert_eq!(feet_frames(&mut engine, 48_000), "12+08");
    // 30 frames later, at 16 frames per foot.
    assert_eq!(feet_frames(&mut engine, 96_000), "14+06");
    assert_eq!(engine.session().edit.selection.start, 48_000);
    let samples = engine.execute("session.format_time", &json!({"at": 48_000, "format": "samples"})).unwrap();
    assert_eq!(samples["text"], "48000");

    // Typed positions go through the same reference.
    engine.execute("edit.select", &json!({"start": "feet_frames:14+06"})).unwrap();
    assert_eq!(engine.session().edit.selection.start, 96_000);
}
