use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_time::Range;

fn render(e: &Engine) -> Vec<Vec<f32>> {
    soundcraft_mix::render_range(e.session(), Range::new(0, 72_000), 512)
}

#[test]
fn midi_track_offset_moves_the_notes_like_moving_the_clip() {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Timing", "instrument": "subtractive_synth"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    let r = e.execute("midi.new_clip", &json!({"track": track, "start": 12_000, "length": 96_000})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0, "length_ticks": 960, "velocity": 100})).unwrap();
    let baseline = render(&e);
    assert!(baseline.iter().flatten().any(|x| x.abs() > 1e-6), "the fixture must make sound");

    e.execute("event.midi_track_offsets", &json!({"track": track, "offset": 24_000})).unwrap();
    let offset = render(&e);
    assert!(offset != baseline, "the offset must change the render");

    e.execute("event.midi_track_offsets", &json!({"track": track, "offset": 0})).unwrap();
    assert!(render(&e) == baseline, "a zero offset plays as before");

    // The offset shifts playback by exactly as much as moving the clip would.
    e.execute("edit.move_clips", &json!({"clip": clip, "by": 24_000})).unwrap();
    assert!(render(&e) == offset, "the offset must match moving the clip by 24000 samples");
}
