use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Session, TrackKind};
use std::sync::Arc;

#[test]
fn truncated_ascii_and_unicode_names_cannot_collide_with_another_track() {
    for character in ['A', '界'] {
        let existing: String = std::iter::repeat_n(character, 64).collect();
        let mut session = Session::default();
        let occupied = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(&existing));
        let target = session.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Target"));
        let mut engine = Engine::new(session);
        let before = engine.session_arc();
        let revision = engine.revision;

        let result = engine.execute("track.rename", &json!({"track": target.0, "name": format!("  {existing} extra  ")}));
        assert!(result.is_err());
        assert!(Arc::ptr_eq(&before, &engine.session_arc()));
        assert_eq!(engine.revision, revision);
        assert_eq!(engine.session().track(occupied).unwrap().name, existing);
        assert_eq!(engine.session().track(target).unwrap().name, "Target");
        assert!(!engine.is_dirty());
        assert!(!engine.can_undo());
    }
}

#[test]
fn rename_returns_the_stored_unicode_name_and_undo_restores_the_document() {
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Surround51, Some("Original"));
    session.track_mut(track).unwrap().comments = "Retain this comment".into();
    let mut engine = Engine::new(session);
    let before = engine.session().to_json().unwrap();
    let stored = "é".repeat(64);

    let result = engine.execute("track.rename", &json!({"track": track.0, "name": format!(" {stored}extra ")})).unwrap();
    assert_eq!(result["name"], stored);
    let renamed = engine.session().track(track).unwrap();
    assert_eq!(renamed.name, stored);
    assert_eq!(renamed.name.chars().count(), 64);
    assert_eq!(renamed.id, track);
    assert_eq!(renamed.format, ChannelFormat::Surround51);
    assert_eq!(renamed.comments, "Retain this comment");
    assert_eq!(engine.undo_history(), ["Rename Track"]);
    assert!(engine.undo());
    assert_eq!(engine.session().to_json().unwrap(), before);
    assert!(engine.redo());
    assert_eq!(engine.session().track(track).unwrap().name, stored);
}

#[test]
fn unchanged_name_after_truncation_does_not_make_an_undo_step() {
    let name = "A".repeat(64);
    let mut session = Session::default();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&name));
    let mut engine = Engine::new(session);
    let before = engine.session_arc();
    let revision = engine.revision;

    let result = engine.execute("track.rename", &json!({"track": track.0, "name": format!("{name} suffix")})).unwrap();
    assert_eq!(result["name"], name);
    assert!(Arc::ptr_eq(&before, &engine.session_arc()));
    assert_eq!(engine.revision, revision);
    assert!(!engine.is_dirty());
    assert!(!engine.can_undo());
}

#[test]
fn routing_folder_rename_preserves_unique_owned_bus_routes_and_history() {
    use soundcraft_model::{BusId, Route, TrackId};

    let stored = "界".repeat(64);
    let first_name = format!("{stored} Bus");
    let second_name = format!("{stored} Bus 2");
    let mut session = Session::default();
    let member = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Member"));
    let user_bus = session.add_bus(&first_name, ChannelFormat::Stereo);
    let second_user_bus = session.add_bus(&second_name, ChannelFormat::Stereo);
    let mut engine = Engine::new(session);
    let unrelated = engine.execute("track.new", &json!({"kind": "folder", "name": "Unrelated"})).unwrap();
    let unrelated = TrackId(unrelated["tracks"][0].as_u64().unwrap());
    let unrelated_route = engine.session().track(unrelated).unwrap().mixer.input.clone();
    let created = engine.execute("track.move_to_new_folder", &json!({"tracks": [member.0], "name": "Initial"})).unwrap();
    let folder = TrackId(created["folder"].as_u64().unwrap());
    let owned_bus = BusId(created["bus"].as_u64().unwrap());
    let before = engine.session().to_json().unwrap();
    let history = engine.undo_history();
    let bus_count = engine.session().busses.len();

    let result = engine.execute("track.rename", &json!({"track": folder.0, "name": format!("  {stored} suffix  ")})).unwrap();
    assert_eq!(result["name"], stored);
    assert_eq!(engine.session().track(folder).unwrap().name, stored);
    assert_eq!(engine.session().bus(owned_bus).unwrap().name, format!("{stored} Bus 3"));
    assert_eq!(engine.session().bus(user_bus).unwrap().name, first_name);
    assert_eq!(engine.session().bus(second_user_bus).unwrap().name, second_name);
    assert_eq!(engine.session().track(unrelated).unwrap().mixer.input, unrelated_route);
    assert_eq!(engine.session().track(folder).unwrap().mixer.input, Route::Bus(owned_bus));
    assert_eq!(engine.session().track(member).unwrap().mixer.output, Route::Bus(owned_bus));
    assert_eq!(engine.session().track(member).unwrap().folder, Some(folder));
    assert_eq!(engine.session().busses.len(), bus_count);
    assert_eq!(engine.undo_history().len(), history.len() + 1);
    assert_eq!(engine.undo_label(), Some("Rename Track"));
    let renamed = engine.session().to_json().unwrap();

    let document = engine.session_arc();
    let revision = engine.revision;
    let history = engine.undo_history();
    engine.execute("track.rename", &json!({"track": folder.0, "name": format!("{stored} more")})).unwrap();
    assert!(Arc::ptr_eq(&document, &engine.session_arc()));
    assert_eq!(engine.revision, revision);
    assert_eq!(engine.undo_history(), history);
    assert!(engine.undo());
    assert_eq!(engine.session().to_json().unwrap(), before);
    assert!(engine.redo());
    assert_eq!(engine.session().to_json().unwrap(), renamed);

    // Current-main same-name folder rename also repairs the folder's own bus, without taking
    // either colliding user bus or changing its members' bus identity.
    engine.session_mut().busses.iter_mut().find(|bus| bus.id == owned_bus).unwrap().name = "Manually named owned bus".into();
    let manual = engine.session().to_json().unwrap();
    engine.execute("track.rename", &json!({"track": folder.0, "name": stored})).unwrap();
    assert_eq!(engine.session().bus(owned_bus).unwrap().name, format!("{stored} Bus 3"));
    assert_eq!(engine.session().bus(user_bus).unwrap().name, first_name);
    assert_eq!(engine.session().bus(second_user_bus).unwrap().name, second_name);
    assert_eq!(engine.session().track(member).unwrap().mixer.output, Route::Bus(owned_bus));
    assert!(engine.undo());
    assert_eq!(engine.session().to_json().unwrap(), manual);
    assert!(engine.redo());
    assert_eq!(engine.session().to_json().unwrap(), renamed);
}

#[test]
fn truncated_folder_collision_refuses_bus_and_document_changes_with_redo_pending() {
    use soundcraft_model::{Route, TrackId};

    for character in ['B', '界'] {
        let occupied_name: String = std::iter::repeat_n(character, 64).collect();
        let mut session = Session::default();
        let occupied = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(&occupied_name));
        let mut engine = Engine::new(session);
        let created = engine.execute("track.new", &json!({"kind": "folder", "name": "Target"})).unwrap();
        let folder = TrackId(created["tracks"][0].as_u64().unwrap());
        let owned_bus = match engine.session().track(folder).unwrap().mixer.input {
            Route::Bus(bus) => bus,
            _ => panic!("the routing folder must own a bus"),
        };
        engine.execute("mix.volume", &json!({"track": folder.0, "db": -3})).unwrap();
        assert!(engine.undo());
        engine.mark_clean();
        let document = engine.session_arc();
        let before = engine.session().to_json().unwrap();
        let revision = engine.revision;
        let journal = engine.journal.clone();
        let undo = engine.undo_history();
        let redo = engine.redo_label().map(str::to_owned);
        assert!(redo.is_some());

        let result = engine.execute("track.rename", &json!({"track": folder.0, "name": format!(" {occupied_name} suffix ")}));
        assert!(result.unwrap_err().to_string().contains(&occupied_name));
        assert!(Arc::ptr_eq(&document, &engine.session_arc()));
        assert_eq!(engine.session().to_json().unwrap(), before);
        assert_eq!(engine.revision, revision);
        assert_eq!(engine.journal, journal);
        assert_eq!(engine.undo_history(), undo);
        assert_eq!(engine.redo_label(), redo.as_deref());
        assert!(!engine.is_dirty());
        assert_eq!(engine.session().track(occupied).unwrap().name, occupied_name);
        assert_eq!(engine.session().track(folder).unwrap().name, "Target");
        assert_eq!(engine.session().bus(owned_bus).unwrap().name, "Target Bus");
        assert_eq!(engine.session().track(folder).unwrap().mixer.input, Route::Bus(owned_bus));
    }
}
