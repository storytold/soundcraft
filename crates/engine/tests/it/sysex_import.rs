use soundcraft_engine::{Engine, edit, io};
use soundcraft_model::ClipContent;
use soundcraft_time::Range;

fn fixture() -> Vec<u8> {
    let body = [0, 0xF0, 8, 0x43, 0x10, 0x4C, 0, 0, 0x7E, 0, 0xF7, 0x83, 0x60, 0xF7, 1, 0xF8, 0x83, 0x60, 0xF0, 2, 0x43, 0xF7, 0, 0xFF, 0x2F, 0];
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x03\xC0MTrk".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn exported(engine: &Engine) -> Vec<soundcraft_midi::SysExEvent> {
    soundcraft_midi::read_smf(&io::export_midi(engine, None).unwrap()).unwrap().tracks.into_iter().flat_map(|t| t.sequence.sysex).collect()
}

#[test]
fn sysex_only_track_is_imported_at_the_requested_offset_and_exported() {
    let mut e = Engine::default();
    let tracks = io::import_midi_bytes(&mut e, &fixture(), 48_000, false).unwrap();
    assert_eq!(tracks.len(), 1, "a SysEx-only setup track must not disappear");
    let clip = &e.session().track(tracks[0]).unwrap().clips()[0];
    assert_eq!(clip.start, 48_000);
    assert!(clip.end() > 72_000, "the final packet must lie inside the clip");
    let packets = exported(&e);
    assert_eq!(packets.iter().map(|e| e.tick).collect::<Vec<_>>(), [1920, 2400, 2880]);
    assert_eq!(packets[0].data, [0x43, 0x10, 0x4C, 0, 0, 0x7E, 0, 0xF7]);
    assert_eq!(packets[1].status, 0xF7);
    assert_eq!(packets[1].data, [0xF8]);
}

#[test]
fn import_command_reports_retention_and_the_remaining_playback_limit() {
    let path = std::env::temp_dir().join(format!("soundcraft-sysex-command-{}.mid", std::process::id()));
    std::fs::write(&path, fixture()).unwrap();
    let mut e = Engine::default();
    let result = e.execute("file.import_midi", &serde_json::json!({"path":path})).unwrap();
    assert_eq!(result["sysex_packets"], 3);
    assert!(result["warnings"][0].as_str().unwrap().contains("playback is not supported"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn oversized_sysex_is_refused_without_changing_the_session() {
    let mut e = Engine::default();
    let before = serde_json::to_value(e.session()).unwrap();
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x03\xC0MTrk\0\0\0\x06".to_vec();
    bytes.extend_from_slice(&[0, 0xF0, 0x88, 0x80, 0x80, 1]);
    assert!(io::import_midi_bytes(&mut e, &bytes, 0, true).unwrap_err().to_string().contains("SysEx"));
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before);
}

#[test]
fn sysex_survives_a_real_session_save_reopen_and_midi_export() {
    let mut e = Engine::default();
    io::import_midi_bytes(&mut e, &fixture(), 0, false).unwrap();
    let before = exported(&e);
    let path = std::env::temp_dir().join(format!("soundcraft-sysex-roundtrip-{}.scraft", std::process::id()));
    io::save_session(&mut e, &path.to_string_lossy()).unwrap();
    let mut reopened = Engine::default();
    assert!(io::open_session(&mut reopened, &path).unwrap().is_empty());
    assert_eq!(exported(&reopened), before);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn splitting_and_consolidating_keep_packet_times_and_order() {
    let mut e = Engine::default();
    let track = io::import_midi_bytes(&mut e, &fixture(), 0, false).unwrap()[0];
    let before = exported(&e);
    let clip = e.session().track(track).unwrap().clips()[0].clone();
    let new_id = e.session_mut().new_clip_id();
    let (left, right) = edit::split_clip(&clip, 12_000, new_id, edit::TimeCtx::of(e.session())).unwrap();
    match (&left.content, &right.content) {
        (ClipContent::Midi { sequence: a }, ClipContent::Midi { sequence: b }) => {
            assert_eq!(a.sysex.len(), 1);
            assert_eq!(b.sysex.iter().map(|e| e.tick).collect::<Vec<_>>(), [0, 480]);
        }
        _ => panic!("expected MIDI"),
    }
    e.session_mut().track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![left, right];
    assert_eq!(exported(&e), before);
    assert!(edit::consolidate_midi(e.session_mut(), track, Range::new(0, clip.end())));
    assert_eq!(exported(&e), before);
}

#[test]
fn trimming_drops_packets_before_the_new_start_and_reanchors_the_rest() {
    let mut e = Engine::default();
    let track = io::import_midi_bytes(&mut e, &fixture(), 0, false).unwrap()[0];
    assert_eq!(edit::trim_start_to(e.session_mut(), track, 12_000), 1);
    let packets = exported(&e);
    assert_eq!(packets.iter().map(|e| e.tick).collect::<Vec<_>>(), [480, 960]);
    assert_eq!(packets[0].status, 0xF7);
}
