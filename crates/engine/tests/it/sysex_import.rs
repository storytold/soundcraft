use soundcraft_engine::{Engine, edit, io};
use soundcraft_model::ClipContent;
use soundcraft_time::Range;

fn fixture() -> Vec<u8> {
    let body = [0, 0xF0, 8, 0x43, 0x10, 0x4C, 0, 0, 0x7E, 0, 0xF7, 0x83, 0x60, 0xF7, 1, 0xF8, 0x83, 0x60, 0xF0, 2, 0x43, 0xF7, 0, 0xFF, 0x2F, 0];
    smf_track(&body)
}

fn smf_track(body: &[u8]) -> Vec<u8> {
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\x03\xC0MTrk".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
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

// Delta 0, F0, then a four-byte VLQ declaring 16 MiB + 1 payload bytes.
const LARGE_SYSEX_HEADER: [u8; 6] = [0, 0xF0, 0x88, 0x80, 0x80, 1];

#[test]
fn truncated_sysex_is_salvaged_without_hitting_the_size_limit() {
    let bytes = smf_track(&LARGE_SYSEX_HEADER);
    assert!(matches!(soundcraft_midi::read_smf(&bytes), Err(soundcraft_midi::MidiError::Truncated(_))));
    let read = soundcraft_midi::read_smf_lenient(&bytes).unwrap();
    assert!(!read.warnings.is_empty());
    assert!(!read.over_limit);
    let mut e = Engine::default();
    assert!(io::import_midi_bytes(&mut e, &bytes, 0, true).unwrap().is_empty());
}

#[test]
fn oversized_sysex_is_refused_without_changing_the_session() {
    let limit = soundcraft_midi::MAX_SYSEX_BYTES;
    let mut body = LARGE_SYSEX_HEADER.to_vec();
    body.resize(body.len() + limit, 0);
    body.push(0xF7);
    body.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
    let bytes = smf_track(&body);
    let expected = soundcraft_midi::MidiError::SysExTooLarge(limit);
    assert_eq!(soundcraft_midi::read_smf(&bytes).unwrap_err(), expected);
    assert!(soundcraft_midi::read_smf_lenient(&bytes).unwrap().over_limit);
    let mut e = Engine::default();
    let before = serde_json::to_value(e.session()).unwrap();
    let error = io::import_midi_bytes(&mut e, &bytes, 0, true).unwrap_err();
    assert!(error.to_string().contains(&expected.to_string()), "{error}");
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

#[test]
fn partial_copy_merge_exports_only_the_selected_midi_events() {
    use serde_json::json;
    use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence, SysExEvent};
    use soundcraft_model::{ChannelFormat, Clip, TrackKind};

    let mut e = Engine::default();
    let s = e.session_mut();
    let source = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let host = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let mut sequence = Sequence::default();
    for (tick, key) in [(119, 60), (120, 61), (240, 62), (360, 63), (480, 64)] {
        sequence.notes.push(Note { start: tick, length: 180, pitch: key, velocity: 100, release_velocity: 64, channel: 0 });
        sequence.ctrls.push(CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(11), value: i32::from(key) });
        sequence.sysex.push(SysExEvent { tick, status: 0xf0, data: vec![0x7d, key, 0xf7] });
    }
    let source_id = s.new_clip_id();
    let host_id = s.new_clip_id();
    edit::place_clip(s, source, Clip::midi(source_id, "source", 0, 24_000, sequence));
    edit::place_clip(s, host, Clip::midi(host_id, "destination", 12_000, 96_000, Sequence::default()));
    // At 120 BPM and 48 kHz, [3000, 9000) samples selects source ticks [120, 360).
    e.execute("edit.copy", &json!({"tracks":[source.0], "start":3000, "end":9000})).unwrap();
    let result = e.execute("edit.paste_merge_midi", &json!({"tracks":[host.0], "at":24_000})).unwrap();
    assert_eq!(result["merged"], 1);

    let smf = soundcraft_midi::read_smf(&io::export_midi(&e, Some(&[host])).unwrap()).unwrap();
    let exported = &smf.tracks.iter().find(|t| !t.sequence.notes.is_empty()).unwrap().sequence;
    let ClipContent::Midi { sequence: merged } = &e.session().find_clip(host_id).unwrap().1.content else { panic!("expected MIDI") };
    // Host-relative ticks and the decoded SMF both retain inclusive start and exclusive end.
    for (seq, base) in [(exported, 960), (merged, 480)] {
        let notes: Vec<_> = seq.notes.iter().map(|n| (n.start, n.pitch, n.length)).collect();
        let ctrls: Vec<_> = seq.ctrls.iter().map(|c| (c.tick, c.kind, c.value)).collect();
        let packets: Vec<_> = seq.sysex.iter().map(|s| (s.tick, s.status, s.data.as_slice())).collect();
        assert_eq!(
            (notes, ctrls, packets),
            (
                vec![(base, 61, 180), (base + 120, 62, 120)],
                vec![(base, CtrlKind::Cc(11), 61), (base + 120, CtrlKind::Cc(11), 62)],
                vec![(base, 0xf0, &[0x7d, 61, 0xf7][..]), (base + 120, 0xf0, &[0x7d, 62, 0xf7][..])],
            )
        );
    }

    // At the faster destination tempo the same sample length spans twice as many ticks.
    // A repeated paste on an empty track must still use the source window.
    e.session_mut().tempo.set_tempo(0, 240.).unwrap();
    let empty = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let result = e.execute("edit.paste_merge_midi", &json!({"tracks":[empty.0], "at":24_000})).unwrap();
    assert_eq!(result["placed"], 1);
    let smf = soundcraft_midi::read_smf(&io::export_midi(&e, Some(&[empty])).unwrap()).unwrap();
    let seq = &smf.tracks.iter().find(|t| !t.sequence.notes.is_empty()).unwrap().sequence;
    assert_eq!(seq.notes.iter().map(|n| (n.start, n.length)).collect::<Vec<_>>(), [(1920, 180), (2040, 120)]);
    assert_eq!(seq.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>(), [1920, 2040]);
    assert_eq!(seq.sysex.iter().map(|s| s.tick).collect::<Vec<_>>(), [1920, 2040]);

    // At half tempo the original sample duration would end at the second retained event.
    // Both placement and extension of a short host must cover the full source-tick window.
    e.session_mut().tempo.set_tempo(0, 60.).unwrap();
    let s = e.session_mut();
    let short = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let short_id = s.new_clip_id();
    edit::place_clip(s, short, Clip::midi(short_id, "short host", 23_000, 2_000, Sequence::default()));
    let blank = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    for (track, action) in [(blank, "placed"), (short, "merged")] {
        let result = e.execute("edit.paste_merge_midi", &json!({"tracks":[track.0], "at":24_000})).unwrap();
        assert_eq!(result[action], 1);
        let smf = soundcraft_midi::read_smf(&io::export_midi(&e, Some(&[track])).unwrap()).unwrap();
        let seq = &smf.tracks.iter().find(|t| !t.sequence.notes.is_empty()).unwrap().sequence;
        assert_eq!(seq.notes.iter().map(|n| (n.start, n.length)).collect::<Vec<_>>(), [(480, 180), (600, 120)]);
        assert_eq!(seq.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>(), [480, 600]);
        assert_eq!(seq.sysex.iter().map(|s| s.tick).collect::<Vec<_>>(), [480, 600]);
    }

    let s = e.session_mut();
    let adjacent = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let neighbor_id = s.new_clip_id();
    edit::place_clip(s, adjacent, Clip::midi(neighbor_id, "neighbor", 32_000, 4_000, Sequence::default()));
    e.execute("edit.paste_merge_midi", &json!({"tracks":[adjacent.0], "at":24_000})).unwrap();
    let clips = e.session().track(adjacent).unwrap().clips();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].end(), clips[1].id, clips[1].start, clips[1].length), (32_000, neighbor_id, 32_000, 4_000));
}

#[test]
fn recovered_track_over_the_sysex_budget_reports_the_payload_limit() {
    // A damaged first track sends import down the lenient path before it sees track two.
    let mut bytes = smf_track(&[0, 0x90, 60]);
    bytes[9] = 1; // SMF format 1
    bytes[11] = 2;
    let mut body = LARGE_SYSEX_HEADER.to_vec();
    body.resize(body.len() + soundcraft_midi::MAX_SYSEX_BYTES + 1, 0);
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    assert!(matches!(soundcraft_midi::read_smf(&bytes), Err(soundcraft_midi::MidiError::Truncated(_))));
    assert!(soundcraft_midi::read_smf_lenient(&bytes).unwrap().over_limit);
    let mut e = Engine::default();
    let before = serde_json::to_value(e.session()).unwrap();
    let error = io::import_midi_bytes(&mut e, &bytes, 0, true).unwrap_err().to_string();
    assert!(error.contains("SysEx") && error.contains(&soundcraft_midi::MAX_SYSEX_BYTES.to_string()), "{error}");
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before);
}
