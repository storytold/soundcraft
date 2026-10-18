//! Synthetic callback/control regressions: no device or installed plugin is required.

use super::*;
use soundcraft_model::{AutoParam, AutomationLane, AutomationPoint, ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};

fn source(amplitude: f32) -> Arc<Session> {
    let mut session = Session::default();
    session.edit.click = false;
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    session.pool.insert(
        SourceId(5),
        Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![amplitude; 8192]] })),
    );
    let clip = session.new_clip_id();
    let track = session.track_mut(track).unwrap();
    track.playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(5), 0, 0, 8192));
    track.automation = vec![
        AutomationLane {
            param: AutoParam::Volume,
            points: vec![AutomationPoint { at: 0, value: -6.0 }, AutomationPoint { at: 64, value: 0.0 }],
            enabled: true,
        },
        AutomationLane {
            param: AutoParam::Pan(0),
            points: vec![AutomationPoint { at: 0, value: -0.5 }, AutomationPoint { at: 64, value: 0.5 }],
            enabled: true,
        },
        AutomationLane {
            param: AutoParam::Mute,
            points: vec![AutomationPoint { at: 0, value: 0.0 }, AutomationPoint { at: 96, value: 1.0 }],
            enabled: true,
        },
    ];
    Arc::new(session)
}

fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    })
}

fn state(source: Arc<Session>, device_rate: f64) -> (Sender<Cmd>, AudioState) {
    let (tx, rx) = channel();
    (tx, AudioState::new(rx, shared(), source, device_rate, channel().0))
}

#[test]
fn initial_callback_state_owns_the_exact_control_prepared_source() {
    let source = source(0.1);
    let audio = Arc::clone(source.pool.get(SourceId(5)).unwrap());
    let audio_owners = Arc::strong_count(&audio);
    let (_, state) = state(Arc::clone(&source), 48_000.0);
    assert!(state.session.is_indexed());
    assert!(Arc::ptr_eq(state.session.owned_source().unwrap(), &source));
    assert!(Arc::ptr_eq(state.session.source().pool.get(SourceId(5)).unwrap(), &audio));
    assert_eq!(Arc::strong_count(&audio), audio_owners, "preparation must not clone source audio");
    assert_eq!(Arc::strong_count(&source), 2, "the pair owns one document reference");
    drop(source);
    assert_eq!(Arc::strong_count(state.session.owned_source().unwrap()), 1);
}

#[test]
fn player_update_publishes_an_indexed_pair_before_callback_adoption() {
    let source = source(0.1);
    let (tx, rx) = channel();
    let player = Player {
        shared: shared(),
        fault: Arc::new(Fault::default()),
        #[cfg(not(target_arch = "wasm32"))]
        watch: Mutex::new(None),
        tx,
        side: Mutex::new(Side::new(48_000.0, channel().1)),
        _stream: None,
        device_name: "synthetic".into(),
        device_rate: 48_000,
        device_channels: 2,
        device_buffer: None,
        silent: true,
    };
    player.update_session(Arc::clone(&source));
    let (snapshot, instances) = match rx.try_recv().unwrap() {
        Cmd::Session(snapshot, instances) => Some((snapshot, instances)),
        _ => None,
    }
    .unwrap();
    assert!(snapshot.is_indexed(), "the queued command must already own its index");
    assert!(Arc::ptr_eq(snapshot.owned_source().unwrap(), &source));
    assert!(instances.is_empty());
    assert_eq!(Arc::strong_count(&source), 2);
}

#[test]
fn prepared_callback_blocks_match_the_ordinary_mix_across_automation_points() {
    let source = source(0.1);
    let (_, mut callback) = state(Arc::clone(&source), 48_000.0);
    callback.playing = true;
    let mut ordinary = new_engine(48_000.0);
    let mut expected = vec![vec![0.0; BLOCK]; main_channels(&source)];
    for frames in [1, 7, 23, 33, 31, 1, 17] {
        let at = callback.pos;
        ordinary.render(&source, at, frames, &mut expected);
        callback.render_next(frames);
        assert_eq!(callback.pending_len, frames);
        assert_eq!(callback.pos, at + frames as i64);
        for (actual, expected) in callback.pending.iter().zip(&expected) {
            assert_eq!(
                actual[..frames].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected[..frames].iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
    assert!(callback.pending.iter().all(|channel| channel[..callback.pending_len].iter().all(|value| *value == 0.0)), "mute endpoint was crossed");
}

#[test]
fn same_rate_update_preserves_pending_pcm_and_fractional_device_cursor() {
    let (tx, mut callback) = state(source(0.05), 96_000.0);
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut first = [0.0; 2];
    callback.fill(&mut first, 2);
    assert_eq!((callback.pos, callback.pending_len, callback.read), (1, 1, 0.5));
    let pending = callback.pending.clone();
    let next = source(0.25);
    tx.send(Cmd::Session(prepare_snapshot(Arc::clone(&next)), Vec::new())).unwrap();
    callback.drain();
    assert_eq!((callback.pos, callback.pending_len, callback.read), (1, 1, 0.5));
    assert_eq!(callback.pending, pending, "an ordinary update cannot discard unread audio");
    let mut second = [0.0; 2];
    callback.fill(&mut second, 2);
    for (channel, actual) in second.iter().enumerate() {
        let a = pending[channel][0];
        let b = pending[channel][1];
        assert_eq!(actual.to_bits(), (a + (b - a) * 0.5).to_bits());
    }
    assert_eq!((callback.pos, callback.pending_len, callback.read), (1, 1, 1.0));
    let mut third = [0.0; 2];
    callback.fill(&mut third, 2);
    assert_eq!(callback.pos, 2);
    assert!(third[0] > first[0] * 4.0, "the following block uses the new snapshot");
    assert!(Arc::ptr_eq(callback.session.owned_source().unwrap(), &next));
}

#[test]
fn queued_document_and_index_pairs_retire_together_on_the_control_side() {
    let initial = source(0.1);
    let initial_document = Arc::downgrade(&initial);
    let (tx, rx) = channel();
    let (gtx, grx) = channel();
    let mut callback = AudioState::new(rx, shared(), initial, 48_000.0, gtx);
    let initial_pair = Arc::downgrade(&callback.session);
    let mut side = Side::new(48_000.0, grx);
    side.return_old_sessions(&tx);
    let mut pairs = vec![initial_pair];
    let mut documents = vec![initial_document];
    for amplitude in [0.2, 0.3, 0.4] {
        let document = source(amplitude);
        documents.push(Arc::downgrade(&document));
        let pair = prepare_snapshot(document);
        pairs.push(Arc::downgrade(&pair));
        tx.send(Cmd::Session(pair, Vec::new())).unwrap();
    }
    callback.drain();
    assert!(pairs.iter().all(|owner| owner.strong_count() == 1));
    assert!(documents.iter().all(|owner| owner.strong_count() == 1));
    side.collect_garbage();
    assert!(pairs[..3].iter().all(|owner| owner.strong_count() == 0));
    assert!(documents[..3].iter().all(|owner| owner.strong_count() == 0));
    assert_eq!(pairs[3].strong_count(), 1);
    assert_eq!(documents[3].strong_count(), 1);
    assert!(callback.session.is_indexed());
}

#[test]
fn refused_index_budget_keeps_the_same_document_and_ordinary_callback_pcm() {
    let source = source(0.1);
    let fallback = Arc::new(PreparedSession::owned_with_budget(Arc::clone(&source), 0));
    assert!(!fallback.is_indexed());
    assert!(Arc::ptr_eq(fallback.owned_source().unwrap(), &source));
    let (tx, mut callback) = state(Arc::new(Session::default()), 48_000.0);
    tx.send(Cmd::Session(fallback, Vec::new())).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    callback.drain();
    let mut expected = vec![vec![0.0; BLOCK]; main_channels(&source)];
    new_engine(48_000.0).render(&source, 0, 64, &mut expected);
    callback.render_next(64);
    assert!(!callback.session.is_indexed());
    assert!(Arc::ptr_eq(callback.session.owned_source().unwrap(), &source));
    for (actual, expected) in callback.pending.iter().zip(&expected) {
        assert_eq!(&actual[..64], &expected[..64]);
    }
}

#[test]
fn rate_only_and_main_format_updates_preserve_the_existing_cursor_policy() {
    let source = source(0.1);
    let (tx, mut callback) = state(Arc::clone(&source), 48_000.0);
    callback.pos = 23;
    callback.pending_len = 17;
    callback.read = 4.5;
    callback.pending[0][4] = 0.125;
    let pending = callback.pending.clone();
    let mut changed = (*source).clone();
    changed.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
    tx.send(Cmd::Session(prepare_snapshot(Arc::new(changed.clone())), Vec::new())).unwrap();
    callback.drain();
    assert_eq!((callback.pos, callback.pending_len, callback.read), (23, 17, 4.5));
    assert_eq!(callback.pending, pending);
    changed.outputs[0].format = ChannelFormat::Surround51;
    tx.send(Cmd::Session(prepare_snapshot(Arc::new(changed)), Vec::new())).unwrap();
    callback.drain();
    assert_eq!((callback.pos, callback.pending_len, callback.read), (23, 0, 0.0));
    assert_eq!(callback.pending.len(), 6);
}
