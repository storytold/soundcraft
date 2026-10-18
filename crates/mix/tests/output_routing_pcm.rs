use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_dsp::pan::PanLaw;
use soundcraft_mix::{MAX_CHANNELS, MixEngine, render_bus};
use soundcraft_model::{
    AutoParam, AutomationMode, Bus, BusId, ChannelFormat, Clip, Insert, Route, SendSlot, Session, Source, SourceAudio, SourceId, SurroundPan,
    TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::Arc;

const FRAMES: usize = 1024;
const EPS: f32 = 2e-6;
const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn signal(channel: usize, frame: usize) -> f32 {
    (channel + 1) as f32 / 16.0 + (frame % 7) as f32 / 512.0
}
fn data(width: usize) -> Vec<Vec<f32>> {
    (0..width).map(|channel| (0..FRAMES).map(|frame| signal(channel, frame)).collect()).collect()
}
struct Fixture {
    session: Session,
    track: TrackId,
    source: SourceId,
}
fn fixture(format: ChannelFormat, main: ChannelFormat) -> Fixture {
    fixture_with_data(format, main, data(format.channels()))
}
fn fixture_with_data(format: ChannelFormat, main: ChannelFormat, channels: Vec<Vec<f32>>) -> Fixture {
    let mut session = Session::default();
    session.outputs[0].format = main;
    let track = session.add_track(TrackKind::Audio, format, Some("Synthetic output"));
    let source = SourceId(session.alloc());
    let frames = channels[0].len();
    session.sources.push(Source {
        id: source,
        name: "Synthetic".into(),
        path: String::new(),
        channels: channels.len() as u16,
        frames: frames as u64,
        sample_rate: 48_000,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: true,
    });
    session.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels })));
    let clip = session.new_clip_id();
    let strip = session.track_mut(track).unwrap();
    strip.mixer.pan = if format == ChannelFormat::Stereo { vec![-1.0, 1.0] } else { vec![0.0] };
    strip.mixer.output = Route::Main;
    strip.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Synthetic", source, 0, 0, frames as i64));
    Fixture { session, track, source }
}
fn block(engine: &mut MixEngine, fixture: &Fixture, position: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; frames]; fixture.session.main_format().channels()];
    engine.render(&fixture.session, position as i64, frames, &mut out);
    out
}
fn expect(out: &[Vec<f32>], expected: impl Fn(usize, usize) -> f32) {
    for (channel, samples) in out.iter().enumerate() {
        for (frame, actual) in samples.iter().enumerate() {
            let wanted = expected(channel, frame);
            assert!((*actual - wanted).abs() < EPS, "channel {channel}, frame {frame}: {actual} != {wanted}");
        }
    }
}
fn aux(session: &mut Session, bus: BusId, format: ChannelFormat, pan: [f32; 2]) -> TrackId {
    let track = session.add_track(TrackKind::Aux, format, None);
    let strip = session.track_mut(track).unwrap();
    strip.mixer.input = Route::Bus(bus);
    strip.mixer.pan = pan.to_vec();
    track
}

#[test]
fn mono_main_output_updates_all_pan_laws_on_one_engine() {
    let fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, (law, db)) in [(PanLaw::Minus3, -3.0), (PanLaw::Minus4_5, -4.5), (PanLaw::Minus6, -6.0), (PanLaw::Zero, 0.0)].into_iter().enumerate()
    {
        engine.pan_law = law;
        let position = index * 32;
        let gain = 10.0f32.powf(db / 20.0);
        expect(&block(&mut engine, &fixture, position, 32), |_, frame| signal(0, position + frame) * gain);
    }
}

#[test]
fn stereo_output_uses_both_panners_zero_law_and_unclipped_headroom() {
    let mut fixture = fixture_with_data(ChannelFormat::Stereo, ChannelFormat::Stereo, vec![vec![1.25; FRAMES], vec![-0.75; FRAMES]]);
    let mut engine = MixEngine::new(48_000.0, 32);
    engine.pan_law = PanLaw::Minus6;
    for (index, (pan, wanted)) in
        [(vec![-1.0, 1.0], [1.25, -0.75]), (vec![0.0, 0.0], [0.5, 0.5]), (vec![1.0, -1.0], [-0.75, 1.25]), (Vec::new(), [1.25, 0.5])]
            .into_iter()
            .enumerate()
    {
        fixture.session.track_mut(fixture.track).unwrap().mixer.pan = pan;
        expect(&block(&mut engine, &fixture, index * 32, 32), |channel, _| wanted[channel]);
    }
}

#[test]
fn pan_fader_and_mute_edits_keep_output_samples_live() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let mut engine = MixEngine::new(48_000.0, 32);
    engine.pan_law = PanLaw::Zero;
    for (index, (pan, db, mute, gains)) in
        [(-1.0, 0.0, false, [1.0, 0.0]), (0.0, -6.0, false, [1.0, 1.0]), (1.0, -6.0, true, [0.0, 0.0]), (1.0, 0.0, false, [0.0, 1.0])]
            .into_iter()
            .enumerate()
    {
        let mixer = &mut fixture.session.track_mut(fixture.track).unwrap().mixer;
        mixer.pan = vec![pan];
        mixer.volume_db = db;
        mixer.mute = mute;
        let position = index * 32;
        let gain = 10.0f32.powf(db / 20.0);
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| signal(0, position + frame) * gain * gains[channel]);
    }
}

#[test]
fn bus_outputs_keep_mono_sum_and_surround_speaker_mapping() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let mono = fixture.session.add_bus("Mono capture", ChannelFormat::Mono);
    fixture.session.track_mut(fixture.track).unwrap().mixer.output = Route::Bus(mono);
    let out = render_bus(&fixture.session, mono, Range::new(0, 32), 32);
    assert_eq!(out.len(), 1);
    expect(&out, |_, frame| 0.5 * (signal(0, frame) + signal(1, frame)));
    let surround = fixture.session.add_bus("Surround capture", ChannelFormat::Surround51);
    fixture.session.track_mut(fixture.track).unwrap().mixer.output = Route::Bus(surround);
    let out = render_bus(&fixture.session, surround, Range::new(0, 32), 32);
    assert_eq!(out.len(), 6);
    expect(&out, |channel, frame| if channel < 2 { signal(channel, frame) } else { 0.0 });
}

#[test]
fn same_engine_output_retargets_main_busses_missing_and_readded_destinations() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    let a = fixture.session.add_bus("Left return", ChannelFormat::Stereo);
    let b = fixture.session.add_bus("Right return", ChannelFormat::Stereo);
    aux(&mut fixture.session, a, ChannelFormat::Stereo, [-1.0, -1.0]);
    aux(&mut fixture.session, b, ChannelFormat::Stereo, [1.0, 1.0]);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, (route, destination)) in [
        (Route::Bus(a), Some(0)),
        (Route::Bus(b), Some(1)),
        (Route::Main, Some(2)),
        (Route::None, None),
        (Route::Hardware("Output 1".into()), None),
        (Route::Bus(BusId(999_999)), None),
    ]
    .into_iter()
    .enumerate()
    {
        fixture.session.track_mut(fixture.track).unwrap().mixer.output = route;
        let position = index * 32;
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| match destination {
            Some(2) => signal(channel, position + frame),
            Some(side) if side == channel => signal(0, position + frame) + signal(1, position + frame),
            _ => 0.0,
        });
    }
    fixture.session.track_mut(fixture.track).unwrap().mixer.output = Route::Bus(a);
    fixture.session.busses.retain(|bus| bus.id != a);
    expect(&block(&mut engine, &fixture, 192, 32), |_, _| 0.0);
    fixture.session.busses.push(Bus { id: a, name: "Left restored".into(), format: ChannelFormat::Stereo });
    expect(&block(&mut engine, &fixture, 224, 32), |channel, frame| if channel == 0 { signal(0, 224 + frame) + signal(1, 224 + frame) } else { 0.0 });
}

#[test]
fn main_layout_edits_include_equal_width_layout_changes() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Quad);
    fixture.session.track_mut(fixture.track).unwrap().mixer.surround = Some(SurroundPan::default());
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, format) in [ChannelFormat::Quad, ChannelFormat::Lcrs, ChannelFormat::Surround51, ChannelFormat::Stereo].into_iter().enumerate() {
        fixture.session.outputs[0].format = format;
        let position = index * 32;
        let out = block(&mut engine, &fixture, position, 32);
        assert_eq!(out.len(), format.channels());
        expect(&out, |channel, frame| {
            let gain = match format {
                ChannelFormat::Quad if channel < 2 => H,
                ChannelFormat::Lcrs | ChannelFormat::Surround51 if channel == 2 => 1.0,
                ChannelFormat::Stereo => 10.0f32.powf(-3.0 / 20.0),
                _ => 0.0,
            };
            signal(0, position + frame) * gain
        });
    }
}

#[test]
fn source_layout_and_width_edits_refresh_effective_output_geometry() {
    let mut fixture = fixture(ChannelFormat::Quad, ChannelFormat::Stereo);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, format) in [ChannelFormat::Quad, ChannelFormat::Lcrs, ChannelFormat::Stereo, ChannelFormat::Mono].into_iter().enumerate() {
        let strip = fixture.session.track_mut(fixture.track).unwrap();
        strip.format = format;
        strip.mixer.pan = if format == ChannelFormat::Stereo { vec![-1.0, 1.0] } else { vec![-1.0] };
        let position = index * 32;
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| {
            let x = |source| signal(source, position + frame);
            match format {
                ChannelFormat::Quad => x(channel) + H * x(channel + 2),
                ChannelFormat::Lcrs => x(channel) + H * x(2) + 0.5 * x(3),
                ChannelFormat::Stereo => x(channel),
                ChannelFormat::Mono if channel == 0 => x(0),
                _ => 0.0,
            }
        });
    }
}

#[test]
fn surround_output_precedence_and_front_rear_center_lfe_edits_are_preserved() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Surround51);
    fixture.session.track_mut(fixture.track).unwrap().mixer.pan = vec![-1.0];
    let mut engine = MixEngine::new(48_000.0, 32);
    expect(&block(&mut engine, &fixture, 0, 32), |channel, frame| if channel == 0 { signal(0, frame) } else { 0.0 });
    for (index, (surround, gains)) in [
        (SurroundPan::default(), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]),
        (SurroundPan { center: 0.0, ..SurroundPan::default() }, [H, H, 0.0, 0.0, 0.0, 0.0]),
        (SurroundPan { x: 1.0, ..SurroundPan::default() }, [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        (SurroundPan { x: -1.0, y: -1.0, lfe_db: -6.0, ..SurroundPan::default() }, [0.0, 0.0, 0.0, 10.0f32.powf(-6.0 / 20.0), 1.0, 0.0]),
    ]
    .into_iter()
    .enumerate()
    {
        fixture.session.track_mut(fixture.track).unwrap().mixer.surround = Some(surround);
        let position = (index + 1) * 32;
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| signal(0, position + frame) * gains[channel]);
    }
    fixture.session.outputs[0].format = ChannelFormat::Stereo;
    expect(&block(&mut engine, &fixture, 160, 32), |channel, frame| if channel == 0 { signal(0, 160 + frame) } else { 0.0 });
}

#[test]
fn surround_height_and_divergence_edits_preserve_energy_and_move_sound() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Atmos512);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, (z, divergence)) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)].into_iter().enumerate() {
        fixture.session.track_mut(fixture.track).unwrap().mixer.surround = Some(SurroundPan { x: -1.0, z, divergence, ..SurroundPan::default() });
        let position = index * 32;
        let out = block(&mut engine, &fixture, position, 32);
        for frame in 0..32 {
            let energy: f32 = out.iter().map(|channel| channel[frame].powi(2)).sum();
            assert!((energy - signal(0, position + frame).powi(2)).abs() < EPS);
            assert_eq!(out[3][frame], 0.0);
            if z == 1.0 {
                assert!(out[..6].iter().all(|channel| channel[frame].abs() < EPS));
                assert!(out[6][frame].abs() + out[7][frame].abs() > 0.01);
            } else if divergence == 1.0 {
                assert!(out[4][frame] > 0.0 && out[5][frame] > 0.0);
            } else {
                assert!((out[0][frame] - signal(0, position + frame)).abs() < EPS);
            }
        }
    }
}

#[test]
fn pan_automation_uses_timeline_grid_at_nonzero_starts_for_all_partitions() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let lane = fixture.session.track_mut(fixture.track).unwrap().lane_mut(&AutoParam::Pan(0));
    lane.set_point(0, -1.0);
    lane.set_point(128, 1.0);
    let mut reference = None;
    for partition in [37, 64, 127, 1024] {
        let mut engine = MixEngine::new(48_000.0, partition);
        engine.pan_law = PanLaw::Zero;
        let mut whole = vec![Vec::new(), Vec::new()];
        let mut position = 13;
        while position < 205 {
            let frames = partition.min(205 - position);
            let out = block(&mut engine, &fixture, position, frames);
            expect(&out, |channel, frame| {
                let at = position + frame;
                let gain = match (at / 64, channel) {
                    (0, 0) | (1, _) | (2.., 1) => 1.0,
                    _ => 0.0,
                };
                signal(0, at) * gain
            });
            for (destination, channel) in whole.iter_mut().zip(out) {
                destination.extend(channel);
            }
            position += frames;
        }
        if let Some(expected) = &reference {
            assert_eq!(&whole, expected);
        } else {
            reference = Some(whole);
        }
    }
}

#[test]
fn stereo_pan_lanes_read_modes_and_live_point_edits_update_on_one_engine() {
    let mut fixture = fixture(ChannelFormat::Stereo, ChannelFormat::Stereo);
    for (index, start, end) in [(0, -1.0, 1.0), (1, 1.0, -1.0)] {
        let lane = fixture.session.track_mut(fixture.track).unwrap().lane_mut(&AutoParam::Pan(index));
        lane.set_point(0, start);
        lane.set_point(128, end);
    }
    let mut engine = MixEngine::new(48_000.0, 32);
    expect(&block(&mut engine, &fixture, 64, 32), |_, frame| signal(0, 64 + frame) + signal(1, 64 + frame));
    fixture.session.track_mut(fixture.track).unwrap().mixer.automation_mode = AutomationMode::Off;
    expect(&block(&mut engine, &fixture, 96, 32), |channel, frame| signal(channel, 96 + frame));
    let strip = fixture.session.track_mut(fixture.track).unwrap();
    strip.mixer.automation_mode = AutomationMode::Write;
    strip.lane_mut(&AutoParam::Pan(0)).set_point(128, -1.0);
    strip.lane_mut(&AutoParam::Pan(1)).set_point(128, -1.0);
    expect(&block(&mut engine, &fixture, 128, 32), |channel, frame| if channel == 0 { signal(0, 128 + frame) + signal(1, 128 + frame) } else { 0.0 });
}

#[test]
fn sends_continue_before_none_hardware_or_missing_output_routes() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Stereo);
    let bus = fixture.session.add_bus("Send return", ChannelFormat::Stereo);
    aux(&mut fixture.session, bus, ChannelFormat::Stereo, [-1.0, 1.0]);
    let strip = fixture.session.track_mut(fixture.track).unwrap();
    strip.mixer.pan = vec![1.0];
    let mut send = SendSlot::new(Route::Bus(bus));
    send.level_db = 0.0;
    send.pan = -1.0;
    strip.mixer.sends[0] = Some(send);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, route) in [Route::None, Route::Hardware("Unavailable".into()), Route::Bus(BusId(999_999)), Route::Main].into_iter().enumerate() {
        let main = route == Route::Main;
        fixture.session.track_mut(fixture.track).unwrap().mixer.output = route;
        let position = index * 32;
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| if channel == 0 || main { signal(0, position + frame) } else { 0.0 });
    }
}

#[test]
fn disconnected_outputs_preserve_builtin_delay_tail_until_reconnect() {
    for disconnected in [Route::None, Route::Bus(BusId(999_999))] {
        let mut impulse = vec![0.0; 8192];
        impulse[0] = 0.5;
        let mut fixture = fixture_with_data(ChannelFormat::Mono, ChannelFormat::Stereo, vec![impulse]);
        let strip = fixture.session.track_mut(fixture.track).unwrap();
        strip.mixer.pan = vec![-1.0];
        strip.playlist_mut().unwrap().clips[0].length = 1;
        let mut delay = Insert::new("mod_delay");
        delay.params.extend([("time".into(), 1.0), ("feedback".into(), 50.0), ("mix".into(), 100.0), ("mod_depth".into(), 0.0)]);
        strip.mixer.inserts[0] = Some(delay);
        let reference = Fixture { session: fixture.session.clone(), track: fixture.track, source: fixture.source };
        fixture.session.track_mut(fixture.track).unwrap().mixer.output = disconnected;
        let mut control = MixEngine::new(48_000.0, 32);
        let mut engine = MixEngine::new(48_000.0, 32);
        block(&mut control, &reference, 0, 32);
        expect(&block(&mut engine, &fixture, 0, 32), |_, _| 0.0);
        fixture.session.track_mut(fixture.track).unwrap().mixer.output = Route::Main;
        let mut tail = 0.0;
        for position in (32..8192).step_by(32) {
            let wanted = block(&mut control, &reference, position, 32);
            let actual = block(&mut engine, &fixture, position, 32);
            assert_eq!(actual, wanted);
            tail += actual[0].iter().map(|value| value.abs()).sum::<f32>();
        }
        assert!(tail > 0.01, "the preserved processor must emit an audible delayed tail");
    }
}

#[test]
fn duplicate_track_ids_keep_the_legacy_effective_strip_width() {
    let mut fixture = fixture_with_data(ChannelFormat::Lcrs, ChannelFormat::Stereo, data(10));
    let mut duplicate = fixture.session.track(fixture.track).unwrap().clone();
    duplicate.format = ChannelFormat::Atmos702;
    duplicate.playlist_mut().unwrap().clips.clear();
    fixture.session.tracks.push(duplicate);
    let mut engine = MixEngine::new(48_000.0, 32);
    for (index, width) in [9, 10].into_iter().enumerate() {
        fixture.session.tracks.last_mut().unwrap().format = if width == 9 { ChannelFormat::Atmos702 } else { ChannelFormat::Atmos712 };
        let position = index * 32;
        expect(&block(&mut engine, &fixture, position, 32), |channel, frame| {
            signal(channel, position + frame) + H * (2..width).map(|source| signal(source, position + frame)).sum::<f32>()
        });
    }
}

#[test]
fn duplicate_bus_ids_keep_the_last_effective_buffer_layout() {
    let mut fixture = fixture(ChannelFormat::Mono, ChannelFormat::Lcrs);
    let bus = fixture.session.add_bus("Declared Quad", ChannelFormat::Quad);
    fixture.session.busses.push(Bus { id: bus, name: "Effective LCRS".into(), format: ChannelFormat::Lcrs });
    let strip = fixture.session.track_mut(fixture.track).unwrap();
    strip.mixer.surround = Some(SurroundPan::default());
    strip.mixer.output = Route::Bus(bus);
    aux(&mut fixture.session, bus, ChannelFormat::Lcrs, [-1.0, 1.0]);
    let mut engine = MixEngine::new(48_000.0, 32);
    expect(&block(&mut engine, &fixture, 0, 32), |channel, frame| if channel == 2 { signal(0, frame) } else { 0.0 });
}

#[test]
fn oversized_internal_outputs_clamp_and_invalid_main_formats_fall_back() {
    let fixture = fixture(ChannelFormat::Ambisonics(7), ChannelFormat::Ambisonics(3));
    let mut engine = MixEngine::new(48_000.0, 32);
    let out = block(&mut engine, &fixture, 0, 32);
    assert_eq!(out.len(), MAX_CHANNELS);
    expect(&out, signal);
    let mut mono = fixture_with_data(ChannelFormat::Mono, ChannelFormat::Ambisonics(7), data(1));
    let mut engine = MixEngine::new(48_000.0, 32);
    let out = block(&mut engine, &mono, 0, 32);
    assert_eq!(out.len(), 2);
    expect(&out, |_, frame| signal(0, frame) * 10.0f32.powf(-3.0 / 20.0));
    mono.session.outputs[0].format = ChannelFormat::Stereo;
    expect(&block(&mut engine, &mono, 32, 32), |_, frame| signal(0, 32 + frame) * 10.0f32.powf(-3.0 / 20.0));
}
