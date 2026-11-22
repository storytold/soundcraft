use super::*;
use soundcraft_model::{ClipContent, TrackId};
use soundcraft_playback::{midi::RecordedMidi, record::Recorder};

#[test]
fn coast_keeps_the_audio_and_midi_take_through_clock_loss_and_continue() {
    record_through_loss("coast");
}

#[test]
fn stop_on_clock_loss_commits_the_audio_and_midi_take() {
    record_through_loss("stop");
}

fn armed(app: &mut SoundApp, kind: &str) -> TrackId {
    let id = app.run("track.new", json!({"kind":kind,"format":"mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    app.run("mix.record_arm", json!({"tracks":[id],"value":true})).unwrap();
    TrackId(id)
}

// Inject at the MIDI callback's timestamped output boundary.
fn play_note(app: &mut SoundApp, port: u8, pitch: u8) {
    let at = app.position();
    for (at, status) in [(at, 0x90), (at + 48, 0x80)] {
        app.midi_take.push(RecordedMidi { at, port, status, data1: pitch, data2: 90 });
    }
}

fn record_through_loss(policy: &str) {
    let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
    let audio = armed(&mut app, "audio");
    let midi = armed(&mut app, "midi");
    let (recorder, mut capture) = Recorder::simulated_input();
    app.recorder = Some(recorder);
    app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into(), "Keys".into()]));
    app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true,"loss_policy":policy})).unwrap();
    let feed = app.midi_inputs.as_ref().unwrap().feed().clone();
    let src = feed.ports().assign("DIN");
    let keys = feed.ports().assign("Keys");

    // The source supplies 100 BPM to a 120 BPM session before transport Start.
    for pulse in 0..24 {
        feed.set_clock(pulse * 25_000);
        feed.receive(src, &[0xf8]);
    }
    feed.set_clock(600_000);
    feed.receive(src, &[0xfa]);
    feed.receive(src, &[0xf8]);
    app.service_clock_follow();
    app.handle_transport(0.);
    assert_eq!(app.engine.transport_tempo().tempo_at_tick(0), 100.);
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.);
    assert!(app.is_playing() && app.engine.transport.recording, "{}", app.ui.status);
    let plan = app.take_plan.as_ref().unwrap();
    let path = plan.takes[0].path.clone();
    let start = plan.start;
    capture(&[0.25; 12000]);
    play_note(&mut app, keys, 60);
    app.handle_transport(0.25);

    // Exercise the actual missing-clock timeout, without sending a MIDI Stop.
    feed.set_clock(850_000);
    app.service_clock_follow();
    app.handle_transport(0.);
    let coast = policy == "coast";
    assert_eq!(app.clock_follow.snapshot.state, if coast { State::Coasting } else { State::Stopped });
    assert_eq!((app.is_playing(), app.engine.transport.recording), (coast, coast));
    capture(&[-0.5; 12000]);
    if coast {
        play_note(&mut app, keys, 67);
        app.handle_transport(0.25);
        let before = app.position();
        // Re-lock one pulse late; a new play request would move the take's playhead.
        feed.set_clock(1_125_000);
        feed.receive(src, &[0xfb]);
        feed.receive(src, &[0xf8]);
        app.service_clock_follow();
        app.handle_transport(0.);
        assert_eq!(app.clock_follow.snapshot.state, State::Locked);
        assert_eq!(app.position(), before, "Continue must not seek inside the rolling take");
        assert!(app.engine.transport.recording);
        play_note(&mut app, keys, 72);
        capture(&[0.125; 1200]);
        app.handle_transport(0.025);
        feed.receive(src, &[0xfc]);
        app.service_clock_follow();
        app.handle_transport(0.);
    }
    assert!(!app.is_playing() && !app.engine.transport.recording);
    let s = app.engine.session();
    let clips = s.track(audio).unwrap().clips();
    let expected_len = if coast { 25200 } else { 12000 };
    assert_eq!((clips.len(), clips[0].start, clips[0].range().len()), (1, start, expected_len));
    assert!(app.clock_follow.audio_sources.contains(&clips[0].source().unwrap()), "retain the grid for this recorded audio source");
    let samples = &craft_audio::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels[0];
    assert_eq!(samples.len(), expected_len as usize);
    for (i, sample) in samples.iter().enumerate() {
        let expected = match i {
            0..12000 => 0.25,
            12000..24000 => -0.5,
            _ => 0.125,
        };
        assert!((sample - expected).abs() < 0.0001);
    }
    let live = app.engine.playback_session();
    let clips = live.track(midi).unwrap().clips();
    assert_eq!((clips.len(), clips[0].start, clips[0].range().len()), (1, start, expected_len));
    assert!(app.clock_follow.audio_sources.contains(&clips[0].source().unwrap()), "retain the grid for this recorded audio source");
    let ClipContent::Midi { sequence } = &clips[0].content else { panic!("expected MIDI") };
    let notes: Vec<_> = sequence.notes.iter().map(|n| (n.pitch, n.start)).collect();
    assert_eq!(notes, if coast { vec![(60, 0), (67, 400), (72, 800)] } else { vec![(60, 0)] });
    app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
    let reopened = soundcraft_model::Session::from_json(&app.engine.session().to_json().unwrap()).unwrap();
    let clip = &reopened.track(midi).unwrap().clips()[0];
    let ClipContent::Midi { sequence } = &clip.content else {
        panic!("expected saved MIDI");
    };
    let samples: Vec<_> = sequence.notes.iter().map(|n| reopened.tempo.tick_to_samples(n.start, reopened.sample_rate)).collect();
    assert_eq!(samples, if coast { vec![0, 12_000, 24_000] } else { vec![0] });
    std::fs::remove_dir_all(app.fallback_take_dir.as_ref().unwrap()).unwrap();
}

#[test]
fn hour_one_clock_take_keeps_its_onsets_after_history_compaction_and_reopen() {
    use soundcraft_engine::io::{MidiTakeEvent, add_midi_recording};
    use soundcraft_time::Range;
    let mut app = SoundApp::new(Default::default(), None, crate::Services::default());
    let track = armed(&mut app, "midi");
    app.midi_inputs = Some(MidiInputs::with_port_names(vec![], vec!["DIN".into()]));
    app.run("setup.midi_clock_follow", json!({"input":"DIN","enabled":true})).unwrap();
    let sr = app.engine.session().sample_rate;
    let mut grid = TempoMap::default();
    let quarter = TICKS_PER_QUARTER;
    // Alternating observed tempi, one external estimate every 30 s. Capture well
    // inside the first hour and keep generating changes into hour six.
    let step = 60 * quarter;
    for i in 0..100 {
        grid.set_tempo(i * step, if i % 2 == 0 { 120. } else { 135. }).unwrap();
    }
    let first = 34 * step;
    let last = 37 * step;
    let range = Range::new(grid.tick_to_samples(first, sr), grid.tick_to_samples(last, sr));
    let note_at = grid.tick_to_samples(first + step, sr);
    let off = grid.tick_to_samples(first + 2 * step, sr);
    let events = [
        MidiTakeEvent { at: note_at, device: 0, status: 0x90, data1: 64, data2: 100 },
        MidiTakeEvent { at: off, device: 0, status: 0x80, data1: 64, data2: 0 },
    ];
    app.engine.playback_tempo = Some(grid.clone());
    let take = add_midi_recording(&mut app.engine, range, &events, &["keys".into()], false);
    assert_eq!(take.clips.len(), 1);
    let saved_before = app.engine.session().track(track).unwrap().clips()[0].clone();
    for i in 100..850 {
        let tempo = if i % 2 == 0 { 120. } else { 135. };
        grid.set_tempo(i * step, tempo).unwrap();
        app.engine.playback_tempo.as_mut().unwrap().set_tempo(i * step, tempo).unwrap();
        app.clock_follow.grid_dirty = true;
        app.service_clock_follow();
    }
    let compacted = app.engine.playback_tempo.as_ref().unwrap();
    assert!(compacted.tempos().len() < grid.tempos().len() && compacted.tempos().len() <= 512);
    for tick in (first..=last).step_by(19).chain([850 * step, 900 * step]) {
        assert_eq!(compacted.tick_to_samples(tick, sr), grid.tick_to_samples(tick, sr), "sample position at tick {tick}");
    }
    let play = app.engine.playback_session();
    let clip = play.find_clip(take.clips[0]).unwrap().1;
    let ClipContent::Midi { sequence } = &clip.content else { panic!("expected MIDI take") };
    let origin = play.tempo.samples_to_ticks(clip.start, sr);
    assert_eq!(play.tempo.tick_to_samples(origin + sequence.notes[0].start, sr), note_at);
    assert_eq!(play.tempo.tick_to_samples(origin + sequence.notes[0].start + sequence.notes[0].length, sr), off);
    app.run("setup.midi_clock_follow", json!({"enabled":false})).unwrap();
    let reopened = soundcraft_model::Session::from_json(&app.engine.session().to_json().unwrap()).unwrap();
    let clip = reopened.find_clip(take.clips[0]).unwrap().1;
    assert_eq!(clip, &saved_before);
    let ClipContent::Midi { sequence } = &clip.content else { panic!("expected saved take") };
    let origin = reopened.tempo.samples_to_ticks(clip.start, sr);
    let note = sequence.notes[0];
    assert_eq!(reopened.tempo.tick_to_samples(origin + note.start, sr), note_at);
    assert_eq!(reopened.tempo.tick_to_samples(origin + note.start + note.length, sr), off);
}
