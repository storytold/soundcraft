use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, TrackKind};

#[test]
fn mixed_rows_create_in_order_select_every_track_and_undo_in_one_step() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Existing"})).unwrap();
    let before = e.session().to_json().unwrap();
    let result = e
        .execute(
            "track.new",
            &json!({"rows": [
                {"count": 2, "kind": "audio", "format": "mono", "name": "Dialogue", "timebase": "samples"},
                {"kind": "aux", "format": "stereo", "name": "Reverb", "timebase": "samples"},
                {"kind": "instrument", "format": "stereo", "name": "Keys", "timebase": "ticks", "instrument": "subtractive_synth"},
                {"kind": "folder", "format": "stereo", "name": "Music"}
            ]}),
        )
        .unwrap();
    let s = e.session();
    let created = &s.tracks[1..];
    assert_eq!(created.len(), 5);
    assert_eq!(
        created.iter().map(|t| t.kind).collect::<Vec<_>>(),
        [TrackKind::Audio, TrackKind::Audio, TrackKind::Aux, TrackKind::Instrument, TrackKind::Folder]
    );
    assert_eq!(
        created.iter().map(|t| t.format).collect::<Vec<_>>(),
        [ChannelFormat::Mono, ChannelFormat::Mono, ChannelFormat::Stereo, ChannelFormat::Stereo, ChannelFormat::Stereo]
    );
    assert!(!created[0].ticks_timebase);
    assert!(created[3].ticks_timebase && created[3].instrument.is_some());
    assert_eq!(s.busses.len(), 1);
    let ids = created.iter().map(|t| t.id).collect::<Vec<_>>();
    assert_eq!(s.edit.selected_tracks, ids);
    assert_eq!(result["tracks"], json!(ids));
    let after = s.to_json().unwrap();
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), before);
    assert!(e.redo());
    assert_eq!(e.session().to_json().unwrap(), after);
}

#[test]
fn invalid_later_row_leaves_no_partial_creation_or_undo_step() {
    for field in [json!({"kind": "unknown"}), json!({"format": "unknown"})] {
        let mut e = Engine::default();
        let before = e.session().to_json().unwrap();
        assert!(e.execute("track.new", &json!({"rows": [{"kind": "folder"}, field]})).is_err());
        assert_eq!(e.session().to_json().unwrap(), before);
        assert!(!e.can_undo());
        assert!(e.journal.is_empty());
    }
}

#[test]
fn row_shape_and_total_size_are_bounded() {
    for rows in [json!(null), json!({}), json!([]), json!([true]), json!([{"rows": []}]), json!([{"count": 128}, {}]), json!(vec![json!({}); 129])] {
        let mut e = Engine::default();
        assert!(e.execute("track.new", &json!({"rows": rows})).is_err(), "{rows}");
        assert!(e.session().tracks.is_empty());
        assert!(!e.can_undo());
    }
    let mut e = Engine::default();
    e.execute("track.new", &json!({"rows": [{"count": 64}, {"count": 64}]})).unwrap();
    assert_eq!(e.session().tracks.len(), 128);
}

#[test]
fn legacy_single_row_parameters_keep_their_defaults_and_count_clamp() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "aux", "count": -1})).unwrap();
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].format, ChannelFormat::Stereo);
    e.execute("track.new", &json!({"count": 200})).unwrap();
    assert_eq!(e.session().tracks.len(), 129);
}
