use super::*;
use soundcraft_model::{ChannelFormat, Session, Track};

/// An engine with mono audio tracks "Gtr" and "Bass" and a Master Fader, in that order.
fn engine() -> Engine {
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Gtr"));
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Main Out"));
    Engine::new(s)
}

/// The track names, top to bottom.
fn order(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().map(|t| t.name.clone()).collect()
}

/// The track named `name`.
fn track<'a>(e: &'a Engine, name: &str) -> &'a Track {
    e.session().tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no track {name}: {:?}", order(e)))
}

/// Where send `slot` of track `name` goes.
fn send_target(e: &Engine, name: &str, slot: usize) -> Option<Route> {
    track(e, name).mixer.sends.get(slot).cloned().flatten().map(|s| s.target)
}

#[test]
fn the_new_track_takes_the_name_of_its_bus_and_listens_to_it() {
    let mut e = engine();
    let history = e.undo_history().len();
    let out = e
        .execute(CMD, &json!({"track": "Gtr", "slot": 1, "kind": "aux", "format": "stereo", "timebase": "ticks", "name": "Drum Sub"}))
        .unwrap();
    assert_eq!(order(&e), ["Gtr", "Drum Sub", "Bass", "Main Out"], "created next to the track with the send");
    let sub = track(&e, "Drum Sub");
    assert_eq!((sub.kind, sub.format, sub.ticks_timebase), (TrackKind::Aux, ChannelFormat::Stereo, true));
    let bus = e.session().bus_by_name("Drum Sub").cloned().unwrap();
    assert_eq!((bus.name.as_str(), bus.format), ("Drum Sub", ChannelFormat::Stereo), "the bus has the track's name and width");
    assert_eq!(sub.mixer.input, Route::Bus(bus.id), "the bus is the new track's input");
    assert_eq!(sub.mixer.output, Route::Main, "the new track plays out like any new track");
    assert_eq!(send_target(&e, "Gtr", 1), Some(Route::Bus(bus.id)), "the send feeds the bus");
    assert_eq!(send_target(&e, "Gtr", 0), None, "only the chosen slot");
    assert_eq!(out, json!({"track": sub.id, "bus": bus.id, "slot": 1, "name": "Drum Sub"}));
    assert_eq!(e.session().busses.len(), 1);

    assert_eq!(e.undo_history().len(), history + 1, "one Undo step");
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Send to New Track"));
    assert!(e.undo());
    assert_eq!(order(&e), ["Gtr", "Bass", "Main Out"], "Undo removes the track");
    assert!(e.session().busses.is_empty(), "and the bus");
    assert_eq!(send_target(&e, "Gtr", 1), None, "and the send");
}

#[test]
fn without_choices_it_makes_an_aux_in_the_first_free_send() {
    let mut e = engine();
    e.execute("mix.new_bus", &json!({"name": "Verb"})).unwrap();
    e.execute("mix.send", &json!({"track": "Gtr", "slot": 0, "bus": "Verb"})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "name": "  "})).unwrap();
    // A blank name is the default one, numbered like any new Aux Input.
    let aux = track(&e, "Aux 1");
    assert_eq!((aux.kind, aux.format), (TrackKind::Aux, ChannelFormat::Stereo));
    let bus = e.session().bus_by_name("Aux 1").map(|b| b.id).unwrap();
    assert_eq!(aux.mixer.input, Route::Bus(bus));
    assert_eq!(send_target(&e, "Gtr", 1), Some(Route::Bus(bus)), "send B, the first free one");
    assert!(matches!(send_target(&e, "Gtr", 0), Some(Route::Bus(b)) if Some(b) == e.session().bus_by_name("Verb").map(|v| v.id)));
}

#[test]
fn audio_and_instrument_tracks_can_be_made_too() {
    let mut e = engine();
    e.execute(CMD, &json!({"track": "Bass", "slot": "c", "kind": "audio", "format": "mono", "name": "Print"})).unwrap();
    let print = track(&e, "Print");
    assert_eq!((print.kind, print.format), (TrackKind::Audio, ChannelFormat::Mono));
    assert_eq!(e.session().bus_by_name("Print").map(|b| b.format), Some(ChannelFormat::Mono));
    assert_eq!(send_target(&e, "Bass", 2), e.session().bus_by_name("Print").map(|b| Route::Bus(b.id)));

    e.execute(CMD, &json!({"track": "Gtr", "kind": "Instrument Track", "name": "Synth"})).unwrap();
    let synth = track(&e, "Synth");
    assert_eq!((synth.kind, synth.format), (TrackKind::Instrument, ChannelFormat::Stereo));
    assert!(synth.instrument.is_some(), "an Instrument track gets its instrument, as from Track › New");
    assert_eq!(order(&e), ["Gtr", "Synth", "Bass", "Print", "Main Out"]);
}

#[test]
fn a_taken_name_numbers_the_track_and_the_bus_together() {
    let mut e = engine();
    e.execute("track.new", &json!({"kind": "aux", "name": "Verb"})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "name": "Verb"})).unwrap();
    let verb = track(&e, "Verb 1");
    assert_eq!(e.session().busses.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Verb 1"]);
    assert_eq!(verb.mixer.input, Route::Bus(e.session().busses[0].id));

    // Only a bus has the name (in any case): the track keeps it and the bus is numbered, so the
    // send can't land on the old bus.
    let mut e = engine();
    e.execute("mix.new_bus", &json!({"name": "DELAY"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "delay 2"})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "name": "Delay"})).unwrap();
    let bus = e.session().bus_by_name("Delay 3").map(|b| b.id).unwrap();
    assert_eq!(track(&e, "Delay").mixer.input, Route::Bus(bus));
    assert_eq!(send_target(&e, "Gtr", 0), Some(Route::Bus(bus)));
    assert_eq!(e.session().busses.len(), 3);
}

#[test]
fn a_long_name_is_cut_to_the_name_limit() {
    let mut e = engine();
    e.execute(CMD, &json!({"track": "Gtr", "name": "é".repeat(MAX_NAME_CHARS + 50)})).unwrap();
    let name = "é".repeat(MAX_NAME_CHARS);
    assert_eq!(e.session().tracks[1].name, name);
    assert!(e.session().bus_by_name(&name).is_some());
}

#[test]
fn an_existing_send_moves_to_the_new_track_with_its_settings() {
    let mut e = engine();
    e.execute("mix.new_bus", &json!({"name": "Old"})).unwrap();
    e.execute("mix.send", &json!({"track": "Gtr", "slot": 0, "bus": "Old", "level_db": -9.0, "pre_fader": true})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "slot": 0, "name": "New"})).unwrap();
    let send = track(&e, "Gtr").mixer.sends[0].clone().unwrap();
    assert_eq!(Some(send.target), e.session().bus_by_name("New").map(|b| Route::Bus(b.id)));
    assert_eq!((send.level_db, send.pre_fader), (-9.0, true));
}

#[test]
fn next_to_track_off_puts_the_track_where_track_new_would() {
    let mut e = engine();
    e.execute(CMD, &json!({"track": "Gtr", "name": "FX", "next_to_track": false})).unwrap();
    assert_eq!(order(&e), ["Gtr", "Bass", "FX", "Main Out"]);
}

#[test]
fn next_to_a_track_in_a_folder_is_inside_that_folder() {
    let mut e = engine();
    e.execute("track.new", &json!({"kind": "folder", "name": "Band"})).unwrap();
    let s = e.session_mut();
    // Band, then Gtr and Bass inside it, then the Master Fader.
    let band = s.tracks.remove(2);
    let band_id = band.id;
    s.tracks.insert(0, band);
    for t in s.tracks.iter_mut().filter(|t| t.name == "Gtr" || t.name == "Bass") {
        t.folder = Some(band_id);
    }
    e.execute(CMD, &json!({"track": "Gtr", "name": "FX"})).unwrap();
    assert_eq!(order(&e), ["Band", "Gtr", "FX", "Bass", "Main Out"]);
    assert_eq!(track(&e, "FX").folder, Some(band_id), "it joins the folder");

    // From the folder itself, the track goes below everything in it, at the folder's own level.
    e.execute(CMD, &json!({"track": "Band", "name": "Stem"})).unwrap();
    assert_eq!(order(&e), ["Band", "Gtr", "FX", "Bass", "Stem", "Main Out"]);
    assert_eq!(track(&e, "Stem").folder, None);
}

#[test]
fn a_send_from_the_master_fader_still_leaves_it_at_the_bottom() {
    let mut e = engine();
    e.execute(CMD, &json!({"track": "Main Out", "name": "Cue"})).unwrap();
    assert_eq!(order(&e), ["Gtr", "Bass", "Cue", "Main Out"]);
}

#[test]
fn refusals_make_nothing() {
    let mut e = engine();
    for slot in 0..SEND_SLOTS {
        e.execute("mix.new_bus", &json!({"name": format!("B{slot}")})).unwrap();
        e.execute("mix.send", &json!({"track": "Bass", "slot": slot, "bus": format!("B{slot}")})).unwrap();
    }
    let (history, tracks, busses) = (e.undo_history().len(), order(&e), e.session().busses.len());
    let refused = [
        json!({"track": "Gtr", "kind": "midi"}),
        json!({"track": "Gtr", "kind": "vca"}),
        json!({"track": "Gtr", "kind": "folder"}),
        json!({"track": "Gtr", "kind": "master"}),
        json!({"track": "Gtr", "kind": "video"}),
        json!({"track": "Gtr", "kind": "drum"}),
        json!({"track": "Gtr", "format": "quad-ish"}),
        json!({"track": "Gtr", "timebase": "frames"}),
        json!({"track": "Gtr", "kind": 1}),
        json!({"track": "Gtr", "format": ["stereo"]}),
        json!({"track": "Gtr", "name": 7}),
        json!({"track": "Gtr", "next_to_track": "no"}),
        json!({"track": "Gtr", "slot": 10}),
        json!({"track": "Gtr", "slot": "k"}),
        json!({"track": "Nobody"}),
        json!({"track": "Bass"}),
    ];
    for params in refused {
        assert!(e.execute(CMD, &params).is_err(), "{params} was accepted");
        assert_eq!((e.undo_history().len(), order(&e), e.session().busses.len()), (history, tracks.clone(), busses), "{params} changed the session");
    }
    assert!(track(&e, "Gtr").mixer.sends.iter().all(Option::is_none));
}
