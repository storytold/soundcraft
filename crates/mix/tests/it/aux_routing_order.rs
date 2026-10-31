//! Aux and routing-folder dependencies preserve PCM beyond eight hops. Cyclic graphs keep
//! the established whole-graph fallback, including its session-order-dependent audio.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range};
use soundcraft_model::{BusId, ChannelFormat, Clip, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const FRAMES: usize = 4097;
const DEPTH: usize = 12;

fn signal() -> Vec<Vec<f32>> {
    // Distinct, exactly representable channels expose lost, duplicated, swapped and delayed PCM.
    vec![
        (0..FRAMES).map(|frame| ((frame % 31) as i32 - 15) as f32 / 256.0).collect(),
        (0..FRAMES).map(|frame| ((frame % 17) as i32 - 8) as f32 / 512.0).collect(),
    ]
}

fn scaled_signal(gain: f32) -> Vec<Vec<f32>> {
    signal().into_iter().map(|channel| channel.into_iter().map(|value| value * gain).collect()).collect()
}

fn session(pdc: bool) -> Session {
    let mut session = Session::default();
    session.edit.click = false;
    session.edit.delay_compensation = pdc;
    session
}

fn add_source(session: &mut Session, output: Route) -> TrackId {
    let source = SourceId(session.alloc());
    session.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: signal() })));
    let clip = session.new_clip_id();
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("PCM source"));
    let track_state = session.track_mut(track).unwrap();
    track_state.mixer.output = output;
    track_state.playlist_mut().unwrap().clips.push(Clip::audio(clip, "PCM", source, 0, 0, FRAMES as i64));
    track
}

fn add_aux(session: &mut Session, kind: TrackKind, input: BusId, output: Route) -> TrackId {
    let track = session.add_track(kind, ChannelFormat::Stereo, None);
    let mixer = &mut session.track_mut(track).unwrap().mixer;
    mixer.input = Route::Bus(input);
    mixer.output = output;
    track
}

fn send(bus: BusId) -> SendSlot {
    let mut send = SendSlot::new(Route::Bus(bus));
    send.level_db = 0.0;
    send
}

struct Chain {
    session: Session,
    buses: Vec<BusId>,
    /// Logical routing order, deliberately different from the session's creation order.
    tracks: Vec<TrackId>,
}

fn reverse_chain(pdc: bool, send_links: bool, folders: bool) -> Chain {
    let mut session = session(pdc);
    let buses: Vec<_> = (0..DEPTH).map(|stage| session.add_bus(&format!("Stage {stage}"), ChannelFormat::Stereo)).collect();
    let mut tracks = Vec::new();
    for stage in (0..DEPTH).rev() {
        let kind = if folders && stage % 3 == 1 { TrackKind::Folder } else { TrackKind::Aux };
        let output = if stage + 1 == DEPTH {
            Route::Main
        } else if send_links {
            Route::None
        } else {
            Route::Bus(buses[stage + 1])
        };
        let track = add_aux(&mut session, kind, buses[stage], output);
        if send_links && stage + 1 < DEPTH {
            session.track_mut(track).unwrap().mixer.sends[0] = Some(send(buses[stage + 1]));
        }
        tracks.push(track);
    }
    tracks.reverse();
    add_source(&mut session, Route::Bus(buses[0]));
    Chain { session, buses, tracks }
}

fn assert_pcm(actual: &[Vec<f32>], expected: &[Vec<f32>], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: channel count");
    for (channel, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.len(), expected.len(), "{context}: channel {channel} length");
        for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(actual, expected, "{context}: channel {channel}, frame {frame}");
        }
    }
}

fn render(session: &Session, block: usize) -> Vec<Vec<f32>> {
    render_range(session, Range::new(0, FRAMES as i64), block)
}

fn engine_block(engine: &mut MixEngine, session: &Session) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; 128]; 2];
    engine.render(session, 0, 128, &mut out);
    out
}

fn first_block(pcm: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    pcm.into_iter().map(|channel| channel.into_iter().take(128).collect()).collect()
}

#[test]
fn reverse_created_twelve_aux_chain_preserves_every_sample() {
    for pdc in [false, true] {
        let chain = reverse_chain(pdc, false, false);
        assert_pcm(&render(&chain.session, 1024), &signal(), "deep output chain");
    }
}

#[test]
fn deep_dependencies_through_sends_without_outputs_preserve_every_sample() {
    for pdc in [false, true] {
        let chain = reverse_chain(pdc, true, false);
        assert_pcm(&render(&chain.session, 97), &signal(), "deep send-only chain");
    }
}

#[test]
fn output_and_send_to_the_same_bus_sum_audio_without_duplicate_dependency() {
    for pdc in [false, true] {
        let mut chain = reverse_chain(pdc, false, false);
        chain.session.track_mut(chain.tracks[4]).unwrap().mixer.sends[0] = Some(send(chain.buses[5]));
        assert_pcm(&render(&chain.session, 256), &scaled_signal(2.0), "duplicate destination");
    }
}

#[test]
fn short_and_deep_branches_both_reach_the_join() {
    for pdc in [false, true] {
        let mut chain = reverse_chain(pdc, false, false);
        add_aux(&mut chain.session, TrackKind::Aux, chain.buses[0], Route::Bus(chain.buses[DEPTH - 1]));
        assert_pcm(&render(&chain.session, 512), &scaled_signal(2.0), "branch join");
    }
}

#[test]
fn routing_folders_participate_in_deep_dependencies() {
    for pdc in [false, true] {
        let chain = reverse_chain(pdc, true, true);
        assert_pcm(&render(&chain.session, 63), &signal(), "aux and folder send chain");
    }
}

#[test]
fn output_edit_reorders_the_same_engine_before_the_next_block() {
    let mut session = session(true);
    let root = session.add_bus("Root", ChannelFormat::Stereo);
    let join = session.add_bus("Join", ChannelFormat::Stereo);
    add_aux(&mut session, TrackKind::Aux, join, Route::Main);
    let producer = add_aux(&mut session, TrackKind::Aux, root, Route::Main);
    add_source(&mut session, Route::Bus(root));
    let mut engine = MixEngine::new(48_000.0, 128);
    let expected = first_block(signal());
    assert_pcm(&engine_block(&mut engine, &session), &expected, "before output edit");
    session.track_mut(producer).unwrap().mixer.output = Route::Bus(join);
    assert_pcm(&engine_block(&mut engine, &session), &expected, "after output edit");
}

#[test]
fn input_edit_reorders_the_same_engine_before_the_next_block() {
    let mut session = session(true);
    let root = session.add_bus("Root", ChannelFormat::Stereo);
    let join = session.add_bus("Join", ChannelFormat::Stereo);
    let consumer = add_aux(&mut session, TrackKind::Folder, root, Route::Main);
    add_aux(&mut session, TrackKind::Aux, root, Route::Bus(join));
    add_source(&mut session, Route::Bus(root));
    let mut engine = MixEngine::new(48_000.0, 128);
    let expected = first_block(signal());
    assert_pcm(&engine_block(&mut engine, &session), &expected, "before input edit");
    session.track_mut(consumer).unwrap().mixer.input = Route::Bus(join);
    assert_pcm(&engine_block(&mut engine, &session), &expected, "after input edit");
}

#[test]
fn send_add_retarget_remove_and_readd_update_the_same_engine() {
    let mut session = session(true);
    let root = session.add_bus("Root", ChannelFormat::Stereo);
    let join = session.add_bus("Join", ChannelFormat::Stereo);
    let unused = session.add_bus("Unused", ChannelFormat::Stereo);
    add_aux(&mut session, TrackKind::Aux, join, Route::Main);
    let producer = add_aux(&mut session, TrackKind::Folder, root, Route::None);
    add_source(&mut session, Route::Bus(root));
    let mut engine = MixEngine::new(48_000.0, 128);
    let expected = first_block(signal());
    let silence = vec![vec![0.0; 128]; 2];
    assert_pcm(&engine_block(&mut engine, &session), &silence, "before send");
    session.track_mut(producer).unwrap().mixer.sends[0] = Some(send(join));
    assert_pcm(&engine_block(&mut engine, &session), &expected, "send added");
    session.track_mut(producer).unwrap().mixer.sends[0].as_mut().unwrap().target = Route::Bus(unused);
    assert_pcm(&engine_block(&mut engine, &session), &silence, "send retargeted");
    session.track_mut(producer).unwrap().mixer.sends[0].as_mut().unwrap().target = Route::Bus(join);
    assert_pcm(&engine_block(&mut engine, &session), &expected, "send retargeted back");
    session.track_mut(producer).unwrap().mixer.sends[0] = None;
    assert_pcm(&engine_block(&mut engine, &session), &silence, "send removed");
    session.track_mut(producer).unwrap().mixer.sends[0] = Some(send(join));
    assert_pcm(&engine_block(&mut engine, &session), &expected, "send readded");
}

#[test]
fn track_reorder_removal_and_new_identity_rebuild_the_same_engine() {
    let mut chain = reverse_chain(true, false, false);
    let mut engine = MixEngine::new(48_000.0, 128);
    let expected = first_block(signal());
    assert_pcm(&engine_block(&mut engine, &chain.session), &expected, "initial track order");
    chain.session.tracks.reverse();
    assert_pcm(&engine_block(&mut engine, &chain.session), &expected, "tracks reordered");
    chain.session.tracks.retain(|track| track.id != chain.tracks[6]);
    let silence = vec![vec![0.0; 128]; 2];
    assert_pcm(&engine_block(&mut engine, &chain.session), &silence, "track removed");
    let replacement = add_aux(&mut chain.session, TrackKind::Aux, chain.buses[6], Route::Bus(chain.buses[7]));
    assert_ne!(replacement, chain.tracks[6]);
    assert_pcm(&engine_block(&mut engine, &chain.session), &expected, "track readded with new identity");
}

#[test]
fn strip_and_bus_format_edits_refresh_the_same_engine_workspace() {
    let mut chain = reverse_chain(true, false, false);
    let mut engine = MixEngine::new(48_000.0, 128);
    let stereo = first_block(signal());
    assert_pcm(&engine_block(&mut engine, &chain.session), &stereo, "initial stereo formats");
    chain.session.track_mut(chain.tracks[5]).unwrap().format = ChannelFormat::Mono;
    // A stereo bus folds to mono at -6 dB; the retained hard-left pan sends that mono to L.
    let mono: Vec<_> = stereo[0].iter().zip(&stereo[1]).map(|(left, right)| 0.5 * (left + right)).collect();
    assert_pcm(&engine_block(&mut engine, &chain.session), &[mono.clone(), vec![0.0; 128]], "mono strip");
    chain.session.track_mut(chain.tracks[5]).unwrap().format = ChannelFormat::Stereo;
    assert_pcm(&engine_block(&mut engine, &chain.session), &stereo, "stereo strip restored");
    chain.session.busses.iter_mut().find(|bus| bus.id == chain.buses[5]).unwrap().format = ChannelFormat::Mono;
    // A mono bus feeds both channels of the following stereo aux at unity.
    assert_pcm(&engine_block(&mut engine, &chain.session), &[mono.clone(), mono], "mono bus");
}

#[test]
fn deep_routing_is_invariant_to_offline_and_callback_block_sizes() {
    let chain = reverse_chain(true, true, true);
    let expected = signal();
    for block in [16, 63, 97, 256, 512, 1024] {
        assert_pcm(&render(&chain.session, block), &expected, "offline block size");
    }
    let mut engine = MixEngine::new(48_000.0, 1024);
    let callbacks = [1, 63, 257, 17, 511, 1024];
    let mut out = vec![Vec::new(), Vec::new()];
    let mut position = 0;
    let mut callback = 0;
    while position < FRAMES {
        let frames = callbacks[callback % callbacks.len()].min(FRAMES - position);
        let mut block = vec![vec![0.0; frames]; 2];
        engine.render(&chain.session, position as i64, frames, &mut block);
        for (destination, channel) in out.iter_mut().zip(block) {
            destination.extend(channel);
        }
        position += frames;
        callback += 1;
    }
    assert_pcm(&out, &expected, "varying callback sizes");
}

#[test]
fn self_loop_preserves_audio_and_selects_fallback_for_the_whole_graph() {
    for pdc in [false, true] {
        let mut chain = reverse_chain(pdc, false, false);
        let loop_bus = chain.session.add_bus("Self loop", ChannelFormat::Stereo);
        let aux = add_aux(&mut chain.session, TrackKind::Aux, loop_bus, Route::Main);
        chain.session.track_mut(aux).unwrap().mixer.sends[0] = Some(send(loop_bus));
        add_source(&mut chain.session, Route::Bus(loop_bus));
        // The self-loop reads its source once, then writes back to the already-read bus.
        // The disconnected deep chain remains silent under the established eight-hop fallback.
        assert_pcm(&render(&chain.session, 97), &signal(), "self-loop whole-graph fallback");
    }
}

#[test]
fn two_aux_cycle_preserves_session_order_audio_and_whole_graph_fallback() {
    for pdc in [false, true] {
        for reverse in [false, true] {
            let mut chain = reverse_chain(pdc, false, false);
            let a_bus = chain.session.add_bus("Cycle A", ChannelFormat::Stereo);
            let b_bus = chain.session.add_bus("Cycle B", ChannelFormat::Stereo);
            let (a, b) = if reverse {
                let b = add_aux(&mut chain.session, TrackKind::Aux, b_bus, Route::Main);
                let a = add_aux(&mut chain.session, TrackKind::Folder, a_bus, Route::Main);
                (a, b)
            } else {
                let a = add_aux(&mut chain.session, TrackKind::Folder, a_bus, Route::Main);
                let b = add_aux(&mut chain.session, TrackKind::Aux, b_bus, Route::Main);
                (a, b)
            };
            chain.session.track_mut(a).unwrap().mixer.sends[0] = Some(send(b_bus));
            chain.session.track_mut(b).unwrap().mixer.sends[0] = Some(send(a_bus));
            add_source(&mut chain.session, Route::Bus(a_bus));
            // A then B: both hear the source. B then A: B reads its still-empty bus.
            // Buses clear each block, so neither order feeds a previous block back into this one.
            let expected = scaled_signal(if reverse { 1.0 } else { 2.0 });
            assert_pcm(&render(&chain.session, 256), &expected, "two-aux whole-graph fallback");
        }
    }
}
