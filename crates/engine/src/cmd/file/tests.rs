use super::*;
use crate::Engine;
use soundcraft_audio_io::{AudioBuffer, EncodeOptions};
use soundcraft_model::ChannelFormat;

fn tone_wav(dir: &std::path::Path, name: &str) -> String {
    let tone: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.1).sin() * 0.2).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
    let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

/// Issue #597: the app decodes on a worker thread and hands the audio over; the command then
/// only places it. Audio decoded for another rate is ignored and the file is read again.
#[test]
fn import_audio_places_audio_offered_by_a_worker() {
    let dir = std::env::temp_dir().join(format!("sc-import-offered-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wav = tone_wav(&dir, "tone.wav");

    let mut e = Engine::default();
    let rate = e.session().sample_rate.hz();
    let decoded = std::thread::spawn({
        let wav = wav.clone();
        move || crate::io::decode_import(&wav, rate)
    })
    .join()
    .unwrap()
    .unwrap();
    assert_eq!(decoded.frames(), 4_800);
    e.offer_import(decoded);
    // The file changes after decoding: the offered audio is placed, not the new bytes.
    std::fs::write(&wav, b"not audio").unwrap();
    let r = e.execute("file.import_audio", &json!({"path": wav, "at": 960})).unwrap();
    assert_eq!(r["imported"][0]["frames"], 4_800);
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].clips()[0].start, 960);
    assert_eq!(e.session().sources[0].path, wav);
    // Taken once: the next import reads the (now broken) file.
    assert!(e.execute("file.import_audio", &json!({"path": wav})).is_err());

    // Decoded for another rate: ignored, so the broken file is read and refused.
    let good = tone_wav(&dir, "good.wav");
    let other = crate::io::decode_import(&good, rate / 2).unwrap();
    std::fs::write(&good, b"not audio").unwrap();
    e.offer_import(other);
    assert!(e.execute("file.import_audio", &json!({"path": good})).is_err());
    e.drop_offered_imports();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_session_text_needs_replace_to_overwrite() {
    let dir = std::env::temp_dir().join(format!("sc-export-text-replace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("info.txt");
    let path_str = path.to_string_lossy().into_owned();
    let mut e = Engine::default();
    e.execute("file.export_session_text", &json!({"path": path_str})).unwrap();
    std::fs::write(&path, "keep me").unwrap();
    let err = e.execute("file.export_session_text", &json!({"path": path_str})).unwrap_err();
    assert!(err.to_string().contains("replace: true"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
    e.execute("file.export_session_text", &json!({"path": path_str, "replace": true})).unwrap();
    assert_ne!(std::fs::read_to_string(&path).unwrap(), "keep me");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_audio_honours_new_tracks() {
    let dir = std::env::temp_dir().join(format!("sc-import-new-tracks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wav = tone_wav(&dir, "tone.wav");

    // Default / new_tracks:true: create a new track even when one is selected.
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
    let target = e.session().tracks[0].id;
    e.session_mut().edit.selected_tracks = vec![target];
    e.execute("file.import_audio", &json!({"path": wav, "new_tracks": true})).unwrap();
    assert_eq!(e.session().tracks.len(), 2);
    assert_eq!(e.session().track(target).unwrap().clips().len(), 0);
    assert_eq!(e.session().tracks[1].clips().len(), 1);

    // new_tracks:false with a selected audio track: import onto it.
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
    let target = e.session().tracks[0].id;
    e.session_mut().edit.selected_tracks = vec![target];
    e.execute("file.import_audio", &json!({"path": wav, "new_tracks": false})).unwrap();
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().track(target).unwrap().clips().len(), 1);

    // new_tracks:false with no selection and no track: clear error.
    let mut e = Engine::default();
    let err = e.execute("file.import_audio", &json!({"path": wav, "new_tracks": false})).unwrap_err();
    assert!(err.to_string().contains("new_tracks: false"), "{err}");

    // Explicit track wins over new_tracks:true.
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
    e.execute("file.import_audio", &json!({"path": wav, "track": "Target", "new_tracks": true})).unwrap();
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].clips().len(), 1);

    // Omitted new_tracks still creates a track on a blank session.
    let mut e = Engine::default();
    e.execute("file.import_audio", &json!({"path": wav})).unwrap();
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].kind, TrackKind::Audio);
    assert_eq!(e.session().tracks[0].format, ChannelFormat::Mono);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bounce_refuses_formats_it_cannot_write() {
    let mut e = Engine::default();
    let dir = std::env::temp_dir().join(format!("sc-bounce-ext-{}", std::process::id()));
    assert!(std::fs::create_dir_all(&dir).is_ok());
    for name in ["mix.mp3", "mix.ogg", "mix.xyz"] {
        let path = dir.join(name);
        let err = e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "start": 0, "end": 480})).map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("supported: wav"), "{name}: {err}");
        assert!(!path.exists(), "{name} must not be written");
    }
    let ok = dir.join("mix.flac");
    assert!(e.execute("file.bounce_mix", &json!({"path": ok.to_string_lossy(), "start": 0, "end": 480})).is_ok());
    assert!(std::fs::remove_dir_all(&dir).is_ok());
}

#[test]
fn bounce_source_renders_the_named_bus() {
    let dir = std::env::temp_dir().join(format!("sc-bounce-source-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wav = tone_wav(&dir, "tone.wav");
    let mut e = Engine::default();
    e.execute("file.import_audio", &json!({"path": wav})).unwrap();
    let stem = e.session_mut().add_bus("Stem", ChannelFormat::Stereo);
    e.session_mut().add_bus("Empty", ChannelFormat::Stereo);
    let track = e.session().tracks[0].id;
    {
        let tr = e.session_mut().track_mut(track).unwrap();
        let mut send = soundcraft_model::SendSlot::new(soundcraft_model::Route::Bus(stem));
        send.level_db = -12.0;
        tr.mixer.sends[0] = Some(send);
    }
    let bounce = |e: &mut Engine, name: &str, source: Option<&str>| {
        let path = dir.join(name);
        let mut params = json!({"path": path.to_string_lossy(), "start": 0, "end": 4800, "bit_depth": "32f", "dither": false});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        let value = e.execute("file.bounce_mix", &params).unwrap();
        let peak = value.get("peak_db").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        (std::fs::read(&path).unwrap(), peak)
    };
    let (main_bytes, main_peak) = bounce(&mut e, "main.wav", None);
    let (named_main, named_peak) = bounce(&mut e, "named-main.wav", Some("main"));
    let (stem_bytes, stem_peak) = bounce(&mut e, "stem.wav", Some("STEM"));
    let (empty_bytes, empty_peak) = bounce(&mut e, "empty.wav", Some("Empty"));
    assert_eq!(main_bytes, named_main);
    assert!((main_peak - named_peak).abs() < 1e-4, "{main_peak} vs {named_peak}");
    assert!(main_peak > -20.0, "{main_peak}");
    assert_ne!(stem_bytes, main_bytes);
    assert!(main_peak - stem_peak > 6.0 && main_peak - stem_peak < 18.0, "main {main_peak} stem {stem_peak}");
    assert_ne!(empty_bytes, main_bytes);
    assert!(empty_peak < -100.0, "{empty_peak}");
    let missing = dir.join("missing.wav");
    let err = e
        .execute("file.bounce_mix", &json!({"path": missing.to_string_lossy(), "source": "NoSuchBus", "start": 0, "end": 4800, "dither": false}))
        .unwrap_err();
    assert!(err.to_string().contains("unknown bus"), "{err}");
    assert!(!missing.exists());
    assert!(std::fs::remove_dir_all(&dir).is_ok());
}

fn remove_media(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            remove_media(&p);
        } else if p.extension().is_some_and(|x| x != "scraft") {
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[test]
fn session_open_reports_missing_media() {
    let mut e = crate::demo::demo_engine();
    let dir = std::env::temp_dir().join(format!("soundcraft-open-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Gone.scraft").to_string_lossy().into_owned();
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    let mut ok = Engine::default();
    assert!(ok.execute("session.open", &json!({"path": path})).unwrap()["missing"].as_array().is_some_and(Vec::is_empty));
    remove_media(&dir);
    let mut r = Engine::default();
    let res = r.execute("session.open", &json!({"path": path})).unwrap();
    assert!(res["missing"].as_array().is_some_and(|m| !m.is_empty()), "{res}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_a_session_without_audio_creates_its_folder() {
    let dir = std::env::temp_dir().join(format!("soundcraft-new-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Song").join("Song.scraft");
    Engine::default().execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

fn tone_engine() -> (Engine, u64) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Tone", "format": "mono"})).unwrap();
    let track = e.session().tracks[0].id;
    let tone: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
    let src = crate::io::add_source(e.session_mut(), "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
    let id = e.session_mut().new_clip_id();
    crate::edit::place_clip(e.session_mut(), track, soundcraft_model::Clip::audio(id, "tone", src, 0, 0, 4_800));
    e.session_mut().edit.selected_tracks = vec![track];
    (e, track.0)
}

#[test]
fn same_named_clips_export_as_separate_files() {
    let (mut e, track) = tone_engine();
    let first = e.session().tracks[0].clips()[0].id;
    let src = e.session().tracks[0].clips()[0].source().unwrap();
    let second = e.session_mut().new_clip_id();
    crate::edit::place_clip(e.session_mut(), soundcraft_model::TrackId(track), soundcraft_model::Clip::audio(second, "tone", src, 0, 10_000, 4_800));
    for id in [first, second] {
        e.session_mut().find_clip_mut(id).unwrap().name = "Take".into();
    }
    let dir = std::env::temp_dir().join(format!("sc-export-clips-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let n = e.execute("file.export_clips", &json!({"dir": dir.to_string_lossy(), "clips": [first.0, second.0]})).unwrap();
    assert_eq!(n["written"], 2);
    let mut wavs: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    wavs.sort();
    assert_eq!(wavs, vec!["Take-2.wav".to_string(), "Take.wav".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exports_the_selected_clips_and_refuses_an_empty_selection() {
    let mut e = crate::demo::demo_engine();
    let dir = std::env::temp_dir().join(format!("soundcraft-clips-{}", std::process::id()));
    let p = json!({"dir": dir.to_string_lossy()});
    e.execute("edit.select_none", &json!({})).unwrap();
    assert!(e.execute("file.export_clips", &p).is_err());
    e.execute("edit.select", &json!({"tracks": ["Kick"], "start": {"seconds": 10.0}, "end": {"seconds": 11.0}})).unwrap();
    let r = e.execute("file.export_clips", &p).unwrap();
    let files = std::fs::read_dir(&dir).map_or(0, Iterator::count);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(files >= 1 && r["written"] == files, "{r}, {files} files");
}

fn midi_engine() -> Engine {
    use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
    use soundcraft_model::{ChannelFormat, Clip, TrackKind};
    let mut s = Session::default();
    let keys = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let drums = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Drums"));
    let _audio = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Room"));
    let mut keys_seq = Sequence::default();
    keys_seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
    keys_seq.ctrls.push(CtrlEvent { tick: 10, channel: 0, kind: CtrlKind::Cc(7), value: 90 });
    let mut drum_seq = Sequence::default();
    drum_seq.notes.push(Note { pitch: 36, velocity: 110, release_velocity: 64, channel: 9, start: 0, length: 120 });
    drum_seq.ctrls.push(CtrlEvent { tick: 20, channel: 9, kind: CtrlKind::Cc(11), value: 40 });
    let a = s.new_clip_id();
    let b = s.new_clip_id();
    crate::edit::place_clip(&mut s, keys, Clip::midi(a, "Keys", 0, 48_000, keys_seq));
    crate::edit::place_clip(&mut s, drums, Clip::midi(b, "Drums", 0, 48_000, drum_seq));
    Engine::new(s)
}

fn cc_numbers(bytes: &[u8]) -> Vec<(String, u8, i32)> {
    let smf = soundcraft_midi::read_smf(bytes).unwrap();
    let mut out = Vec::new();
    for t in &smf.tracks {
        for c in &t.sequence.ctrls {
            if let soundcraft_midi::CtrlKind::Cc(n) = c.kind {
                out.push((t.name.clone(), n, c.value));
            }
        }
    }
    out
}

#[test]
fn midi_export_keeps_controllers_and_honours_tracks() {
    let mut e = midi_engine();
    let dir = std::env::temp_dir().join(format!("sc-midi-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let all = dir.join("all.mid");
    e.execute("file.export_midi", &json!({"path": all.to_string_lossy()})).unwrap();
    let ctrls = cc_numbers(&std::fs::read(&all).unwrap());
    assert!(ctrls.iter().any(|(n, cc, v)| n == "Keys" && *cc == 7 && *v == 90), "{ctrls:?}");
    assert!(ctrls.iter().any(|(n, cc, v)| n == "Drums" && *cc == 11 && *v == 40), "{ctrls:?}");
    assert!(ctrls.iter().all(|(n, _, _)| n != "Room"));

    let keys = dir.join("keys.mid");
    e.execute("file.export_midi", &json!({"path": keys.to_string_lossy(), "tracks": ["Keys"]})).unwrap();
    let ctrls = cc_numbers(&std::fs::read(&keys).unwrap());
    assert_eq!(ctrls.len(), 1, "{ctrls:?}");
    assert_eq!(ctrls[0].0, "Keys");

    let err = e.execute("file.export_midi", &json!({"path": dir.join("nope.mid").to_string_lossy(), "tracks": ["Room"]})).unwrap_err();
    assert!(err.to_string().contains("none of the requested tracks hold MIDI"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_surround_creates_a_matching_track() {
    let channels: Vec<Vec<f32>> = (0..6).map(|ch| vec![0.1 * (ch as f32 + 1.0); 32]).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels };
    let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
    let mut e = Engine::default();
    let v = crate::io::import_audio_bytes(&mut e, "bed.wav", &bytes, None, None, 0).unwrap();
    assert_eq!(v["channels"], 6);
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].format, ChannelFormat::Surround51);
    let src = e.session().sources[0].id;
    let audio = e.session().pool.get(src).unwrap();
    assert_eq!(audio.buffer.channels.len(), 6);
    assert!((audio.buffer.channels[5][0] - 0.6).abs() < 1e-3);

    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Bed", "format": "5.1"})).unwrap();
    let id = e.session().tracks[0].id;
    crate::io::import_audio_bytes(&mut e, "bed.wav", &bytes, None, Some(id), 0).unwrap();
    assert_eq!(e.session().tracks.len(), 1);
    assert_eq!(e.session().tracks[0].format, ChannelFormat::Surround51);
    assert_eq!(e.session().tracks[0].clips().len(), 1);
}

#[test]
fn repeated_consolidate_keeps_distinct_audio_files() {
    let (mut e, track) = tone_engine();
    e.execute("edit.consolidate", &json!({"track": track, "start": 0, "end": 4800})).unwrap();
    let cid = e.session().tracks[0].clips()[0].id;
    e.execute("clip.gain", &json!({"clip": cid.0, "db": -24.0})).unwrap();
    e.execute("edit.consolidate", &json!({"track": track, "start": 0, "end": 4800})).unwrap();
    let consolidated: Vec<_> = e.session().sources.iter().filter(|s| s.name.contains("consolidated")).map(|s| s.path.clone()).collect();
    assert_eq!(consolidated.len(), 2, "{consolidated:?}");
    assert_ne!(consolidated[0], consolidated[1]);

    let dir = std::env::temp_dir().join(format!("sc-consolidate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Tone.scraft");
    e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    let mut opened = Engine::default();
    opened.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let files: Vec<_> = opened.session().sources.iter().filter(|s| s.name.contains("consolidated")).map(|s| dir.join(&s.path)).collect();
    assert_eq!(files.len(), 2, "{:?}", opened.session().sources);
    let a = std::fs::read(&files[0]).unwrap();
    let b = std::fs::read(&files[1]).unwrap();
    assert_ne!(a, b, "the second consolidation replaced the first file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_copy_leaves_the_open_session_alone() {
    let (mut e, _) = tone_engine();
    let dir = std::env::temp_dir().join(format!("sc-save-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("Original.scraft");
    e.execute("session.save_as", &json!({"path": original.to_string_lossy()})).unwrap();
    e.execute("track.new", &json!({"count": 1, "name": "Extra", "format": "mono"})).unwrap();
    assert!(e.is_dirty());
    let copy_dir = dir.join("copy");
    std::fs::create_dir_all(&copy_dir).unwrap();
    let copy = copy_dir.join("Copy.scraft");
    e.execute("session.save_copy", &json!({"path": copy.to_string_lossy()})).unwrap();
    assert_eq!(e.path.as_deref(), Some(original.to_string_lossy().as_ref()));
    assert_eq!(e.session().name, "Original");
    assert!(e.is_dirty());
    assert_eq!(e.session().tracks.len(), 2);

    let mut from_original = Engine::default();
    from_original.execute("session.open", &json!({"path": original.to_string_lossy()})).unwrap();
    assert_eq!(from_original.session().tracks.len(), 1);
    let mut from_copy = Engine::default();
    from_copy.execute("session.open", &json!({"path": copy.to_string_lossy()})).unwrap();
    assert_eq!(from_copy.session().tracks.len(), 2);
    assert!(from_copy.session().track_by_name("Extra").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
