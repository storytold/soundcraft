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
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels: vec![x] })));
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
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels })));
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
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels: chans })));
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
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(craft_audio::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800]] })));
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

/// A stopped engine with one record-enabled instrument track and a MIDI feed with one open port.
fn keyboard_session() -> (AudioState, std::sync::mpsc::Sender<Cmd>, Arc<midi::MidiFeed>, soundcraft_model::Session, TrackId) {
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("subtractive_synth"));
    tr.mixer.record_arm = true;
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let st = AudioState::new(rx, shared, Arc::new(s.clone()), 48_000.0, channel().0);
    let feed = Arc::new(midi::MidiFeed::default());
    feed.set_connected(1);
    tx.send(Cmd::Midi(Arc::clone(&feed))).unwrap();
    (st, tx, feed, s, t)
}

/// Runs `callbacks` device callbacks of 1024 frames; whether the last one has sound in it.
fn heard(st: &mut AudioState, callbacks: usize) -> bool {
    let mut out = vec![0.0f32; 2048];
    (0..callbacks).for_each(|_| st.fill(&mut out, 2));
    out.iter().any(|x| x.abs() > 0.003)
}

#[test]
fn a_midi_keyboard_plays_and_records_through_a_record_enabled_instrument() {
    let (mut st, tx, feed, mut s, t) = keyboard_session();
    // The feed's clock stands still: an event's place does not depend on how fast the test runs.
    feed.set_clock(1);
    // Not record-enabled: the engine stays idle and the key press is not kept for later.
    s.track_mut(t).unwrap().mixer.record_arm = false;
    tx.send(Cmd::Session(Arc::new(s.clone()), Vec::new())).unwrap();
    feed.receive(0, &[0x90, 60, 100]);
    assert!(!heard(&mut st, 4));
    s.track_mut(t).unwrap().mixer.record_arm = true;
    tx.send(Cmd::Session(Arc::new(s), Vec::new())).unwrap();
    assert!(!heard(&mut st, 4), "the earlier key press is gone");
    feed.receive(0, &[0x90, 60, 100]);
    assert!(heard(&mut st, 4), "stopped, a record-enabled instrument plays the keyboard");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty(), "nothing is recorded while stopped");
    // Recording: the events come back with the position they were played at.
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 48_000, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 64, 90]);
    st.fill(&mut out, 2);
    feed.receive(0, &[0x80, 64, 0]);
    st.fill(&mut out, 2);
    st.fill(&mut out, 2);
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 64), (0x80, 64)]);
    // One callback is 1024 frames; each event arrived as its callback started.
    assert_eq!((rec[0].at, rec[1].at), (48_000 + 1024, 48_000 + 2048));
    assert_eq!(feed.take_recorded_lost(), 0);
}

#[test]
fn a_full_midi_queue_releases_the_held_note_and_the_pedal_of_its_port_only() {
    // Either queue overflows: the one from the device (1024 events) and the audio thread's own
    // (256 a callback). The releases are among what is dropped.
    for flood in [midi::INCOMING, callback::MIDI_EVENTS] {
        let (mut st, _tx, feed, ..) = keyboard_session();
        // Port 0 holds one note with the pedal and one with a key; port 1 holds a key.
        for (port, msg) in [(0, [0xB0, 64, 127]), (0, [0x90, 60, 100]), (0, [0x80, 60, 0]), (0, [0x90, 64, 100]), (1, [0x90, 72, 100])] {
            feed.receive(port, &msg);
        }
        assert!(heard(&mut st, 4));
        // Port 0 floods the queue, so its note-off and pedal-up do not get through.
        for _ in 0..flood {
            feed.receive(0, &[0xB0, 1, 0]);
        }
        feed.receive(0, &[0x80, 64, 0]);
        feed.receive(0, &[0xB0, 64, 0]);
        assert!(heard(&mut st, 60), "{flood}: port 1 still holds its key");
        feed.receive(1, &[0x80, 72, 0]);
        assert!(!heard(&mut st, 60), "{flood}: port 0 holds nothing after its events were dropped");
        // The pedal went too: a new note ends with its key.
        feed.receive(0, &[0x90, 60, 100]);
        assert!(heard(&mut st, 4));
        feed.receive(0, &[0x80, 60, 0]);
        assert!(!heard(&mut st, 60), "{flood}");
    }
}

#[test]
fn a_note_on_queued_before_lost_events_is_not_played_after_the_release() {
    let (mut st, _tx, feed, ..) = keyboard_session();
    // One callback's worth: a key goes down, the flood drops its note-off.
    feed.receive(0, &[0x90, 60, 100]);
    for _ in 0..callback::MIDI_EVENTS {
        feed.receive(0, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    assert!(!heard(&mut st, 60), "the note-on went with the events that were dropped");
}

#[test]
fn keys_held_while_nothing_listens_or_the_port_closes_do_not_stay_on() {
    let (mut st, tx, feed, mut s, t) = keyboard_session();
    feed.receive(0, &[0x90, 60, 100]);
    assert!(heard(&mut st, 4));
    // The track leaves record with the key down; its note-off comes while nothing listens.
    s.track_mut(t).unwrap().mixer.record_arm = false;
    tx.send(Cmd::Session(Arc::new(s.clone()), Vec::new())).unwrap();
    heard(&mut st, 2);
    feed.receive(0, &[0x80, 60, 0]);
    heard(&mut st, 2);
    s.track_mut(t).unwrap().mixer.record_arm = true;
    tx.send(Cmd::Session(Arc::new(s), Vec::new())).unwrap();
    assert!(!heard(&mut st, 60), "the note does not come back");
    // The device is unplugged with a key down: no note-off will come.
    feed.receive(0, &[0x90, 62, 100]);
    assert!(heard(&mut st, 4));
    feed.port_lost(0);
    assert!(!heard(&mut st, 60));
}

#[test]
fn lost_midi_input_is_written_into_the_take() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    feed.receive(0, &[0x90, 60, 100]);
    heard(&mut st, 2);
    for _ in 0..midi::INCOMING {
        feed.receive(0, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    heard(&mut st, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost: Vec<&midi::RecordedMidi> = rec.iter().filter(|e| e.is_lost()).collect();
    assert_eq!(lost.iter().map(|e| e.port).collect::<Vec<_>>(), [0], "one entry for the port that lost input");
    assert_eq!((rec[0].status, rec[0].data1), (0x90, 60));
    assert!(lost[0].at >= rec[0].at, "the held note ends at or after its start");
    assert!(!rec.iter().any(|e| e.status == 0x80), "its note-off was among the dropped events");
    assert!(feed.dropped() > 0);
}

#[test]
fn live_midi_keeps_its_order_and_its_time_at_another_device_rate() {
    // The device runs at 96 kHz, the session at 48 kHz: a callback of 1024 device frames is 512
    // session frames, and that is the delay every event gets.
    let (_, tx, feed, s, _) = keyboard_session();
    feed.set_clock(1);
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(1000),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx2, rx) = channel();
    drop(tx);
    let mut st = AudioState::new(rx, shared, Arc::new(s), 96_000.0, channel().0);
    tx2.send(Cmd::Midi(Arc::clone(&feed))).unwrap();
    tx2.send(Cmd::Recording(true, 0)).unwrap();
    tx2.send(Cmd::Play { from: 10_000, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    for note in [60, 62, 64, 65] {
        feed.receive(0, &[0x90, note, 100]);
    }
    for _ in 0..3 {
        st.fill(&mut out, 2);
    }
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| e.data1).collect::<Vec<_>>(), [60, 62, 64, 65], "in the order they arrived");
    assert!(rec.windows(2).all(|w| w[0].at <= w[1].at), "and in time order: {rec:?}");
    // They arrived as the second callback started, at 10 000 + 512.
    assert!(rec.iter().all(|e| e.at == 10_000 + 512), "{rec:?}");
}

#[test]
fn the_last_device_going_away_during_a_record_pass_is_written_into_the_take() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    feed.receive(0, &[0xB0, 64, 127]);
    feed.receive(0, &[0x90, 60, 100]);
    assert!(heard(&mut st, 4));
    // Unplugged with the key and the pedal down: no port is open any more.
    feed.port_lost(0);
    feed.set_connected(0);
    assert!(!heard(&mut st, 60), "nothing is left sounding");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost: Vec<&midi::RecordedMidi> = rec.iter().filter(|e| e.is_lost()).collect();
    assert_eq!(lost.iter().map(|e| e.port).collect::<Vec<_>>(), [0], "once, for the port that went");
    assert!(lost[0].at >= rec[1].at, "the note that was held ends there");
    // Playing on without a device adds nothing to the take.
    heard(&mut st, 10);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty());
}

#[test]
fn a_note_played_just_before_stop_is_in_the_take_and_the_end_of_the_pass_is_acknowledged() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The key goes down, and Stop arrives before the audio thread runs again: the note has not
    // been played yet (it waits out its delay), but it belongs to the pass.
    feed.receive(0, &[0x90, 60, 100]);
    let pass = feed.end_pass();
    tx.send(Cmd::Stop).unwrap();
    tx.send(Cmd::Recording(false, pass)).unwrap();
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty() && !feed.captured(pass), "the audio thread has not seen the stop yet");
    st.fill(&mut out, 2);
    assert!(feed.captured(pass), "it says when everything up to the stop is written down");
    feed.take_recorded(&mut rec);
    // The pass stopped at 1024, where its range ends: the key is the last thing inside it.
    assert_eq!(rec.iter().map(|e| (e.status, e.data1, e.at)).collect::<Vec<_>>(), [(0x90, 60, 1023)]);
    assert_eq!(feed.stopped_at(pass), Some(1024), "the answer says where the pass stopped");
    // What is played after the stop is not part of it.
    feed.receive(0, &[0x80, 60, 0]);
    st.fill(&mut out, 2);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty());
}

#[test]
fn the_recorded_position_does_not_move_with_the_size_of_later_callbacks() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let (mut long, mut short) = (vec![0.0f32; 2048], vec![0.0f32; 256]);
    st.fill(&mut long, 2);
    // Arrives before a callback of 1024 frames, so it is due 1024 frames later; the callbacks
    // that follow are 128 frames long.
    feed.receive(0, &[0x90, 60, 100]);
    st.fill(&mut long, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.len(), 1, "written down when it is taken, not when it is played");
    assert_eq!(rec[0].at, 1024);
    let mut sounded = false;
    for _ in 0..40 {
        st.fill(&mut short, 2);
        sounded |= short.iter().any(|x| x.abs() > 0.003);
    }
    assert!(sounded, "and it is still played");
    feed.take_recorded(&mut rec);
    assert_eq!(rec.len(), 1, "once");
}

#[test]
fn more_events_than_a_callback_can_play_are_still_all_recorded() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 60, 100]);
    for _ in 0..callback::MIDI_EVENTS {
        feed.receive(0, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.len(), callback::MIDI_EVENTS + 2, "the take has them all");
    assert!(!rec.iter().any(|e| e.is_lost()), "nothing is missing from it, so it is not marked incomplete");
    assert_eq!((rec[0].status, rec.last().unwrap().status), (0x90, 0x80));
    assert!(!heard(&mut st, 60), "what could not be played ended the port's notes");
}

#[test]
fn what_a_port_sends_after_it_lost_input_is_kept_and_follows_the_loss_in_the_take() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    // A key goes down, the port loses its input (the device is replugged: the key's release
    // will not come), and the device then plays another key. All before the next callback.
    feed.receive(0, &[0x90, 60, 100]);
    std::thread::sleep(std::time::Duration::from_millis(2));
    feed.port_lost(0);
    std::thread::sleep(std::time::Duration::from_millis(2));
    feed.receive(0, &[0x90, 64, 100]);
    assert!(heard(&mut st, 60), "the key pressed after the loss plays, and is still held");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(
        rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(),
        [(0x90, 60), (midi::RecordedMidi::LOST, 0), (0x90, 64)],
        "the loss ends the first note in the take, not the one played after it"
    );
    assert!(rec.windows(2).all(|w| w[0].at <= w[1].at), "{rec:?}");
    // Only that second key is down: its release leaves nothing sounding.
    feed.receive(0, &[0x80, 64, 0]);
    assert!(!heard(&mut st, 60), "the key from before the loss was not played after the release");
}

#[test]
fn the_age_of_a_midi_event_is_counted_at_the_transport_speed() {
    let (_, tx, feed, s, _) = keyboard_session();
    drop(tx);
    // Half speed: the playhead covers 24 000 session frames a second.
    let shared = Arc::new(Shared {
        position: AtomicI64::new(0),
        playing: AtomicBool::new(false),
        speed: AtomicU32::new(500),
        meters: Mutex::new(MeterSnapshot::default()),
    });
    let (tx, rx) = channel();
    let mut st = AudioState::new(rx, shared, Arc::new(s), 48_000.0, channel().0);
    tx.send(Cmd::Midi(Arc::clone(&feed))).unwrap();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 48_000, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The event waits 20 ms for its callback: 480 session frames at half speed, not 960.
    let before = std::time::Instant::now();
    feed.receive(0, &[0x90, 60, 100]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    st.fill(&mut out, 2);
    let waited = before.elapsed().as_secs_f64();
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.len(), 1);
    // The second callback starts at 48 000 + 512 (1024 device frames at half speed).
    let at = 48_000 + 512;
    let oldest = at - (waited * 24_000.0).ceil() as i64 - 1;
    assert!((oldest..=at - 480).contains(&rec[0].at), "{} is not within {oldest}..={}", rec[0].at, at - 480);
}

#[test]
fn loop_recording_says_in_the_take_where_the_loop_went_round() {
    let (mut st, tx, feed, ..) = keyboard_session();
    // The feed's clock stands still, so every event is taken at the very start of its callback.
    feed.set_clock(1);
    // The loop is four callbacks long.
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 4096)) }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // A key goes down at about 1024 in the first round and up at about 2048 in the second: later
    // on the timeline, but a round apart.
    feed.receive(0, &[0x90, 60, 100]);
    for _ in 0..5 {
        st.fill(&mut out, 2);
    }
    feed.receive(0, &[0x80, 60, 0]);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| e.status).collect::<Vec<_>>(), [0x90, midi::RecordedMidi::WRAP, 0x80]);
    assert!(rec[1].is_wrap() && !rec[1].is_lost());
    assert_eq!((rec[0].at, rec[2].at), (1024, 2048), "{rec:?}");
    // A key pressed while the playhead stands at the end of the loop stays inside the loop: just
    // before its end, in the round that is ending, or at its start, after the mark. Never at
    // the end itself, where the take would leave it out.
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    st.fill(&mut out, 2);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().filter(|e| e.is_wrap()).count(), 1, "the loop went round once: {rec:?}");
    let key = rec.iter().position(|e| e.status == 0x90).unwrap();
    let before_the_mark = key == 0;
    assert_eq!(rec.len(), 2, "{rec:?}");
    assert!(if before_the_mark { (4096 - 64..4096).contains(&rec[key].at) } else { rec[key].at == 0 }, "{rec:?}");
}

#[test]
fn the_late_answer_to_an_earlier_pass_does_not_end_the_wait_for_a_later_one() {
    let (mut st, tx, feed, ..) = keyboard_session();
    let mut out = vec![0.0f32; 2048];
    // Two passes end before the audio thread runs; it then sees only the first Stop.
    let (first, second) = (feed.end_pass(), feed.end_pass());
    tx.send(Cmd::Recording(false, first)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(first) && !feed.captured(second), "the second pass is not written down yet");
    tx.send(Cmd::Recording(false, second)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(second));
    // Switching recording off without a pass (a session reset) answers nobody.
    let third = feed.end_pass();
    tx.send(Cmd::Recording(false, 0)).unwrap();
    st.fill(&mut out, 2);
    assert!(!feed.captured(third));
}

#[test]
fn one_ports_loss_does_not_take_along_what_another_port_sent_before_it() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    let pause = || std::thread::sleep(std::time::Duration::from_millis(2));
    // Port 0 loses its input and then plays a new key. After that port 1 loses its input, with
    // a key down. All within one callback.
    feed.receive(0, &[0x90, 60, 100]);
    pause();
    feed.port_lost(0);
    pause();
    feed.receive(0, &[0x90, 64, 100]);
    feed.receive(1, &[0x90, 72, 100]);
    pause();
    feed.port_lost(1);
    assert!(heard(&mut st, 60), "port 0's key from after its own loss plays: port 1's later loss is not its loss");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    assert_eq!(
        rec.iter().map(|e| (e.port, e.status, e.data1)).collect::<Vec<_>>(),
        [(0, 0x90, 60), (0, lost, 0), (0, 0x90, 64), (1, 0x90, 72), (1, lost, 0)],
        "each port's mark is where that port lost its input"
    );
    assert!(rec.windows(2).all(|w| w[0].at <= w[1].at), "{rec:?}");
    // Only port 0's second key sounds: its release leaves nothing.
    feed.receive(0, &[0x80, 64, 0]);
    assert!(!heard(&mut st, 60));
}

#[test]
fn a_loss_met_in_the_queue_is_not_taken_for_a_second_loss_by_the_next_callback() {
    // `late`: the port's flag is raised after the callback has read the flags (it drains the
    // queue meanwhile, and a new note gets in), so only the next callback sees it.
    for late in [false, true] {
        let (mut st, tx, feed, ..) = keyboard_session();
        tx.send(Cmd::Recording(true, 0)).unwrap();
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        heard(&mut st, 1);
        // Port 1 fills the queue, and a release of port 0 does not fit.
        for _ in 0..midi::INCOMING {
            feed.receive(1, &[0xB0, 1, 0]);
        }
        feed.receive(0, &[0x80, 60, 0]);
        let flag = if late { feed.take_lost() } else { 0 };
        // The queue is drained down to a few events, and port 0 plays a new key.
        assert_eq!(std::iter::from_fn(|| feed.next()).take(midi::INCOMING - 8).count(), midi::INCOMING - 8);
        std::thread::sleep(std::time::Duration::from_millis(2));
        feed.receive(0, &[0x90, 64, 100]);
        heard(&mut st, 1);
        feed.put_back_lost(flag);
        assert!(heard(&mut st, 60), "late {late}: the key pressed after the loss is still held");
        let mut rec = Vec::new();
        feed.take_recorded(&mut rec);
        let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
        assert_eq!(port0, [(midi::RecordedMidi::LOST, 0), (0x90, 64)], "late {late}: one mark, before the new key");
        feed.receive(0, &[0x80, 64, 0]);
        assert!(!heard(&mut st, 60), "late {late}");
    }
}

#[test]
fn a_loop_pass_stopped_at_the_end_of_the_loop_does_not_start_another_round() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 4096)) }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 60, 100]);
    // Three more callbacks: the playhead stands exactly at the end of the loop.
    for _ in 0..3 {
        st.fill(&mut out, 2);
    }
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 4096);
    // Another key, and Stop, before the audio thread runs again.
    feed.receive(0, &[0x90, 62, 100]);
    let pass = feed.end_pass();
    tx.send(Cmd::Stop).unwrap();
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    // The loop never went round, and the take does not say it did: with a mark, the first key
    // (held to the stop at the loop end) would count as held into a second round.
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 60), (0x90, 62)], "{rec:?}");
    assert!(rec.iter().all(|e| e.at < 4096), "both keys are inside the loop: {rec:?}");
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 4096, "the pass stopped at the loop end");
    assert_eq!(feed.stopped_at(pass), Some(4096), "and the answer says so");
}

#[test]
fn a_pass_stays_owed_through_a_stalled_callback_whatever_the_devices_do() {
    let (mut st, tx, feed, ..) = keyboard_session();
    let mut out = vec![0.0f32; 2048];
    assert!(!feed.pass_owed());
    // Pass A records a key; then its only device is unplugged, and Stop follows.
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 60, 100]);
    st.fill(&mut out, 2);
    feed.port_lost(0);
    feed.set_connected(0);
    let a = feed.end_pass();
    tx.send(Cmd::Stop).unwrap();
    tx.send(Cmd::Recording(false, a)).unwrap();
    // The audio callback stalls: it has not seen the Stop. The device comes back and a key is
    // played. Pass A is still owed, so pass B must not start: this key would be written down
    // with pass A's state, at pass A's position.
    assert!(feed.pass_owed() && !feed.captured(a));
    feed.set_connected(1);
    feed.receive(0, &[0x90, 72, 100]);
    assert!(feed.pass_owed(), "a device coming back changes nothing");
    // The callback runs again: pass A is confirmed, and only then may B start.
    st.fill(&mut out, 2);
    assert!(feed.captured(a) && !feed.pass_owed());
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    // What reached the take belongs to pass A (the UI clears it when B starts).
    assert!(rec.iter().all(|e| e.at <= 2048), "{rec:?}");
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 96_000, end: None, looped: None }).unwrap();
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 74, 100]);
    st.fill(&mut out, 2);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| e.data1).collect::<Vec<_>>(), [74]);
    assert!(rec[0].at >= 96_000, "pass B's key is at pass B's position: {}", rec[0].at);
    // An engine that is replaced no longer owes its passes.
    let b = feed.end_pass();
    assert!(feed.pass_owed());
    feed.forgive_passes();
    assert!(!feed.pass_owed() && feed.captured(b));
    assert_eq!(feed.stopped_at(b), None, "nobody said where that pass stopped");
}

#[test]
fn a_loop_pass_says_where_it_stopped_with_its_answer_not_where_the_engine_is_later() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 4096)) }).unwrap();
    let mut out = vec![0.0f32; 2048];
    // Three callbacks: the playhead is at 3072, one callback before the loop end. A key is held.
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 60, 100]);
    st.fill(&mut out, 2);
    st.fill(&mut out, 2);
    // Recording ends while playback goes on: in the callback that hears of it the loop goes
    // round, with recording off, so no mark of that round is in the take.
    let pass = feed.end_pass();
    assert_eq!(feed.stopped_at(pass), None, "not confirmed yet");
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.at)).collect::<Vec<_>>(), [(0x90, 1024)], "no round is marked: {rec:?}");
    // The engine is in the next round by now. The stop that goes with the take is in the first.
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 1024);
    assert_eq!(feed.stopped_at(pass), Some(3072));
}

#[test]
fn what_is_played_after_stop_is_not_in_the_take_however_late_the_audio_thread_hears_of_it() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // A controller before Stop and a key after it, both before the audio thread runs again
    // (playback goes on: only recording ends). The key is the only thing that can sound.
    feed.set_clock(1_000);
    feed.receive(0, &[0xB0, 1, 64]);
    feed.set_clock(2_000);
    let pass = feed.end_pass();
    feed.set_clock(3_000);
    feed.receive(0, &[0x90, 62, 100]);
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0xB0, 1)], "the key from after the Stop is not recorded");
    assert!(heard(&mut st, 4), "but it is played");
}

#[test]
fn a_note_that_arrives_between_the_queue_being_read_and_the_stop_is_in_the_take() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The callback has read the MIDI queue (empty). Before it reads its commands a key goes
    // down and Stop follows: the key was played before the Stop, so the pass is not confirmed
    // without it.
    let ended = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (f, tx2, e) = (Arc::clone(&feed), tx.clone(), Arc::clone(&ended));
    st.before_commands = Some(Box::new(move || {
        if e.load(Ordering::Relaxed) == 0 {
            f.receive(0, &[0x90, 60, 100]);
            let pass = f.end_pass();
            e.store(pass, Ordering::Relaxed);
            let _ = tx2.send(Cmd::Recording(false, pass));
        }
    }));
    st.fill(&mut out, 2);
    let pass = ended.load(Ordering::Relaxed);
    assert!(pass != 0 && feed.captured(pass), "confirmed in the same callback");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1, e.at)).collect::<Vec<_>>(), [(0x90, 60, 1023)]);
    assert!(feed.stopped_at(pass).is_some_and(|stop| rec[0].at < stop), "inside the range that ends at the stop");
}

#[test]
fn a_port_closed_after_its_overflow_keeps_a_mark_for_each_loss() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    // Port 1 fills the queue, and a release of port 0 does not fit.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    // The queue is drained down to a few events, port 0 plays a new key (the mark of the
    // overflow goes in ahead of it), and then port 0 is unplugged. All before the next callback.
    assert_eq!(std::iter::from_fn(|| feed.next()).take(midi::INCOMING - 8).count(), midi::INCOMING - 8);
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 64, 100]);
    feed.set_clock(3_000);
    feed.port_lost(0);
    feed.set_clock(4_000);
    assert!(!heard(&mut st, 60), "the key from before the port went is not left sounding");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
    assert_eq!(port0, [(lost, 0), (0x90, 64), (lost, 0)], "the new key ends where the port went, not at the end of the take");
    assert!(rec.windows(2).all(|w| w[0].at <= w[1].at), "{rec:?}");
}

#[test]
fn a_loss_told_by_the_ports_flag_is_not_written_again_when_its_mark_arrives() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    // Port 1 fills the queue, and a release of port 0 does not fit. Nothing more arrives before
    // the callback: it learns of the loss from the port's flag.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    heard(&mut st, 1);
    // The mark of that overflow gets into the queue only now, ahead of the next event.
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 64, 100]);
    heard(&mut st, 1);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    let port0 = |rec: &[midi::RecordedMidi]| rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect::<Vec<_>>();
    assert_eq!(port0(&rec), [(lost, 0), (0x90, 64)], "one mark for one loss");
    // Another loss of the same port is another mark.
    feed.set_clock(3_000);
    feed.port_lost(0);
    heard(&mut st, 1);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert_eq!(port0(&rec), [(lost, 0)]);
}

#[test]
fn a_release_taken_by_a_shorter_callback_is_not_played_before_its_note_on() {
    let (mut st, _tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    let (mut long, mut mid, mut short) = (vec![0.0f32; 1536], vec![0.0f32; 512], vec![0.0f32; 256]);
    // 768 frames: two blocks of 512 are rendered, and 256 frames wait for the next callback.
    st.fill(&mut long, 2);
    // The key goes down before a callback of 256 frames, which plays those frames and renders
    // nothing: the note-on is still waiting, 256 frames from due.
    feed.receive(0, &[0x90, 60, 100]);
    st.fill(&mut mid, 2);
    // The key goes up before a callback of 128 frames. Counted from that callback alone the
    // release would be due first, the instrument would ignore it, and the note would stay on.
    feed.receive(0, &[0x80, 60, 0]);
    // What the engine is handed, block by block (a callback that renders nothing leaves the
    // last block's list in place: the same list twice is one block).
    let mut handed: Vec<Vec<u8>> = Vec::new();
    for _ in 0..500 {
        st.fill(&mut short, 2);
        let block: Vec<u8> = st.mix.midi.iter().map(|e| e.status).collect();
        if !block.is_empty() && handed.last() != Some(&block) {
            handed.push(block);
        }
    }
    assert_eq!(handed.concat(), [0x90, 0x80], "both reach the instrument, the note-on first");
    assert!(st.mix.midi.windows(2).all(|w| w[0].offset <= w[1].offset));
    let sounding = (0..40).any(|_| {
        st.fill(&mut short, 2);
        short.iter().any(|x| x.abs() > 0.003)
    });
    assert!(!sounding, "the note ended with its release");
}

#[test]
fn a_short_loop_that_goes_round_several_times_in_a_callback_marks_one_round_between_two_events_a_round_apart() {
    let (mut st, tx, feed, ..) = keyboard_session();
    // The loop is 128 frames, a callback 512: four rounds to a callback.
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 128)) }).unwrap();
    let mut out = vec![0.0f32; 1024];
    feed.set_clock(0);
    st.fill(&mut out, 2);
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 128, "the playhead stands at the loop end, after three rounds");
    // A key goes down 160 frames before the next callback (in the round before the last, at
    // about 96) and up 112 frames before it (in the last round, at about 16).
    feed.set_clock(16_667);
    feed.receive(0, &[0x90, 60, 100]);
    feed.set_clock(17_667);
    feed.receive(0, &[0x80, 60, 0]);
    feed.set_clock(20_000);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let wrap = midi::RecordedMidi::WRAP;
    // The rounds before the key press come first; one round lies between press and release, so
    // the note is the end of the loop and its start, not the whole loop.
    assert_eq!(rec.iter().map(|e| e.status).collect::<Vec<_>>(), [wrap, wrap, 0x90, wrap, 0x80], "{rec:?}");
    assert!((95..=98).contains(&rec[2].at) && (15..=18).contains(&rec[4].at), "{rec:?}");
}

#[test]
fn a_note_played_before_the_transport_stop_is_in_the_take_when_the_end_of_the_pass_comes_a_callback_later() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The callback has read the MIDI queue. A key goes down, and the transport is stopped: that
    // command is read in this callback, the one that ends the pass only in the next.
    let (f, tx2) = (Arc::clone(&feed), tx.clone());
    let mut once = true;
    st.before_commands = Some(Box::new(move || {
        if std::mem::take(&mut once) {
            f.receive(0, &[0x90, 60, 100]);
            let _ = tx2.send(Cmd::Stop);
        }
    }));
    st.fill(&mut out, 2);
    let pass = feed.end_pass();
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1, e.at)).collect::<Vec<_>>(), [(0x90, 60, 1023)]);
    assert_eq!(feed.stopped_at(pass), Some(1024), "the key is inside the range that ends at the stop");
}

#[test]
fn what_is_played_after_record_is_in_the_take_before_the_audio_thread_hears_of_the_pass() {
    let (mut st, tx, feed, ..) = keyboard_session();
    // Playing, not recording.
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // A key before Record and one after it, both before the audio thread runs again.
    feed.set_clock(1_000);
    feed.receive(0, &[0x90, 60, 100]);
    feed.set_clock(2_000);
    feed.begin_pass();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    feed.set_clock(3_000);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    // The pass goes on as usual, and the next one does not start early.
    feed.receive(0, &[0x80, 62, 0]);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 62), (0x80, 62)], "not the key from before Record");
    tx.send(Cmd::Recording(false, 0)).unwrap();
    st.fill(&mut out, 2);
    feed.receive(0, &[0x90, 64, 100]);
    st.fill(&mut out, 2);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty(), "{rec:?}");
}

#[test]
fn an_overflow_before_stop_is_in_the_take_when_the_port_closes_after_it() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    feed.set_clock(500);
    feed.receive(0, &[0x90, 60, 100]);
    heard(&mut st, 2);
    // Port 1 fills the queue and port 0's release does not fit. Stop. Then port 0 is unplugged,
    // before the audio thread has run: its time of loss is now after the Stop.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    feed.set_clock(2_000);
    let pass = feed.end_pass();
    feed.set_clock(3_000);
    feed.port_lost(0);
    tx.send(Cmd::Recording(false, pass)).unwrap();
    heard(&mut st, 1);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
    assert_eq!(port0, [(0x90, 60), (midi::RecordedMidi::LOST, 0)], "the dropped release is marked: the take is not complete");
}

#[test]
fn a_session_at_another_sample_rate_is_rendered_at_that_rate_from_the_callback_that_installs_it() {
    let (mut st, tx, ..) = keyboard_session();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The device runs at 48 kHz. A 96 kHz session takes two session frames to a device frame.
    let s = soundcraft_model::Session::new("t", soundcraft_time::SampleRate::new(96_000).unwrap());
    tx.send(Cmd::Session(Arc::new(s), Vec::new())).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    st.fill(&mut out, 2);
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 2048, "1024 device frames");
}

#[test]
fn a_speed_change_during_a_callback_does_not_split_what_it_renders_at_from_what_it_keeps() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(0);
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 1024);
    // Half speed is asked for while the callback runs, after it has read the speed.
    let shared = Arc::clone(&st.shared);
    st.before_commands = Some(Box::new(move || shared.speed.store(500, Ordering::Relaxed)));
    st.fill(&mut out, 2);
    st.before_commands = None;
    // It rendered at full speed, the speed it read...
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 2048);
    // ...and that is the speed the next callback counts the age of an event at: 10 ms is 480
    // frames at full speed, though half speed is in force by then.
    feed.receive(0, &[0x90, 60, 100]);
    feed.set_clock(10_000);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| e.at).collect::<Vec<_>>(), [2048 - 480]);
    assert_eq!(st.shared.position.load(Ordering::Relaxed), 2048 + 512, "half speed from the callback after");
}

#[test]
fn what_was_played_before_record_stays_out_of_the_take_when_it_is_taken_in_after_the_start_command() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The callback has read the MIDI queue. A key goes down, then Record: the callback reads
    // the start command now, and takes the key in only with its next run, while recording.
    let (f, tx2) = (Arc::clone(&feed), tx.clone());
    let mut once = true;
    st.before_commands = Some(Box::new(move || {
        if std::mem::take(&mut once) {
            f.set_clock(1_000);
            f.receive(0, &[0x90, 60, 100]);
            f.set_clock(2_000);
            f.begin_pass();
            let _ = tx2.send(Cmd::Recording(true, 0));
        }
    }));
    st.fill(&mut out, 2);
    feed.set_clock(3_000);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 62)], "only the key from after Record");
}

#[test]
fn the_stop_cutoff_holds_before_the_stop_command_arrives() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The pass is ended; the command that says so has not reached the audio thread when it
    // takes in a key played after that.
    feed.set_clock(1_000);
    let pass = feed.end_pass();
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 60, 100]);
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty() && !feed.captured(pass), "{rec:?}");
    // Once the pass is confirmed, the cutoff is gone: the next pass records.
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    tx.send(Cmd::Recording(true, 0)).unwrap();
    st.fill(&mut out, 2);
    feed.set_clock(3_000);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| e.data1).collect::<Vec<_>>(), [62]);
}

#[test]
fn a_close_met_through_the_mark_of_an_earlier_overflow_is_not_a_second_loss_when_its_flag_comes() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    // Port 0 overflows, plays on (the mark of the overflow goes into the queue), and is closed.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    assert_eq!(std::iter::from_fn(|| feed.next()).take(midi::INCOMING - 8).count(), midi::INCOMING - 8);
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 64, 100]);
    feed.set_clock(3_000);
    feed.port_lost(0);
    heard(&mut st, 1);
    // The flag of the close reaches the audio thread a callback late (it was raised while the
    // queue was being read). By then a new connection of the port plays a key.
    feed.put_back_lost(soundcraft_mix::midi_port_bit(0));
    feed.set_clock(4_000);
    feed.receive(0, &[0x90, 67, 100]);
    assert!(heard(&mut st, 60), "the key of the new connection is not ended by the old close");
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
    assert_eq!(port0, [(lost, 0), (0x90, 64), (lost, 0), (0x90, 67)], "one mark for the overflow, one for the close");
}

#[test]
fn two_overflows_of_one_port_in_one_callback_each_keep_their_mark() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    let drain = |left: usize| {
        let take = midi::INCOMING - left;
        assert_eq!(std::iter::from_fn(|| feed.next()).take(take).count(), take);
    };
    // Port 1 fills the queue and a release of port 0 does not fit. The audio thread is part of
    // the way through the queue (500 events are left) when port 0 plays a new key: the mark of
    // the overflow goes in ahead of it.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    drain(500);
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 64, 100]);
    // The queue fills again, and the release of that key does not fit either. The audio thread
    // reads on a little (it has not reached the first mark), and the next message puts the
    // second mark in: both marks are in the queue of one callback.
    feed.set_clock(3_000);
    for _ in 0..midi::INCOMING - 502 {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 64, 0]);
    assert_eq!(feed.dropped(), 2);
    for _ in 0..100 {
        feed.next();
    }
    feed.set_clock(4_000);
    feed.receive(1, &[0xB0, 1, 0]);
    feed.set_clock(5_000);
    heard(&mut st, 1);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
    assert_eq!(port0, [(lost, 0), (0x90, 64), (lost, 0)], "both marks, and the key between them ends at the second");
}

#[test]
fn what_is_played_as_playback_runs_to_its_end_is_in_the_take_and_what_follows_is_not() {
    let (mut st, tx, feed, ..) = keyboard_session();
    feed.set_clock(1_000);
    // Playback ends by itself at 1536, in the second callback.
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: Some(1536), looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // A key goes down after the second callback has read the MIDI queue, before playback
    // reaches the end in it.
    let f = Arc::clone(&feed);
    let mut once = true;
    st.before_commands = Some(Box::new(move || {
        if std::mem::take(&mut once) {
            f.receive(0, &[0x90, 60, 100]);
        }
    }));
    st.fill(&mut out, 2);
    assert!(!st.shared.playing.load(Ordering::Relaxed), "playback has run out");
    // Another key after that, before the UI has noticed and ended the pass.
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    let pass = feed.end_pass();
    tx.send(Cmd::Recording(false, pass)).unwrap();
    st.fill(&mut out, 2);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 60)], "the key from before the end, not the one after");
    // It was played as playback ran out, and that is where it is: the time the audio thread
    // stood still after that does not move it back.
    assert_eq!(rec[0].at, 1535);
}

#[test]
fn a_pass_begun_while_the_queue_is_being_read_records_from_when_it_began() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The callback has seen that no pass is begun. Before it takes the queue in, a key goes
    // down, Record is pressed, and another key goes down.
    let (f, tx2) = (Arc::clone(&feed), tx.clone());
    let mut once = true;
    st.before_capture = Some(Box::new(move || {
        if std::mem::take(&mut once) {
            f.set_clock(1_000);
            f.receive(0, &[0x90, 60, 100]);
            f.set_clock(2_000);
            f.begin_pass();
            let _ = tx2.send(Cmd::Recording(true, 0));
            f.set_clock(3_000);
            f.receive(0, &[0x90, 62, 100]);
        }
    }));
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 62)], "the key from after Record, not the one before");
}

#[test]
fn an_overflow_before_stop_whose_mark_never_came_is_in_the_take_though_an_older_mark_shows_a_later_close() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    heard(&mut st, 1);
    // Port 0 overflows, and plays a key while the audio thread is part of the way through the
    // queue: the mark of that overflow is in the queue, ahead of the key.
    feed.set_clock(1_000);
    for _ in 0..midi::INCOMING {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 60, 0]);
    assert_eq!(std::iter::from_fn(|| feed.next()).take(midi::INCOMING - 500).count(), midi::INCOMING - 500);
    feed.set_clock(2_000);
    feed.receive(0, &[0x90, 64, 100]);
    // The queue fills again and the key's release is dropped. Nothing more arrives, so that
    // overflow has no mark in the queue. Stop; and then the port is closed.
    feed.set_clock(3_000);
    for _ in 0..midi::INCOMING - 502 {
        feed.receive(1, &[0xB0, 1, 0]);
    }
    feed.receive(0, &[0x80, 64, 0]);
    feed.set_clock(3_500);
    let pass = feed.end_pass();
    feed.set_clock(4_000);
    feed.port_lost(0);
    tx.send(Cmd::Recording(false, pass)).unwrap();
    heard(&mut st, 1);
    assert!(feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    let port0: Vec<(u8, u8)> = rec.iter().filter(|e| e.port == 0).map(|e| (e.status, e.data1)).collect();
    assert_eq!(port0, [(lost, 0), (0x90, 64), (lost, 0)], "the key whose release was dropped has its end");
}

#[test]
fn a_loss_in_a_pass_begun_while_the_queue_is_being_read_is_written_into_the_take() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // The callback has seen that no pass is begun. Before it takes the queue in: a port is
    // lost (not part of the pass), Record, a key goes down, and its port is lost.
    let (f, tx2) = (Arc::clone(&feed), tx.clone());
    let mut once = true;
    st.before_capture = Some(Box::new(move || {
        if std::mem::take(&mut once) {
            f.set_clock(1_000);
            f.port_lost(1);
            f.set_clock(2_000);
            f.begin_pass();
            let _ = tx2.send(Cmd::Recording(true, 0));
            f.set_clock(3_000);
            f.receive(0, &[0x90, 62, 100]);
            f.set_clock(4_000);
            f.port_lost(0);
        }
    }));
    st.fill(&mut out, 2);
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    let lost = midi::RecordedMidi::LOST;
    assert_eq!(
        rec.iter().map(|e| (e.port, e.status, e.data1)).collect::<Vec<_>>(),
        [(0, 0x90, 62), (0, lost, 0)],
        "the key ends where its port went"
    );
}

#[test]
fn a_pass_started_and_ended_by_the_commands_of_one_callback_keeps_what_was_played_in_it() {
    let (mut st, tx, feed, ..) = keyboard_session();
    tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
    let mut out = vec![0.0f32; 2048];
    st.fill(&mut out, 2);
    // Record while playing. After the callback has read the MIDI queue, a key goes down and
    // recording is ended: both commands are read by this one callback.
    feed.set_clock(1_000);
    feed.begin_pass();
    tx.send(Cmd::Recording(true, 0)).unwrap();
    let ended = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (f, tx2, e) = (Arc::clone(&feed), tx.clone(), Arc::clone(&ended));
    st.before_commands = Some(Box::new(move || {
        if e.load(Ordering::Relaxed) == 0 {
            f.set_clock(2_000);
            f.receive(0, &[0x90, 60, 100]);
            f.set_clock(3_000);
            let pass = f.end_pass();
            e.store(pass, Ordering::Relaxed);
            let _ = tx2.send(Cmd::Recording(false, pass));
        }
    }));
    st.fill(&mut out, 2);
    let pass = ended.load(Ordering::Relaxed);
    assert!(pass != 0 && feed.captured(pass));
    let mut rec = Vec::new();
    feed.take_recorded(&mut rec);
    assert_eq!(rec.iter().map(|e| (e.status, e.data1)).collect::<Vec<_>>(), [(0x90, 60)]);
    // The next callback is outside any pass.
    st.before_commands = None;
    feed.set_clock(4_000);
    feed.receive(0, &[0x90, 62, 100]);
    st.fill(&mut out, 2);
    rec.clear();
    feed.take_recorded(&mut rec);
    assert!(rec.is_empty(), "{rec:?}");
}
