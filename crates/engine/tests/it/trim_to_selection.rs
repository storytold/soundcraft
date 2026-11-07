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

fn three_clip_session() -> (Session, soundcraft_model::TrackId, [soundcraft_model::ClipId; 3]) {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let ids = [session.new_clip_id(), session.new_clip_id(), session.new_clip_id()];
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![
        Clip::audio(ids[0], "free", SourceId(1), 0, 0, 100),
        Clip::audio(ids[1], "edit-locked", SourceId(1), 0, 100, 100),
        Clip::audio(ids[2], "time-locked", SourceId(1), 0, 200, 100),
    ];
    let pl = session.track_mut(track).unwrap().playlist_mut().unwrap();
    pl.clips[1].edit_locked = true;
    pl.clips[2].time_locked = true;
    (session, track, ids)
}

#[test]
fn trim_to_selection_leaves_locked_clips_whole() {
    let (session, track, ids) = three_clip_session();
    let mut engine = Engine::new(session);
    let result = engine.execute("edit.trim_to_selection", &json!({"tracks": [track.0], "start": 50, "end": 250})).unwrap();
    let clips = engine.session().track(track).unwrap().clips();
    assert_eq!(clips.iter().map(|c| (c.id, c.start, c.length)).collect::<Vec<_>>(), vec![(ids[0], 50, 50), (ids[1], 100, 100), (ids[2], 200, 100)]);
    assert_eq!(result["trimmed"], json!(1));
    engine.undo();
    assert_eq!(engine.session().track(track).unwrap().clips()[0].length, 100);
}

#[test]
fn edit_trim_skips_locked_clips() {
    let (session, track, ids) = three_clip_session();
    let mut engine = Engine::new(session);
    engine.execute("edit.trim_start_to_insertion", &json!({"tracks": [track.0], "at": 150})).unwrap();
    engine.execute("edit.trim_end_to_insertion", &json!({"tracks": [track.0], "at": 250})).unwrap();
    let clips = engine.session().track(track).unwrap().clips();
    assert_eq!(
        clips.iter().map(|c| (c.id, c.start, c.length)).collect::<Vec<_>>(),
        vec![(ids[0], 0, 100), (ids[1], 100, 100), (ids[2], 200, 100)],
        "only unlocked clips trim; the edit-locked and time-locked ones keep their extent"
    );
}
