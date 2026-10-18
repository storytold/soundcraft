use super::*;
use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};

fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    })
}

fn rig() -> (Sender<Cmd>, SyncSender<Box<Publication>>, Side, AudioState) {
    let (tx, rx) = channel();
    let (ret, returned) = sync_channel(PUBLICATIONS);
    let mut state = AudioState::new(rx, shared(), Arc::new(Session::default()), 48_000.0, ret);
    let publications = state.publication_sender();
    (tx, publications, Side::new(48_000.0, returned), state)
}

fn update(side: &mut Side, tx: &Sender<Cmd>, publications: &SyncSender<Box<Publication>>, session: Arc<Session>) -> bool {
    side.latest = Some(session);
    side.publish_latest(tx, publications).unwrap()
}

fn submit(tx: &Sender<Cmd>, publications: &SyncSender<Box<Publication>>, session: Arc<Session>, instances: Vec<PreparedInstance>) {
    publications.send(Publication::new(session, instances)).unwrap();
    tx.send(Cmd::InstallPublication).unwrap();
}

struct Watched {
    inner: Box<dyn Plugin>,
    drops: Arc<Mutex<Vec<std::thread::ThreadId>>>,
    memory: f32,
    tail: bool,
}
impl Drop for Watched {
    fn drop(&mut self) {
        self.drops.lock().unwrap().push(std::thread::current().id());
    }
}
impl Plugin for Watched {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, channels: usize) {
        self.inner.prepare(sr, block, channels);
    }
    fn reset(&mut self) {
        self.memory = 0.0;
        self.inner.reset();
    }
    fn set_param(&mut self, key: &str, value: f32) -> bool {
        self.inner.set_param(key, value)
    }
    fn param(&self, key: &str) -> Option<f32> {
        self.inner.param(key)
    }
    fn process(&mut self, output: &mut [Vec<f32>], frames: usize) {
        if !self.tail {
            self.inner.process(output, frames);
            return;
        }
        if let Some(channel) = output.first_mut() {
            for value in channel.iter_mut().take(frames) {
                self.memory = *value + self.memory * 0.99;
                *value = self.memory;
            }
        }
    }
}

fn watched_session(drops: Arc<Mutex<Vec<std::thread::ThreadId>>>, tail: bool) -> (Arc<Session>, Vec<PreparedInstance>) {
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Watched"));
    s.track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.owned"));
    let mut impulse = vec![0.0; 1024];
    impulse[0] = 0.25;
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![impulse] })));
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "impulse", SourceId(900), 0, 0, 1024));
    let plugin = Box::new(Watched { inner: soundcraft_dsp::create("gain").unwrap(), drops, memory: 0.0, tail });
    (Arc::new(s), vec![PreparedInstance { track, slot: 0, id: "clap:test.owned".into(), ch: 1, plugin }])
}

#[test]
fn outstanding_publication_coalesces_raw_latest_without_preparing_it() {
    let (tx, publications, mut side, mut state) = rig();
    assert!(update(&mut side, &tx, &publications, Arc::new(Session::default())));
    let sentinel = (TrackId(17), 0);
    side.shadow.insert(sentinel, ("synthetic-owned".into(), 1, true));
    let mut latest = None;
    for i in 0..100 {
        let s = Session { name: format!("latest-{i}"), sample_rate: soundcraft_time::SampleRate::new(44_100).unwrap(), ..Session::default() };
        let s = Arc::new(s);
        assert!(!update(&mut side, &tx, &publications, Arc::clone(&s)));
        latest = Some(s);
        assert_eq!(side.rate, 48_000.0);
        assert!(side.shadow.contains_key(&sentinel));
    }
    state.drain();
    assert!(side.outstanding);
    assert!(side.publish_latest(&tx, &publications).unwrap());
    assert_eq!(side.rate, 44_100.0);
    assert!(!side.shadow.contains_key(&sentinel));
    state.drain();
    assert!(Arc::ptr_eq(&state.session, latest.as_ref().unwrap()));
    side.collect_garbage();
    assert!(!side.outstanding && side.latest.is_none());
}

#[test]
fn full_return_retains_owned_publication_and_keeps_playing() {
    let (tx, publications, mut side, mut state) = rig();
    let first = Arc::new(Session::default());
    let second = Arc::new(Session::default());
    let weak = Arc::downgrade(&first);
    submit(&tx, &publications, first, Vec::new());
    state.drain();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    submit(&tx, &publications, second, Vec::new());
    state.drain();
    assert!(state.returning.is_some());
    assert_eq!(weak.strong_count(), 1);
    assert!(state.playing);
    let before = state.pos;
    state.fill(&mut [0.0; 64], 2);
    assert!(state.pos > before, "return backpressure must not silence or stop the transport");
    assert_eq!(weak.strong_count(), 1);
    side.collect_garbage();
    state.drain();
    assert!(state.returning.is_none());
    assert_eq!(weak.strong_count(), 1);
    side.collect_garbage();
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn disconnected_return_retains_plugins_until_control_reclaims_state() {
    let (tx, publications, side, mut state) = rig();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (session, instances) = watched_session(Arc::clone(&drops), false);
    submit(&tx, &publications, session, instances);
    state.drain();
    drop(side);
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    let state = std::thread::spawn(move || {
        state.drain();
        state
    })
    .join()
    .unwrap();
    assert!(state.returning.is_some());
    assert!(drops.lock().unwrap().is_empty());
    let owner = std::thread::current().id();
    drop(state);
    assert_eq!(*drops.lock().unwrap(), vec![owner]);
}

#[test]
fn retired_plugins_are_destroyed_on_collection_owner_not_render_thread() {
    let (tx, publications, mut side, mut state) = rig();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (session, instances) = watched_session(Arc::clone(&drops), false);
    submit(&tx, &publications, session, instances);
    state.drain();
    side.collect_garbage();
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    let state = std::thread::spawn(move || {
        state.drain();
        state
    })
    .join()
    .unwrap();
    assert!(drops.lock().unwrap().is_empty());
    side.collect_garbage();
    assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
    drop(state);
}

#[test]
fn rate_change_returns_whole_old_engine_and_format_change_returns_pending_storage() {
    let (tx, publications, mut side, mut state) = rig();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (session, instances) = watched_session(Arc::clone(&drops), false);
    submit(&tx, &publications, Arc::clone(&session), instances);
    state.drain();
    side.collect_garbage();
    let mut changed = (*session).clone();
    changed.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
    changed.outputs[0].format = ChannelFormat::Surround51;
    let pointer = state.pending[0].as_ptr();
    submit(&tx, &publications, Arc::new(changed), Vec::new());
    state.drain();
    let publication = side.garbage.try_recv().unwrap();
    assert!(publication.old_mix.is_some());
    assert_eq!(publication.old_pending.as_ref().unwrap()[0].as_ptr(), pointer);
    assert_eq!(state.pending.len(), 6);
    assert!(drops.lock().unwrap().is_empty());
    drop(publication);
    assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
}

#[test]
fn compatible_publication_preserves_pending_cursor_and_stateful_plugin_tail() {
    fn prepared() -> (Sender<Cmd>, SyncSender<Box<Publication>>, Side, AudioState) {
        let (tx, publications, mut side, mut state) = rig();
        let (s, instances) = watched_session(Arc::new(Mutex::new(Vec::new())), true);
        submit(&tx, &publications, s, instances);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        state.drain();
        side.collect_garbage();
        state.render_next(128);
        state.read = 3.5;
        (tx, publications, side, state)
    }
    let (_baseline_tx, _baseline_publications, _baseline_side, mut baseline) = prepared();
    let (tx, publications, mut side, mut edited) = prepared();
    let before_pending = edited.pending.clone();
    let mut s = (*edited.session).clone();
    s.tracks[0].name = "Changed label".into();
    submit(&tx, &publications, Arc::new(s), Vec::new());
    edited.drain();
    assert_eq!(edited.pending, before_pending);
    assert_eq!(edited.pending_len, 128);
    assert_eq!(edited.read, 3.5);
    side.collect_garbage();
    for _ in 0..8 {
        let mut a = [0.0; 128];
        let mut b = [0.0; 128];
        baseline.fill(&mut a, 2);
        edited.fill(&mut b, 2);
        assert_eq!(a, b, "a compatible edit must retain the running processor and cursor");
        assert!(b.iter().any(|v| v.abs() > 0.0));
    }
}

#[test]
fn disconnected_command_owner_refuses_without_retaining_an_unaccepted_snapshot() {
    let (tx, publications, mut side, state) = rig();
    drop(state);
    let session = Arc::new(Session::default());
    let weak = Arc::downgrade(&session);
    side.latest = Some(session);
    assert!(side.publish_latest(&tx, &publications).is_err());
    assert_eq!(weak.strong_count(), 0);
    assert!(side.closed && !side.outstanding);
}

#[test]
fn control_shutdown_reclaims_queued_and_parked_publications_before_late_callback_release() {
    let (tx, publications, side, mut state) = rig();
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (session, instances) = watched_session(Arc::clone(&drops), false);
    submit(&tx, &publications, session, instances);
    state.drain();
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    state.drain();
    assert!(state.returning.is_some());
    let (queued, instances) = watched_session(Arc::clone(&drops), false);
    submit(&tx, &publications, queued, instances);
    let slot = Arc::new(Mutex::new(Some(OutputState { state, scratch: vec![0.0; 64] })));
    let late_callback_slot = Arc::clone(&slot);
    let late_shared = Arc::clone(&slot.lock().unwrap().as_ref().unwrap().state.shared);
    late_shared.meters.lock().unwrap().tracks.insert(TrackId(77), StripMeter::default());
    let player = Player {
        shared: Arc::clone(&late_shared),
        fault: Arc::default(),
        #[cfg(not(target_arch = "wasm32"))]
        watch: Mutex::new(None),
        tx,
        publication_tx: publications,
        side: Mutex::new(side),
        state_slot: slot,
        stop_clock: Arc::new(AtomicBool::new(false)),
        #[cfg(not(target_arch = "wasm32"))]
        clock: None,
        _stream: None,
        device_name: "Synthetic owner".into(),
        device_rate: 48_000,
        device_channels: 2,
        device_buffer: None,
        silent: false,
    };
    drop(player);
    assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id(); 2]);
    assert!(late_callback_slot.lock().unwrap().is_none());
    assert_eq!(late_shared.meters.lock().unwrap().tracks.capacity(), 0);
    std::thread::spawn(move || drop(late_callback_slot)).join().unwrap();
    assert_eq!(drops.lock().unwrap().len(), 2);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn silent_clock_shutdown_joins_before_control_destroys_live_plugin() {
    let (tx, rx) = channel();
    let (ret, returned) = sync_channel(PUBLICATIONS);
    let player = Player::silent_clock_with(shared(), tx, rx, Arc::new(Session::default()), Mutex::new(Side::new(48_000.0, returned)), ret);
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (session, instances) = watched_session(Arc::clone(&drops), false);
    submit(&player.tx, &player.publication_tx, session, instances);
    {
        let mut slot = player.state_slot.lock().unwrap();
        slot.as_mut().unwrap().state.drain();
    }
    assert!(drops.lock().unwrap().is_empty());
    drop(player);
    assert_eq!(*drops.lock().unwrap(), vec![std::thread::current().id()]);
}

#[test]
fn full_return_defers_publication_but_services_stop_locate_recording_and_input() {
    let (tx, publications, mut side, mut state) = rig();
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    state.drain(); // Fill the return channel without control collection.
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    state.drain(); // Park the second return.
    assert!(state.returning.is_some());
    let latest = Session { name: "Deferred latest".into(), ..Session::default() };
    submit(&tx, &publications, Arc::new(latest), Vec::new());
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    tx.send(Cmd::Stop).unwrap();
    tx.send(Cmd::Locate(777)).unwrap();
    tx.send(Cmd::Recording(true)).unwrap();
    let ring = record::InputRing::new(1, 48_000);
    tx.send(Cmd::Input(Arc::clone(&ring))).unwrap();
    state.drain();
    assert!(state.returning.is_some() && state.install_pending);
    assert_ne!(state.session.name, "Deferred latest");
    assert!(!state.playing);
    assert_eq!(state.pos, 777);
    assert!(state.recording);
    assert!(Arc::ptr_eq(state.input.as_ref().unwrap(), &ring));
    side.collect_garbage();
    state.drain();
    assert_eq!(state.session.name, "Deferred latest");
    assert!(!state.install_pending);
    assert!(!state.playing);
}

#[test]
fn disconnected_return_still_services_stop_and_locate() {
    let (tx, publications, side, mut state) = rig();
    drop(side);
    submit(&tx, &publications, Arc::new(Session::default()), Vec::new());
    state.drain();
    assert!(state.returning.is_some());
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    tx.send(Cmd::Stop).unwrap();
    tx.send(Cmd::Locate(555)).unwrap();
    state.drain();
    assert!(!state.playing);
    assert_eq!(state.pos, 555);
    assert!(state.returning.is_some());
}

#[test]
fn same_width_rate_change_retires_unread_old_rate_pcm_and_resets_cursor() {
    let (tx, publications, side, mut state) = rig();
    for channel in &mut state.pending {
        channel.fill(0.75);
    }
    state.pending_len = 4;
    state.read = 1.5;
    let pointer = state.pending[0].as_ptr();
    let changed = Session { sample_rate: soundcraft_time::SampleRate::new(44_100).unwrap(), ..Session::default() };
    submit(&tx, &publications, Arc::new(changed), Vec::new());
    state.drain();
    assert_eq!(state.pending_len, 0);
    assert_eq!(state.read, 0.0);
    let retired = side.garbage.try_recv().unwrap();
    assert_eq!(retired.old_pending.as_ref().unwrap()[0].as_ptr(), pointer);
    assert_eq!(retired.old_pending.as_ref().unwrap()[0][0], 0.75);
    let mut output = [1.0; 64];
    state.fill(&mut output, 2);
    assert!(output.iter().all(|sample| *sample == 0.0));
}

#[test]
fn same_width_main_layout_change_retires_unread_pcm() {
    let (tx, publications, mut side, mut state) = rig();
    let mut original = Session::default();
    original.outputs[0].format = ChannelFormat::Quad;
    submit(&tx, &publications, Arc::new(original), Vec::new());
    state.drain();
    side.collect_garbage();
    for channel in &mut state.pending {
        channel.fill(0.5);
    }
    state.pending_len = 3;
    state.read = 0.5;
    let pointer = state.pending[0].as_ptr();
    let mut changed = (*state.session).clone();
    changed.outputs[0].format = ChannelFormat::Lcrs;
    submit(&tx, &publications, Arc::new(changed), Vec::new());
    state.drain();
    assert_eq!(state.pending.len(), 4);
    assert_eq!(state.pending_len, 0);
    assert_eq!(state.read, 0.0);
    let retired = side.garbage.try_recv().unwrap();
    assert_eq!(retired.old_pending.as_ref().unwrap()[0].as_ptr(), pointer);
    assert_eq!(retired.old_pending.as_ref().unwrap()[0][0], 0.5);
}

struct EmittingEditor(Arc<AtomicU32>);
impl PluginEditor for EmittingEditor {
    fn open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn close(&mut self) {}
    fn is_open(&self) -> bool {
        true
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        self.0.fetch_add(1, Ordering::Relaxed);
        vec![("gain".into(), 0.5)]
    }
}

struct IdentityRig {
    tx: Sender<Cmd>,
    publications: SyncSender<Box<Publication>>,
    side: Side,
    _state: AudioState,
    session: Session,
    track: TrackId,
    slot: usize,
    state_reads: Arc<AtomicU32>,
    editor_ticks: Arc<AtomicU32>,
}

fn identity_rig(instrument: bool) -> IdentityRig {
    let (tx, publications, mut side, state) = rig();
    let mut session = Session::default();
    let kind = if instrument { TrackKind::Instrument } else { TrackKind::Audio };
    let track = session.add_track(kind, ChannelFormat::Mono, Some("Accepted"));
    let slot = if instrument { soundcraft_mix::INSTRUMENT_SLOT } else { 0 };
    let insert = Insert::new("clap:test.identity.a");
    if instrument {
        session.track_mut(track).unwrap().instrument = Some(insert);
    } else {
        session.track_mut(track).unwrap().mixer.inserts[0] = Some(insert);
    }
    let state_reads = Arc::new(AtomicU32::new(0));
    let editor_ticks = Arc::new(AtomicU32::new(0));
    // Publish a real first envelope, but leave its callback/collection credit outstanding.
    assert!(update(&mut side, &tx, &publications, Arc::new(Session::default())));
    // Model accepted hosted handles directly, without loading an external plugin catalog.
    side.shadow.insert((track, slot), ("clap:test.identity.a".into(), if instrument { 2 } else { 1 }, true));
    let reads = Arc::clone(&state_reads);
    side.states.insert(
        (track, slot),
        Box::new(move || {
            reads.fetch_add(1, Ordering::Relaxed);
            Some(b"accepted-state".to_vec())
        }),
    );
    side.editors.insert((track, slot), Box::new(EmittingEditor(Arc::clone(&editor_ticks))));
    IdentityRig { tx, publications, side, _state: state, session, track, slot, state_reads, editor_ticks }
}

fn defer_identity(rig: &mut IdentityRig, session: Session) {
    assert!(!update(&mut rig.side, &rig.tx, &rig.publications, Arc::new(session)));
    assert!(rig.side.latest.is_some());
}

fn assert_suppressed_with_maintenance(rig: &mut IdentityRig) {
    assert!(rig.side.capture_states().is_empty());
    assert!(rig.side.editor_edits().is_empty());
    assert_eq!(rig.state_reads.load(Ordering::Relaxed), 0, "do not read a stale identity's state");
    assert_eq!(rig.editor_ticks.load(Ordering::Relaxed), 1, "GUI maintenance continues");
}

#[test]
fn deferred_plugin_replacement_suppresses_old_state_and_editor_gestures() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.track_mut(rig.track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.identity.b"));
    defer_identity(&mut rig, next);
    assert_suppressed_with_maintenance(&mut rig);
}

#[test]
fn deferred_plugin_removal_suppresses_old_state_and_editor_gestures() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.track_mut(rig.track).unwrap().mixer.inserts[0] = None;
    defer_identity(&mut rig, next);
    assert_suppressed_with_maintenance(&mut rig);
}

#[test]
fn deferred_builtin_replacement_cannot_receive_a_hosted_slots_state() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.track_mut(rig.track).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    defer_identity(&mut rig, next);
    assert_suppressed_with_maintenance(&mut rig);
}

#[test]
fn deferred_sample_rate_change_suppresses_old_state_and_editor_gestures() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
    defer_identity(&mut rig, next);
    assert_suppressed_with_maintenance(&mut rig);
}

#[test]
fn deferred_insert_channel_change_suppresses_old_state_and_editor_gestures() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.track_mut(rig.track).unwrap().format = ChannelFormat::Stereo;
    defer_identity(&mut rig, next);
    assert_suppressed_with_maintenance(&mut rig);
}

#[test]
fn matching_deferred_identity_keeps_state_and_editor_gestures() {
    let mut rig = identity_rig(false);
    let mut next = rig.session.clone();
    next.track_mut(rig.track).unwrap().name = "New label".into();
    defer_identity(&mut rig, next);
    assert_eq!(rig.side.capture_states(), vec![(rig.track, rig.slot, b"accepted-state".to_vec())]);
    assert_eq!(rig.side.editor_edits(), vec![(rig.track, rig.slot, "gain".into(), 0.5)]);
    assert_eq!(rig.state_reads.load(Ordering::Relaxed), 1);
    assert_eq!(rig.editor_ticks.load(Ordering::Relaxed), 1);
}

#[test]
fn deferred_instrument_identity_uses_prepared_stereo_width_and_instrument_slot() {
    let mut rig = identity_rig(true);
    let mut matching = rig.session.clone();
    matching.track_mut(rig.track).unwrap().format = ChannelFormat::Stereo;
    defer_identity(&mut rig, matching);
    assert_eq!(rig.side.capture_states(), vec![(rig.track, soundcraft_mix::INSTRUMENT_SLOT, b"accepted-state".to_vec())]);
    assert_eq!(rig.side.editor_edits().len(), 1);
    let mut changed = rig.session.clone();
    changed.track_mut(rig.track).unwrap().format = ChannelFormat::Surround51;
    defer_identity(&mut rig, changed);
    assert!(rig.side.capture_states().is_empty());
    assert!(rig.side.editor_edits().is_empty());
    assert_eq!(rig.state_reads.load(Ordering::Relaxed), 1);
    assert_eq!(rig.editor_ticks.load(Ordering::Relaxed), 2);
}

#[test]
fn public_capture_pumps_latest_after_previous_publication_credit_returns() {
    let (tx, publications, mut side, mut state) = rig();
    assert!(update(&mut side, &tx, &publications, Arc::new(Session::default())));
    side.shadow.insert((TrackId(77), 0), ("clap:test.old".into(), 1, true));
    side.states.insert((TrackId(77), 0), Box::new(|| Some(b"old".to_vec())));
    side.latest = Some(Arc::new(Session::default()));
    state.drain();
    let shared = Arc::clone(&state.shared);
    let player = Player {
        shared,
        fault: Arc::default(),
        #[cfg(not(target_arch = "wasm32"))]
        watch: Mutex::new(None),
        tx,
        publication_tx: publications,
        side: Mutex::new(side),
        state_slot: Arc::new(Mutex::new(Some(OutputState { state, scratch: vec![0.0; 64] }))),
        stop_clock: Arc::new(AtomicBool::new(false)),
        #[cfg(not(target_arch = "wasm32"))]
        clock: None,
        _stream: None,
        device_name: "Synthetic capture owner".into(),
        device_rate: 48_000,
        device_channels: 2,
        device_buffer: None,
        silent: false,
    };
    assert!(player.capture_states().is_empty());
    let side = player.side();
    assert!(side.latest.is_none() && side.outstanding);
    assert!(side.shadow.is_empty());
}

#[derive(Default)]
struct EditorActions {
    opens: AtomicU32,
    closes: AtomicU32,
    maintenance: AtomicU32,
    open: AtomicBool,
    parameters: Mutex<Vec<(String, f32)>>,
}
struct ActionEditor(Arc<EditorActions>);
impl PluginEditor for ActionEditor {
    fn open(&mut self) -> Result<(), String> {
        self.0.opens.fetch_add(1, Ordering::Relaxed);
        self.0.open.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn close(&mut self) {
        self.0.closes.fetch_add(1, Ordering::Relaxed);
        self.0.open.store(false, Ordering::Relaxed);
    }
    fn is_open(&self) -> bool {
        self.0.open.load(Ordering::Relaxed)
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        self.0.maintenance.fetch_add(1, Ordering::Relaxed);
        vec![("gain".into(), 0.25)]
    }
    fn set_param(&mut self, id: &str, value: f32) {
        self.0.parameters.lock().unwrap().push((id.into(), value));
    }
}

fn identity_player(rig: IdentityRig) -> Player {
    let state = rig._state;
    let shared = Arc::clone(&state.shared);
    Player {
        shared,
        fault: Arc::default(),
        #[cfg(not(target_arch = "wasm32"))]
        watch: Mutex::new(None),
        tx: rig.tx,
        publication_tx: rig.publications,
        side: Mutex::new(rig.side),
        state_slot: Arc::new(Mutex::new(Some(OutputState { state, scratch: vec![0.0; 64] }))),
        stop_clock: Arc::new(AtomicBool::new(false)),
        #[cfg(not(target_arch = "wasm32"))]
        clock: None,
        _stream: None,
        device_name: "Synthetic identity owner".into(),
        device_rate: 48_000,
        device_channels: 2,
        device_buffer: None,
        silent: false,
    }
}

#[test]
fn public_stale_editor_cannot_open_take_knob_values_or_clear_refused_state() {
    let mut rig = identity_rig(false);
    let key = (rig.track, rig.slot);
    let mut changed = rig.session.clone();
    changed.track_mut(rig.track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.identity.b"));
    defer_identity(&mut rig, changed);
    let actions = Arc::new(EditorActions::default());
    actions.open.store(true, Ordering::Relaxed);
    rig.side.editors.insert(key, Box::new(ActionEditor(Arc::clone(&actions))));
    rig.side.refused.insert(key);
    let player = identity_player(rig);
    assert!(!player.has_editor(key.0, key.1));
    assert!(!player.editor_open(key.0, key.1));
    assert!(player.any_editor_open(), "actual old windows remain visible to shutdown/maintenance");
    assert!(player.open_editor(key.0, key.1).is_err());
    player.editor_set_param(key.0, key.1, "gain", 0.75);
    assert_eq!(actions.opens.load(Ordering::Relaxed), 0);
    assert!(actions.parameters.lock().unwrap().is_empty());
    assert!(player.side().refused.contains(&key));
    assert!(player.idle().is_empty());
    assert_eq!(actions.maintenance.load(Ordering::Relaxed), 1);
    player.close_editor(key.0, key.1);
    assert_eq!(actions.closes.load(Ordering::Relaxed), 1);
    assert!(!player.any_editor_open());
    assert!(player.side().refused.contains(&key));
}

#[test]
fn public_matching_editor_still_opens_receives_knob_values_and_clears_refusal() {
    let mut rig = identity_rig(false);
    let key = (rig.track, rig.slot);
    let mut changed = rig.session.clone();
    changed.track_mut(rig.track).unwrap().name = "Renamed".into();
    defer_identity(&mut rig, changed);
    let actions = Arc::new(EditorActions::default());
    rig.side.editors.insert(key, Box::new(ActionEditor(Arc::clone(&actions))));
    rig.side.refused.insert(key);
    let player = identity_player(rig);
    assert!(player.has_editor(key.0, key.1));
    assert!(!player.editor_open(key.0, key.1));
    player.open_editor(key.0, key.1).unwrap();
    assert_eq!(actions.opens.load(Ordering::Relaxed), 1);
    assert!(!player.side().refused.contains(&key));
    assert!(player.editor_open(key.0, key.1));
    assert!(player.any_editor_open());
    player.editor_set_param(key.0, key.1, "gain", 0.75);
    assert_eq!(*actions.parameters.lock().unwrap(), vec![("gain".into(), 0.75)]);
    assert_eq!(player.idle(), vec![(key.0, key.1, "gain".into(), 0.25)]);
    assert_eq!(actions.maintenance.load(Ordering::Relaxed), 1);
    player.close_editors();
    assert!(!player.editor_open(key.0, key.1));
    assert_eq!(actions.closes.load(Ordering::Relaxed), 1);
}

#[test]
fn borrowed_host_identity_preserves_width_policy_without_decoding_state() {
    let mut session = Session::default();
    session.outputs[0].format = ChannelFormat::Surround51;
    let audio = session.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let instrument = session.add_track(TrackKind::Instrument, ChannelFormat::Mono, None);
    let master = session.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    for track in [audio, master] {
        let track = session.track_mut(track).unwrap();
        track.inactive = true;
        track.mixer.inserts[0] = Some(Insert::new("clap:test.borrowed"));
    }
    session.track_mut(audio).unwrap().mixer.inserts[1] = Some(Insert::new("gain"));
    session.track_mut(instrument).unwrap().instrument = Some(Insert::new("clap:test.instrument"));
    assert_eq!(soundcraft_mix::instance_identity(&session, audio, 0), Some(("clap:test.borrowed", 1)));
    assert_eq!(soundcraft_mix::instance_identity(&session, instrument, soundcraft_mix::INSTRUMENT_SLOT), Some(("clap:test.instrument", 2)));
    assert_eq!(soundcraft_mix::instance_identity(&session, master, 0), Some(("clap:test.borrowed", 6)));
    assert_eq!(soundcraft_mix::instance_identity(&session, audio, 1), None);
    assert_eq!(soundcraft_mix::instance_identity(&session, audio, 999), None);
    assert_eq!(soundcraft_mix::instance_identity(&session, TrackId(u64::MAX), 0), None);
    let stored = &session.track(audio).unwrap().mixer.inserts[0].as_ref().unwrap().plugin;
    let (borrowed, _) = soundcraft_mix::instance_identity(&session, audio, 0).unwrap();
    assert_eq!(borrowed.as_ptr(), stored.as_ptr(), "the ID is borrowed directly from the document");
}
