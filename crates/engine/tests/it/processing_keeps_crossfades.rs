//! Processing a clip in place (AudioSuite, TCE, Elastic) keeps the crossfade it shares with its
//! neighbour: the neighbour keeps its overlap and its fade, so the edit does not dip to silence.
//! Where the processed clip reaches past its old edges, it still overwrites what is there.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, ClipId, Fade, FadeShape, Source, SourceAudio, SourceId, TrackId};
use std::sync::Arc;

fn source(id: u64, name: &str) -> Source {
    Source {
        id: SourceId(id),
        name: name.into(),
        path: format!("{name}.wav"),
        channels: 1,
        frames: 20_000,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    }
}

/// A on 0..1200 fading out over 1000..1200, B on 1000..3000 fading in over the same 200 samples.
fn crossfade() -> (Engine, TrackId, ClipId, ClipId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap());
    let s = e.session_mut();
    for (id, name) in [(1, "a"), (2, "b")] {
        s.sources.push(source(id, name));
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 20_000]] };
        s.pool.insert(SourceId(id), Arc::new(SourceAudio::new(buf)));
    }
    let (a, b) = (s.new_clip_id(), s.new_clip_id());
    let fade = Fade { len: 200, shape: FadeShape::EqualPower, curvature: 0.0 };
    let mut ca = Clip::audio(a, "a", SourceId(1), 5000, 0, 1200);
    ca.fade_out = fade;
    let mut cb = Clip::audio(b, "b", SourceId(2), 5000, 1000, 2000);
    cb.fade_in = fade;
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![ca, cb];
    (e, t, a, b)
}

/// (start, end, fade in, fade out) of each clip on the track, in order.
fn layout(e: &Engine, t: TrackId) -> Vec<(i64, i64, i64, i64)> {
    e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end(), c.fade_in.len, c.fade_out.len)).collect()
}

fn run(e: &mut Engine, cmd: &str, clip: ClipId, mut params: Value) {
    params["clips"] = json!([clip.0]);
    e.execute(cmd, &params).unwrap_or_else(|err| panic!("{cmd} {params}: {err}"));
}

#[test]
fn processing_either_clip_of_a_crossfade_keeps_the_crossfade() {
    let unchanged = vec![(0, 1200, 0, 200), (1000, 3000, 200, 0)];
    for (cmd, params) in [("audiosuite.process", json!({"plugin": "reverse"})), ("audiosuite.process", json!({"plugin": "invert"}))] {
        for later in [false, true] {
            let (mut e, t, a, b) = crossfade();
            run(&mut e, cmd, if later { b } else { a }, params.clone());
            assert_eq!(layout(&e, t), unchanged, "{cmd} {params} on {}", if later { "B" } else { "A" });
        }
    }
    // TCE over the clip's own range renders it again in place.
    let (mut e, t, _, b) = crossfade();
    run(&mut e, "edit.tce_to_timeline", b, json!({"start": 1000, "end": 3000}));
    assert_eq!(layout(&e, t), unchanged, "TCE of B in place");
}

#[test]
fn a_processed_clip_that_grows_away_from_the_crossfade_keeps_it() {
    // Elastic and TCE that lengthen B keep its start: A keeps its overlap and its fade-out.
    let (mut e, t, _, b) = crossfade();
    e.session_mut().track_mut(t).unwrap().elastic = Some("polyphonic".into());
    run(&mut e, "clip.elastic_properties", b, json!({"ratio": 2.0}));
    assert_eq!(layout(&e, t), vec![(0, 1200, 0, 200), (1000, 5000, 200, 0)], "Elastic x2 on B");
    let (mut e, t, _, b) = crossfade();
    run(&mut e, "edit.tce_to_timeline", b, json!({"start": 1000, "end": 5000}));
    assert_eq!(layout(&e, t)[0], (0, 1200, 0, 200), "TCE of B to 1000..5000");
}

#[test]
fn a_processed_clip_still_overwrites_past_its_old_edges() {
    // TCE of B to 900..2900 starts it inside A: A is cut back to 900, as before.
    let (mut e, t, _, b) = crossfade();
    run(&mut e, "edit.tce_to_timeline", b, json!({"start": 900, "end": 2900}));
    assert_eq!(layout(&e, t)[0].1, 900, "A ends where B now starts");
    assert_eq!(layout(&e, t)[1].0, 900);
}
