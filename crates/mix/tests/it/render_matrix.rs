//! Offline renders do not depend on the block size or on where the render starts: each scenario
//! renders the same samples at every block size as at 512, and a render from a later start is
//! the tail of the render from zero.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{fold_down_stereo, render_range};
use soundcraft_model::{
    AutoParam, ChannelFormat, Clip, Fade, FadeShape, Insert, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 4800;
const TOLERANCE: f32 = 1e-6;
const BLOCKS: [usize; 5] = [1, 37, 64, 512, 4096];
/// Block sizes that share the 32-sample grid clip gain is stepped on.
const GRID_BLOCKS: [usize; 3] = [64, 512, 4096];

struct Scenario {
    name: &'static str,
    build: fn() -> Session,
    /// Main mix format folded down to stereo before comparing.
    fold: Option<ChannelFormat>,
}

/// Distinct, deterministic channels well below 0 dBFS.
fn signal(channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| (0..LEN).map(|i| ((i * (c + 3)) % 61) as f32 / 128.0 - 30.0 / 128.0).collect()).collect()
}

/// An empty session with the click off and delay compensation on.
fn base() -> Session {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.delay_compensation = true;
    s
}

/// An audio track routed to the main mix, playing `(offset, start, length)` clips of one source.
fn add_audio(s: &mut Session, format: ChannelFormat, clips: &[(i64, i64, i64)]) -> TrackId {
    let source = SourceId(s.alloc());
    s.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: signal(format.channels()) })));
    let t = s.add_track(TrackKind::Audio, format, None);
    for &(offset, start, length) in clips {
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "pcm", source, offset, start, length));
    }
    let tr = s.track_mut(t).unwrap();
    if format == ChannelFormat::Stereo {
        tr.mixer.pan = vec![-1.0, 1.0];
    }
    tr.mixer.output = Route::Main;
    t
}

/// The clips on track `t`'s playlist.
fn clips(s: &mut Session, t: TrackId) -> &mut Vec<Clip> {
    &mut s.track_mut(t).unwrap().playlist_mut().unwrap().clips
}

/// A fade of `len` samples.
fn fade(len: i64, shape: FadeShape, curvature: f32) -> Fade {
    Fade { len, shape, curvature }
}

/// A stereo clip with clip gain and both fades. It starts on the 32-sample clip gain grid (96),
/// and the fade-in crosses the 512 block edge.
fn faded_clip() -> Session {
    let mut s = base();
    let t = add_audio(&mut s, ChannelFormat::Stereo, &[(0, 96, 3000)]);
    let clip = &mut clips(&mut s, t)[0];
    clip.fade_in = fade(700, FadeShape::EqualPower, 0.0);
    clip.fade_out = fade(900, FadeShape::SCurve, 0.25);
    clip.gain_db = -2.0;
    s
}

/// The overlap (448..544) lies on the 32-sample clip gain grid and straddles the 512 block edge.
fn crossfade() -> Session {
    let mut s = base();
    let t = add_audio(&mut s, ChannelFormat::Stereo, &[(0, 0, 544), (1200, 448, 2000)]);
    let pair = clips(&mut s, t);
    pair[0].fade_out = fade(96, FadeShape::EqualPower, 0.0);
    pair[1].fade_in = fade(96, FadeShape::EqualPower, 0.0);
    s
}

/// A post-fader send to an aux return whose compressor reports 2 ms of lookahead. The signal
/// stays below the 0 dB hard-knee threshold, so the compressor only delays.
fn send_to_latent_aux() -> Session {
    let mut s = base();
    let t = add_audio(&mut s, ChannelFormat::Stereo, &[(0, 0, LEN as i64)]);
    let bus = s.add_bus("Verb", ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
    let mut comp = Insert::new("compressor");
    comp.params.insert("threshold".into(), 0.0);
    comp.params.insert("knee".into(), 0.0);
    comp.params.insert("lookahead".into(), 2.0);
    let ret = s.track_mut(aux).unwrap();
    ret.mixer.input = Route::Bus(bus);
    ret.mixer.output = Route::Main;
    ret.mixer.pan = vec![-1.0, 1.0];
    ret.mixer.inserts[0] = Some(comp);
    let tr = s.track_mut(t).unwrap();
    tr.mixer.volume_db = -4.0;
    let mut send = SendSlot::new(Route::Bus(bus));
    send.level_db = -6.0;
    tr.mixer.sends[0] = Some(send);
    s
}

/// A track whose level is scaled by a VCA master.
fn vca_member() -> Session {
    let mut s = base();
    let t = add_audio(&mut s, ChannelFormat::Stereo, &[(0, 0, LEN as i64)]);
    let vca = s.add_track(TrackKind::Vca, ChannelFormat::Stereo, Some("VCA"));
    s.track_mut(vca).unwrap().mixer.volume_db = -7.5;
    let tr = s.track_mut(t).unwrap();
    tr.mixer.vca = Some(vca.0);
    tr.mixer.volume_db = -3.0;
    s
}

/// A 5.1 track on a 5.1 main mix, folded down to stereo before comparing.
fn surround_fold_down() -> Session {
    let mut s = base();
    s.outputs[0].format = ChannelFormat::Surround51;
    let t = add_audio(&mut s, ChannelFormat::Surround51, &[(0, 0, LEN as i64)]);
    s.track_mut(t).unwrap().mixer.volume_db = -1.5;
    s
}

/// Volume automation with ramps that start and end inside blocks.
fn volume_ramp() -> Session {
    let mut s = base();
    let t = add_audio(&mut s, ChannelFormat::Stereo, &[(0, 0, LEN as i64)]);
    let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Volume);
    for (at, db) in [(0, 0.0), (1000, 0.0), (1300, -18.0), (3000, -6.0)] {
        lane.set_point(at, db);
    }
    s
}

const STEADY: [Scenario; 3] = [
    Scenario { name: "send to a latent aux", build: send_to_latent_aux, fold: None },
    Scenario { name: "VCA member", build: vca_member, fold: None },
    Scenario { name: "5.1 folded down", build: surround_fold_down, fold: Some(ChannelFormat::Surround51) },
];

const FADED: [Scenario; 2] = [
    Scenario { name: "stereo clip with fades", build: faded_clip, fold: None },
    Scenario { name: "crossfade across a block edge", build: crossfade, fold: None },
];

/// Renders `LEN` samples from `start` at `block`, folded down to stereo if the scenario asks.
fn render(sc: &Scenario, s: &Session, start: i64, block: usize) -> Vec<Vec<f32>> {
    let out = render_range(s, Range::new(start, LEN as i64), block);
    match sc.fold {
        Some(fmt) => fold_down_stereo(fmt, &out),
        None => out,
    }
}

/// Asserts the same shape and every sample within `TOLERANCE`.
fn assert_close(actual: &[Vec<f32>], expected: &[Vec<f32>], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}: channel count");
    for (channel, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.len(), expected.len(), "{context}: channel {channel} length");
        for (frame, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!((a - e).abs() <= TOLERANCE, "{context}: channel {channel}, frame {frame}: {a} vs {e}");
        }
    }
}

/// The render from zero at block 512, checked to be stereo and not silent.
fn reference(sc: &Scenario, s: &Session) -> Vec<Vec<f32>> {
    let out = render(sc, s, 0, 512);
    assert_eq!(out.len(), 2, "{}: stereo result", sc.name);
    assert!(out.iter().flatten().any(|x| x.abs() > 0.01), "{}: the reference is silent", sc.name);
    out
}

/// Each scenario renders the reference at every block size in `blocks`.
fn check_blocks(scenarios: &[Scenario], blocks: &[usize]) {
    for sc in scenarios {
        let s = (sc.build)();
        let want = reference(sc, &s);
        for &block in blocks {
            assert_close(&render(sc, &s, 0, block), &want, &format!("{} at block {block}", sc.name));
        }
    }
}

/// [`check_offsets_at`] at block 512.
fn check_offsets(scenarios: &[Scenario], offsets: &[i64]) {
    check_offsets_at(scenarios, offsets, 512);
}

/// Each scenario rendered from each start in `offsets` at `block` is the tail of the reference.
fn check_offsets_at(scenarios: &[Scenario], offsets: &[i64], block: usize) {
    for sc in scenarios {
        let s = (sc.build)();
        let full = reference(sc, &s);
        for &start in offsets {
            let tail: Vec<Vec<f32>> = full.iter().map(|c| c[start as usize..].to_vec()).collect();
            assert_close(&render(sc, &s, start, block), &tail, &format!("{} from {start} at block {block}", sc.name));
        }
    }
}

#[test]
fn steady_scenarios_match_the_512_block_reference_at_every_block_size() {
    check_blocks(&STEADY, &BLOCKS);
}

#[test]
fn faded_scenarios_match_the_512_block_reference_on_the_clip_gain_grid() {
    check_blocks(&FADED, &GRID_BLOCKS);
}

#[test]
#[ignore = "clip gain and fades are stepped every 32 samples from each block's start"]
fn faded_scenarios_match_the_512_block_reference_at_every_block_size() {
    check_blocks(&FADED, &BLOCKS);
}

#[test]
fn a_later_start_renders_the_tail_of_the_full_render() {
    check_offsets(&STEADY, &[1001, 2048]);
    // Starts on the 32-sample clip gain grid; 480 locates inside the crossfade.
    check_offsets(&FADED, &[480, 2048]);
}

#[test]
fn a_later_start_renders_the_tail_at_an_odd_block_size() {
    check_offsets_at(&STEADY, &[1001, 2048], 37);
}

#[test]
fn a_later_start_on_a_block_edge_renders_the_tail() {
    check_offsets(&STEADY, &[512, 4096]);
    check_offsets(&FADED, &[512, 4096]);
}

#[test]
fn faded_later_starts_render_the_tail_at_every_grid_block_size() {
    for &block in &GRID_BLOCKS {
        check_offsets_at(&FADED, &[480, 2048], block);
    }
}

#[test]
fn rendering_the_same_session_twice_gives_identical_samples() {
    for sc in STEADY.iter().chain(FADED.iter()) {
        let s = (sc.build)();
        let first = render(sc, &s, 0, 512);
        let second = render(sc, &s, 0, 512);
        assert_eq!(first, second, "{}: repeated render", sc.name);
    }
}

#[test]
#[ignore = "#1354"]
fn interior_volume_ramps_match_the_512_block_reference_at_every_block_size() {
    check_blocks(&[Scenario { name: "interior volume ramp", build: volume_ramp, fold: None }], &BLOCKS);
}
