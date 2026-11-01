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

#[test]
fn export_midi_leaves_out_what_a_trimmed_clip_no_longer_plays() {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Trim", "instrument": "subtractive_synth"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    let r = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96_000})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0, "length_ticks": 960, "velocity": 100})).unwrap();
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 62, "start_ticks": 800, "length_ticks": 960, "velocity": 100})).unwrap();
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 64, "start_ticks": 2000, "length_ticks": 960, "velocity": 100})).unwrap();
    // 24000 samples is 960 ticks at 120 bpm.
    let id = e.session().tracks[0].clips()[0].id;
    e.session_mut().find_clip_mut(id).unwrap().trim_end_to(24_000);

    let bytes = soundcraft_engine::io::export_midi(&e, None).unwrap();
    let smf = soundcraft_midi::read_smf(&bytes).unwrap();
    let notes = &smf.tracks[0].sequence.notes;
    assert_eq!(notes.len(), 2, "only notes starting inside the clip are exported");
    let late = notes.iter().find(|n| n.pitch == 62).unwrap();
    assert_eq!(late.start + late.length, 960, "a note running past the clip end is cut at it");
}

#[test]
fn export_midi_releases_a_sustain_held_past_the_trim() {
    use soundcraft_midi::{CtrlEvent, CtrlKind};
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Sus", "instrument": "subtractive_synth"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96_000})).unwrap();
    let id = e.session().tracks[0].clips()[0].id;
    {
        let clip = e.session_mut().find_clip_mut(id).unwrap();
        if let soundcraft_model::ClipContent::Midi { sequence } = &mut clip.content {
            let cc = |tick, channel, value| CtrlEvent { tick, channel, kind: CtrlKind::Cc(64), value };
            sequence.ctrls = vec![cc(100, 0, 127), cc(200, 1, 127), cc(300, 1, 0), cc(2000, 0, 0)];
        }
        clip.trim_end_to(24_000);
    }
    let bytes = soundcraft_engine::io::export_midi(&e, None).unwrap();
    let smf = soundcraft_midi::read_smf(&bytes).unwrap();
    let ctrls = &smf.tracks[0].sequence.ctrls;
    let sus: Vec<_> = ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(64)).collect();
    assert!(sus.iter().any(|c| c.channel == 0 && c.tick == 960 && c.value == 0), "{sus:?}");
    assert!(!sus.iter().any(|c| c.channel == 1 && c.tick == 960), "released channel gets no extra event: {sus:?}");
}
