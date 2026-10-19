//! Send automation follows the delayed audio at its tap, without changing existing PDC routing.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::pan::PanLaw;
use soundcraft_mix::{MixEngine, render_bus};
use soundcraft_model::{
    AutoParam, AutomationLane, BusId, ChannelFormat, Clip, Insert, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::Arc;

// The built-in Maximizer uses 1.5 ms of lookahead: 72 samples at 48 kHz, with a declared
// and physical delay of 72 - 1 = 71. Its untouched threshold 0 dB gives unity drive;
// every synthetic level stays below the default -0.3 dB ceiling, so there is no limiting.
// Expected timelines use these declared values, not mixer latency fields or the clock helper.
const LOOKAHEAD: usize = 71;
const SOURCE_FRAMES: usize = 2048;
const MUTE_AT: i64 = 300;
const AMPLITUDE: f32 = 0.25;

struct Graph {
    session: Session,
    capture: BusId,
    receiver: TrackId,
    clip_start: i64,
    pan_start: i64,
    pan_end: i64,
}

fn maximizers(session: &mut Session, track: TrackId, count: usize) {
    for slot in session.track_mut(track).unwrap().mixer.inserts.iter_mut().take(count) {
        *slot = Some(Insert::new("maximizer"));
    }
}

fn source(session: &mut Session, insert_count: usize, clip_start: i64) -> TrackId {
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Declared DC"));
    let id = SourceId(901);
    session.pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![AMPLITUDE; SOURCE_FRAMES]] })));
    let clip = session.new_clip_id();
    let t = session.track_mut(track).unwrap();
    t.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Declared DC", id, 0, clip_start, SOURCE_FRAMES as i64));
    t.mixer.pan = vec![0.0];
    t.mixer.output = Route::None;
    maximizers(session, track, insert_count);
    track
}

fn silent_audio(session: &mut Session, insert_count: usize) {
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Unrelated silent lookahead"));
    session.track_mut(track).unwrap().mixer.output = Route::None;
    maximizers(session, track, insert_count);
}

fn aux(session: &mut Session, input: BusId, format: ChannelFormat, insert_count: usize) -> TrackId {
    let track = session.add_track(TrackKind::Aux, format, None);
    let t = session.track_mut(track).unwrap();
    t.mixer.input = Route::Bus(input);
    t.mixer.output = Route::None;
    t.mixer.pan = if format == ChannelFormat::Mono { vec![0.0] } else { vec![-1.0, 1.0] };
    maximizers(session, track, insert_count);
    track
}

fn silent_aux(session: &mut Session, insert_count: usize) {
    let input = session.add_bus("Unrelated empty input", ChannelFormat::Mono);
    aux(session, input, ChannelFormat::Mono, insert_count);
}

fn return_to_main(session: &mut Session, input: BusId) -> TrackId {
    let track = aux(session, input, ChannelFormat::Stereo, 0);
    session.track_mut(track).unwrap().mixer.output = Route::Main;
    track
}

fn automation(session: &mut Session, track: TrackId, param: AutoParam, points: &[(i64, f32)]) {
    let mut lane = AutomationLane::new(param);
    for &(at, value) in points {
        lane.set_point(at, value);
    }
    session.track_mut(track).unwrap().automation.push(lane);
}

fn send(session: &mut Session, sender: TrackId, capture: BusId, pre: bool, pan_start: i64, pan_end: i64, automated_pan: bool) {
    let mut send = SendSlot::new(Route::Bus(capture));
    send.level_db = 0.0;
    send.pre_fader = pre;
    send.pan = -1.0;
    session.track_mut(sender).unwrap().mixer.sends[0] = Some(send);
    automation(session, sender, AutoParam::SendMute(0), &[(-512, 0.0), (MUTE_AT, 1.0)]);
    if automated_pan {
        automation(session, sender, AutoParam::SendPan(0), &[(pan_start, -1.0), (pan_end, 1.0)]);
    }
}

fn ordinary(pre: bool, own_inserts: usize, silent_inserts: usize, pdc: bool, clip_start: i64, pan_start: i64, pan_end: i64) -> Graph {
    let mut session = Session::default();
    session.edit.delay_compensation = pdc;
    let sender = source(&mut session, own_inserts, clip_start);
    if silent_inserts > 0 {
        silent_audio(&mut session, silent_inserts);
    }
    let capture = session.add_bus("Automation capture", ChannelFormat::Stereo);
    send(&mut session, sender, capture, pre, pan_start, pan_end, true);
    let receiver = return_to_main(&mut session, capture);
    Graph { session, capture, receiver, clip_start, pan_start, pan_end }
}

fn make_mix() -> MixEngine {
    let mut mix = MixEngine::new(48_000.0, 1024);
    mix.pan_law = PanLaw::Zero;
    mix
}

fn capture_pcm(mix: &mut MixEngine, graph: &Graph, start: i64, length: usize, sizes: &[usize]) -> Vec<Vec<f32>> {
    let mut main = vec![Vec::new(), Vec::new()];
    let mut offset = 0;
    let mut block = 0;
    while offset < length {
        let n = sizes[block % sizes.len()].min(length - offset);
        let mut main_block = vec![vec![0.0; n]; 2];
        mix.render(&graph.session, start + offset as i64, n, &mut main_block);
        for (output, samples) in main.iter_mut().zip(main_block) {
            assert_eq!(samples.len(), n);
            output.extend(samples);
        }
        offset += n;
        block += 1;
    }
    main
}

fn numeric_pan(at: i64, first: i64, last: i64) -> [f32; 2] {
    let pan = if at <= first {
        -1.0
    } else if at >= last {
        1.0
    } else {
        (-1.0 + 2.0 * (at - first) as f64 / (last - first) as f64) as f32
    };
    // Independent declared zero-depth pan law; no routing or automation helper is called.
    [(1.0 - pan).min(1.0), (1.0 + pan).min(1.0)]
}

fn check_known(output: &[Vec<f32>], graph: &Graph, start: i64, delay: usize, length: usize) {
    assert_eq!(output.len(), 2);
    for (channel, samples) in output.iter().enumerate() {
        assert_eq!(samples.len(), length);
        for (offset, &sample) in samples.iter().enumerate() {
            let source_time = start + offset as i64 - delay as i64;
            let audible =
                offset >= delay && source_time >= graph.clip_start && source_time < graph.clip_start + SOURCE_FRAMES as i64 && source_time < MUTE_AT;
            let expected = if audible { AMPLITUDE * numeric_pan(source_time, graph.pan_start, graph.pan_end)[channel] } else { 0.0 };
            assert!(sample.is_finite());
            assert!(
                (sample - expected).abs() < 2e-7,
                "channel {channel}, emit {}, source {source_time}, delay {delay}: {sample} != {expected}",
                start + offset as i64
            );
        }
    }
    assert!(output.iter().flatten().any(|sample| sample.abs() > 0.02), "declared capture must be audible");
}

#[test]
fn unrelated_silent_lookahead_keeps_audio_pre_post_pan_and_last_source_sample_299() {
    for pre in [false, true] {
        let graph = ordinary(pre, 0, 1, true, 0, 0, 600);
        let mut mix = make_mix();
        let output = capture_pcm(&mut mix, &graph, 0, 800, &[1, 7, 37, 100, 3, 64]);
        assert_eq!(mix.latency, LOOKAHEAD, "the unrelated real Maximizer actually contributes latency");
        check_known(&output, &graph, 0, LOOKAHEAD, 800);
        let last = output[0].iter().rposition(|sample| sample.abs() > 1e-6).unwrap();
        assert_eq!(last, MUTE_AT as usize - 1 + LOOKAHEAD);
        // Public render_bus uses its default -3 dB law. This separate mute-only fixture
        // holds the static send at its left endpoint, whose factors are exactly1/0.
        let mut offline_session = graph.session.clone();
        offline_session.tracks[0].automation.retain(|lane| lane.param != AutoParam::SendPan(0));
        let offline = render_bus(&offline_session, graph.capture, Range::new(0, 640), 37);
        assert_eq!(offline[0].len(), 640);
        assert_eq!(offline[0].iter().rposition(|sample| sample.abs() > 1e-6), Some(299));
        for (channel, samples) in offline.iter().enumerate() {
            for (at, &sample) in samples.iter().enumerate() {
                let expected = if at < MUTE_AT as usize && channel == 0 { AMPLITUDE } else { 0.0 };
                assert!((sample - expected).abs() < 2e-7);
            }
        }
    }
}

#[test]
fn aux_pre_tap_omits_post_alignment_while_both_taps_include_upstream_and_own_delay() {
    for pdc in [false, true] {
        for pre in [false, true] {
            let mut session = Session::default();
            session.edit.delay_compensation = pdc;
            let source = source(&mut session, 1, 0);
            let input = session.add_bus("Own upstream", ChannelFormat::Mono);
            session.track_mut(source).unwrap().mixer.output = Route::Bus(input);
            let sender = aux(&mut session, input, ChannelFormat::Mono, 1);
            silent_aux(&mut session, 2);
            let capture = session.add_bus("Aux capture", ChannelFormat::Stereo);
            send(&mut session, sender, capture, pre, 0, 600, true);
            let receiver = return_to_main(&mut session, capture);
            let graph = Graph { session, capture, receiver, clip_start: 0, pan_start: 0, pan_end: 600 };
            let tap_delay = if pdc && !pre { 3 * LOOKAHEAD } else { 2 * LOOKAHEAD };
            let observation_delay = if pdc { 2 * LOOKAHEAD } else { 0 };
            let mut mix = make_mix();
            let output = capture_pcm(&mut mix, &graph, 0, 900, &[100, 17, 1, 37, 64]);
            assert_eq!(mix.latency, if pdc { 3 * LOOKAHEAD } else { 0 });
            check_known(&output, &graph, 0, tap_delay + observation_delay, 900);
        }
    }
}

#[test]
fn nested_aux_taps_accumulate_the_actual_upstream_path_instead_of_global_latency() {
    for pre in [false, true] {
        let mut session = Session::default();
        session.edit.delay_compensation = true;
        let source = source(&mut session, 1, 0);
        silent_audio(&mut session, 2);
        let first_bus = session.add_bus("Nested first", ChannelFormat::Mono);
        session.track_mut(source).unwrap().mixer.output = Route::Bus(first_bus);
        let first_aux = aux(&mut session, first_bus, ChannelFormat::Mono, 1);
        let second_bus = session.add_bus("Nested second", ChannelFormat::Mono);
        session.track_mut(first_aux).unwrap().mixer.output = Route::Bus(second_bus);
        let sender = aux(&mut session, second_bus, ChannelFormat::Mono, 1);
        silent_aux(&mut session, 2);
        let capture = session.add_bus("Nested capture", ChannelFormat::Stereo);
        send(&mut session, sender, capture, pre, 0, 600, true);
        let receiver = return_to_main(&mut session, capture);
        let graph = Graph { session, capture, receiver, clip_start: 0, pan_start: 0, pan_end: 600 };
        // Upstream audio2*71; first Aux own+alignment2*71; target pre adds1*71,
        // target post adds2*71. The scalar global mix delay is only4*71.
        let tap_delay = if pre { 5 * LOOKAHEAD } else { 6 * LOOKAHEAD };
        let observation_delay = 2 * LOOKAHEAD;
        let mut mix = make_mix();
        let output = capture_pcm(&mut mix, &graph, 0, 1100, &[37, 3, 100, 64, 1]);
        assert_eq!(mix.latency, 4 * LOOKAHEAD);
        check_known(&output, &graph, 0, tap_delay + observation_delay, 1100);
    }
}

#[test]
fn output_extra_downstream_aux_and_master_delay_do_not_shift_the_send_clock() {
    for pre in [false, true] {
        let mut graph = ordinary(pre, 0, 1, true, 0, 0, 600);
        let source = graph.session.tracks[0].id;
        graph.session.track_mut(source).unwrap().mixer.output = Route::Main;
        maximizers(&mut graph.session, graph.receiver, 1);
        silent_aux(&mut graph.session, 2);
        let master = graph.session.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        maximizers(&mut graph.session, master, 1);
        let mut mix = make_mix();
        let main = capture_pcm(&mut mix, &graph, 0, 900, &[100, 37, 1, 64]);
        assert_eq!(mix.latency, 4 * LOOKAHEAD);
        assert!(main.iter().all(|channel| channel[..4 * LOOKAHEAD].iter().all(|sample| *sample == 0.0)));
        assert!(main[0][4 * LOOKAHEAD] > 0.4, "main really traverses the excluded output/Aux/Master delays");
        for (channel, samples) in main.iter().enumerate() {
            for (at, &sample) in samples.iter().enumerate() {
                let source_time = at as i64 - (4 * LOOKAHEAD) as i64;
                let expected = if source_time < 0 {
                    0.0
                } else {
                    let sent = if source_time < MUTE_AT { AMPLITUDE * numeric_pan(source_time, 0, 600)[channel] } else { 0.0 };
                    AMPLITUDE + sent
                };
                assert!((sample - expected).abs() < 2e-7, "exclusion channel {channel}, emit {at}: {sample} != {expected}");
            }
        }
    }
}

#[test]
fn unequal_aux_pre_and_direct_bus_ages_keep_the_documented_raw_clock_fallback() {
    for target_pre in [false, true] {
        let mut session = Session::default();
        session.edit.delay_compensation = true;
        let source = source(&mut session, 1, 0);
        let input = session.add_bus("Mixed upstream", ChannelFormat::Mono);
        session.track_mut(source).unwrap().mixer.output = Route::Bus(input);
        let contributor = aux(&mut session, input, ChannelFormat::Mono, 1);
        silent_aux(&mut session, 2);
        let mixed_bus = session.add_bus("Unequal actual ages", ChannelFormat::Mono);
        session.track_mut(contributor).unwrap().mixer.output = Route::Bus(mixed_bus);
        let mut pre_send = SendSlot::new(Route::Bus(mixed_bus));
        pre_send.level_db = 0.0;
        pre_send.pre_fader = true;
        session.track_mut(contributor).unwrap().mixer.sends[0] = Some(pre_send);
        let sender = aux(&mut session, mixed_bus, ChannelFormat::Mono, 0);
        let capture = session.add_bus("Mixed capture", ChannelFormat::Stereo);
        send(&mut session, sender, capture, target_pre, 0, 600, false);
        let receiver = return_to_main(&mut session, capture);
        let graph = Graph { session, capture, receiver, clip_start: 0, pan_start: 0, pan_end: 600 };
        let mut mix = make_mix();
        let output = capture_pcm(&mut mix, &graph, 0, 900, &[37, 100, 1, 3, 64]);
        let extra = if target_pre { 0 } else { 2 * LOOKAHEAD };
        let observation_delay = 2 * LOOKAHEAD;
        let delays = [2 * LOOKAHEAD + extra + observation_delay, 3 * LOOKAHEAD + extra + observation_delay];
        for (at, (&left, &right)) in output[0].iter().zip(&output[1]).enumerate() {
            let contributions = delays.iter().filter(|&&delay| at >= delay).count();
            let expected = if at < MUTE_AT as usize + observation_delay { AMPLITUDE * contributions as f32 } else { 0.0 };
            assert!((left - expected).abs() < 2e-7, "mixed tap pre={target_pre} emit{at}: {left} != {expected}");
            assert_eq!(right, 0.0);
        }
        assert!(output[0].iter().any(|sample| *sample > 0.02));
    }
}

#[test]
fn negative_positions_and_reset_seek_use_the_delayed_source_clock_with_pdc_off() {
    let graph = ordinary(false, 1, 0, false, -512, -300, 300);
    let mut mix = make_mix();
    let negative = capture_pcm(&mut mix, &graph, -128, 256, &[1, 37, 3, 64, 17]);
    assert_eq!(mix.latency, 0, "PDC is off while the insert still physically delays its source");
    check_known(&negative, &graph, -128, LOOKAHEAD, 256);
    mix.reset();
    let seek = capture_pcm(&mut mix, &graph, 240, 400, &[100, 1, 37, 3]);
    check_known(&seek, &graph, 240, LOOKAHEAD, 400);
    assert_eq!(seek[1].iter().rposition(|sample| sample.abs() > 1e-6), Some(130));
}
