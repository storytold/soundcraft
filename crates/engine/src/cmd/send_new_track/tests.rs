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
    let out =
        e.execute(CMD, &json!({"track": "Gtr", "slot": 1, "kind": "aux", "format": "stereo", "timebase": "ticks", "name": "Drum Sub"})).unwrap();
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

/// The peak of the main mix while "Gtr" plays a tone that reaches it only through send A, at
/// 0 dB, to a `kind` track this command makes.
fn peak_through_a_new_track(kind: &str) -> f32 {
    use craft_audio::AudioBuffer;
    use soundcraft_model::{Clip, SourceAudio, SourceId};
    use soundcraft_time::Range;
    use std::sync::Arc;
    let mut e = engine();
    let n = 4096;
    let tone: Vec<f32> = (0..n).map(|i| (i as f32 * 0.07).sin() * 0.5).collect();
    let s = e.session_mut();
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![tone] })));
    let clip = s.new_clip_id();
    let gtr = s.tracks.iter_mut().find(|t| t.name == "Gtr").unwrap();
    gtr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(1), 0, 0, n as i64));
    gtr.mixer.output = Route::None;
    let peak =
        |e: &Engine| soundcraft_mix::render_range(e.session(), Range::new(0, n as i64), 512).iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    assert_eq!(peak(&e), 0.0, "without the send, nothing reaches the main mix");
    if e.execute(CMD, &json!({"track": "Gtr", "slot": 0, "kind": kind, "name": "Return"})).is_ok() {
        e.execute("mix.send", &json!({"track": "Gtr", "slot": 0, "bus": "Return", "level_db": 0.0})).unwrap();
    }
    peak(&e)
}

/// The new Aux Input plays what the send feeds it: the 0.5 tone at the -3 dB centre of the pan law.
#[test]
fn the_new_aux_plays_what_the_send_feeds_it() {
    let peak = peak_through_a_new_track("aux");
    assert!((peak - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "peak {peak}");
}

/// Audio and Instrument tracks don't play the bus on their input, so the send would be lost: the
/// command refuses them (and makes nothing) rather than make a track that never hears it.
#[test]
fn tracks_that_would_drop_the_send_are_refused() {
    for kind in ["audio", "Audio Track", "instrument", "Instrument Track"] {
        assert_eq!(peak_through_a_new_track(kind), 0.0, "{kind}");
        let mut e = engine();
        let err = e.execute(CMD, &json!({"track": "Gtr", "kind": kind, "name": "Return"})).unwrap_err();
        assert!(err.to_string().contains("doesn't play the bus a send feeds"), "{kind}: {err}");
        assert_eq!((order(&e), e.session().busses.len()), (vec!["Gtr".to_string(), "Bass".into(), "Main Out".into()], 0), "{kind}");
    }
}

#[test]
fn a_taken_name_numbers_the_track_and_the_bus_together() {
    let mut e = engine();
    e.execute("track.new", &json!({"kind": "aux", "name": "Verb"})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "name": "Verb"})).unwrap();
    let verb = track(&e, "Verb 1");
    assert_eq!(e.session().busses.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Verb 1"]);
    assert_eq!(verb.mixer.input, Route::Bus(e.session().busses[0].id));

    // Only a bus has the name (in any case): the track and the bus are numbered together, so they
    // still share a name and the send can't land on the old bus.
    let mut e = engine();
    e.execute("mix.new_bus", &json!({"name": "DELAY"})).unwrap();
    e.execute("mix.new_bus", &json!({"name": "delay 2"})).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "name": "Delay"})).unwrap();
    let bus = e.session().bus_by_name("Delay 3").map(|b| b.id).unwrap();
    assert_eq!(e.session().bus(bus).map(|b| b.name.as_str()), Some("Delay 3"));
    assert_eq!(order(&e), ["Gtr", "Delay 3", "Bass", "Main Out"], "the track has the bus's name");
    assert_eq!(track(&e, "Delay 3").mixer.input, Route::Bus(bus));
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

/// A bus already has the longest name allowed: the new bus's number fits within the limit, so the
/// session read back from its file still tells the two buses apart.
#[test]
fn a_numbered_bus_name_stays_within_the_name_limit() {
    let mut e = engine();
    let long = "b".repeat(MAX_NAME_CHARS);
    e.execute("mix.new_bus", &json!({"name": long})).unwrap();
    let old = e.session().bus_by_name(&long).map(|b| b.id).unwrap();
    e.execute(CMD, &json!({"track": "Gtr", "slot": 0, "name": long})).unwrap();
    let name = format!("{} 2", "b".repeat(MAX_NAME_CHARS - 2));
    let Route::Bus(new) = track(&e, &name).mixer.input else { panic!("the new track has no bus input") };
    assert_eq!(e.session().bus(new).map(|b| b.name.as_str()), Some(name.as_str()), "the track and the bus share the name");
    assert_eq!(send_target(&e, "Gtr", 0), Some(Route::Bus(new)));

    let back = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(back.bus(new).map(|b| b.name.as_str()), Some(name.as_str()), "the name is kept as it is");
    assert_eq!(back.bus_by_name(&name).map(|b| b.id), Some(new), "the name still finds the new bus");
    assert_eq!(back.bus_by_name(&long).map(|b| b.id), Some(old));
}

/// A track already has the longest name allowed: the new track and its bus share a numbered name
/// within the limit, so the session read back from its file still tells the two tracks apart.
#[test]
fn a_numbered_track_name_stays_within_the_name_limit() {
    let mut e = engine();
    let long = "t".repeat(MAX_NAME_CHARS);
    e.execute("track.new", &json!({"kind": "aux", "name": long})).unwrap();
    let old = track(&e, &long).id;
    let out = e.execute(CMD, &json!({"track": "Gtr", "name": long})).unwrap();
    let name = format!("{} 2", "t".repeat(MAX_NAME_CHARS - 2));
    assert_eq!(out["name"], json!(name));
    let new = track(&e, &name).id;
    assert_eq!(e.session().bus_by_name(&name).map(|b| Route::Bus(b.id)), Some(track(&e, &name).mixer.input.clone()));

    let back = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(back.track_by_name(&name).map(|t| t.id), Some(new), "the name still finds the new track");
    assert_eq!(back.track_by_name(&long).map(|t| t.id), Some(old));
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

/// As in Pro Tools, a Master Fader has no sends, and the mixer never plays one: the command
/// refuses it rather than make an Aux Input that never hears anything.
#[test]
fn a_master_fader_source_is_refused() {
    let mut e = engine();
    let err = e.execute(CMD, &json!({"track": "Main Out", "name": "Cue"})).unwrap_err();
    assert!(err.to_string().contains("a Master Fader has no sends"), "{err}");
    assert_eq!((order(&e), e.session().busses.len()), (vec!["Gtr".to_string(), "Bass".into(), "Main Out".into()], 0));
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
        json!({"track": "Main Out"}),
        json!({"track": "Bass"}),
    ];
    for params in refused {
        assert!(e.execute(CMD, &params).is_err(), "{params} was accepted");
        assert_eq!((e.undo_history().len(), order(&e), e.session().busses.len()), (history, tracks.clone(), busses), "{params} changed the session");
    }
    assert!(track(&e, "Gtr").mixer.sends.iter().all(Option::is_none));
}
