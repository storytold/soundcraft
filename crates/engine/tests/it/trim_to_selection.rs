use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, Session, SourceId, TrackKind};

#[test]
fn trim_to_selection_keeps_clips_sorted() {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let long = session.new_clip_id();
    let short = session.new_clip_id();
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips =
        vec![Clip::audio(long, "long", SourceId(1), 0, 0, 30), Clip::audio(short, "short", SourceId(1), 0, 5, 5)];
    let mut engine = Engine::new(session);
    engine.execute("edit.trim_to_selection", &json!({"tracks": [track.0], "start": 15, "end": 20})).unwrap();
    let clips = engine.session().track(track).unwrap().clips();
    assert_eq!(clips.iter().map(|c| (c.id, c.start, c.length)).collect::<Vec<_>>(), vec![(short, 5, 5), (long, 15, 5)]);
    engine.undo();
    let clips = engine.session().track(track).unwrap().clips();
    assert_eq!(clips.iter().map(|c| (c.id, c.start, c.length)).collect::<Vec<_>>(), vec![(long, 0, 30), (short, 5, 5)]);
}
