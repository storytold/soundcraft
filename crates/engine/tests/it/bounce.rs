//! Bounces stream the main mix or a bus to their file, exactly as rendered in memory.

use craft_audio::{BitDepth, EncodeOptions};

use soundcraft_engine::io::{Bounce, bounce_stems};
use soundcraft_model::FileFormat;
use soundcraft_time::Range;
use std::path::PathBuf;

#[test]
fn float_bounce_preserves_headroom_and_reports_the_written_levels() {
    let mut e = soundcraft_engine::Engine::default();
    let sr = e.session().sample_rate.hz();
    let frames = sr as usize + 17;
    let samples: Vec<f32> = (0..frames).map(|i| 1.5 * (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin()).collect();
    let audio = craft_audio::AudioBuffer { sample_rate: sr, channels: vec![samples.clone(), samples] };
    let s = e.session_mut();
    // A new track has no inserts, so no limiter can mask encoder clipping.
    let track = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Headroom"));
    let source = soundcraft_engine::io::add_source(s, "headroom", audio, None, FileFormat::Wav);
    let clip = soundcraft_model::Clip::audio(s.new_clip_id(), "headroom", source, 0, 0, frames as i64);
    soundcraft_engine::edit::place_clip(s, track, clip);
    let mix = soundcraft_mix::render_range(e.session(), Range::new(0, frames as i64), 1024);
    assert!(soundcraft_dsp::offline::peak(&mix) > 1.0);
    let dir = scratch("bounce-float-headroom");
    for extension in ["wav", "aiff"] {
        for normalize in [false, true] {
            let path = dir.join(format!("Mix.{extension}"));
            let result = e
                .execute(
                    "file.bounce_mix",
                    &serde_json::json!({
                        "path": path, "bit_depth": "32f", "dither": false, "normalize": normalize,
                        "start": 0, "end": frames,
                    }),
                )
                .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let (_, file) = craft_audio::decode(&bytes, None).unwrap();
            let mut expected = mix.clone();
            if normalize {
                soundcraft_dsp::offline::normalize(&mut expected, -0.1, false);
                assert!(file.peak() < 1.0);
            } else {
                assert!(file.peak() > 1.0, "{extension}: headroom was clipped");
            }
            assert_eq!(file.channels, expected, "{extension}, normalize={normalize}");
            let mut meter = soundcraft_dsp::meter::LoudnessMeter::new(sr as f32, file.channels.len());
            meter.process(&file.channels, file.frames());
            for (key, value) in [
                ("peak_db", soundcraft_dsp::offline::peak_db(&file.channels)),
                ("lufs", meter.integrated_lufs()),
                ("true_peak_db", meter.true_peak_db()),
            ] {
                assert_eq!(result[key].as_f64(), Some(f64::from(value)), "{extension}, normalize={normalize}, {key}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_integer_bounce_reports_the_levels_of_the_clipped_file() {
    let mut e = soundcraft_engine::Engine::default();
    let sr = e.session().sample_rate.hz();
    let frames = sr as usize;
    let samples: Vec<f32> = (0..frames).map(|i| 1.5 * (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin()).collect();
    let audio = craft_audio::AudioBuffer { sample_rate: sr, channels: vec![samples.clone(), samples] };
    let s = e.session_mut();
    let track = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Hot"));
    let source = soundcraft_engine::io::add_source(s, "hot", audio, None, FileFormat::Wav);
    let clip = soundcraft_model::Clip::audio(s.new_clip_id(), "hot", source, 0, 0, frames as i64);
    soundcraft_engine::edit::place_clip(s, track, clip);
    let mix = soundcraft_mix::render_range(e.session(), Range::new(0, frames as i64), 1024);
    assert!(soundcraft_dsp::offline::peak(&mix) > 1.0);
    let dir = scratch("bounce-int-clipped");
    let path = dir.join("Mix.wav");
    let r = e.execute("file.bounce_mix", &serde_json::json!({"path": path, "bit_depth": 16, "dither": false, "start": 0, "end": frames})).unwrap();
    let (_, file) = craft_audio::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    assert!(file.peak() <= 1.0);
    let mut meter = soundcraft_dsp::meter::LoudnessMeter::new(sr as f32, file.num_channels());
    meter.process(&file.channels, file.frames());
    for (key, value, tolerance) in [
        ("peak_db", soundcraft_dsp::offline::peak_db(&file.channels), 0.01),
        ("lufs", meter.integrated_lufs(), 0.05),
        ("true_peak_db", meter.true_peak_db(), 0.05),
    ] {
        let reported = r[key].as_f64().unwrap();
        assert!((reported - f64::from(value)).abs() < tolerance, "{key}: reported {reported}, file {value}");
    }
    assert!(r["peak_db"].as_f64().unwrap() <= 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bounces_write_exactly_the_mix_and_report_its_levels() {
    let e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-exact");
    let path = dir.join("Mix.wav");
    let r = Range::new(s.sample_rate.samples(10.0) + 7, s.sample_rate.samples(13.0));
    let opts = EncodeOptions::new(craft_audio::EncodeFormat::Wav).with_bit_depth(BitDepth::Float32);
    let verb = s.bus_by_name("Verb").unwrap().id;
    for bus in [None, Some(verb)] {
        let mut mix = match bus {
            Some(b) => soundcraft_mix::render_bus(s, b, r, 1024),
            None => soundcraft_mix::render_range(s, r, 1024),
        };
        assert!(mix.iter().flatten().any(|v| v.abs() > 1e-3), "{bus:?}");
        for normalize in [false, true] {
            let mut bounce = Bounce::new(e.session_arc(), &path, r, &opts, bus, normalize, false).unwrap();
            let mut progress = vec![bounce.progress()];
            let done = loop {
                if let Some(done) = bounce.step(|| false).unwrap() {
                    break done;
                }
                progress.push(bounce.progress());
            };
            let last = progress[progress.len() - 1];
            assert!(progress.len() > 100 && progress.is_sorted() && progress[0] < 0.01 && last > 0.99 && last <= 1.0, "{progress:?}");
            if normalize {
                soundcraft_dsp::offline::normalize(&mut mix, -0.1, false);
            }
            let bytes = std::fs::read(&path).unwrap();
            let (_, file) = craft_audio::decode(&bytes, None).unwrap();
            assert_eq!(file.channels, mix, "{bus:?} normalize: {normalize}");
            let mut lm = soundcraft_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, mix.len());
            lm.process(&mix, mix[0].len());
            let levels = (soundcraft_dsp::offline::peak_db(&mix), lm.integrated_lufs(), lm.true_peak_db());
            assert_eq!((done.bytes, (done.peak_db, done.lufs, done.true_peak_db)), (bytes.len() as u64, levels));
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bounce_too_big_for_its_format_fails_and_keeps_the_old_file() {
    let e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-too-big");
    let path = dir.join("Mix.aif");
    std::fs::write(&path, b"old").unwrap();
    let opts = EncodeOptions::new(craft_audio::EncodeFormat::Aiff).with_bit_depth(BitDepth::Float32);
    let err = Bounce::new(e.session_arc(), &path, Range::new(0, s.sample_rate.samples(3.9 * 3600.0)), &opts, None, false, false).err().unwrap();
    assert!(err.to_string().contains("4 GiB"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let err = bounce_stems(&e, &dir.to_string_lossy(), Range::new(0, s.sample_rate.samples(4.1 * 3600.0)), &opts).unwrap_err();
    assert!(err.to_string().contains("4 hours"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A WAV bounce is a Broadcast Wave stamped with where it starts on the session's timecode (`bext`
/// TimeReference, samples since midnight), as other DAWs write it, so post tools can place it.
#[test]
fn wav_bounces_carry_their_timecode_position_as_a_bwf_time_reference() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let sr = u64::from(e.session().sample_rate.hz());
    // The session starts at 01:00:00:00; the bounce at 2 s into it.
    e.session_mut().timecode_start = (3600 * sr) as i64;
    let (start, end) = (2 * sr, 3 * sr);
    let dir = scratch("bounce-bext");
    let stamp = |path: &std::path::Path| {
        let ext = path.extension().and_then(|x| x.to_str());
        craft_audio::probe(&std::fs::read(path).unwrap(), ext).unwrap().bwf.map(|b| b.time_reference)
    };
    let mix = dir.join("Mix.wav");
    e.execute("file.bounce_mix", &serde_json::json!({"path": mix, "start": start, "end": end})).unwrap();
    assert_eq!(stamp(&mix), Some(3600 * sr + start), "mix");
    let stems = dir.join("stems");
    let files = e.execute("file.bounce_stems", &serde_json::json!({"dir": stems, "start": start, "end": end})).unwrap();
    let files = files["files"].as_array().unwrap();
    assert!(!files.is_empty());
    for f in files {
        let f = std::path::PathBuf::from(f.as_str().unwrap());
        assert_eq!(stamp(&f), Some(3600 * sr + start), "{}", f.display());
    }
    // A session that starts at zero still says where the bounce starts.
    e.session_mut().timecode_start = 0;
    e.execute("file.bounce_mix", &serde_json::json!({"path": mix, "start": start, "end": end})).unwrap();
    assert_eq!(stamp(&mix), Some(start));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_an_unfinished_bounce_cancels_it_and_keeps_the_old_file() {
    let e = soundcraft_engine::demo::demo_engine();
    let dir = scratch("bounce-cancel");
    let path = dir.join("Mix.wav");
    std::fs::write(&path, b"old").unwrap();
    let mut bounce = Bounce::new(e.session_arc(), &path, Range::new(0, 48_000), &EncodeOptions::default(), None, false, false).unwrap();
    assert_eq!(bounce.step(|| false).unwrap(), None);
    assert!(dir.join("Mix.wav.part").exists());
    drop(bounce);
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

const PINNED: [[u32; 3]; 2] = [[0xc195_6706, 0xc093_0031, 0xc093_0031], [0xc162_e790, 0xbdcc_ccbf, 0xbdcc_ccbf]];

#[test]
fn demo_bounce_levels_are_pinned() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let dir = scratch("bounce-demo-pinned");
    let mut got = Vec::new();
    for (name, extra) in [("Mix.wav", serde_json::json!({})), ("Norm.wav", serde_json::json!({"normalize": true, "bit_depth": 16}))] {
        let path = dir.join(name);
        let mut p = serde_json::json!({"path": path, "start": 1_296_000, "end": 1_440_000});
        if let (Some(m), Some(x)) = (p.as_object_mut(), extra.as_object()) {
            m.extend(x.clone());
        }
        let r = e.execute("file.bounce_mix", &p).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 44, "{name} has audio");
        let level = |k: &str| r[k].as_f64().unwrap() as f32;
        got.push([level("lufs"), level("true_peak_db"), level("peak_db")]);
    }
    let _ = std::fs::remove_dir_all(&dir);
    for (g, pin) in got.iter().zip(PINNED) {
        assert!(g.iter().zip(pin).all(|(v, p)| (v - f32::from_bits(p)).abs() < 1e-3), "{got:?}");
    }
}
