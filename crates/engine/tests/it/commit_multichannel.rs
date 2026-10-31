use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, Insert, Route, SendSlot, Session, TrackId, TrackKind};
use soundcraft_time::{Range, SampleRate};
use std::sync::Arc;

const SIGNAL_FRAMES: usize = 128;

fn fixture(format: ChannelFormat) -> (Engine, TrackId, Vec<Vec<f32>>) {
    let mut session = Session::new("Commit layout", SampleRate::new(8_000).unwrap());
    session.outputs[0].format = format;
    let track = session.add_track(TrackKind::Audio, format, Some("Distinct channels"));
    // Each channel has a different bipolar waveform: swapping, summing or dropping channels
    // cannot pass a width-only assertion or an identical-signal comparison.
    let channels: Vec<Vec<f32>> = (0..format.channels())
        .map(|ch| (0..SIGNAL_FRAMES).map(|frame| (((frame * (ch + 3)) % 31) as f32 - 15.0) * (ch + 1) as f32 / 3_200.0).collect())
        .collect();
    let source =
        io::add_source(&mut session, "Distinct channels", AudioBuffer { sample_rate: 8_000, channels: channels.clone() }, None, FileFormat::Wav);
    let clip = session.new_clip_id();
    let tr = session.track_mut(track).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Signal", source, 0, 0, SIGNAL_FRAMES as i64));
    tr.mixer.inserts[0] = Some(Insert::new("invert"));
    session.edit.selected_tracks = vec![track];
    (Engine::new(session), track, channels)
}

fn commit(e: &mut Engine, track: TrackId) -> TrackId {
    TrackId(e.execute("track.commit", &json!({"tracks": [track.0]})).unwrap()["tracks"][0].as_u64().unwrap())
}

fn committed_audio(e: &Engine, track: TrackId) -> &AudioBuffer {
    let ClipContent::Audio { source, .. } = e.session().track(track).unwrap().clips()[0].content else { panic!("commit must create audio") };
    &e.session().pool.get(source).unwrap().buffer
}

fn assert_audio_eq(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), expected.len());
    for (ch, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual.len(), expected.len(), "channel {ch} length");
        for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!((actual - expected).abs() < 1e-6, "channel {ch}, frame {frame}: {actual} != {expected}");
        }
    }
}

#[test]
fn commit_keeps_every_renderable_exact_format_and_processed_channel_order() {
    for format in ChannelFormat::ALL.into_iter().filter(|format| format.channels() <= soundcraft_mix::MAX_CHANNELS) {
        let (mut e, track, signal) = fixture(format);
        let committed = commit(&mut e, track);
        assert_eq!(e.session().track(committed).unwrap().format, format, "{}", format.label());
        let audio = committed_audio(&e, committed);
        assert_eq!(audio.num_channels(), format.channels(), "{}", format.label());
        assert_eq!(audio.frames(), SIGNAL_FRAMES + 16_000);
        // Existing mono rendering uses a -3 dB center pan followed by SQRT_2 compensation.
        // Model that small baseline gain explicitly, retaining the same strict signal tolerance.
        let gain = if format == ChannelFormat::Mono { 10.0_f32.powf(-3.0 / 20.0) * std::f32::consts::SQRT_2 } else { 1.0 };
        let expected: Vec<Vec<f32>> = signal.iter().map(|ch| ch.iter().map(|sample| -sample * gain).collect()).collect();
        let head: Vec<Vec<f32>> = audio.channels.iter().map(|ch| ch[..SIGNAL_FRAMES].to_vec()).collect();
        assert_audio_eq(&head, &expected);
        assert!(audio.channels.iter().all(|ch| ch[SIGNAL_FRAMES..].iter().all(|sample| sample.abs() < 1e-6)));
        assert!(e.session().track(track).unwrap().inactive && e.session().track(track).unwrap().hidden);
        assert_eq!(e.session().track_index(committed), e.session().track_index(track).map(|index| index + 1));
    }
}

#[test]
fn commit_preserves_bus_and_send_audio_after_undo_reopen_and_wav_export() {
    for format in [ChannelFormat::Surround60, ChannelFormat::Atmos504, ChannelFormat::Ambisonics(1)] {
        let (mut e, track, _) = fixture(format);
        let s = e.session_mut();
        let bus = s.add_bus("Same-layout bus", format);
        let aux = s.add_track(TrackKind::Aux, format, Some("Bus return"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let tr = s.track_mut(track).unwrap();
        tr.mixer.output = Route::Bus(bus);
        tr.mixer.volume_db = -4.5;
        let mut send = SendSlot::new(Route::Main);
        send.level_db = -9.0;
        tr.mixer.sends[0] = Some(send);
        let mixer = tr.mixer.clone();
        let range = Range::new(0, SIGNAL_FRAMES as i64);
        let before = soundcraft_mix::render_range(e.session(), range, 64);
        let committed = commit(&mut e, track);
        let tr = e.session().track(committed).unwrap();
        assert_eq!(tr.mixer.output, mixer.output);
        assert_eq!(tr.mixer.sends, mixer.sends);
        assert_eq!(tr.mixer.volume_db, mixer.volume_db);
        assert!(tr.mixer.inserts.iter().all(Option::is_none), "processing is baked once");
        assert_audio_eq(&soundcraft_mix::render_range(e.session(), range, 64), &before);
        assert!(e.undo());
        assert!(e.session().track(committed).is_none());
        assert!(!e.session().track(track).unwrap().inactive);
        assert_audio_eq(&soundcraft_mix::render_range(e.session(), range, 64), &before);
        assert!(e.redo());

        let dir = std::env::temp_dir().join(format!("soundcraft-commit-layout-{}-{}", std::process::id(), format.channels()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Commit.scraft").to_string_lossy().into_owned();
        io::save_session(&mut e, &path).unwrap();
        let mut reopened = Engine::default();
        assert!(io::open_session(&mut reopened, &path).unwrap().is_empty());
        assert_eq!(reopened.session().track(committed).unwrap().format, format);
        assert_audio_eq(&committed_audio(&reopened, committed).channels, &committed_audio(&e, committed).channels);
        assert_audio_eq(&soundcraft_mix::render_range(reopened.session(), range, 64), &before);

        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        let wav_path = dir.join("mix.wav");
        io::Bounce::new(reopened.session_arc(), &wav_path, range, &opts, None, false, false).unwrap().run().unwrap();
        let wav = std::fs::read(&wav_path).unwrap();
        let (_, decoded) = soundcraft_audio_io::decode(&wav, Some("wav")).unwrap();
        assert_audio_eq(&decoded.channels, &before);
        // The WAV extensible mask must describe this layout, not another layout of equal width.
        let fmt_at = wav.windows(4).position(|tag| tag == b"fmt ").unwrap();
        let mask = u32::from_le_bytes(wav[fmt_at + 28..fmt_at + 32].try_into().unwrap());
        assert_eq!(mask, format.channel_mask());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn oversized_or_overflowing_commit_rolls_back_all_selected_tracks() {
    for (format, length) in [
        (ChannelFormat::Mono, (soundcraft_mix::MAX_RENDER_SAMPLES / 2 + 1) as i64),
        (ChannelFormat::Surround51, (soundcraft_mix::MAX_RENDER_SAMPLES / 6 + 1) as i64),
        (ChannelFormat::Ambisonics(3), (soundcraft_mix::MAX_RENDER_SAMPLES / 16 + 1) as i64),
        (ChannelFormat::Surround51, i64::MAX),
    ] {
        let (mut e, first, _) = fixture(format);
        let s = e.session_mut();
        let second = s.add_track(TrackKind::Audio, format, Some("Too long"));
        let mut clip = s.track(first).unwrap().clips()[0].clone();
        clip.id = s.new_clip_id();
        clip.length = length;
        s.track_mut(second).unwrap().playlist_mut().unwrap().clips.push(clip);
        let before = e.session_arc();
        let dirty = e.is_dirty();
        let result = e.execute("track.commit", &json!({"tracks": [first.0, second.0]}));
        assert!(result.unwrap_err().to_string().contains("offline render limit"));
        assert!(Arc::ptr_eq(&before, &e.session_arc()), "no hidden track, truncated source or partially committed first track");
        assert_eq!(e.is_dirty(), dirty);
        assert!(!e.can_undo(), "failed commit must not add an undo step");
    }
}

#[test]
fn commit_rejects_formats_wider_than_the_renderer_without_replacing_tracks() {
    for format in ChannelFormat::ALL.into_iter().filter(|format| format.channels() > soundcraft_mix::MAX_CHANNELS) {
        let (mut e, track, _) = fixture(format);
        let before = e.session_arc();
        let result = e.execute("track.commit", &json!({"tracks": [track.0]}));
        assert!(result.unwrap_err().to_string().contains("at most 16 channels"));
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert!(!e.session().track(track).unwrap().inactive);
        assert!(!e.session().track(track).unwrap().hidden);
        assert!(!e.can_undo());
    }
}
