use super::*;

/// A real aux lookahead queues the direct strip's impulse for 240 samples. No master
/// latency is present, so any later physical output comes from the per-strip alignment.
fn primed_aux_alignment() -> (Session, TrackId, MixEngine, Vec<Vec<f32>>) {
    let mut session = Session::default();
    session.edit.click = false;
    session.edit.delay_compensation = true;
    let direct = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let track = session.track_mut(direct).unwrap();
    track.mixer.input_monitor = true;
    track.mixer.input = Route::Hardware("In 1".into());
    track.mixer.output = Route::Hardware("Out 1".into());
    let bus = session.add_bus("latent aux", ChannelFormat::Mono);
    let aux = session.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
    session.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    let mut lookahead = Insert::new("compressor");
    lookahead.params.insert("lookahead".into(), 5.0);
    lookahead.params.insert("ratio".into(), 1.0);
    session.track_mut(aux).unwrap().mixer.inserts[0] = Some(lookahead);
    let mut engine = MixEngine::new(48_000.0, 64);
    engine.set_hardware_channels(48);
    engine.monitor_only = true;
    engine.input = vec![vec![0.0; 64]];
    engine.input[0][0] = 0.25;
    let mut main = vec![vec![0.0; 64]; 2];
    engine.render(&session, 0, 64, &mut main);
    assert_eq!(engine.aux_latency, 240);
    assert_eq!(engine.latency, 240);
    assert!(engine.master_latencies.is_empty());
    assert!(engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0));
    engine.input[0].fill(0.0);
    (session, direct, engine, main)
}

/// A queued Out 1 impulse must not be delivered to newly selected Out 2 merely because
/// both destinations share Main and have the same one-channel, 5 ms alignment buffer.
#[test]
fn pending_physical_audio_is_discarded_when_the_destination_changes() {
    let (mut session, direct, mut engine, mut main) = primed_aux_alignment();
    session.track_mut(direct).unwrap().mixer.output = Route::Hardware("Out 2".into());
    for block in 1..9 {
        engine.render(&session, block * 64, 64, &mut main);
        assert!(engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0), "queued Out 1 audio leaked after rerouting in block {block}");
    }
}

/// Enlarging a path changes its physical identity even when its first channel is unchanged.
#[test]
fn pending_physical_audio_is_discarded_when_the_destination_width_changes() {
    let (mut session, direct, mut engine, mut main) = primed_aux_alignment();
    session.track_mut(direct).unwrap().format = ChannelFormat::Stereo;
    engine.sync(&session);
    for block in 1..9 {
        engine.render(&session, block * 64, 64, &mut main);
        assert!(
            engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0),
            "queued mono audio leaked into the replacement stereo span in block {block}"
        );
    }
}

/// An ordinary session update and an equivalent route spelling must retain the current
/// destination's queued samples rather than creating a brief gap in uninterrupted playback.
#[test]
fn unchanged_physical_destination_preserves_pending_audio_across_session_updates() {
    for route in ["Out 1", "Out 1-1"] {
        let (mut session, direct, mut engine, mut main) = primed_aux_alignment();
        let track = session.track_mut(direct).unwrap();
        track.name = "renamed while running".into();
        track.mixer.output = Route::Hardware(route.into());
        engine.sync(&session);
        let mut actual = Vec::new();
        for block in 1..9 {
            engine.render(&session, block * 64, 64, &mut main);
            actual.extend_from_slice(&engine.hardware_output()[0]);
            assert!(engine.hardware_output()[1..].iter().flatten().all(|sample| *sample == 0.0));
        }
        assert_eq!(impulse_at(&actual), vec![240 - 64], "unchanged route {route}");
        assert!((actual[240 - 64] - 0.25).abs() < 1e-6);
    }
}

/// Leaving a shared physical route clears its private history, including a temporary invalid,
/// disjoint, or nonhardware assignment before another shared destination is selected.
#[test]
fn invalid_disjoint_and_nonhardware_transitions_do_not_restore_pending_audio() {
    for route in [Route::Hardware("Out 49".into()), Route::Hardware("Out 48".into()), Route::Main, Route::None] {
        let (mut session, direct, mut engine, mut main) = primed_aux_alignment();
        session.track_mut(direct).unwrap().mixer.output = route.clone();
        engine.render(&session, 64, 64, &mut main);
        assert!(engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0));
        session.track_mut(direct).unwrap().mixer.output = Route::Hardware("Out 2".into());
        for block in 2..9 {
            engine.render(&session, block * 64, 64, &mut main);
            assert!(
                engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0),
                "pending audio survived transition through {route:?} in block {block}"
            );
        }
    }
}

/// Inactive strips do not run their routing step. Reactivation must not revive audio that
/// was pending before the strip stopped, even when route edits end at its original output.
#[test]
fn reactivating_a_track_does_not_restore_pending_physical_audio() {
    for reroute_while_inactive in [false, true] {
        let (mut session, direct, mut engine, mut main) = primed_aux_alignment();
        session.track_mut(direct).unwrap().inactive = true;
        for block in 1..9 {
            if reroute_while_inactive {
                session.track_mut(direct).unwrap().mixer.output = Route::Hardware(if block < 5 { "Out 2" } else { "Out 1" }.into());
            }
            engine.render(&session, block * 64, 64, &mut main);
            assert!(engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0), "inactive track produced physical audio");
        }
        session.track_mut(direct).unwrap().inactive = false;
        for block in 9..17 {
            engine.render(&session, block * 64, 64, &mut main);
            assert!(
                engine.hardware_output().iter().flatten().all(|sample| *sample == 0.0),
                "reactivation restored pending audio, rerouted={reroute_while_inactive}, block={block}"
            );
        }
    }
}
