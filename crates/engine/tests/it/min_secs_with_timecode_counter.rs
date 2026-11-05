use serde_json::json;
use soundcraft_engine::Engine;

#[test]
fn min_secs_string_is_not_misread_as_timecode_when_the_counter_is_timecode() {
    let mut e = Engine::default();
    e.execute("view.main_counter", &json!({"format": "timecode"})).unwrap();
    e.execute("edit.select", &json!({"start": "0:12.500", "end": "0:20.000"})).unwrap();
    let sel = e.session().edit.selection;
    assert_eq!((sel.start, sel.end), (600_000, 960_000));
}
