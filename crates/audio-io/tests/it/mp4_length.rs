//! The length `probe` reports for AAC in MP4 is the length `decode` produces (issue #1113).

use soundcraft_audio_io::{decode, probe};

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
}

/// A full atom (version 0, no flags) whose body is `fields` as big-endian u32s.
fn full(kind: &[u8; 4], fields: &[u32]) -> Vec<u8> {
    let body: Vec<u8> = [0].iter().chain(fields).flat_map(|v: &u32| v.to_be_bytes()).collect();
    atom(kind, &body)
}

/// An MPEG-4 descriptor with a one-byte length.
fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    [&[tag, body.len() as u8][..], body].concat()
}

/// A silent AAC-LC raw frame for one channel: an SCE with `max_sfb` 0, then ID_END.
const SILENT_SCE: [u8; 4] = [0x00, 0xC8, 0x00, 0x07];

/// A mono 48 kHz AAC-LC MP4 without an edit list: `packets` frames, the last lasting `last_dur` in stts.
fn aac_mp4(packets: u32, last_dur: u32) -> Vec<u8> {
    // AudioSpecificConfig: AAC-LC, 48 kHz, one channel.
    let dsi = descriptor(5, &[0x11, 0x88]);
    let dcd = descriptor(4, &[&[0x40, 0x15, 0, 0, 0][..], &[0; 8], &dsi].concat());
    let es = descriptor(3, &[&[0, 1, 0][..], &dcd, &descriptor(6, &[2])].concat());
    let esds = atom(b"esds", &[&[0; 4][..], &es].concat());
    let base = [0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0];
    let entry = atom(b"mp4a", &[&base[..], &(48_000u32 << 16).to_be_bytes(), &esds].concat());
    let total = (packets - 1) * 1024 + last_dur;
    let moov = |offset: u32| {
        let stsd = atom(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &entry].concat());
        let stbl = [
            stsd,
            full(b"stts", &[2, packets - 1, 1024, 1, last_dur]),
            full(b"stsc", &[1, 1, packets, 1]),
            full(b"stsz", &[SILENT_SCE.len() as u32, packets]),
            full(b"stco", &[1, offset]),
        ];
        let minf = [full(b"smhd", &[0]), atom(b"stbl", &stbl.concat())].concat();
        let mdia = [full(b"mdhd", &[0, 0, 48_000, total, 0]), full(b"hdlr", &[0, u32::from_be_bytes(*b"soun"), 0, 0, 0, 0]), atom(b"minf", &minf)];
        let trak = [full(b"tkhd", &[0; 20]), atom(b"mdia", &mdia.concat())].concat();
        atom(b"moov", &[full(b"mvhd", &[[0, 0, 48_000, total, 1 << 16].as_slice(), &[0; 18], &[2]].concat()), atom(b"trak", &trak)].concat())
    };
    let ftyp = atom(b"ftyp", b"M4A \0\0\0\0M4A isom");
    let offset = (ftyp.len() + moov(0).len() + 8) as u32;
    [ftyp, moov(offset), atom(b"mdat", &SILENT_SCE.repeat(packets as usize))].concat()
}

#[test]
fn aac_in_mp4_probe_length_matches_the_decoded_length() {
    // A last sample lasting until the track end (longer than a packet), and a control with a short one.
    for last_dur in [1145, 546] {
        let file = aac_mp4(10, last_dur);
        let (_, buf) = decode(&file, Some("m4a")).unwrap();
        assert_eq!(buf.frames(), 10 * 1024, "decoded frames, last stts duration {last_dur}");
        assert_eq!(probe(&file, Some("m4a")).unwrap().frames, buf.frames() as u64, "probed frames, last stts duration {last_dur}");
    }
}
