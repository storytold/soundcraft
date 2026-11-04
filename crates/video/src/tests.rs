//! Movie tests on fixtures generated in process (no binary fixtures): H.264 made of I_PCM
//! macroblocks (exact, known colours), ProRes from the bundled encoder and Motion JPEG from the
//! `image` encoder, muxed with the bundled MP4/MOV writer. When `ffmpeg` with libx264 is on PATH,
//! real High-profile streams with B-frames are checked too (skipped otherwise).

use crate::isobmff::{Brand, ByteSource, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use crate::synth::{frame_color, h264_mp4, mjpeg_mov, prores_mov};
use crate::{Movie, VideoError};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn expected_rgb(i: usize) -> [u8; 3] {
    let [y, cb, cr] = frame_color(i);
    crate::convert::Matrix::new(false, false).rgb(y, cb, cr)
}

fn assert_solid(f: &crate::Frame, rgb: [u8; 3], tol: i32) {
    assert_eq!(f.rgba.len(), (f.width * f.height * 4) as usize);
    for px in f.rgba.as_chunks::<4>().0.iter().step_by(37) {
        for c in 0..3 {
            assert!((i32::from(px[c]) - i32::from(rgb[c])).abs() <= tol, "frame {} pixel {:?} expected {:?}", f.index, px, rgb);
        }
        assert_eq!(px[3], 255);
    }
}

#[test]
fn h264_info_and_exact_frames() {
    let bytes = h264_mp4(30, 10).unwrap();
    let mut m = Movie::open(bytes).unwrap();
    let info = m.info().clone();
    assert_eq!((info.width, info.height), (64, 48));
    assert_eq!(info.codec, "h264");
    assert_eq!(info.codec_detail, "H.264 Baseline");
    assert!(info.decodable && !info.has_audio && !info.quicktime);
    assert_eq!(info.frame_count, 30);
    assert!((info.frame_rate - 24.0).abs() < 1e-9);
    assert!((info.duration - 1.25).abs() < 1e-9);
    // Sequential playback: every frame exact, decoding forward (one decode per frame).
    for i in 0..30 {
        let f = m.frame_at(i as f64 / 24.0 + 0.001).unwrap();
        assert_eq!(f.index, i);
        assert_solid(&f, expected_rgb(i), 1);
    }
    assert!(m.decoded <= 31, "sequential decode re-decoded frames: {}", m.decoded);
    assert!(m.frame_at(1.25).is_err() && m.frame_at(-0.1).is_err() && m.frame_at(f64::NAN).is_err());
}

#[test]
fn h264_random_seeks_decode_from_the_keyframe() {
    let mut m = Movie::open(h264_mp4(40, 20).unwrap()).unwrap();
    m.set_cache_frames(1);
    for &i in &[37usize, 3, 19, 20, 5, 39, 0, 21, 21, 12] {
        let f = m.frame(i).unwrap();
        assert_eq!(f.index, i);
        assert_solid(&f, expected_rgb(i), 1);
    }
    // Backwards seek to 12 decoded at most frames 0..=12 again (+ the read-ahead).
    let before = m.decoded;
    m.frame(13).unwrap();
    assert!(m.decoded - before <= 1, "forward step should not restart");
}

#[test]
fn h264_thumbnails_use_keyframes() {
    let mut m = Movie::open(h264_mp4(30, 10).unwrap()).unwrap();
    let th = m.thumbnail_at(0.6, 12).unwrap();
    assert_eq!(th.height, 12);
    assert_eq!(th.width, 16);
    assert_eq!(th.index, 10, "thumbnail snaps to the keyframe before 0.6 s (frame 14)");
    assert_solid(&th, expected_rgb(10), 2);
    let all = m.thumbnails(6, 24);
    assert_eq!(all.len(), 6);
    assert!(all.iter().all(|(_, r)| r.as_ref().is_ok_and(|f| f.height == 24)));
}

#[test]
fn prores_and_mjpeg_decode() {
    let mut m = Movie::open(prores_mov(5).unwrap()).unwrap();
    assert_eq!(m.info().codec, "prores");
    assert_eq!(m.info().codec_detail, "Apple ProRes 422 HQ");
    assert!(m.info().quicktime);
    assert!((m.info().frame_rate - 25.0).abs() < 1e-9);
    for i in [4usize, 0, 2] {
        let f = m.frame(i).unwrap();
        assert_eq!((f.width, f.height), (64, 48));
        let y = 16 + 8 * i as u8;
        let grey = crate::convert::Matrix::new(false, false).rgb(y, 128, 128);
        assert_solid(&f, grey, 3);
    }
    let mut j = Movie::open(mjpeg_mov(4).unwrap()).unwrap();
    assert_eq!(j.info().codec, "mjpeg");
    for i in 0..4 {
        let f = j.frame_at(i as f64 / 10.0).unwrap();
        assert_eq!((f.width, f.height), (32, 24));
        assert_solid(&f, [(30 * i) as u8; 3], 4);
    }
}

/// A 64x48 ProRes frame whose header claims `w`x`h`.
fn prores_frame_claiming(w: u16, h: u16) -> Vec<u8> {
    use crate::prores::{ChromaFormat, Encoder, Frame, Profile};
    let mut data = Encoder::new(Profile::Hq, 64, 48).encode(&Frame::new(64, 48, ChromaFormat::Yuv422, 10, false)).unwrap();
    data[16..18].copy_from_slice(&w.to_be_bytes());
    data[18..20].copy_from_slice(&h.to_be_bytes());
    data
}

#[test]
fn prores_frame_size_is_checked_before_decoding() {
    use crate::prores::{DecodeOptions, Error, decode_frame_with};
    let huge = decode_frame_with(&prores_frame_claiming(65535, 65535), &DecodeOptions::default());
    assert!(matches!(huge, Err(Error::Unsupported("frame size"))), "{huge:?}");
    // A frame that differs from the track's declared size is refused, not decoded at its own size.
    let mut mw = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
    let t = mw.add_track(TrackConfig::new(SampleEntry::prores(crate::isobmff::FourCc(*b"apch"), 64, 48), 25)).unwrap();
    mw.write_sample(t, WriteSample { data: &prores_frame_claiming(56, 48), duration: 1, composition_offset: 0, is_sync: true }).unwrap();
    let mut m = Movie::open(mw.finish().unwrap().into_inner()).unwrap();
    assert!(matches!(m.frame(0), Err(VideoError::Decode(e)) if e.contains("56x48 frame in a 64x48 track")));
}

/// In-memory movie bytes, zero-padded up to `len` without storing the padding. Records the
/// largest read asked of it and refuses reads over 1 MiB (the test movies are much smaller).
struct PaddedSource {
    bytes: Vec<u8>,
    len: u64,
    largest: AtomicUsize,
}

impl ByteSource for PaddedSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.largest.fetch_max(buf.len(), Ordering::Relaxed);
        if buf.len() > 1 << 20 {
            return Err(std::io::Error::other("read too large for the test source"));
        }
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.bytes.get(start.saturating_add(i)).copied().unwrap_or(0);
        }
        Ok(())
    }
}

#[test]
fn h264_sample_reads_are_capped() {
    // One frame whose `stsz` entry claims 300 MiB, in a source long enough to hold it.
    let mut bytes = h264_mp4(1, 1).unwrap();
    let at = bytes.windows(4).position(|w| w == b"stsz").unwrap();
    // After the type: version and flags, the constant size (0: one size per sample), the count.
    let field = if bytes[at + 8..at + 12] == [0; 4] { at + 16 } else { at + 8 };
    bytes[field..field + 4].copy_from_slice(&(300u32 << 20).to_be_bytes());
    let src = Arc::new(PaddedSource { bytes, len: 1 << 30, largest: AtomicUsize::new(0) });
    let mut m = Movie::open_source(src.clone()).unwrap();
    let r = m.frame(0);
    assert!(matches!(&r, Err(VideoError::Decode(e)) if e.contains("sample larger than 256 MiB")), "{r:?}");
    let largest = src.largest.load(Ordering::Relaxed);
    assert!(largest <= 256 << 20, "a {largest}-byte read reached the source");
}

#[test]
fn unsupported_codec_opens_but_does_not_decode() {
    let mut mw = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)).unwrap();
    let t = mw.add_track(TrackConfig::new(SampleEntry::hevc(Default::default(), 64, 48), 24)).unwrap();
    mw.write_sample(t, WriteSample { data: &[0, 0, 0, 1, 0x40], duration: 1, composition_offset: 0, is_sync: true }).unwrap();
    let bytes = mw.finish().unwrap().into_inner();
    let mut m = Movie::open(bytes).unwrap();
    assert_eq!(m.info().codec, "hevc");
    assert!(!m.info().decodable);
    assert!(matches!(m.frame(0), Err(VideoError::Unsupported(_))));
    assert!(matches!(m.thumbnail_at(0.0, 20), Err(VideoError::Unsupported(_))));
}

#[test]
fn garbage_and_audio_only_are_errors() {
    assert!(matches!(Movie::open(Vec::new()), Err(VideoError::Container(_))));
    assert!(Movie::open(vec![0u8; 64]).is_err());
    assert!(Movie::open(b"RIFF\0\0\0\0WAVEfmt ".to_vec()).is_err());
}

/// Deterministic xorshift.
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn mutated_and_truncated_movies_never_panic() {
    let sources = [h264_mp4(12, 4).unwrap(), prores_mov(3).unwrap(), mjpeg_mov(3).unwrap()];
    let mut seed = 0x5eed_u64;
    for src in &sources {
        for round in 0..150 {
            let mut b = src.clone();
            if round % 3 == 0 {
                let cut = (rng(&mut seed) as usize) % b.len().max(1);
                b.truncate(cut);
            } else {
                for _ in 0..1 + rng(&mut seed) % 8 {
                    let at = (rng(&mut seed) as usize) % b.len().max(1);
                    if let Some(x) = b.get_mut(at) {
                        *x = rng(&mut seed) as u8;
                    }
                }
            }
            if let Ok(mut m) = Movie::open(b) {
                let d = m.info().duration;
                for k in 0..4 {
                    let _ = m.frame_at(d * f64::from(k) / 4.0);
                }
                let _ = m.thumbnails(3, 16);
            }
        }
    }
}

/// Real encoder output (B-frames, CABAC, 8x8 transform), when ffmpeg + libx264 are available.
#[test]
fn ffmpeg_h264_high_with_bframes() {
    let Some(dir) = ffmpeg_dir() else { return };
    let path = dir.join("high_bframes.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x96:rate=24:duration=2", "-c:v", "libx264", "-profile:v", "high"])
        .args(["-pix_fmt", "yuv420p", "-bf", "3", "-g", "12", "-movflags", "+faststart"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipping: ffmpeg could not encode H.264 (libx264 missing?)");
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let mut m = Movie::open(bytes.clone()).unwrap();
    assert_eq!(m.info().codec_detail, "H.264 High");
    assert_eq!((m.info().width, m.info().height), (160, 96));
    assert_eq!(m.info().frame_count, 48);
    // Sequential frames must equal frames decoded after random seeks (decoder state independence).
    let seq: Vec<Vec<u8>> = (0..48).map(|i| m.frame(i).unwrap().rgba.clone()).collect();
    let mut r = Movie::open(bytes).unwrap();
    r.set_cache_frames(1);
    for &i in &[47usize, 13, 0, 25, 24, 11, 36, 2] {
        assert!(r.frame(i).unwrap().rgba == seq[i], "frame {i} differs after a seek");
    }
    // testsrc2 frames differ from each other.
    assert!(seq[0] != seq[1]);
    let _ = std::fs::remove_dir_all(dir);
}

fn ffmpeg_dir() -> Option<std::path::PathBuf> {
    let found = std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipping: ffmpeg not on PATH");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("soundcraft-video-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[test]
fn test_pattern_moves() {
    let mut m = Movie::open(crate::synth::test_pattern_mp4(1).unwrap()).unwrap();
    assert_eq!((m.info().width, m.info().height, m.info().frame_count), (320, 192, 24));
    let white_x = |f: &crate::Frame| (0..20).find(|&mx| f.rgba[((9 * 16 + 8) * 320 + mx * 16 + 8) * 4] > 250);
    let f3 = m.frame(3).unwrap();
    assert_eq!(white_x(&f3), Some(3));
    let f17 = m.frame_at(17.5 / 24.0).unwrap();
    assert_eq!(white_x(&f17), Some(17));
}
