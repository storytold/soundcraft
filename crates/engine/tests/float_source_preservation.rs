use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::{Engine, io};
use soundcraft_model::{ChannelFormat, Clip, Session, SourceId, TrackKind};
use soundcraft_time::SampleRate;

fn generated_source() -> (Engine, SourceId, AudioBuffer) {
    let values = [1.25, -2.5, 0.0, -0.0, 3.75, -4.125, f32::from_bits(1), f32::from_bits(0x8000_0001)];
    let buffer = AudioBuffer {
        sample_rate: 48_000,
        channels: (0..6).map(|channel| (0..values.len()).map(|frame| values[(frame + channel) % values.len()]).collect()).collect(),
    };
    let mut session = Session::new("Float headroom", SampleRate::new(48_000).unwrap());
    let source = io::add_source(&mut session, "Generated headroom", buffer.clone(), None, FileFormat::Wav);
    let track = session.add_track(TrackKind::Audio, ChannelFormat::Surround51, None);
    let clip = session.new_clip_id();
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Generated", source, 0, 0, buffer.frames() as i64));
    (Engine::new(session), source, buffer)
}

fn assert_source_bits(engine: &Engine, source: SourceId, expected: &AudioBuffer) {
    let actual = &engine.session().pool.get(source).unwrap().buffer;
    assert_eq!(actual.sample_rate, expected.sample_rate);
    assert_eq!(actual.num_channels(), expected.num_channels());
    for (actual, expected) in actual.channels.iter().zip(&expected.channels) {
        assert_eq!(
            actual.iter().map(|sample| sample.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|sample| sample.to_bits()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn generated_headroom_survives_folder_save_reopen_and_copy() {
    let (mut engine, source, buffer) = generated_source();
    let dir = std::env::temp_dir().join(format!("soundcraft-float-source-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Original/Headroom.scraft").to_string_lossy().into_owned();
    assert_eq!(io::save_session(&mut engine, &path).unwrap(), 1);
    assert_source_bits(&engine, source, &buffer);
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, &path).unwrap().is_empty());
    assert_source_bits(&reopened, source, &buffer);

    let copy = dir.join("Copy/Headroom.scraft").to_string_lossy().into_owned();
    assert_eq!(io::save_session_copy(&reopened, &copy).unwrap(), 1);
    let mut copied = Engine::default();
    assert!(io::open_session(&mut copied, &copy).unwrap().is_empty());
    assert_source_bits(&copied, source, &buffer);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_headroom_survives_session_bundle_round_trip() {
    let (engine, source, buffer) = generated_source();
    let (bytes, _) = io::session_bundle(engine.session()).unwrap();
    let mut reopened = Engine::default();
    assert!(io::open_session_bytes(&mut reopened, "Headroom.scraftz", &bytes).unwrap().is_empty());
    assert_source_bits(&reopened, source, &buffer);
}
