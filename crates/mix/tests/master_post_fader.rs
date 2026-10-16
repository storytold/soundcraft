//! Independent signal-order references; no hosted plugin inventory or audio device.
use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range};
use soundcraft_model::{
    AutoParam, AutomationMode, ChannelFormat, Clip, Insert, MasterInsertOrder, Route, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Range;
use std::sync::Arc;

const LEN: usize = 2049;

fn signal(ch: usize, n: usize) -> f32 {
    let level = match n % 83 {
        0..=30 => 0.75,
        31..=59 => 1.25,
        _ => 0.125,
    };
    (level + ch as f32 * 0.03125) * if (n / 11 + ch).is_multiple_of(2) { 1.0 } else { -1.0 }
}
fn input(channels: usize, start: usize, frames: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|ch| (start..start + frames).map(|n| signal(ch, n)).collect()).collect()
}
fn fixture(format: ChannelFormat) -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    s.outputs[0].format = format;
    let audio = s.add_track(TrackKind::Audio, format, Some("Original"));
    let source = SourceId(900);
    s.pool.insert(source, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: input(format.channels(), 0, LEN) })));
    let id = s.new_clip_id();
    s.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "Original synthetic", source, 0, 0, LEN as i64));
    // The Master buffer follows the main format, including when the declared track differs.
    let master = s.add_track(TrackKind::Master, ChannelFormat::Mono, Some("Master"));
    s.track_mut(master).unwrap().mixer.master_insert_order = MasterInsertOrder::PostFader;
    (s, audio, master)
}
fn insert(s: &mut Session, track: TrackId, slot: usize, threshold: f32) {
    let mut ins = Insert::new("compressor");
    ins.params.extend([
        ("threshold".into(), threshold),
        ("attack".into(), 0.01),
        ("release".into(), 5.0),
        ("knee".into(), 0.0),
        ("makeup".into(), 0.0),
        ("limiter".into(), 1.0),
        ("lookahead".into(), 0.0),
        ("sc_hpf_on".into(), 0.0),
        ("mix".into(), 100.0),
    ]);
    s.track_mut(track).unwrap().mixer.inserts[slot] = Some(ins);
}
fn gain(db: f64) -> f64 {
    if db <= -144.0 { 0.0 } else { 10f64.powf(db / 20.0) }
}
fn scaled(input: &[Vec<f32>], gains: &[f64]) -> Vec<Vec<f32>> {
    input.iter().map(|ch| ch.iter().zip(gains).map(|(&x, &g)| (f64::from(x) * g) as f32).collect()).collect()
}
// Independent log-domain limiter recurrence: never calls production DSP/automation helpers.
fn limited(input: &[Vec<f32>], gains: &[f64], threshold: f64, reduction: &mut f64) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; gains.len()]; input.len()];
    for (n, &g) in gains.iter().enumerate() {
        let level = input.iter().map(|ch| (f64::from(ch[n]) * g).abs()).fold(0.0, f64::max);
        let db = if level == 0.0 { -144.0 } else { (20.0 * level.log10()).max(-144.0) };
        let target = (db - threshold).max(0.0) * (1.0 - 1.0 / 1000.0);
        let time_ms = if target > *reduction { 0.01 } else { 5.0 };
        let coefficient = (-1.0f64 / (time_ms * 0.001 * 48_000.0)).exp();
        *reduction = target + (*reduction - target) * coefficient;
        let attenuation = 10f64.powf(-*reduction / 20.0);
        for (ch, values) in input.iter().enumerate() {
            out[ch][n] = (f64::from(values[n]) * g * attenuation) as f32;
        }
    }
    out
}
fn check(actual: &[Vec<f32>], expected: &[Vec<f32>], channels: usize, frames: usize) {
    assert_eq!(actual.len(), channels);
    assert_eq!(expected.len(), channels);
    for (ch, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.len(), frames);
        assert_eq!(expected.len(), frames);
        for (n, (&a, &e)) in actual.iter().zip(expected).enumerate() {
            assert!(a.is_finite() && e.is_finite());
            assert!((a - e).abs() < 3e-6, "channel {ch}, frame {n}: {a} vs {e}");
        }
    }
}
fn audible(out: &[Vec<f32>]) {
    assert!(out.iter().flatten().any(|x| x.abs() > 0.05));
}
fn chunks(engine: &mut MixEngine, s: &Session, start: i64, frames: usize, sizes: &[usize]) -> Vec<Vec<f32>> {
    let channels = s.main_format().channels();
    let mut out = vec![Vec::new(); channels];
    let mut done = 0;
    let mut k = 0;
    while done < frames {
        let n = sizes[k % sizes.len()].min(frames - done);
        let mut block = vec![vec![0.0; n]; channels];
        engine.render(s, start + done as i64, n, &mut block);
        for (out, block) in out.iter_mut().zip(block) {
            out.extend(block);
        }
        done += n;
        k += 1;
    }
    out
}

#[test]
fn nonlinear_master_matches_gain_then_limiter_and_differs_from_old_order() {
    let (mut s, _, master) = fixture(ChannelFormat::Stereo);
    insert(&mut s, master, 0, -6.0);
    let source = input(2, 0, LEN);
    for db in [-144.0, -6.0, 0.0, 6.0] {
        s.track_mut(master).unwrap().mixer.volume_db = db as f32;
        let expected = limited(&source, &vec![gain(db); LEN], -6.0, &mut 0.0);
        for block in [16, 37, 100, 512, 1024] {
            let actual = render_range(&s, Range::new(0, LEN as i64), block);
            check(&actual, &expected, 2, LEN);
            if db > -144.0 {
                audible(&actual);
            } else {
                assert!(actual.iter().flatten().all(|x| *x == 0.0));
            }
        }
        if db == -6.0 {
            let before_fader = limited(&source, &vec![1.0; LEN], -6.0, &mut 0.0);
            let wrong = scaled(&before_fader, &vec![gain(db); LEN]);
            assert!(expected.iter().flatten().zip(wrong.iter().flatten()).any(|(a, b)| (a - b).abs() > 0.1));
        }
    }
}

#[test]
fn serial_slots_active_bypass_and_inactive_master_preserve_order() {
    let (mut s, _, master) = fixture(ChannelFormat::Stereo);
    let source = input(2, 0, LEN);
    s.track_mut(master).unwrap().mixer.volume_db = -3.0;
    insert(&mut s, master, 0, -6.0);
    insert(&mut s, master, 9, -12.0);
    let first = limited(&source, &vec![gain(-3.0); LEN], -6.0, &mut 0.0);
    let both = limited(&first, &vec![1.0; LEN], -12.0, &mut 0.0);
    check(&render_range(&s, Range::new(0, LEN as i64), 37), &both, 2, LEN);
    audible(&both);
    s.track_mut(master).unwrap().mixer.inserts[0].as_mut().unwrap().bypass = true;
    let only_j = limited(&source, &vec![gain(-3.0); LEN], -12.0, &mut 0.0);
    check(&render_range(&s, Range::new(0, LEN as i64), 100), &only_j, 2, LEN);
    s.track_mut(master).unwrap().mixer.inserts[9].as_mut().unwrap().active = false;
    check(&render_range(&s, Range::new(0, LEN as i64), 512), &scaled(&source, &vec![gain(-3.0); LEN]), 2, LEN);
    s.track_mut(master).unwrap().inactive = true;
    assert_eq!(render_range(&s, Range::new(0, LEN as i64), 37), source);
}

#[test]
fn ordinary_audio_and_aux_inserts_remain_before_their_faders() {
    for on_aux in [false, true] {
        let (mut s, audio, master) = fixture(ChannelFormat::Stereo);
        s.track_mut(master).unwrap().inactive = true;
        let target = if on_aux {
            let bus = s.add_bus("Original bus", ChannelFormat::Stereo);
            let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
            s.track_mut(audio).unwrap().mixer.output = Route::Bus(bus);
            s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
            aux
        } else {
            audio
        };
        s.track_mut(target).unwrap().mixer.volume_db = -6.0;
        insert(&mut s, target, 0, -6.0);
        let pre = limited(&input(2, 0, LEN), &vec![1.0; LEN], -6.0, &mut 0.0);
        let expected = scaled(&pre, &vec![gain(-6.0); LEN]);
        let actual = render_range(&s, Range::new(0, LEN as i64), 37);
        check(&actual, &expected, 2, LEN);
        audible(&actual);
    }
}

#[test]
fn multiple_masters_main_widths_and_output_meters_keep_current_cascade() {
    for format in [ChannelFormat::Stereo, ChannelFormat::Surround51, ChannelFormat::Atmos916] {
        let (mut s, _, master) = fixture(format);
        let channels = format.channels();
        let source = input(channels, 0, LEN);
        let unchanged = render_range(&s, Range::new(0, LEN as i64), 37);
        assert_eq!(unchanged, source);
        s.track_mut(master).unwrap().mixer.volume_db = -6.0;
        insert(&mut s, master, 0, -6.0);
        let next = s.add_track(TrackKind::Master, ChannelFormat::Mono, Some("Second"));
        s.track_mut(next).unwrap().mixer.volume_db = 3.0;
        s.track_mut(next).unwrap().mixer.master_insert_order = MasterInsertOrder::PostFader;
        insert(&mut s, next, 9, -12.0);
        let first = limited(&source, &vec![gain(-6.0); LEN], -6.0, &mut 0.0);
        let expected = limited(&first, &vec![gain(3.0); LEN], -12.0, &mut 0.0);
        let mut mix = MixEngine::new(48_000.0, LEN);
        let actual = chunks(&mut mix, &s, 0, LEN, &[LEN]);
        check(&actual, &expected, channels, LEN);
        audible(&actual);
        for (id, reference) in [(master, &first), (next, &expected)] {
            let meter = mix.meters.get(&id).unwrap();
            assert_eq!(meter.peaks.len(), channels);
            for (actual, samples) in meter.peaks.iter().zip(reference) {
                let peak = samples.iter().map(|v| v.abs()).fold(0.0, f32::max);
                assert!((actual - peak).abs() < 3e-6);
            }
        }
        assert_eq!(mix.main_meter.peaks, mix.meters.get(&next).unwrap().peaks);
    }
}

#[test]
fn next_block_volume_knot_does_not_ramp_the_preceding_master_block() {
    let (mut s, _, master) = fixture(ChannelFormat::Stereo);
    let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
    for (at, value) in [(0, 0.0), (99, 0.0), (100, -6.0), (199, -6.0)] {
        lane.set_point(at, value);
    }
    let mut mix = MixEngine::new(48_000.0, 100);
    let actual = chunks(&mut mix, &s, 0, 200, &[100]);
    let gains: Vec<f64> = (0..200).map(|n| if n < 100 { 1.0 } else { gain(-6.0) }).collect();
    check(&actual, &scaled(&input(2, 0, 200), &gains), 2, 200);
    assert_eq!(actual[0][99], signal(0, 99));
    assert!((actual[0][100] - (f64::from(signal(0, 100)) * gain(-6.0)) as f32).abs() < 1e-7);
}

const CURVE: [(i64, f64); 7] = [(0, 0.0), (100, -6.0), (101, 3.0), (137, -12.0), (263, 0.0), (264, -6.0), (2048, -6.0)];
fn db_at(at: i64) -> f64 {
    if at <= CURVE[0].0 {
        return CURVE[0].1;
    }
    for pair in CURVE.windows(2) {
        if at < pair[1].0 {
            return pair[0].1 + (pair[1].1 - pair[0].1) * (at - pair[0].0) as f64 / (pair[1].0 - pair[0].0) as f64;
        }
    }
    CURVE[6].1
}
fn endpoint_gains(start: i64, frames: usize, sizes: &[usize]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut done = 0;
    let mut k = 0;
    while done < frames {
        let n = sizes[k % sizes.len()].min(frames - done);
        let first = gain(db_at(start + done as i64));
        let last = gain(db_at(start + done as i64 + n as i64 - 1));
        for i in 0..n {
            out.push(if n == 1 { first } else { first + (last - first) * i as f64 / (n - 1) as f64 });
        }
        done += n;
        k += 1;
    }
    out
}

#[test]
fn automated_master_matches_declared_endpoint_reference_for_each_chunk_schedule() {
    for sizes in [vec![1], vec![37], vec![64], vec![100], vec![512], vec![1024], vec![1, 37, 64, 3, 100, 19]] {
        let (mut s, _, master) = fixture(ChannelFormat::Stereo);
        insert(&mut s, master, 0, -6.0);
        let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
        for &(at, value) in &CURVE {
            lane.set_point(at, value as f32);
        }
        let mut mix = MixEngine::new(48_000.0, 1024);
        let actual = chunks(&mut mix, &s, 0, LEN, &sizes);
        let expected = limited(&input(2, 0, LEN), &endpoint_gains(0, LEN, &sizes), -6.0, &mut 0.0);
        check(&actual, &expected, 2, LEN);
        audible(&actual);
        // Interior knots can still make endpoint interpolation depend on the callback partition.
        // This asserts its explicit reference, not complete sample-exact Volume automation.
    }
}

#[test]
fn master_zero_one_frame_and_saturating_endpoint_preserve_volume_reader_policy() {
    let (mut s, audio, master) = fixture(ChannelFormat::Stereo);
    s.track_mut(audio).unwrap().mixer.input_monitor = true;
    s.track_mut(master).unwrap().mixer.volume_db = -3.0;
    let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
    lane.enabled = false;
    lane.set_point(i64::MAX - 1, -6.0);
    lane.set_point(i64::MAX, -12.0);
    for mode in AutomationMode::ALL {
        s.track_mut(master).unwrap().mixer.automation_mode = mode;
        let db = if matches!(mode, AutomationMode::Off | AutomationMode::Write) { -3.0 } else { -12.0 };
        let mut mix = MixEngine::new(48_000.0, 16);
        mix.input = vec![vec![0.75; 16], vec![-0.375; 16]];
        let mut untouched = vec![vec![7.0; 4]; 2];
        mix.render(&s, i64::MAX, 0, &mut untouched);
        assert_eq!(untouched, vec![vec![7.0; 4]; 2]);
        for frames in [1, 16] {
            let mut actual = vec![vec![0.0; frames]; 2];
            mix.render(&s, i64::MAX, frames, &mut actual);
            let expected = scaled(&[vec![0.75; frames], vec![-0.375; frames]], &vec![gain(db); frames]);
            check(&actual, &expected, 2, frames);
            audible(&actual);
        }
    }
    // Missing/empty lanes still fall back to static volume, without a global reader migration.
    for empty in [false, true] {
        s.track_mut(master).unwrap().automation.clear();
        if empty {
            s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
        }
        let mut mix = MixEngine::new(48_000.0, 16);
        mix.input = vec![vec![0.75; 16], vec![-0.375; 16]];
        let mut actual = vec![vec![0.0; 16]; 2];
        mix.render(&s, i64::MAX, 16, &mut actual);
        check(&actual, &scaled(&[vec![0.75; 16], vec![-0.375; 16]], &[gain(-3.0); 16]), 2, 16);
    }
}

#[test]
fn retained_limiter_seek_loop_and_reset_match_their_own_state_reference() {
    let (mut s, _, master) = fixture(ChannelFormat::Stereo);
    s.track_mut(master).unwrap().mixer.volume_db = -6.0;
    insert(&mut s, master, 0, -6.0);
    let mut mix = MixEngine::new(48_000.0, 512);
    let mut reduction = 0.0;
    for (start, frames, reset) in [(0, 512, false), (101, 76, false), (101, 76, false), (37, 264, true), (37, 264, false)] {
        if reset {
            mix.reset();
            reduction = 0.0;
        }
        let actual = chunks(&mut mix, &s, start, frames, &[37, 1, 19]);
        let expected = limited(&input(2, start as usize, frames), &vec![gain(-6.0); frames], -6.0, &mut reduction);
        check(&actual, &expected, 2, frames);
        audible(&actual);
    }
    let mut fresh = MixEngine::new(48_000.0, 37);
    let callback = chunks(&mut fresh, &s, 0, LEN, &[37]);
    let offline = render_range(&s, Range::new(0, LEN as i64), 37);
    check(&callback, &offline, 2, LEN);
    assert_eq!(callback, offline);
}

#[test]
fn omitted_master_order_preserves_legacy_nonlinear_signal_and_serialization() {
    let (mut s, _, master) = fixture(ChannelFormat::Stereo);
    s.track_mut(master).unwrap().mixer.master_insert_order = MasterInsertOrder::PreFader;
    insert(&mut s, master, 0, -6.0);
    let document = s.to_json().unwrap();
    assert!(!document.contains("\"master_insert_order\""));
    let mut legacy = Session::from_json(&document).unwrap();
    legacy.pool = s.pool.clone();
    assert_eq!(legacy.track(master).unwrap().mixer.master_insert_order, MasterInsertOrder::PreFader);
    assert!(!legacy.to_json().unwrap().contains("\"master_insert_order\""));
    let source = input(2, 0, LEN);
    let limited_before_fader = limited(&source, &vec![1.0; LEN], -6.0, &mut 0.0);
    for db in [-6.0, 0.0, 6.0] {
        legacy.track_mut(master).unwrap().mixer.volume_db = db as f32;
        let expected = scaled(&limited_before_fader, &vec![gain(db); LEN]);
        for block in [1, 37, 100, 512] {
            check(&render_range(&legacy, Range::new(0, LEN as i64), block), &expected, 2, LEN);
        }
        if db == -6.0 {
            let post_order = limited(&source, &vec![gain(db); LEN], -6.0, &mut 0.0);
            assert!(expected.iter().flatten().zip(post_order.iter().flatten()).any(|(a, b)| (a - b).abs() > 0.1));
        }
    }
}

fn legacy_endpoint_gains(start: i64, frames: usize, sizes: &[usize]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut done = 0;
    let mut k = 0;
    while done < frames {
        let n = sizes[k % sizes.len()].min(frames - done);
        let first = gain(db_at(start + done as i64));
        let next = gain(db_at(start + done as i64 + n as i64));
        for i in 0..n {
            out.push(first + (next - first) * i as f64 / n as f64);
        }
        done += n;
        k += 1;
    }
    out
}

#[test]
fn legacy_automation_retains_next_sample_full_block_gain_law() {
    for sizes in [vec![1], vec![37], vec![100], vec![512], vec![1, 37, 64, 3, 100, 19]] {
        let (mut s, _, master) = fixture(ChannelFormat::Stereo);
        s.track_mut(master).unwrap().mixer.master_insert_order = MasterInsertOrder::PreFader;
        insert(&mut s, master, 0, -6.0);
        let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
        for &(at, value) in &CURVE {
            lane.set_point(at, value as f32);
        }
        let before_gain = limited(&input(2, 0, LEN), &vec![1.0; LEN], -6.0, &mut 0.0);
        let expected = scaled(&before_gain, &legacy_endpoint_gains(0, LEN, &sizes));
        let mut mix = MixEngine::new(48_000.0, 1024);
        let actual = chunks(&mut mix, &s, 0, LEN, &sizes);
        check(&actual, &expected, 2, LEN);
        audible(&actual);
    }
}

#[test]
fn legacy_extreme_endpoints_preserve_reader_modes_without_overflow() {
    let (mut s, audio, master) = fixture(ChannelFormat::Stereo);
    s.track_mut(audio).unwrap().mixer.input_monitor = true;
    s.track_mut(master).unwrap().mixer.master_insert_order = MasterInsertOrder::PreFader;
    s.track_mut(master).unwrap().mixer.volume_db = -3.0;
    let lane = s.track_mut(master).unwrap().lane_mut(&AutoParam::Volume);
    lane.enabled = false;
    lane.set_point(i64::MAX - 1, -6.0);
    lane.set_point(i64::MAX, -12.0);
    for mode in AutomationMode::ALL {
        s.track_mut(master).unwrap().mixer.automation_mode = mode;
        let db = if matches!(mode, AutomationMode::Off | AutomationMode::Write) { -3.0 } else { -12.0 };
        let mut mix = MixEngine::new(48_000.0, 16);
        mix.input = vec![vec![0.75; 16], vec![-0.375; 16]];
        let mut untouched = vec![vec![7.0; 4]; 2];
        mix.render(&s, i64::MAX, 0, &mut untouched);
        assert_eq!(untouched, vec![vec![7.0; 4]; 2]);
        for frames in [1, 16] {
            let mut actual = vec![vec![0.0; frames]; 2];
            mix.render(&s, i64::MAX, frames, &mut actual);
            let expected = scaled(&[vec![0.75; frames], vec![-0.375; frames]], &vec![gain(db); frames]);
            check(&actual, &expected, 2, frames);
        }
    }
}

#[test]
fn non_master_stored_order_does_not_change_ordinary_insert_processing() {
    for on_aux in [false, true] {
        let (mut s, audio, master) = fixture(ChannelFormat::Stereo);
        s.track_mut(master).unwrap().inactive = true;
        let target = if on_aux {
            let bus = s.add_bus("Bus", ChannelFormat::Stereo);
            let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
            s.track_mut(audio).unwrap().mixer.output = Route::Bus(bus);
            s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
            aux
        } else {
            audio
        };
        s.track_mut(target).unwrap().mixer.master_insert_order = MasterInsertOrder::PostFader;
        s.track_mut(target).unwrap().mixer.volume_db = -6.0;
        insert(&mut s, target, 0, -6.0);
        let before_gain = limited(&input(2, 0, LEN), &vec![1.0; LEN], -6.0, &mut 0.0);
        let expected = scaled(&before_gain, &vec![gain(-6.0); LEN]);
        check(&render_range(&s, Range::new(0, LEN as i64), 37), &expected, 2, LEN);
    }
}

#[test]
fn legacy_and_post_fader_masters_keep_track_order_cascade() {
    let (mut s, _, first) = fixture(ChannelFormat::Stereo);
    s.track_mut(first).unwrap().mixer.master_insert_order = MasterInsertOrder::PreFader;
    s.track_mut(first).unwrap().mixer.volume_db = -6.0;
    insert(&mut s, first, 0, -6.0);
    let second = s.add_track(TrackKind::Master, ChannelFormat::Mono, None);
    s.track_mut(second).unwrap().mixer.master_insert_order = MasterInsertOrder::PostFader;
    s.track_mut(second).unwrap().mixer.volume_db = 3.0;
    insert(&mut s, second, 9, -12.0);
    let first_reference = scaled(&limited(&input(2, 0, LEN), &vec![1.0; LEN], -6.0, &mut 0.0), &vec![gain(-6.0); LEN]);
    let expected = limited(&first_reference, &vec![gain(3.0); LEN], -12.0, &mut 0.0);
    check(&render_range(&s, Range::new(0, LEN as i64), 37), &expected, 2, LEN);
}
