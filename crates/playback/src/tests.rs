use super::*;

/// A stream that builds; `starts` says whether the device lets it play (it can refuse after a
/// reconnect).
struct Fake {
    starts: bool,
}

impl cpal::traits::StreamTrait for Fake {
    /// Play reports the device's refusal when `starts` is false.
    fn play(&self) -> Result<(), cpal::Error> {
        if self.starts { Ok(()) } else { Err(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable)) }
    }
    /// Pause always succeeds on the fake stream.
    fn pause(&self) -> Result<(), cpal::Error> {
        Ok(())
    }
    /// The fake stream has no clock and no fixed buffer; neither is used by these tests.
    fn buffer_size(&self) -> Result<cpal::FrameCount, cpal::Error> {
        Ok(0)
    }
    /// The fake stream has no usable clock; these tests do not read it.
    fn now(&self) -> cpal::StreamInstant {
        cpal::StreamInstant::from_nanos(0)
    }
}

#[test]
fn a_stream_that_will_not_start_is_reported_as_failed() {
    assert!(!start_stream(&Fake { starts: false }));
    assert!(start_stream(&Fake { starts: true }));
}

#[test]
fn the_first_config_that_starts_wins_and_later_ones_are_not_tried() {
    let mut tried = Vec::new();
    let won = first_started(
        [1u32, 2, 3],
        |&n| {
            tried.push(n);
            Ok::<_, String>(Fake { starts: n != 1 })
        },
        start_stream,
    );
    assert_eq!(won.map(|(n, _)| n), Some(2));
    assert_eq!(tried, vec![1, 2]);
}

#[test]
fn a_config_that_fails_to_build_falls_through_to_the_next() {
    let won = first_started([1u32, 2], |&n| if n == 1 { Err("no such buffer size".to_string()) } else { Ok(Fake { starts: true }) }, start_stream);
    assert_eq!(won.map(|(n, _)| n), Some(2));
}

/// Build a cpal configuration range for tests with a 64..1024-frame buffer.
fn range(channels: u16, min: u32, max: u32, format: cpal::SampleFormat) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(channels, min, max, cpal::SupportedBufferSize::Range { min: 64, max: 1024 }, format)
}

/// Rates are tried in order, so a stale configured rate falls through to the session rate; when no requested rate
/// is offered the caller uses the device default and resamples.
#[test]
fn output_config_uses_the_first_requested_rate_the_device_offers() {
    let configs = [
        range(2, 44_100, 48_000, cpal::SampleFormat::I16),
        range(2, 44_100, 96_000, cpal::SampleFormat::F32),
        range(8, 44_100, 96_000, cpal::SampleFormat::F32),
    ];
    let (config, buffers, format) = pick_output_config(&configs, &[96_000, 48_000], 8).unwrap();
    assert_eq!((config.sample_rate, config.channels, config.buffer_size), (96_000, 8, cpal::BufferSize::Default));
    assert_eq!(format, cpal::SampleFormat::F32);
    assert_eq!(buffers, cpal::SupportedBufferSize::Range { min: 64, max: 1024 });
    // 192 kHz is not offered: the session's 48 kHz is tried next.
    let (config, _, format) = pick_output_config(&configs, &[192_000, 48_000], 8).unwrap();
    assert_eq!((config.sample_rate, config.channels, format), (48_000, 8, cpal::SampleFormat::F32));
    // No requested rate is offered: the caller uses the device's default config and resamples.
    assert!(pick_output_config(&configs, &[192_000], 8).is_none());
}

/// At a supported rate, a preferred sample format wins over more channels; output stays between stereo and the
/// requested channel cap, and mono-only or wider devices take the default path.
#[test]
fn output_config_prefers_format_over_channels_and_needs_stereo_within_the_cap() {
    let configs = [range(8, 44_100, 48_000, cpal::SampleFormat::I16), range(2, 44_100, 48_000, cpal::SampleFormat::F32)];
    let (config, _, format) = pick_output_config(&configs, &[48_000], 8).unwrap();
    assert_eq!((config.channels, format), (2, cpal::SampleFormat::F32), "a preferred format wins over more channels");
    let (config, _, _) = pick_output_config(&configs, &[48_000], 2).unwrap();
    assert_eq!(config.channels, 2, "the channel cap applies");
    let mono = [range(1, 44_100, 48_000, cpal::SampleFormat::F32)];
    assert!(pick_output_config(&mono, &[48_000], 8).is_none(), "a mono-only device takes the default config path");
    let surround = [range(6, 44_100, 48_000, cpal::SampleFormat::F32)];
    assert!(pick_output_config(&surround, &[48_000], 2).is_none(), "a wider device is not narrowed here");
    assert_eq!(pick_output_config(&surround, &[48_000], 6).map(|(c, _, _)| c.channels), Some(6));
}

#[test]
fn no_config_that_starts_yields_none_so_the_caller_falls_back_to_a_silent_clock() {
    let mut tried = 0;
    let won = first_started(
        [1u32, 2],
        |_| {
            tried += 1;
            Ok::<_, String>(Fake { starts: false })
        },
        start_stream,
    );
    assert!(won.is_none());
    assert_eq!(tried, 2);
}

#[test]
fn reusable_meter_snapshot_updates_and_drops_stale_tracks() {
    let keep = TrackId(1);
    let stale = TrackId(2);
    let mut source = MeterSnapshot::default();
    source.tracks.insert(keep, StripMeter { peak: [0.5, 0.25], peaks: vec![0.5, 0.25], gain_reduction_db: 3.0, ..StripMeter::default() });
    source.main.peaks = vec![0.75, 0.5];
    let mut out = MeterSnapshot::default();
    out.tracks.insert(stale, StripMeter::default());
    copy_meter_snapshot(&source, &mut out);
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[&keep], source.tracks[&keep]);
    assert_eq!(out.main, source.main);
}

/// A mono track playing a quiet signal with one short spike in the block starting at
/// `spike_at`, and an audio thread ready to play it.
fn spike_session(spike_at: usize) -> (Arc<Shared>, AudioState, std::sync::mpsc::Sender<Cmd>, TrackId) {
    use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let mut x = vec![0.01f32; 48_000];
    x[spike_at..spike_at + 32].fill(0.9);
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![x] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    (shared, st, tx, t)
}

#[test]
fn a_spike_in_one_block_among_quiet_ones_reaches_the_ui() {
    // Eight 512-frame blocks per callback, as between two UI polls; the spike is in the 3rd.
    let (shared, mut st, _tx, t) = spike_session(2 * 512 + 100);
    let mut out = vec![0.0f32; 2 * 8 * 512];
    st.fill(&mut out, 2);
    // The last block alone (what the UI used to see) is quiet.
    assert!(st.mix.meters[&t].peak[0] < 0.05);
    let mut ui = MeterSnapshot::default();
    take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
    assert!(ui.tracks[&t].peak[0] > 0.85, "{:?}", ui.tracks[&t]);
    assert!(ui.tracks[&t].peaks[0] > 0.85);
    assert!(ui.main.peak[0] > 0.5 && ui.main.peaks.iter().all(|p| *p > 0.5), "{:?}", ui.main);
    // Taking starts a new period: the next quiet blocks read quiet.
    st.fill(&mut out, 2);
    take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
    assert!(ui.tracks[&t].peak[0] < 0.05 && ui.main.peak[0] < 0.05);
    // No block since the last take: the reading stays rather than dropping to silence.
    take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
    assert!(ui.tracks[&t].peak[0] > 0.005);
}

#[test]
fn a_spike_rendered_while_the_ui_holds_the_meters_arrives_later() {
    let (shared, mut st, _tx, t) = spike_session(100);
    let mut out = vec![0.0f32; 2 * 512];
    {
        let _held = shared.meters.lock().unwrap();
        st.fill(&mut out, 2);
    }
    st.fill(&mut out, 2);
    let mut ui = MeterSnapshot::default();
    take_meter_snapshot(&mut shared.meters.lock().unwrap(), &mut ui);
    assert!(ui.tracks[&t].peak[0] > 0.85, "{:?}", ui.tracks[&t]);
}

#[test]
fn stopping_zeroes_meters_in_place_and_removed_tracks_are_pruned() {
    let (shared, mut st, tx, t) = spike_session(100);
    let mut out = vec![0.0f32; 2 * 512];
    st.fill(&mut out, 2);
    tx.send(Cmd::Stop).unwrap();
    st.fill(&mut out, 2);
    {
        let m = shared.meters.lock().unwrap();
        let e = m.tracks.get(&t).expect("kept while stopped, so playing again does not allocate it");
        assert!(e.peak == [0.0; 2] && e.peaks.iter().all(|p| *p == 0.0) && e.peaks.len() == 1, "{e:?}");
        assert!(m.fresh, "the UI sees the silence");
    }
    // The track is deleted: the UI side works out which tracks went and says so.
    let mut side = Side::new(48_000.0, channel().1);
    side.gone_tracks(&st.session);
    let empty = soundcraft_model::Session::default();
    let gone = side.gone_tracks(&empty);
    assert_eq!(gone, vec![t]);
    tx.send(Cmd::Session(Arc::new(empty), Vec::new())).unwrap();
    tx.send(Cmd::Forget(gone)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    st.fill(&mut out, 2);
    assert!(!shared.meters.lock().unwrap().tracks.contains_key(&t));
    assert!(!st.unsent.tracks.contains_key(&t) && !st.mix.meters.contains_key(&t));
}

#[test]
fn a_take_without_new_blocks_still_drops_deleted_tracks() {
    let gone = TrackId(7);
    let mut source = MeterSnapshot::default();
    source.accumulate(&StripMeter::default(), &HashMap::from([(gone, StripMeter::default())]));
    let mut ui = MeterSnapshot::default();
    take_meter_snapshot(&mut source, &mut ui);
    source.forget(&[gone]);
    take_meter_snapshot(&mut source, &mut ui);
    assert!(ui.tracks.is_empty());
}

#[test]
fn a_channel_count_change_takes_the_new_stereo_peaks() {
    let mut acc = StripMeter { peak: [0.9, 0.9], peaks: vec![0.9], ..StripMeter::default() };
    acc.accumulate(&StripMeter { peak: [0.9, 0.0], peaks: vec![0.9, 0.0], ..StripMeter::default() });
    assert_eq!(acc.peak, [0.9, 0.0]);
}

#[test]
fn gain_reduction_keeps_its_maximum_until_taken() {
    let mut acc = StripMeter::default();
    for gr in [1.0, 9.0, 2.0, f32::NAN] {
        acc.accumulate(&StripMeter { peak: [0.1, 0.1], peaks: vec![0.1], gain_reduction_db: gr, ..StripMeter::default() });
    }
    assert_eq!(acc.gain_reduction_db, 9.0);
    acc.reset_levels();
    assert_eq!((acc.gain_reduction_db, acc.peak, acc.peaks.len()), (0.0, [0.0; 2], 1));
}

#[test]
fn a_block_that_panics_goes_silent_and_is_reported_once() {
    let shared = Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(true),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    };
    let fault = Fault::default();
    let mut out = [0.5f32; 8];
    guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
    assert_eq!((out, fault.trouble(false)), ([0.25; 8], None));
    guarded_block(&fault, &shared, &mut out, |_| std::panic::panic_any("a bug in the mix"));
    guarded_block(&fault, &shared, &mut out, |o| o.fill(0.25));
    assert_eq!(out, [0.0; 8]);
    assert!(!shared.playing.load(Ordering::Relaxed));
    assert_eq!((fault.trouble(false), fault.trouble(false)), (Some(OutputTrouble::Bug), None));
    assert_eq!(fault.beats.load(Ordering::Relaxed), 3);
}

#[test]
fn a_lost_or_stalled_device_is_reported_once() {
    let ago = |seen: &mut Option<(u64, std::time::Instant)>| {
        if let Some(s) = seen {
            s.1 = std::time::Instant::now().checked_sub(STALL).unwrap();
        }
    };
    let mut seen = None;
    assert!(!stalled(&mut seen, 5, true));
    ago(&mut seen);
    assert!(stalled(&mut seen, 5, true));
    assert!(!stalled(&mut seen, 6, true), "it asked again");
    ago(&mut seen);
    assert!(!stalled(&mut seen, 6, false), "stopped, so nothing to ask for");
    assert_eq!(Fault::default().trouble(true), Some(OutputTrouble::Stalled));
    let fault = Fault::default();
    fault.gone.store(true, Ordering::Relaxed);
    assert_eq!((fault.trouble(false), fault.trouble(true)), (Some(OutputTrouble::Gone), None));
}

#[test]
fn audio_state_plays_and_stops_at_end() {
    let s = Arc::new(soundcraft_model::Session::default());
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
    let mut out = vec![0.0f32; 4096];
    st.fill(&mut out, 2);
    assert!(shared.position.load(Ordering::Relaxed) >= 1000);
    assert!(!shared.playing.load(Ordering::Relaxed));
}

#[test]
fn short_device_callbacks_render_short_mix_blocks() {
    let s = Arc::new(soundcraft_model::Session::default());
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2 * 64];
    st.fill(&mut out, 2);
    assert_eq!(shared.position.load(Ordering::Relaxed), 64);
}

fn audio_output_state(format: soundcraft_model::ChannelFormat, channels: Vec<Vec<f32>>) -> AudioState {
    use soundcraft_model::{Clip, SourceAudio, SourceId, TrackKind};
    let mut s = Session::default();
    s.edit.click = false;
    s.outputs[0].format = format;
    let t = s.add_track(TrackKind::Audio, format, None);
    let frames = channels[0].len();
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "output", SourceId(5), 0, 0, frames as i64));
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let st = AudioState::new(rx, shared, Arc::new(s), 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    st
}

#[test]
fn mono_device_averages_stereo_before_clipping() {
    use soundcraft_model::ChannelFormat;
    // Right only, left only, identical, opposite polarity, and levels that must be averaged
    // before clipping either side to the device's range.
    let left = [0.0, 0.8, 0.8, 0.8, 2.0, -2.0, 2.0];
    let right = [0.8, 0.0, 0.8, -0.8, -0.5, 0.5, 2.0];
    let expected = [0.4, 0.4, 0.8, 0.0, 0.75, -0.75, 1.0];
    for callback in [1, 37, 512] {
        let channels = [left, right].map(|pattern| (0..2048).map(|i| pattern[i % pattern.len()]).collect());
        let mut st = audio_output_state(ChannelFormat::Stereo, channels.into());
        let mut out = vec![0.0; callback];
        for block in 0..3 {
            st.fill(&mut out, 1);
            for (i, actual) in out.iter().enumerate() {
                let at = block * callback + i;
                assert!((actual - expected[at % expected.len()]).abs() < 1e-6, "callback {callback}, sample {at}: {actual}");
            }
        }
    }
}

#[test]
fn resampling_at_a_short_block_edge_does_not_mix_in_stale_frames() {
    use soundcraft_model::ChannelFormat;
    // DC for the first long block, then silence. The device runs slower than the session, so
    // reads land between frames; a short block leaves the long block's DC past `pending_len`.
    let mut st = audio_output_state(ChannelFormat::Stereo, vec![vec![0.8; BLOCK]; 2]);
    st.device_rate = 44_100.0;
    let mut long = vec![0.0; 2 * 470];
    st.fill(&mut long, 2);
    assert_eq!(st.pending_len, BLOCK);
    let mut short = [0.0; 2 * 8];
    for call in 0..20 {
        st.fill(&mut short, 2);
        assert!(st.pending_len < BLOCK);
        // The first frame of the first short call still reads the DC block's last frame.
        let from = if call == 0 { 2 } else { 0 };
        assert!(short[from..].iter().all(|x| x.abs() < 1e-6), "call {call}: {short:?}");
    }
}

#[test]
fn mono_device_preserves_centered_mono_level_and_includes_surround_right_channels() {
    use soundcraft_model::ChannelFormat;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let mut mono = audio_output_state(ChannelFormat::Mono, vec![vec![0.8; 512]]);
    let mut out = [0.0; 128];
    mono.fill(&mut out, 1);
    let centered = 0.8 * soundcraft_dsp::db_to_gain(-3.0);
    assert!(out.iter().all(|x| (*x - centered).abs() < 1e-6), "a centered mono track keeps its rendered level under the -3 dB pan law");

    // Independently exercise each SMPTE 5.1 channel: L, R, C, LFE, Ls, Rs.
    let channels = (0..6).map(|c| (0..512).map(|i| if i % 6 == c { 0.8 } else { 0.0 }).collect()).collect();
    let mut surround = audio_output_state(ChannelFormat::Surround51, channels);
    surround.fill(&mut out, 1);
    let expected = [0.4, 0.4, 0.8 * h, 0.0, 0.4 * h, 0.4 * h];
    for (i, actual) in out.iter().enumerate() {
        assert!((actual - expected[i % expected.len()]).abs() < 1e-6, "surround sample {i}: {actual}");
    }
}

#[test]
fn mono_device_contains_overflow_from_opposite_surround_sides() {
    use soundcraft_model::ChannelFormat;
    let channels = [f32::MAX, -f32::MAX, 0.0, 0.0, f32::MAX, -f32::MAX].map(|v| vec![v; 128]);
    let mut st = audio_output_state(ChannelFormat::Surround51, channels.into());
    let mut out = [0.5; 128];
    st.fill(&mut out, 1);
    assert!(out.iter().all(|x| *x == 0.0), "overflow in the stereo fold-down must not send NaN to a mono device");
}

#[test]
fn interpolation_overflow_between_huge_samples_never_reaches_the_device() {
    use soundcraft_model::ChannelFormat;
    let alternating: Vec<f32> = (0..512).map(|i| if i % 2 == 0 { -3.4e38 } else { 3.4e38 }).collect();
    for (format, nch, device) in [(ChannelFormat::Stereo, 2, 2), (ChannelFormat::Stereo, 2, 8), (ChannelFormat::Mono, 1, 1)] {
        let mut st = audio_output_state(format, vec![alternating.clone(); nch]);
        let mut out = vec![0.5; 64 * device];
        st.fill(&mut out, device);
        assert!(out.iter().all(|x| x.is_finite() && (-1.0..=1.0).contains(x)), "{format:?} on {device} channels: {out:?}");
    }
}

#[test]
fn surround_main_maps_to_device_channels_or_folds_down() {
    use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    let mut s = soundcraft_model::Session::default();
    s.outputs[0].format = ChannelFormat::Surround51;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Surround51, None);
    let chans: Vec<Vec<f32>> = (0..6).map(|c| vec![0.1 * (c + 1) as f32; 48_000]).collect();
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: chans })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
    let s = Arc::new(s);
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    // An 8-channel device: channels in order, the rest silent.
    let mut out = vec![0.0f32; 8 * 256];
    st.fill(&mut out, 8);
    let f = &out[8 * 100..8 * 101];
    for c in 0..6 {
        assert!((f[c] - 0.1 * (c + 1) as f32).abs() < 1e-5, "{f:?}");
    }
    assert!(f[6] == 0.0 && f[7] == 0.0);
    // A stereo device: ITU fold-down.
    let mut out = vec![0.0f32; 2 * 256];
    st.fill(&mut out, 2);
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let l = (0.1 + h * 0.3 + h * 0.5f32).min(1.0);
    assert!((out[200] - l).abs() < 1e-4, "{} vs {l}", out[200]);
    assert_eq!(shared.meters.lock().unwrap().main.peaks.len(), 6);
}

#[test]
fn instances_arrive_with_the_session_and_leave_through_the_return_channel() {
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.fake"));
    let s = Arc::new(s);
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let (gtx, grx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(soundcraft_model::Session::default()), 48_000.0, gtx);
    let plugin = soundcraft_dsp::create("gain").unwrap();
    tx.send(Cmd::Session(Arc::clone(&s), vec![PreparedInstance { track: t, slot: 1, id: "clap:test.fake".into(), ch: 2, plugin }])).unwrap();
    let mut out = vec![0.0f32; 256];
    st.fill(&mut out, 2);
    assert!(grx.try_recv().is_err(), "nothing retired yet");
    // A session without the insert retires the instance to the return channel.
    tx.send(Cmd::Session(Arc::new(soundcraft_model::Session::default()), Vec::new())).unwrap();
    st.fill(&mut out, 2);
    assert_eq!(grx.try_recv().unwrap().len(), 1);
    // A sample-rate change rebuilds the engine without dropping plugins on this thread.
    tx.send(Cmd::Session(
        Arc::clone(&s),
        vec![PreparedInstance { track: t, slot: 1, id: "clap:test.fake".into(), ch: 2, plugin: soundcraft_dsp::create("gain").unwrap() }],
    ))
    .unwrap();
    st.fill(&mut out, 2);
    let mut s44 = (*s).clone();
    s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
    tx.send(Cmd::Session(Arc::new(s44), Vec::new())).unwrap();
    st.fill(&mut out, 2);
    assert_eq!(grx.try_recv().unwrap().len(), 1);
}

#[test]
fn plugin_states_are_read_without_the_audio_thread() {
    let mut side = Side::new(48_000.0, channel().1);
    let (a, b) = (TrackId(1), TrackId(2));
    side.states.insert((b, 0), Box::new(|| Some(b"b0".to_vec())));
    side.states.insert((a, soundcraft_mix::INSTRUMENT_SLOT), Box::new(|| Some(b"synth".to_vec())));
    side.states.insert((a, 3), Box::new(|| Some(b"a3".to_vec())));
    side.states.insert((a, 4), Box::new(|| None));
    let want = vec![(a, 3, b"a3".to_vec()), (a, soundcraft_mix::INSTRUMENT_SLOT, b"synth".to_vec()), (b, 0, b"b0".to_vec())];
    assert_eq!(side.capture_states(), want);
    side.forget((a, 3));
    assert_eq!(side.capture_states(), want[1..]);
}

struct Window;

impl PluginEditor for Window {
    fn open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn close(&mut self) {}
    fn is_open(&self) -> bool {
        false
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
}

#[test]
fn a_refused_state_is_kept_until_the_editor_opens() {
    let mut side = Side::new(48_000.0, channel().1);
    let k = (TrackId(1), 0);
    side.states.insert(k, Box::new(|| Some(b"defaults".to_vec())));
    side.editors.insert(k, Box::new(Window));
    side.refused.insert(k);
    assert!(side.capture_states().is_empty());
    side.open_editor(k).unwrap();
    assert_eq!(side.capture_states(), vec![(TrackId(1), 0, b"defaults".to_vec())]);
}

/// An editor whose plugin said its state changed (CLAP `state.mark_dirty`).
struct Dirty(bool);

impl PluginEditor for Dirty {
    fn open(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn close(&mut self) {}
    fn is_open(&self) -> bool {
        true
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
    fn state_changed(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }
}

#[test]
fn changed_states_are_reported_once_and_read_for_the_session() {
    let mut side = Side::new(48_000.0, channel().1);
    let (a, b) = (TrackId(1), TrackId(2));
    let synth = (a, soundcraft_mix::INSTRUMENT_SLOT);
    side.states.insert((a, 0), Box::new(|| Some(b"a0".to_vec())));
    side.states.insert((a, 1), Box::new(|| Some(b"a1".to_vec())));
    side.states.insert(synth, Box::new(|| Some(b"synth".to_vec())));
    side.states.insert((b, 0), Box::new(|| Some(b"b0".to_vec())));
    side.editors.insert((a, 0), Box::new(Dirty(true)));
    side.editors.insert((a, 1), Box::new(Dirty(false)));
    side.editors.insert((b, 0), Box::new(Dirty(true)));
    side.refused.insert((b, 0));
    let mut changed = side.changed_states();
    changed.sort();
    assert_eq!(changed, vec![(a, 0), (b, 0)]);
    assert!(side.changed_states().is_empty(), "each change is reported once");
    let want = vec![(a, 0, b"a0".to_vec()), (a, soundcraft_mix::INSTRUMENT_SLOT, b"synth".to_vec())];
    assert_eq!(side.capture_states_of(&[(a, 0), synth, (b, 0)]), want);
}

#[test]
fn a_state_marked_changed_while_loading_is_not_a_change() {
    let mut side = Side::new(48_000.0, channel().1);
    let k = (TrackId(1), 0);
    side.states.insert(k, Box::new(|| Some(b"loaded".to_vec())));
    side.add_editor(k, Box::new(Dirty(true)));
    assert!(side.changed_states().is_empty());
}

#[test]
fn side_creates_each_missing_instance_once() {
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};
    let mut side = Side::new(48_000.0, channel().1);
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:does.not.exist"));
    s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("gain"));
    assert!(side.prepare(&s).is_empty(), "the plugin is not installed; built-ins stay with the engine");
    assert_eq!(side.shadow.get(&(t, 0)), Some(&("clap:does.not.exist".to_string(), 1, false)));
    assert!(side.prepare(&s).is_empty());
    assert_eq!(side.shadow.len(), 1, "failures are remembered, not retried every update");
    s.track_mut(t).unwrap().mixer.inserts[0] = None;
    side.prepare(&s);
    assert!(side.shadow.is_empty());
}

#[test]
fn replaced_sessions_are_freed_on_the_ui_thread() {
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let (gtx, grx) = channel();
    let mut st = AudioState::new(rx, shared, Arc::new(soundcraft_model::Session::default()), 48_000.0, gtx);
    let mut side = Side::new(48_000.0, grx);
    side.return_old_sessions(&tx);
    let mut out = vec![0.0f32; 256];
    let mut watched = Vec::new();
    for _ in 0..3 * OLD_SESSIONS {
        let s = Arc::new(soundcraft_model::Session::default());
        watched.push(Arc::downgrade(&s));
        side.collect_garbage();
        tx.send(Cmd::Session(s, Vec::new())).unwrap();
        st.fill(&mut out, 2);
        let replaced = watched.len().checked_sub(2).and_then(|i| watched.get(i));
        assert!(replaced.is_none_or(|w| w.strong_count() == 1), "freed on the audio thread");
    }
    side.collect_garbage();
    assert_eq!(watched.iter().filter(|w| w.strong_count() > 0).count(), 1, "only the live snapshot is left");
}

#[test]
fn looping_wraps() {
    let s = Arc::new(soundcraft_model::Session::default());
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
    tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 1000)) }).unwrap();
    let mut out = vec![0.0f32; 20_000];
    st.fill(&mut out, 2);
    let p = shared.position.load(Ordering::Relaxed);
    assert!(p <= 1000, "position {p} should stay inside the loop");
    assert!(shared.playing.load(Ordering::Relaxed));
}

#[test]
fn locate_while_playing_drops_the_old_position_audio() {
    use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackKind};
    for (plugin, params) in [("compressor", &[("lookahead", 5.0), ("ratio", 1.0)][..]), ("room_reverb", &[][..])] {
        let mut s = soundcraft_model::Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.pool.insert(
            SourceId(5),
            Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800]] })),
        );
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 4800));
        let mut ins = Insert::new(plugin);
        for (k, v) in params {
            ins.params.insert((*k).into(), *v);
        }
        tr.mixer.inserts[0] = Some(ins);
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 512];
        let mut before = 0.0f32;
        for _ in 0..9 {
            st.fill(&mut out, 2);
            before = out.iter().fold(before, |m, x| m.max(x.abs()));
        }
        assert!(before > 0.1, "{plugin}: the clip plays before the locate");
        tx.send(Cmd::Locate(1_000_000)).unwrap();
        for _ in 0..8 {
            st.fill(&mut out, 2);
            assert!(out.iter().all(|x| x.abs() < 1e-6), "{plugin}: audio from before the locate");
        }
    }
}

#[test]
fn armed_tracks_are_heard_and_metered_after_playback_stops() {
    use soundcraft_model::{ChannelFormat, TrackKind};
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.track_mut(t).unwrap().mixer.record_arm = true;
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
    let ring = record::InputRing::new(1, 48_000);
    tx.send(Cmd::Input(Arc::clone(&ring))).unwrap();
    tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
    let mut out = vec![0.0f32; 4096];
    st.fill(&mut out, 2);
    assert!(!shared.playing.load(Ordering::Relaxed));
    ring.push(&[0.5; 4096]);
    st.fill(&mut out, 2);
    assert!(out.iter().any(|x| x.abs() > 0.1), "the input is heard");
    assert!(shared.meters.lock().unwrap().tracks.get(&t).is_some_and(|m| m.peak[0] > 0.1), "and metered");
}
