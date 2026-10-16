use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, Session, TrackKind};
use soundcraft_time::{Range, SampleRate};
use std::sync::Arc;

fn fixture(format: ChannelFormat) -> Engine {
    let mut session = Session::new("Rate delivery", SampleRate::HZ_48000);
    session.outputs[0].format = format;
    let track = session.add_track(TrackKind::Audio, format, Some("Distinct signals"));
    let channels = (0..format.channels())
        .map(|ch| {
            (0..26_000)
                .map(|i| {
                    let t = i as f32 / 48_000.0;
                    (std::f32::consts::TAU * (220.0 + 137.0 * ch as f32) * t).sin() * (ch + 1) as f32 / 40.0
                })
                .collect()
        })
        .collect();
    let source = io::add_source(&mut session, "Signal", AudioBuffer { sample_rate: 48_000, channels }, None, FileFormat::Wav);
    let clip = session.new_clip_id();
    let tr = session.track_mut(track).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Signal", source, 0, 0, 26_000));
    let mut shift = Insert::new("time_shift");
    shift.params.insert("delay_ms".into(), 3.0);
    tr.mixer.inserts[0] = Some(shift);
    Engine::new(session)
}

fn extension(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Wav => "wav",
        FileFormat::Aiff => "aiff",
        FileFormat::Flac => "flac",
        _ => panic!("fixture format"),
    }
}

fn opts(format: FileFormat, depth: BitDepth) -> EncodeOptions {
    EncodeOptions { format, bit_depth: depth, dither: true, bwf: None }
}

fn old_delivery(e: &Engine, range: Range, options: &EncodeOptions, normalize: bool) -> (Vec<u8>, (f32, f32, f32)) {
    let mut channels = soundcraft_mix::render_range(e.session(), range, 1024);
    if normalize {
        soundcraft_dsp::offline::normalize(&mut channels, -0.1, false);
    }
    let stats = meter(&channels, 48_000);
    let audio = AudioBuffer { sample_rate: 48_000, channels };
    (soundcraft_audio_io::encode_with_channel_mask(&audio, options, e.session().main_format().channel_mask()).unwrap(), stats)
}

fn meter(channels: &[Vec<f32>], rate: u32) -> (f32, f32, f32) {
    let mut meter = soundcraft_dsp::meter::LoudnessMeter::new(rate as f32, channels.len());
    meter.process(channels, channels[0].len());
    (soundcraft_dsp::offline::peak_db(channels), meter.integrated_lufs(), meter.true_peak_db())
}

fn stats_bits(stats: (f32, f32, f32)) -> [u32; 3] {
    [stats.0.to_bits(), stats.1.to_bits(), stats.2.to_bits()]
}

#[test]
fn default_wrappers_and_explicit_session_rate_retain_old_bytes_and_measurements() {
    let e = fixture(ChannelFormat::Surround60);
    let range = Range::new(1003, 22004);
    for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
        for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Float32] {
            for normalize in [false, true] {
                let options = opts(format, depth);
                if format == FileFormat::Flac && depth == BitDepth::Float32 {
                    let previous_error = soundcraft_audio_io::encode_with_channel_mask(
                        &AudioBuffer { sample_rate: 48000, channels: soundcraft_mix::render_range(e.session(), range, 1024) },
                        &options,
                        e.session().main_format().channel_mask(),
                    )
                    .unwrap_err()
                    .to_string();
                    assert!(io::bounce_bytes(&e, range, &options, normalize).unwrap_err().to_string().contains(&previous_error));
                    assert!(
                        io::bounce_source_at_rate(&e, range, &options, normalize, false, None, SampleRate::HZ_48000)
                            .unwrap_err()
                            .to_string()
                            .contains(&previous_error)
                    );
                    continue;
                }
                let expected = old_delivery(&e, range, &options, normalize);
                let old_api = io::bounce_bytes(&e, range, &options, normalize).unwrap();
                let explicit = io::bounce_source_at_rate(&e, range, &options, normalize, false, None, SampleRate::HZ_48000).unwrap();
                assert_eq!(old_api.0, expected.0);
                assert_eq!(explicit.bytes, expected.0);
                assert_eq!(stats_bits(old_api.1), stats_bits(expected.1));
                assert_eq!(stats_bits(explicit.stats), stats_bits(expected.1));
                assert_eq!(explicit.frames, 21001);
                assert_eq!(explicit.sample_rate.hz(), 48000);
            }
        }
    }
}

#[test]
fn converted_multichannel_delivery_has_exact_frames_mask_signal_and_output_rate_metrics() {
    let mut e = fixture(ChannelFormat::Surround60);
    let range = Range::new(1003, 22004);
    let dir = std::env::temp_dir().join(format!("sc-bounce-rate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let before = e.session_arc();
    for rate in [44_100, 48_000, 96_000] {
        for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
            let path = dir.join(format!("{rate}-{}.audio", extension(format)));
            let value = e
                .execute(
                    "file.bounce_mix",
                    &json!({"path": path, "format": extension(format), "bit_depth": "24", "dither": false,
                "start": range.start, "end": range.end, "sample_rate": rate, "normalize": true}),
                )
                .unwrap();
            let expected_frames = (21001_u64 * u64::from(rate) + 24000) / 48000;
            assert_eq!(value["sample_rate"], rate);
            assert_eq!(value["frames"], expected_frames);
            assert_eq!(value["seconds"].as_f64().unwrap(), expected_frames as f64 / f64::from(rate));
            let bytes = std::fs::read(&path).unwrap();
            let (_, decoded) = soundcraft_audio_io::decode(&bytes, Some(extension(format))).unwrap();
            assert_eq!(decoded.sample_rate, rate);
            assert_eq!(decoded.frames(), expected_frames as usize);
            assert_eq!(decoded.num_channels(), 6);
            let rendered = soundcraft_mix::render_range(e.session(), range, 1024);
            let mut reference = if rate == 48000 { rendered } else { soundcraft_dsp::offline::resample(&rendered, 48000, rate) };
            soundcraft_dsp::offline::normalize(&mut reference, -0.1, false);
            let expected_stats = meter(&reference, rate);
            for (name, expected) in [("peak_db", expected_stats.0), ("lufs", expected_stats.1), ("true_peak_db", expected_stats.2)] {
                assert_eq!(value[name].as_f64().unwrap(), f64::from(expected), "{name} at {rate}");
            }
            for (ch, expected) in decoded.channels.iter().zip(&reference) {
                assert_eq!(ch.len(), expected.len());
                for (sample, expected) in ch.iter().zip(expected) {
                    assert!((sample - expected).abs() < 2e-7);
                }
            }
            if format == FileFormat::Wav {
                let at = bytes.windows(4).position(|tag| tag == b"fmt ").unwrap();
                assert_eq!(u32::from_le_bytes(bytes[at + 28..at + 32].try_into().unwrap()), ChannelFormat::Surround60.channel_mask());
            }
            assert!(Arc::ptr_eq(&before, &e.session_arc()), "bounce must not rewrite source, rate, clip, routing or plugin state");
            assert!(!e.is_dirty());
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn converted_bus_and_stereo_fold_retain_routing_and_delivery_shape() {
    let mut e = fixture(ChannelFormat::Surround60);
    let bus = e.session_mut().add_bus("Stem", ChannelFormat::Surround60);
    e.session_mut().tracks[0].mixer.output = Route::Bus(bus);
    let range = Range::new(1003, 22004);
    for fold in [false, true] {
        let output =
            io::bounce_source_at_rate(&e, range, &opts(FileFormat::Wav, BitDepth::Float32), false, fold, Some(bus), SampleRate::new(96000).unwrap())
                .unwrap();
        let (_, audio) = soundcraft_audio_io::decode(&output.bytes, Some("wav")).unwrap();
        let bus_audio = soundcraft_mix::render_bus(e.session(), bus, range, 1024);
        let delivered = if fold { soundcraft_mix::fold_down_stereo(ChannelFormat::Surround60, &bus_audio) } else { bus_audio };
        let expected = soundcraft_dsp::offline::resample(&delivered, 48000, 96000);
        assert_eq!(audio.channels, expected);
        assert_eq!(audio.num_channels(), if fold { 2 } else { 6 });
        assert_eq!(audio.frames(), 42002);
    }
}

#[test]
fn half_frame_rounds_up_and_actual_delivery_duration_is_reported() {
    let mut e = fixture(ChannelFormat::Stereo);
    let dir = std::env::temp_dir().join(format!("sc-bounce-half-{}", std::process::id()));
    let value = e.execute("file.bounce_mix", &json!({"path": dir, "start": 200, "end": 201, "sample_rate": 24000})).unwrap();
    let (_, audio) = soundcraft_audio_io::decode(&std::fs::read(&dir).unwrap(), Some("wav")).unwrap();
    assert_eq!(audio.frames(), 1);
    assert_eq!(value["frames"], 1);
    assert_eq!(value["seconds"].as_f64().unwrap(), 1.0 / 24000.0);
    std::fs::remove_file(&dir).unwrap();
}

#[test]
fn malformed_zero_output_and_huge_requests_fail_before_writes_without_session_changes() {
    let mut e = fixture(ChannelFormat::Surround60);
    let path = std::env::temp_dir().join(format!("sc-bounce-reject-{}.wav", std::process::id()));
    let before = e.session_arc();
    let malformed = [
        json!(null),
        json!("44100"),
        json!(true),
        json!(48000.5),
        json!(-1),
        json!(0),
        json!(7999),
        json!(768001),
        json!(u64::MAX),
        json!({}),
        json!([]),
    ];
    for rate in malformed {
        let error = e.execute("file.bounce_mix", &json!({"path": path, "start": 100, "end": 200, "sample_rate": rate})).unwrap_err();
        assert!(error.to_string().contains("sample"));
        assert!(!path.exists());
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
    }
    let requests = [
        json!({"start": 100, "end": 101, "sample_rate": 8000}),
        json!({"start": 0, "end": (soundcraft_mix::MAX_RENDER_SAMPLES / 6 + 1), "sample_rate": 48000}),
        json!({"start": 0, "end": 4_000_000, "sample_rate": 768000}),
        json!({"start": 0, "end": 80_000_000, "sample_rate": 24000}),
        json!({"start": i64::MIN, "end": i64::MAX, "sample_rate": 8000}),
        json!({"start": 0, "end": 691_200_001, "sample_rate": 8000}),
    ];
    for mut request in requests {
        request["path"] = json!(path);
        let error = e.execute("file.bounce_mix", &request).unwrap_err();
        assert!(!matches!(error, soundcraft_engine::EngineError::Internal(..)), "{error}");
        assert!(!path.exists());
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
    }
    // An invalid conversion must also leave an existing destination intact.
    std::fs::write(&path, b"keep existing destination").unwrap();
    assert!(e.execute("file.bounce_mix", &json!({"path": path, "start": 0, "end": 4000000, "sample_rate": 768000})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"keep existing destination");
    assert!(!e.is_dirty());
    assert!(!e.can_undo());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn supported_rate_boundaries_and_mono_bus_working_width_are_validated() {
    let mut e = fixture(ChannelFormat::Stereo);
    let options = opts(FileFormat::Wav, BitDepth::Float32);
    for (rate, frames) in [(8000, 11), (768000, 1024)] {
        let output = io::bounce_source_at_rate(&e, Range::new(1003, 1067), &options, false, false, None, SampleRate::new(rate).unwrap()).unwrap();
        let (_, audio) = soundcraft_audio_io::decode(&output.bytes, Some("wav")).unwrap();
        assert_eq!(audio.sample_rate, rate);
        assert_eq!(audio.frames(), frames);
    }
    let mono = e.session_mut().add_bus("Mono", ChannelFormat::Mono);
    let too_long = Range::new(0, (soundcraft_mix::MAX_RENDER_SAMPLES / 2 + 1) as i64);
    assert!(
        io::bounce_source_at_rate(&e, too_long, &options, false, false, Some(mono), SampleRate::HZ_48000)
            .unwrap_err()
            .to_string()
            .contains("allocation limit")
    );
    let wide = e.session_mut().add_bus("Wide", ChannelFormat::Ambisonics(4));
    assert!(
        io::bounce_source_at_rate(&e, Range::new(0, 10), &options, false, false, Some(wide), SampleRate::HZ_48000)
            .unwrap_err()
            .to_string()
            .contains("at most 16")
    );
    e.session_mut().outputs[0].format = ChannelFormat::Ambisonics(4);
    let before = e.session_arc();
    let path = std::env::temp_dir().join(format!("sc-wide-main-{}.wav", std::process::id()));
    for fold in ["none", "stereo"] {
        let error = e.execute("file.bounce_mix", &json!({"path": path, "start": 0, "end": 10, "sample_rate": 48000, "fold_down": fold})).unwrap_err();
        assert!(error.to_string().contains("at most 16"));
        assert!(!path.exists(), "an unsupported Main layout must not silently become stereo");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
    }
    e.session_mut().sample_rate = serde_json::from_value(json!(0)).unwrap();
    assert!(io::bounce_bytes(&e, Range::new(0, 10), &options, false).is_err());
}

#[test]
fn omitted_rate_keeps_large_streaming_default_but_explicit_delivery_is_bounded() {
    let e = fixture(ChannelFormat::Stereo);
    let dir = std::env::temp_dir().join(format!("sc-bounce-stream-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mix.wav");
    std::fs::write(&path, b"existing mix").unwrap();
    let end = soundcraft_mix::MAX_RENDER_SAMPLES / 2 + 1;
    let mut params = json!({"path":path,"start":0,"end":end,"bit_depth":"32f"});
    params["sample_rate"] = json!(48000);
    assert!(soundcraft_engine::cmd::bounce_job(&e, &params).err().unwrap().to_string().contains("allocation limit"));
    assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
    assert!(!dir.join("Mix.wav.part").exists());
    params.as_object_mut().unwrap().remove("sample_rate");
    let job = soundcraft_engine::cmd::bounce_job(&e, &params).unwrap();
    assert_eq!(job.sample_rate(), SampleRate::HZ_48000);
    assert_eq!(job.frames(), end);
    assert_eq!(job.progress(), 0.0);
    drop(job); // Never render or allocate the large delivery.
    assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn converted_job_yields_during_src_and_cancel_preserves_destination_and_source() {
    let e = fixture(ChannelFormat::Stereo);
    let before = e.session_arc();
    let dir = std::env::temp_dir().join(format!("sc-bounce-src-cancel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mix.wav");
    std::fs::write(&path, b"existing mix").unwrap();
    let mut job = io::Bounce::new_at_rate(
        e.session_arc(),
        &path,
        Range::new(200, 16000),
        &opts(FileFormat::Wav, BitDepth::Int24),
        None,
        true,
        false,
        SampleRate::new(96000).unwrap(),
    )
    .unwrap();
    let mut last = job.progress();
    let mut slices = 0;
    while job.progress() < 0.5 {
        assert!(job.step(|| false).unwrap().is_none());
        let progress = job.progress();
        assert!(progress >= last && progress < 0.85);
        last = progress;
        slices += 1;
        assert!(slices < 10_000);
    }
    assert!(slices > 2, "rendering, kernel preparation and SRC must yield");
    assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
    assert!(dir.join("Mix.wav.part").exists());
    drop(job);
    assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn default_streaming_constructor_retains_empty_render_while_explicit_delivery_rejects_it() {
    let e = fixture(ChannelFormat::Stereo);
    let dir = std::env::temp_dir().join(format!("sc-bounce-empty-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mix.wav");
    let options = opts(FileFormat::Wav, BitDepth::Float32);
    let range = Range::point(200);
    assert!(io::Bounce::new_at_rate(e.session_arc(), &path, range, &options, None, false, false, SampleRate::HZ_48000).is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let job = io::Bounce::new(e.session_arc(), &path, range, &options, None, false, false).unwrap();
    assert_eq!(job.frames(), 0);
    job.run().unwrap();
    let (_, audio) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), Some("wav")).unwrap();
    assert_eq!(audio.frames(), 0);
    assert_eq!(audio.sample_rate, 48000);
    assert_eq!(audio.num_channels(), 2);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn failed_output_open_preserves_an_unowned_part_symlink_and_destination() {
    let e = fixture(ChannelFormat::Stereo);
    let dir = std::env::temp_dir().join(format!("sc-bounce-unowned-part-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mix.wav");
    let part = dir.join("Mix.wav.part");
    let absent_target = dir.join("missing-parent").join("output.wav");
    std::fs::write(&path, b"existing mix").unwrap();
    std::os::unix::fs::symlink(&absent_target, &part).unwrap();
    let options = opts(FileFormat::Wav, BitDepth::Int24);
    for rate in [None, Some(SampleRate::new(96000).unwrap())] {
        let result = match rate {
            None => io::Bounce::new(e.session_arc(), &path, Range::new(200, 264), &options, None, false, false),
            Some(rate) => io::Bounce::new_at_rate(e.session_arc(), &path, Range::new(200, 264), &options, None, false, false, rate),
        };
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing mix");
        assert!(std::fs::symlink_metadata(&part).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&part).unwrap(), absent_target);
        assert!(!absent_target.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
