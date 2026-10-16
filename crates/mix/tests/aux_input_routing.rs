//! Public PCM compatibility for Aux/Folder bus-input conversion and document edits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::pan::PanLaw;
use soundcraft_mix::{MAX_CHANNELS, MixEngine, main_channels};
use soundcraft_model::{BusId, ChannelFormat, Clip, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const FRAMES: usize = 389;

fn signal(channels: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|channel| (0..FRAMES).map(|frame| (((frame + channel * 3) % 19) as i32 - 9) as f32 * (channel + 1) as f32 / 1024.0).collect())
        .collect()
}

fn add_source(session: &mut Session, bus: BusId, format: ChannelFormat) -> TrackId {
    let source = SourceId(session.alloc());
    session.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: signal(format.channels()) })));
    let clip = session.new_clip_id();
    let track = session.add_track(TrackKind::Audio, format, None);
    let track_state = session.track_mut(track).unwrap();
    track_state.mixer.output = Route::Bus(bus);
    track_state.playlist_mut().unwrap().clips.push(Clip::audio(clip, "PCM", source, 0, 0, FRAMES as i64));
    track
}

struct Fixture {
    session: Session,
    bus: BusId,
    source: TrackId,
    aux: TrackId,
}

fn fixture(source: ChannelFormat, destination: ChannelFormat, kind: TrackKind) -> Fixture {
    let mut session = Session::default();
    session.edit.click = false;
    session.outputs[0].format = destination;
    let bus = session.add_bus("Input", source);
    let source_track = add_source(&mut session, bus, source);
    let aux = session.add_track(kind, destination, None);
    session.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
    Fixture { session, bus, source: source_track, aux }
}

fn engine() -> MixEngine {
    let mut engine = MixEngine::new(48_000.0, 256);
    // Unity mono routing into the source bus and out of a mono consumer isolates input gains.
    // The Aux input conversion itself retains its existing fixed -3 dB pan law.
    engine.pan_law = PanLaw::Zero;
    engine
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

fn stereo_fold(format: ChannelFormat, source: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    (0..2)
        .map(|side| {
            (0..FRAMES)
                .map(|frame| match format {
                    ChannelFormat::Quad => source[side][frame] + h * source[side + 2][frame],
                    ChannelFormat::Lcrs => source[side][frame] + h * source[2][frame] + 0.5 * source[3][frame],
                    ChannelFormat::Surround51 => source[side][frame] + h * source[2][frame] + h * source[side + 4][frame],
                    ChannelFormat::Ambisonics(1) => {
                        // ±30-degree first-order stereo decoder: W/2, ±3Y/4, no Z, 3√3 X/4.
                        let y = if side == 0 { 0.75 } else { -0.75 };
                        0.5 * source[0][frame] + y * source[1][frame] + (3.0 * 3.0f32.sqrt() / 4.0) * source[3][frame]
                    }
                    _ => panic!("only explicit fixture layouts have a fold oracle"),
                })
                .collect()
        })
        .collect()
}

#[test]
fn mono_bus_duplicates_to_stereo_at_unity() {
    let fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo, TrackKind::Aux);
    let mono = signal(1).remove(0);
    check(&mut engine(), &fixture, &[mono.clone(), mono]);
}

#[test]
fn equal_surround_layout_passes_every_channel_bit_exact() {
    let fixture = fixture(ChannelFormat::Surround51, ChannelFormat::Surround51, TrackKind::Aux);
    let expected: Vec<_> = signal(6).into_iter().map(|channel| channel[..64].to_vec()).collect();
    assert_eq!(block(&mut engine(), &fixture, 0, 64), expected);
}

#[test]
fn surround_bus_folds_to_stereo_and_drops_lfe() {
    for kind in [TrackKind::Aux, TrackKind::Folder] {
        let fixture = fixture(ChannelFormat::Surround51, ChannelFormat::Stereo, kind);
        check(&mut engine(), &fixture, &stereo_fold(ChannelFormat::Surround51, &signal(6)));
    }
}

#[test]
fn ambisonics_bus_decodes_first_order_stereo_coefficients() {
    let fixture = fixture(ChannelFormat::Ambisonics(1), ChannelFormat::Stereo, TrackKind::Aux);
    check(&mut engine(), &fixture, &stereo_fold(ChannelFormat::Ambisonics(1), &signal(4)));
}

#[test]
fn input_retarget_delete_readd_none_and_hardware_refresh_one_engine() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo, TrackKind::Aux);
    let mut engine = engine();
    let mono = signal(1).remove(0);
    check(&mut engine, &fixture, &[mono.clone(), mono.clone()]);
    let other = fixture.session.add_bus("Other", ChannelFormat::Stereo);
    add_source(&mut fixture.session, other, ChannelFormat::Stereo);
    fixture.session.track_mut(fixture.aux).unwrap().mixer.input = Route::Bus(other);
    check(&mut engine, &fixture, &signal(2));
    let saved_bus = fixture.session.busses.iter().find(|bus| bus.id == other).unwrap().clone();
    fixture.session.busses.retain(|bus| bus.id != other);
    let silence = vec![vec![0.0; FRAMES]; 2];
    check(&mut engine, &fixture, &silence);
    fixture.session.busses.push(saved_bus);
    check(&mut engine, &fixture, &signal(2));
    for input in [Route::None, Route::Hardware("In 1".into())] {
        fixture.session.track_mut(fixture.aux).unwrap().mixer.input = input;
        check(&mut engine, &fixture, &silence);
    }
    fixture.session.track_mut(fixture.aux).unwrap().mixer.input = Route::Bus(fixture.bus);
    check(&mut engine, &fixture, &[mono.clone(), mono]);
}

#[test]
fn equal_width_bus_layout_edits_change_the_input_coefficients() {
    let mut fixture = fixture(ChannelFormat::Quad, ChannelFormat::Stereo, TrackKind::Aux);
    let mut engine = engine();
    let pcm = signal(4);
    check(&mut engine, &fixture, &stereo_fold(ChannelFormat::Quad, &pcm));
    for format in [ChannelFormat::Lcrs, ChannelFormat::Quad] {
        fixture.session.busses[0].format = format;
        fixture.session.track_mut(fixture.source).unwrap().format = format;
        check(&mut engine, &fixture, &stereo_fold(format, &pcm));
    }
}

#[test]
fn equal_width_consumer_layout_edits_change_the_input_coefficients() {
    let mut fixture = fixture(ChannelFormat::Lcrs, ChannelFormat::Lcrs, TrackKind::Folder);
    let mut engine = engine();
    let pcm = signal(4);
    check(&mut engine, &fixture, &pcm);
    fixture.session.track_mut(fixture.aux).unwrap().format = ChannelFormat::Quad;
    fixture.session.outputs[0].format = ChannelFormat::Quad;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    // Front centre is phantom L/R; rear centre splits at equal power to Ls/Rs.
    let quad = vec![
        pcm[0].iter().zip(&pcm[2]).map(|(left, center)| left + h * center).collect(),
        pcm[1].iter().zip(&pcm[2]).map(|(right, center)| right + h * center).collect(),
        pcm[3].iter().map(|rear| h * rear).collect(),
        pcm[3].iter().map(|rear| h * rear).collect(),
    ];
    check(&mut engine, &fixture, &quad);
    fixture.session.track_mut(fixture.aux).unwrap().format = ChannelFormat::Lcrs;
    fixture.session.outputs[0].format = ChannelFormat::Lcrs;
    check(&mut engine, &fixture, &pcm);
}

#[test]
fn source_bus_and_consumer_width_edits_refresh_one_engine() {
    let mut fixture = fixture(ChannelFormat::Surround51, ChannelFormat::Stereo, TrackKind::Aux);
    let mut engine = engine();
    let pcm = signal(6);
    let folded = stereo_fold(ChannelFormat::Surround51, &pcm);
    check(&mut engine, &fixture, &folded);
    fixture.session.track_mut(fixture.source).unwrap().format = ChannelFormat::Stereo;
    fixture.session.track_mut(fixture.source).unwrap().mixer.pan = vec![-1.0, 1.0];
    check(&mut engine, &fixture, &pcm[..2]);
    fixture.session.track_mut(fixture.source).unwrap().format = ChannelFormat::Surround51;
    fixture.session.busses[0].format = ChannelFormat::Stereo;
    check(&mut engine, &fixture, &folded);
    fixture.session.busses[0].format = ChannelFormat::Surround51;
    fixture.session.track_mut(fixture.aux).unwrap().format = ChannelFormat::Surround51;
    fixture.session.outputs[0].format = ChannelFormat::Surround51;
    check(&mut engine, &fixture, &pcm);
    fixture.session.track_mut(fixture.aux).unwrap().format = ChannelFormat::Mono;
    fixture.session.track_mut(fixture.aux).unwrap().mixer.pan = vec![0.0];
    fixture.session.outputs[0].format = ChannelFormat::Stereo;
    let mono: Vec<_> = folded[0].iter().zip(&folded[1]).map(|(left, right)| 0.5 * (left + right)).collect();
    check(&mut engine, &fixture, &[mono.clone(), mono]);
}

#[test]
fn formats_wider_than_the_mix_limit_keep_the_first_sixteen_channels() {
    let mut fixture = fixture(ChannelFormat::Ambisonics(7), ChannelFormat::Ambisonics(7), TrackKind::Aux);
    // Capture through a valid 16-channel output; wider output paths fall back to stereo.
    fixture.session.outputs[0].format = ChannelFormat::Ambisonics(3);
    check(&mut engine(), &fixture, &signal(64)[..MAX_CHANNELS]);
}

#[test]
fn duplicate_bus_ids_use_the_last_effective_buffer_layout_and_width() {
    for (first, last) in [(ChannelFormat::Quad, ChannelFormat::Lcrs), (ChannelFormat::Mono, ChannelFormat::Surround51)] {
        let mut fixture = fixture(last, ChannelFormat::Stereo, TrackKind::Aux);
        let last_bus = fixture.session.busses[0].clone();
        fixture.session.busses[0].format = first;
        fixture.session.busses.push(last_bus);
        check(&mut engine(), &fixture, &stereo_fold(last, &signal(last.channels())));
    }
}

#[test]
fn duplicate_track_ids_keep_each_consumers_conversion_with_shared_strip_width() {
    let mut fixture = fixture(ChannelFormat::Lcrs, ChannelFormat::Stereo, TrackKind::Aux);
    fixture.session.track_mut(fixture.aux).unwrap().format = ChannelFormat::Quad;
    let mut duplicate = fixture.session.track(fixture.aux).unwrap().clone();
    duplicate.format = ChannelFormat::Lcrs;
    fixture.session.tracks.push(duplicate);
    let mut expected = stereo_fold(ChannelFormat::Lcrs, &signal(4));
    for channel in &mut expected {
        for value in channel {
            *value *= 2.0;
        }
    }
    check(&mut engine(), &fixture, &expected);
}

#[test]
fn input_conversion_preserves_pcm_across_callback_splits() {
    let fixture = fixture(ChannelFormat::Ambisonics(1), ChannelFormat::Stereo, TrackKind::Folder);
    let expected = stereo_fold(ChannelFormat::Ambisonics(1), &signal(4));
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
