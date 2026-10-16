//! Public PCM compatibility for send coefficients and live send controls.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::pan::PanLaw;
use soundcraft_mix::{MAX_CHANNELS, MixEngine, main_channels};
use soundcraft_model::{BusId, ChannelFormat, Clip, Route, SEND_SLOTS, SendSlot, Session, SourceAudio, SourceId, SurroundPan, TrackId, TrackKind};
use std::sync::Arc;

const FRAMES: usize = 389;
const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn signal(channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|channel| (0..FRAMES).map(|frame| (channel + 1) as f32 / 64.0 + (frame % 7) as f32 / 1024.0).collect()).collect()
}
fn scaled(pcm: &[Vec<f32>], gain: f32) -> Vec<Vec<f32>> {
    pcm.iter().map(|channel| channel.iter().map(|value| value * gain).collect()).collect()
}
fn mono(gains: &[f32]) -> Vec<Vec<f32>> {
    let pcm = signal(1);
    gains.iter().map(|gain| pcm[0].iter().map(|value| value * gain).collect()).collect()
}
fn send(bus: BusId) -> SendSlot {
    let mut send = SendSlot::new(Route::Bus(bus));
    send.level_db = 0.0;
    send
}
struct Fixture {
    session: Session,
    source: TrackId,
    bus: BusId,
    consumer: TrackId,
}
fn destination(fixture: &mut Fixture, format: ChannelFormat) {
    fixture.session.busses.iter_mut().find(|bus| bus.id == fixture.bus).unwrap().format = format;
    let capture = if format == ChannelFormat::Mono { ChannelFormat::Stereo } else { format };
    fixture.session.outputs[0].format = if capture.channels() > MAX_CHANNELS { ChannelFormat::Ambisonics(3) } else { capture };
    let consumer = fixture.session.track_mut(fixture.consumer).unwrap();
    consumer.format = capture;
    consumer.mixer.pan = if capture == ChannelFormat::Stereo { vec![-1.0, 1.0] } else { Vec::new() };
}
fn fixture(source_format: ChannelFormat, bus_format: ChannelFormat) -> Fixture {
    let mut session = Session::default();
    session.edit.click = false;
    let bus = session.add_bus("Send", bus_format);
    let source = SourceId(session.alloc());
    session.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: signal(source_format.channels()) })));
    let clip = session.new_clip_id();
    let source_track = session.add_track(TrackKind::Audio, source_format, None);
    let track = session.track_mut(source_track).unwrap();
    track.mixer.output = Route::None;
    track.mixer.sends[0] = Some(send(bus));
    track.playlist_mut().unwrap().clips.push(Clip::audio(clip, "PCM", source, 0, 0, FRAMES as i64));
    let consumer = session.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    session.track_mut(consumer).unwrap().mixer.input = Route::Bus(bus);
    let mut fixture = Fixture { session, source: source_track, bus, consumer };
    destination(&mut fixture, bus_format);
    fixture
}
fn send_mut(fixture: &mut Fixture) -> &mut SendSlot {
    fixture.session.track_mut(fixture.source).unwrap().mixer.sends[0].as_mut().unwrap()
}
fn engine() -> MixEngine {
    MixEngine::new(48_000.0, 256)
}
fn block(engine: &mut MixEngine, fixture: &Fixture, position: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; frames]; main_channels(&fixture.session)];
    engine.render(&fixture.session, position as i64, frames, &mut out);
    out
}
fn assert_pcm(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), expected.len(), "channel count");
    for (channel, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.len(), expected.len(), "channel {channel} length");
        for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!((actual - expected).abs() < 2e-6, "channel {channel}, frame {frame}: {actual} versus {expected}");
        }
    }
}
fn check(engine: &mut MixEngine, fixture: &Fixture, expected: &[Vec<f32>]) {
    let expected: Vec<_> = expected.iter().map(|channel| channel[..64].to_vec()).collect();
    assert_pcm(&block(engine, fixture, 0, 64), &expected);
}

#[test]
fn mono_send_uses_own_pan_and_refreshes_public_pan_law() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let mut engine = engine();
    fixture.session.track_mut(fixture.source).unwrap().mixer.pan = vec![1.0];
    for (pan, gains) in [(-1.0, [1.0, 0.0]), (1.0, [0.0, 1.0])] {
        send_mut(&mut fixture).pan = pan;
        check(&mut engine, &fixture, &mono(&gains));
    }
    send_mut(&mut fixture).pan = 0.0;
    for (law, db) in [(PanLaw::Minus3, -3.0), (PanLaw::Minus4_5, -4.5), (PanLaw::Minus6, -6.0), (PanLaw::Zero, 0.0)] {
        engine.pan_law = law;
        let gain = 10f32.powf(db / 20.0);
        check(&mut engine, &fixture, &mono(&[gain, gain]));
    }
}

#[test]
fn stereo_send_keeps_channels_independent_of_send_and_track_pan() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let mut engine = engine();
    fixture.session.track_mut(fixture.source).unwrap().mixer.pan = vec![1.0, -1.0];
    for pan in [-1.0, 0.0, 1.0] {
        send_mut(&mut fixture).pan = pan;
        check(&mut engine, &fixture, &signal(2));
    }
}

#[test]
fn pre_post_switches_read_live_samples_and_fader_gain() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let mut engine = engine();
    fixture.session.track_mut(fixture.source).unwrap().mixer.volume_db = -6.0;
    let gain = 10f32.powf(-6.0 / 20.0);
    check(&mut engine, &fixture, &scaled(&signal(2), gain));
    send_mut(&mut fixture).pre_fader = true;
    check(&mut engine, &fixture, &signal(2));
    fixture.session.track_mut(fixture.source).unwrap().mixer.volume_db = -12.0;
    check(&mut engine, &fixture, &signal(2));
    send_mut(&mut fixture).pre_fader = false;
    check(&mut engine, &fixture, &scaled(&signal(2), gain * gain));
}

#[test]
fn send_gain_send_mute_and_track_mute_remain_live() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let mut engine = engine();
    check(&mut engine, &fixture, &signal(2));
    send_mut(&mut fixture).level_db = -6.0;
    let expected = scaled(&signal(2), 10f32.powf(-6.0 / 20.0));
    check(&mut engine, &fixture, &expected);
    send_mut(&mut fixture).mute = true;
    check(&mut engine, &fixture, &mono(&[0.0, 0.0]));
    send_mut(&mut fixture).mute = false;
    send_mut(&mut fixture).pre_fader = true;
    fixture.session.track_mut(fixture.source).unwrap().mixer.mute = true;
    check(&mut engine, &fixture, &mono(&[0.0, 0.0]));
    fixture.session.track_mut(fixture.source).unwrap().mixer.mute = false;
    check(&mut engine, &fixture, &expected);
}

#[test]
fn same_format_retarget_moves_audio_between_distinguishable_consumers() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let mut engine = engine();
    send_mut(&mut fixture).pan = -1.0;
    fixture.session.track_mut(fixture.consumer).unwrap().mixer.pan = vec![-1.0, -1.0];
    let other = fixture.session.add_bus("Other", ChannelFormat::Stereo);
    let consumer = fixture.session.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let mixer = &mut fixture.session.track_mut(consumer).unwrap().mixer;
    mixer.input = Route::Bus(other);
    mixer.pan = vec![1.0, 1.0];
    check(&mut engine, &fixture, &mono(&[1.0, 0.0]));
    send_mut(&mut fixture).target = Route::Bus(other);
    check(&mut engine, &fixture, &mono(&[0.0, 1.0]));
    send_mut(&mut fixture).target = Route::None;
    check(&mut engine, &fixture, &mono(&[0.0, 0.0]));
    let original = fixture.bus;
    send_mut(&mut fixture).target = Route::Bus(original);
    check(&mut engine, &fixture, &mono(&[1.0, 0.0]));
}

#[test]
fn format_layout_and_width_edits_refresh_one_engine() {
    let mut fixture = fixture(ChannelFormat::Lcrs, ChannelFormat::Lcrs);
    let mut engine = engine();
    let pcm = signal(4);
    check(&mut engine, &fixture, &pcm);
    destination(&mut fixture, ChannelFormat::Quad);
    let quad = vec![
        pcm[0].iter().zip(&pcm[2]).map(|(left, center)| left + H * center).collect(),
        pcm[1].iter().zip(&pcm[2]).map(|(right, center)| right + H * center).collect(),
        pcm[3].iter().map(|rear| H * rear).collect(),
        pcm[3].iter().map(|rear| H * rear).collect(),
    ];
    check(&mut engine, &fixture, &quad);
    fixture.session.track_mut(fixture.source).unwrap().format = ChannelFormat::Quad;
    check(&mut engine, &fixture, &pcm);
    destination(&mut fixture, ChannelFormat::Stereo);
    let stereo: Vec<_> = (0..2).map(|side| pcm[side].iter().zip(&pcm[side + 2]).map(|(front, rear)| front + H * rear).collect()).collect();
    check(&mut engine, &fixture, &stereo);
    let source = fixture.session.track_mut(fixture.source).unwrap();
    source.format = ChannelFormat::Stereo;
    source.mixer.pan = vec![-1.0, 1.0];
    check(&mut engine, &fixture, &pcm[..2]);
    destination(&mut fixture, ChannelFormat::Mono);
    let mono: Vec<_> = pcm[0].iter().zip(&pcm[1]).map(|(left, right)| 0.5 * (left + right)).collect();
    check(&mut engine, &fixture, &[mono.clone(), mono]);
}

#[test]
fn own_surround_overrides_followed_track_and_follow_can_be_disabled() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Surround51);
    let mut engine = engine();
    fixture.session.track_mut(fixture.source).unwrap().mixer.surround = Some(SurroundPan::default());
    check(&mut engine, &fixture, &mono(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]));
    send_mut(&mut fixture).surround = Some(SurroundPan { x: 1.0, y: -1.0, ..SurroundPan::default() });
    fixture.session.track_mut(fixture.source).unwrap().mixer.surround.as_mut().unwrap().x = -1.0;
    check(&mut engine, &fixture, &mono(&[0.0, 0.0, 0.0, 0.0, 0.0, 1.0]));
    send_mut(&mut fixture).surround = None;
    check(&mut engine, &fixture, &mono(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
    send_mut(&mut fixture).follow_main_pan = false;
    let gain = 10f32.powf(-3.0 / 20.0);
    check(&mut engine, &fixture, &mono(&[gain, gain, 0.0, 0.0, 0.0, 0.0]));
    send_mut(&mut fixture).follow_main_pan = true;
    check(&mut engine, &fixture, &mono(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
}

#[test]
fn surround_x_y_center_and_lfe_edits_refresh_independently() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Surround51);
    let mut engine = engine();
    for (surround, gains) in [
        (SurroundPan { x: 1.0, ..SurroundPan::default() }, [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        (SurroundPan { y: -1.0, ..SurroundPan::default() }, [0.0, 0.0, 0.0, 0.0, H, H]),
        (SurroundPan { center: 0.0, ..SurroundPan::default() }, [H, H, 0.0, 0.0, 0.0, 0.0]),
        (SurroundPan { lfe_db: 0.0, ..SurroundPan::default() }, [0.0, 0.0, 1.0, 1.0, 0.0, 0.0]),
    ] {
        send_mut(&mut fixture).surround = Some(SurroundPan::default());
        check(&mut engine, &fixture, &mono(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]));
        send_mut(&mut fixture).surround = Some(surround);
        check(&mut engine, &fixture, &mono(&gains));
    }
}

#[test]
fn height_and_divergence_edits_preserve_direction_and_energy() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Atmos714);
    let mut engine = engine();
    send_mut(&mut fixture).surround = Some(SurroundPan::default());
    let mut gains = [0.0; 12];
    gains[2] = 1.0;
    check(&mut engine, &fixture, &mono(&gains));
    send_mut(&mut fixture).surround.as_mut().unwrap().z = 1.0;
    gains[2] = 0.0;
    gains[8] = H;
    gains[9] = H;
    check(&mut engine, &fixture, &mono(&gains));
    send_mut(&mut fixture).surround = Some(SurroundPan::default());
    gains[2] = 1.0;
    gains[8] = 0.0;
    gains[9] = 0.0;
    check(&mut engine, &fixture, &mono(&gains));
    send_mut(&mut fixture).surround.as_mut().unwrap().divergence = 1.0;
    let out = block(&mut engine, &fixture, 0, 64);
    let source = signal(1);
    for frame in 0..64 {
        let energy: f32 = out.iter().enumerate().filter(|(channel, _)| *channel != 3).map(|(_, pcm)| pcm[frame].powi(2)).sum();
        assert!((energy - source[0][frame].powi(2)).abs() < 1e-8);
        assert!(out[..8].iter().filter(|pcm| pcm[frame].abs() > 1e-6).count() >= 3);
        assert!(out[8..].iter().all(|pcm| pcm[frame].abs() < 1e-6));
        assert_eq!(out[3][frame], 0.0);
    }
}

#[test]
fn sends_beyond_ten_slots_remain_audible_and_live() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let mut engine = engine();
    let mixer = &mut fixture.session.track_mut(fixture.source).unwrap().mixer;
    mixer.sends.resize(SEND_SLOTS + 2, None);
    mixer.sends[SEND_SLOTS] = Some(send(fixture.bus));
    check(&mut engine, &fixture, &scaled(&signal(2), 2.0));
    fixture.session.track_mut(fixture.source).unwrap().mixer.sends[SEND_SLOTS].as_mut().unwrap().mute = true;
    check(&mut engine, &fixture, &signal(2));
}

#[test]
fn oversized_internal_route_keeps_sixteen_channels_with_valid_capture() {
    let fixture = fixture(ChannelFormat::Ambisonics(7), ChannelFormat::Ambisonics(7));
    check(&mut engine(), &fixture, &signal(64)[..MAX_CHANNELS]);
}

#[test]
fn static_send_pcm_is_invariant_to_callback_splits() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    send_mut(&mut fixture).pan = 1.0;
    let expected = mono(&[0.0, 1.0]);
    for callbacks in [&[32][..], &[97][..], &[1, 63, 17, 256][..]] {
        let mut engine = engine();
        let mut out = vec![Vec::new(), Vec::new()];
        let mut position = 0;
        let mut callback = 0;
        while position < FRAMES {
            let frames = callbacks[callback % callbacks.len()].min(FRAMES - position);
            for (destination, channel) in out.iter_mut().zip(block(&mut engine, &fixture, position, frames)) {
                destination.extend(channel);
            }
            position += frames;
            callback += 1;
        }
        assert_pcm(&out, &expected);
    }
}
