use proptest::prelude::*;
use soundcraft_midi::*;

fn vlq(mut v: u32) -> Vec<u8> {
    let mut out = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        out.insert(0, (v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out
}

fn header(format: u16, ntrks: u16, division: u16) -> Vec<u8> {
    let mut b = b"MThd".to_vec();
    b.extend_from_slice(&6u32.to_be_bytes());
    b.extend_from_slice(&format.to_be_bytes());
    b.extend_from_slice(&ntrks.to_be_bytes());
    b.extend_from_slice(&division.to_be_bytes());
    b
}

fn track(body: &[u8]) -> Vec<u8> {
    let mut b = b"MTrk".to_vec();
    b.extend_from_slice(&(body.len() as u32).to_be_bytes());
    b.extend_from_slice(body);
    b
}

fn file(format: u16, division: u16, bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut b = header(format, bodies.len() as u16, division);
    for body in bodies {
        b.extend(track(body));
    }
    b
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

#[test]
fn round_trip() {
    let mut seq = Sequence {
        notes: vec![
            note(60, 0, 480),
            Note { pitch: 64, velocity: 90, release_velocity: 30, channel: 3, start: 480, length: 960 },
            note(67, 960, 1),
            note(60, 1920, 240),
        ],
        ctrls: vec![
            CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Program, value: 5 },
            CtrlEvent { tick: 10, channel: 0, kind: CtrlKind::Cc(7), value: 100 },
            CtrlEvent { tick: 20, channel: 1, kind: CtrlKind::PitchBend, value: -8192 },
            CtrlEvent { tick: 30, channel: 1, kind: CtrlKind::PitchBend, value: 8191 },
            CtrlEvent { tick: 40, channel: 1, kind: CtrlKind::PitchBend, value: 0 },
            CtrlEvent { tick: 50, channel: 2, kind: CtrlKind::ChannelPressure, value: 77 },
            CtrlEvent { tick: 60, channel: 2, kind: CtrlKind::PolyPressure(61), value: 12 },
        ],
    };
    seq.sort();
    let smf = Smf {
        format: 1,
        tracks: vec![
            SmfTrack { name: "Piano".into(), sequence: seq.clone() },
            SmfTrack { name: "Bass ♪".into(), sequence: Sequence { notes: vec![note(36, 100, 50)], ctrls: vec![] } },
        ],
        tempos: vec![(0, 120.0), (3840, 90.0)],
        meters: vec![(0, 4, 4), (3840, 6, 8)],
        markers: vec![(0, "Intro".into()), (7680, "Verse".into())],
        key_sigs: vec![(0, -3, true), (3840, 2, false)],
    };
    let bytes = write_smf(&smf).unwrap();
    let back = read_smf(&bytes).unwrap();
    assert_eq!(back.format, 1);
    assert_eq!(back.tracks.len(), 2);
    assert_eq!(back.tracks[0].name, "Piano");
    assert_eq!(back.tracks[0].sequence, seq);
    assert_eq!(back.tracks[1].name, "Bass ♪");
    assert_eq!(back.tracks[1].sequence.notes, vec![note(36, 100, 50)]);
    assert_eq!(back.tempos.len(), 2);
    assert!((back.tempos[0].1 - 120.0).abs() < 1e-9);
    assert!((back.tempos[1].1 - 90.0).abs() < 1e-3);
    assert_eq!(back.tempos[1].0, 3840);
    assert_eq!(back.meters, smf.meters);
    assert_eq!(back.markers, smf.markers);
    assert_eq!(back.key_sigs, smf.key_sigs);
    // Writing again is byte-identical.
    let mut again = back.clone();
    again.tempos = smf.tempos.clone();
    assert_eq!(write_smf(&again).unwrap(), bytes);
}

#[test]
fn first_track_name_wins_and_ignored_meta_preserves_following_notes() {
    let meta = |kind, data: &[u8]| [vec![0, 0xFF, kind], vlq(data.len() as u32), data.to_vec()].concat();
    for first in [&b"First"[..], &b""[..], &[0xFF][..]] {
        let body = [
            meta(0x03, first),
            meta(0x03, b"Second"),
            meta(0x7F, &[0x03, 0xFF, 0x2F]), // Unhandled metadata must remain ignored.
            vec![0, 0x90, 60, 100, 16, 0x80, 60, 64, 0, 0xFF, 0x2F, 0],
        ]
        .concat();
        let smf = read_smf(&file(0, 960, &[body])).unwrap();
        assert_eq!(smf.tracks.len(), 1);
        assert_eq!(smf.tracks[0].name, String::from_utf8_lossy(first));
        assert_eq!(smf.tracks[0].sequence.notes, vec![note(60, 0, 16)]);
    }
}

#[test]
fn write_rejects_bad_values() {
    let bad_tempo = Smf { tempos: vec![(0, 0.0)], ..Default::default() };
    assert!(write_smf(&bad_tempo).is_err());
    let nan_tempo = Smf { tempos: vec![(0, f64::NAN)], ..Default::default() };
    assert!(write_smf(&nan_tempo).is_err());
    let bad_meter = Smf { meters: vec![(0, 4, 3)], ..Default::default() };
    assert!(write_smf(&bad_meter).is_err());
    let huge_gap = Smf {
        tracks: vec![SmfTrack { name: String::new(), sequence: Sequence { notes: vec![note(60, 0x1000_0000, 10)], ctrls: vec![] } }],
        ..Default::default()
    };
    assert!(write_smf(&huge_gap).is_err());
}

#[test]
fn division_conversion_480_to_960() {
    // Note at tick 480 (one quarter at 480 PPQ), length 240.
    let body = [vlq(480), vec![0x90, 60, 100], vlq(240), vec![0x80, 60, 0], vec![0, 0xFF, 0x2F, 0]].concat();
    let smf = read_smf(&file(0, 480, &[body])).unwrap();
    assert_eq!(smf.tracks.len(), 1);
    assert_eq!(smf.tracks[0].sequence.notes, vec![Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start: 960, length: 480 }]);
}

#[test]
fn division_conversion_96_and_smpte() {
    let body = [vlq(96), vec![0x90, 60, 100], vlq(48), vec![0x80, 60, 64]].concat();
    let smf = read_smf(&file(0, 96, std::slice::from_ref(&body))).unwrap();
    assert_eq!(smf.tracks[0].sequence.notes[0].start, 960);
    assert_eq!(smf.tracks[0].sequence.notes[0].length, 480);

    // SMPTE 25 fps, 40 ticks/frame = 1000 ticks/s; 96 ticks = 96 ms; at 120 bpm 0.5 s = 960 ticks.
    let div = u16::from_be_bytes([(-25i8) as u8, 40]);
    let body = [vlq(500), vec![0x90, 60, 100], vlq(250), vec![0x80, 60, 64]].concat();
    let smf = read_smf(&file(0, div, &[body])).unwrap();
    assert_eq!(smf.tracks[0].sequence.notes[0].start, 960);
    assert_eq!(smf.tracks[0].sequence.notes[0].length, 480);
}

#[test]
fn running_status_and_velocity_zero_note_off() {
    // Note-on with running status for both on and off (velocity 0).
    let body = [
        vec![0x00, 0x91, 60, 100], // on C3 ch1
        vec![0x00, 64, 90],        // running: on E3
        vlq(960),
        vec![60, 0], // running: note-on vel 0 = off C3
        vec![0x00, 64, 0],
        vec![0x00, 0xB1, 7, 100], // CC7
        vec![0x10, 10, 64],       // running CC10
    ]
    .concat();
    let smf = read_smf(&file(0, 960, &[body])).unwrap();
    let seq = &smf.tracks[0].sequence;
    assert_eq!(
        seq.notes,
        vec![
            Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 1, start: 0, length: 960 },
            Note { pitch: 64, velocity: 90, release_velocity: 64, channel: 1, start: 0, length: 960 },
        ]
    );
    assert_eq!(
        seq.ctrls,
        vec![
            CtrlEvent { tick: 960, channel: 1, kind: CtrlKind::Cc(7), value: 100 },
            CtrlEvent { tick: 976, channel: 1, kind: CtrlKind::Cc(10), value: 64 },
        ]
    );
}

#[test]
fn meta_cancels_running_status() {
    let body = [vec![0x00, 0x90, 60, 100], vec![0x00, 0xFF, 0x01, 0x01, b'x'], vec![0x00, 60, 0]].concat();
    assert!(read_smf(&file(0, 960, &[body])).is_err());
}

#[test]
fn unmatched_note_off_ignored_and_hanging_notes_closed() {
    let body = [
        vec![0x00, 0x80, 50, 0],   // unmatched off
        vec![0x00, 0x90, 60, 100], // hanging
        vlq(100),
        vec![0x90, 62, 100],
        vlq(100),
        vec![0x80, 62, 10],
        vlq(800),
        vec![0xFF, 0x2F, 0x00],
    ]
    .concat();
    let smf = read_smf(&file(0, 960, &[body])).unwrap();
    let notes = &smf.tracks[0].sequence.notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0], Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 1000 });
    assert_eq!(notes[1], Note { pitch: 62, velocity: 100, release_velocity: 10, channel: 0, start: 100, length: 100 });
}

#[test]
fn overlapping_same_pitch_fifo_and_zero_length() {
    let body = [
        vec![0x00, 0x90, 60, 100],
        vec![0x10, 0x90, 60, 80],
        vec![0x10, 0x80, 60, 0],
        vec![0x10, 0x80, 60, 0],
        vec![0x00, 0x90, 70, 1],
        vec![0x00, 0x80, 70, 0],
    ]
    .concat();
    let smf = read_smf(&file(0, 960, &[body])).unwrap();
    let notes = &smf.tracks[0].sequence.notes;
    assert_eq!((notes[0].start, notes[0].length, notes[0].velocity), (0, 32, 100));
    assert_eq!((notes[1].start, notes[1].length, notes[1].velocity), (16, 32, 80));
    assert_eq!((notes[2].pitch, notes[2].length), (70, 1));
}

#[test]
fn format1_conductor_folded_and_unknown_chunks_skipped() {
    let conductor = [
        vec![0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20], // 500000 us = 120 bpm
        vec![0x00, 0xFF, 0x58, 0x04, 3, 2, 24, 8],      // 3/4
        vec![0x00, 0xFF, 0x2F, 0x00],
    ]
    .concat();
    let t1 = [vec![0x00, 0xFF, 0x03, 0x03], b"Org".to_vec(), vec![0x00, 0x90, 60, 100, 0x60, 0x80, 60, 0]].concat();
    let mut bytes = header(1, 2, 960);
    bytes.extend(track(&conductor));
    bytes.extend(b"XFIH\x00\x00\x00\x03abc");
    bytes.extend(track(&t1));
    let smf = read_smf(&bytes).unwrap();
    assert_eq!(smf.tracks.len(), 1);
    assert_eq!(smf.tracks[0].name, "Org");
    assert_eq!(smf.tempos, vec![(0, 120.0)]);
    assert_eq!(smf.meters, vec![(0, 3, 4)]);
}

#[test]
fn sysex_skipped() {
    let body = [vec![0x00, 0xF0, 0x03, 0x7E, 0x7F, 0xF7], vec![0x00, 0x90, 60, 100, 0x10, 0x80, 60, 0]].concat();
    let smf = read_smf(&file(0, 960, &[body])).unwrap();
    assert_eq!(smf.tracks[0].sequence.notes.len(), 1);
}

#[test]
fn malformed_inputs_are_errors() {
    assert!(matches!(read_smf(b""), Err(MidiError::NotSmf)));
    assert!(matches!(read_smf(b"RIFF\x00\x00\x00\x06"), Err(MidiError::NotSmf)));
    // Header truncated.
    assert!(read_smf(b"MThd\x00\x00\x00\x06\x00\x01").is_err());
    // Header length < 6.
    assert!(read_smf(b"MThd\x00\x00\x00\x02\x00\x01").is_err());
    // Division 0.
    assert!(read_smf(&header(0, 0, 0)).is_err());
    // Format 3.
    assert!(read_smf(&header(3, 0, 96)).is_err());
    // Chunk length larger than the data.
    let mut b = header(0, 1, 96);
    b.extend(b"MTrk\xFF\xFF\xFF\xF0\x00\x90");
    assert!(matches!(read_smf(&b), Err(MidiError::BadChunkLength { .. })));
    // Truncated chunk header.
    let mut b = header(0, 1, 96);
    b.extend(b"MTr");
    assert!(read_smf(&b).is_err());
    // Huge VLQ (5 continuation bytes).
    let body = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x90, 60, 100];
    assert!(matches!(read_smf(&file(0, 96, &[body])), Err(MidiError::VlqTooLong(_))));
    // Meta length past the end of the track.
    let body = vec![0x00, 0xFF, 0x03, 0x7F, b'a'];
    assert!(matches!(read_smf(&file(0, 96, &[body])), Err(MidiError::Truncated(_))));
    // Truncated channel message.
    let body = vec![0x00, 0x90, 60];
    assert!(read_smf(&file(0, 96, &[body])).is_err());
    // Data byte with no running status.
    let body = vec![0x00, 60, 100];
    assert!(read_smf(&file(0, 96, &[body])).is_err());
    // System common inside a track.
    let body = vec![0x00, 0xF2, 0, 0];
    assert!(read_smf(&file(0, 96, &[body])).is_err());
    // Header only (no tracks) is fine.
    assert_eq!(read_smf(&header(1, 0, 96)).unwrap().tracks.len(), 0);
}

#[test]
fn event_cap_reached_is_error() {
    // MAX_EVENTS + 1 one-byte running-status program changes (2 bytes each with delta).
    let mut body = vec![0x00, 0xC0, 0x00];
    body.reserve(MAX_EVENTS * 2);
    for _ in 0..MAX_EVENTS {
        body.extend_from_slice(&[0x00, 0x01]);
    }
    assert!(matches!(read_smf(&file(0, 96, &[body])), Err(MidiError::TooManyEvents(_))));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = read_smf(&data);
    }

    #[test]
    fn random_track_bodies_never_panic(body in proptest::collection::vec(any::<u8>(), 0..512), div in any::<u16>(), fmt in 0u16..4) {
        let _ = read_smf(&file(fmt, div, &[body]));
    }

    #[test]
    fn written_files_read_back(notes in proptest::collection::vec((0u8..128, 1u8..128, 0u8..16, 0i64..100_000, 1i64..10_000), 0..64)) {
        let mut seq = Sequence {
            notes: notes.iter().map(|&(pitch, velocity, channel, start, length)| Note { pitch, velocity, release_velocity: 64, channel, start, length }).collect(),
            ctrls: vec![],
        };
        seq.remove_duplicates();
        let smf = Smf { format: 1, tracks: vec![SmfTrack { name: "t".into(), sequence: seq.clone() }], ..Default::default() };
        let back = read_smf(&write_smf(&smf).unwrap()).unwrap();
        // Overlapping same-pitch notes may re-pair, but count and starts survive.
        let got = back.tracks.first().map(|t| t.sequence.notes.clone()).unwrap_or_default();
        prop_assert_eq!(got.len(), seq.notes.len());
        let mut a: Vec<_> = got.iter().map(|n| (n.start, n.pitch, n.channel)).collect();
        let mut b: Vec<_> = seq.notes.iter().map(|n| (n.start, n.pitch, n.channel)).collect();
        a.sort();
        b.sort();
        prop_assert_eq!(a, b);
    }
}

/// #472: out-of-range pitches, release velocities and controller numbers clamp to 127 instead of
/// wrapping into other notes and controllers (128 & 0x7F would be 0).
#[test]
fn writer_clamps_out_of_range_data_bytes() {
    for raw in [127u8, 128, 255] {
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: raw, velocity: 100, release_velocity: raw, channel: 0, start: 0, length: 960 });
        seq.ctrls.push(CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(raw), value: 64 });
        seq.ctrls.push(CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::PolyPressure(raw), value: 64 });
        let smf = Smf { tracks: vec![SmfTrack { name: String::new(), sequence: seq }], ..Smf::default() };
        let back = read_smf(&write_smf(&smf).unwrap()).unwrap();
        let s = &back.tracks.last().unwrap().sequence;
        assert_eq!((s.notes[0].pitch, s.notes[0].release_velocity), (127, 127), "input {raw}");
        let kinds: Vec<CtrlKind> = s.ctrls.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&CtrlKind::Cc(127)) && kinds.contains(&CtrlKind::PolyPressure(127)), "input {raw}: {kinds:?}");
    }
}
