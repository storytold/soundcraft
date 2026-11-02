//! MP3 frames that symphonia refuses (a last granule with no bits, as the Windows Media Foundation
//! encoder writes them) must not shorten the audio or shift what follows (#710).

use soundcraft_audio_io::{decode, probe};

/// Big-endian bit writer for the side info.
struct Bits {
    out: Vec<u8>,
    n: usize,
}

impl Bits {
    fn put(&mut self, value: u32, width: usize) {
        for i in (0..width).rev() {
            if self.n.is_multiple_of(8) {
                self.out.push(0);
            }
            if (value >> i) & 1 == 1
                && let Some(b) = self.out.last_mut()
            {
                *b |= 0x80 >> (self.n % 8);
            }
            self.n += 1;
        }
    }
}

/// One MPEG-1 Layer III frame, 320 kbps, 44.1 kHz, stereo, no CRC: 4 header bytes, 32 bytes of
/// side info and 1008 bytes of main data. Silent frames use count1 table A over all-one bits (an
/// all-zero spectrum); a loud frame uses count1 table B over zero bits (every line +1).
fn frame(part2_3: [u32; 4], loud: bool) -> Vec<u8> {
    let mut side = Bits { out: Vec::new(), n: 0 };
    side.put(0, 9); // main_data_begin
    side.put(0, 3); // private bits
    side.put(0, 8); // scfsi
    for len in part2_3 {
        side.put(len, 12);
        side.put(0, 9); // big_values
        side.put(if loud { 180 } else { 0 }, 8); // global_gain
        side.put(0, 4); // scalefac_compress
        side.put(0, 1); // window_switching_flag
        side.put(0, 15); // table_select
        side.put(0, 4); // region0_count
        side.put(0, 3); // region1_count
        side.put(u32::from(loud), 3); // preflag, scalefac_scale, count1table_select
    }
    let mut f = vec![0xFF, 0xFB, 0xE0, 0x00];
    f.extend_from_slice(&side.out);
    assert_eq!(f.len(), 36);
    f.resize(1044, if loud { 0x00 } else { 0xFF });
    f
}

#[test]
fn mp3_frames_with_an_empty_last_granule_keep_their_length() {
    let bad = [0usize, 4, 9];
    let loud = 5;
    let mut file = Vec::new();
    for i in 0..10 {
        file.extend(frame(if bad.contains(&i) { [4092, 852, 3120, 0] } else { [2016; 4] }, i == loud));
    }
    assert_eq!(probe(&file, Some("mp3")).unwrap().frames, 10 * 1152);
    let (info, buf) = decode(&file, Some("mp3")).unwrap();
    assert_eq!(info.channels, 2);
    assert_eq!(info.frames, 10 * 1152);
    assert_eq!(buf.frames(), 10 * 1152);
    // The loud frame right after a bad one stays in its place: nothing sounds before it.
    let onset = buf.channels[0].iter().position(|s| s.abs() > 1e-6).unwrap();
    assert!((loud * 1152..(loud + 1) * 1152).contains(&onset), "loud frame starts at {onset}");
}
