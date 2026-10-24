//! [`Movie`]: open an MP4/MOV, report its picture format, and decode frames at times to RGBA.

use crate::convert::{Matrix, Planes, planar_to_rgba, scale_rgba};
use crate::isobmff::{self, ByteSource, CodecConfig, Mp4File, TrackKind};
use crate::{Result, VideoError};
use std::collections::VecDeque;
use std::sync::Arc;

/// A random-access byte source a [`Movie`] can read from (in-memory bytes, or a file on native).
pub type SharedSource = Arc<dyn ByteSource + Send + Sync>;

/// Largest picture we decode (bounds every allocation derived from the file).
pub(crate) const MAX_PIXELS: u64 = 8192 * 8192;

/// A decoded picture, packed RGBA (8 bits per channel, `width * height * 4` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Presentation index (0-based, in display order).
    pub index: usize,
    /// Presentation time in seconds from the start of the movie.
    pub time: f64,
}

impl Frame {
    /// Downscaled copy that fits inside `max_w` × `max_h` (aspect kept; never upscales).
    pub fn fit(&self, max_w: u32, max_h: u32) -> Frame {
        let (w, h) = fit_size(self.width, self.height, max_w, max_h);
        if (w, h) == (self.width, self.height) {
            return self.clone();
        }
        Frame {
            width: w,
            height: h,
            rgba: scale_rgba(&self.rgba, self.width as usize, self.height as usize, w as usize, h as usize),
            index: self.index,
            time: self.time,
        }
    }
}

/// The size that fits `w` × `h` inside `max_w` × `max_h`, keeping the aspect ratio (≥ 1 px).
pub fn fit_size(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let s = (f64::from(max_w.max(1)) / f64::from(w)).min(f64::from(max_h.max(1)) / f64::from(h)).min(1.0);
    (((f64::from(w) * s).round() as u32).max(1), ((f64::from(h) * s).round() as u32).max(1))
}

/// What the movie's picture is, and whether we can decode it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MovieInfo {
    /// Coded picture size (after cropping), in pixels.
    pub width: u32,
    pub height: u32,
    /// Pixel aspect ratio (1.0 = square pixels).
    pub pixel_aspect: f64,
    /// Length of the picture track in seconds.
    pub duration: f64,
    /// Nominal frame rate (frames per second).
    pub frame_rate: f64,
    pub frame_count: usize,
    /// Short codec id: `h264`, `prores`, `mjpeg`, `hevc`, …
    pub codec: String,
    /// Human-readable codec description (e.g. "H.264 High", "Apple ProRes 422 HQ").
    pub codec_detail: String,
    /// True when SoundCraft can decode this picture (otherwise frame requests return `Err`).
    pub decodable: bool,
    /// The file also has an audio track.
    pub has_audio: bool,
    /// QuickTime (`.mov`) rather than ISO MP4 brand.
    pub quicktime: bool,
}

impl MovieInfo {
    /// Display aspect ratio (width / height, including pixel aspect).
    pub fn display_aspect(&self) -> f64 {
        if self.height == 0 { 16.0 / 9.0 } else { f64::from(self.width) * self.pixel_aspect / f64::from(self.height) }
    }
}

enum Codec {
    H264 { avcc: Vec<u8> },
    ProRes,
    Jpeg,
    Unsupported(String),
}

struct H264State {
    dec: crate::h264::Decoder,
    /// Next sample (decode order) to feed.
    next: usize,
    /// Last presentation index output, if any.
    last_out: Option<usize>,
    /// Output pictures not requested yet (presentation index → picture).
    ready: std::collections::BTreeMap<usize, crate::h264::Picture>,
}

/// A tiny LRU of decoded frames.
struct Lru<K: PartialEq + Copy> {
    cap: usize,
    items: VecDeque<(K, Arc<Frame>)>,
}

impl<K: PartialEq + Copy> Lru<K> {
    fn new(cap: usize) -> Self {
        Lru { cap: cap.max(1), items: VecDeque::new() }
    }
    fn get(&mut self, k: K) -> Option<Arc<Frame>> {
        let i = self.items.iter().position(|(x, _)| *x == k)?;
        let item = self.items.remove(i)?;
        let f = Arc::clone(&item.1);
        self.items.push_back(item);
        Some(f)
    }
    fn put(&mut self, k: K, f: Arc<Frame>) {
        self.items.retain(|(x, _)| *x != k);
        self.items.push_back((k, f));
        while self.items.len() > self.cap {
            self.items.pop_front();
        }
    }
    fn contains(&self, k: K) -> bool {
        self.items.iter().any(|(x, _)| *x == k)
    }
}

/// An opened movie: container index plus decoder state and frame caches.
pub struct Movie {
    src: SharedSource,
    mp4: Mp4File,
    track: usize,
    info: MovieInfo,
    codec: Codec,
    /// Presentation index → sample index (decode order).
    order: Vec<usize>,
    /// Sample index → presentation index.
    rank: Vec<usize>,
    /// Presentation index → time in seconds.
    times: Vec<f64>,
    h264: Option<H264State>,
    cache: Lru<usize>,
    thumbs: Lru<(usize, u32)>,
    /// Decoded frames since open (statistics for tests and the UI).
    pub decoded: u64,
}

impl std::fmt::Debug for Movie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Movie").field("info", &self.info).finish_non_exhaustive()
    }
}

impl Movie {
    /// Open a movie held in memory.
    pub fn open(bytes: impl Into<Arc<[u8]>>) -> Result<Movie> {
        let b: Arc<[u8]> = bytes.into();
        Movie::open_source(Arc::new(b))
    }

    /// Open a movie from any random-access source (e.g. a `std::fs::File` on native targets, so
    /// large movies are not read into memory).
    pub fn open_source(src: SharedSource) -> Result<Movie> {
        let mp4 = isobmff::open(&*src).map_err(|e| VideoError::Container(e.to_string()))?;
        let track = mp4
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Video && t.enabled && !t.samples.is_empty())
            .or_else(|| mp4.tracks.iter().position(|t| t.kind == TrackKind::Video && !t.samples.is_empty()))
            .ok_or(VideoError::NoVideoTrack)?;
        let has_audio = mp4.tracks.iter().any(|t| t.kind == TrackKind::Audio && !t.samples.is_empty());
        let t = mp4.tracks.get(track).ok_or(VideoError::NoVideoTrack)?;
        let ts = f64::from(t.timescale.max(1));
        let n = t.samples.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| (t.samples.get(i).map_or(0, |s| s.pts), i));
        let mut rank = vec![0usize; n];
        for (p, &i) in order.iter().enumerate() {
            if let Some(r) = rank.get_mut(i) {
                *r = p;
            }
        }
        let times: Vec<f64> = order.iter().map(|&i| t.presentation_pts(i).unwrap_or(0) as f64 / ts).collect();
        let first = times.first().copied().unwrap_or(0.0);
        let end = order.iter().filter_map(|&i| Some(t.presentation_pts(i)?.saturating_add(i64::from(t.samples.get(i)?.duration)))).max().unwrap_or(0)
            as f64
            / ts;
        let duration = (end - first.min(0.0)).max(0.0);
        // Nominal rate: the most common sample duration.
        let mut durs: Vec<u32> = t.samples.iter().map(|s| s.duration).filter(|&d| d > 0).collect();
        durs.sort_unstable();
        let mut best = (0u32, 0usize);
        let mut k = 0;
        while k < durs.len() {
            let d = durs.get(k).copied().unwrap_or(0);
            let run = durs.iter().skip(k).take_while(|&&x| x == d).count().max(1);
            if run > best.1 {
                best = (d, run);
            }
            k += run;
        }
        let frame_rate = if best.0 > 0 {
            ts / f64::from(best.0)
        } else if duration > 0.0 {
            n as f64 / duration
        } else {
            0.0
        };
        let vp = t.video();
        let (mut width, mut height) = vp.map_or((t.width >> 16, t.height >> 16), |v| (u32::from(v.width), u32::from(v.height)));
        if width == 0 || height == 0 {
            (width, height) = (t.width >> 16, t.height >> 16);
        }
        let pixel_aspect = vp.and_then(|v| v.pixel_aspect).filter(|&(h, v)| h > 0 && v > 0).map_or(1.0, |(h, v)| f64::from(h) / f64::from(v));
        let (codec, name, detail) = match t.codec() {
            Some(CodecConfig::Avc(c)) => {
                let (ok, prof) = match c.profile {
                    66 => (true, "Baseline"),
                    77 => (true, "Main"),
                    88 => (true, "Extended"),
                    100 => (true, "High"),
                    110 => (false, "High 10"),
                    122 => (false, "High 4:2:2"),
                    244 => (false, "High 4:4:4"),
                    _ => (true, "?"),
                };
                let detail = format!("H.264 {prof}");
                if ok {
                    (Codec::H264 { avcc: c.to_bytes() }, "h264", detail)
                } else {
                    (Codec::Unsupported(format!("{detail} (only 8-bit 4:2:0 H.264 is supported)")), "h264", detail)
                }
            }
            Some(CodecConfig::ProRes { fourcc }) => {
                let f = fourcc.0;
                let flavour = match &f {
                    b"apco" => "422 Proxy",
                    b"apcs" => "422 LT",
                    b"apcn" => "422",
                    b"apch" => "422 HQ",
                    b"ap4h" => "4444",
                    b"ap4x" => "4444 XQ",
                    _ => "",
                };
                (Codec::ProRes, "prores", format!("Apple ProRes {flavour}"))
            }
            Some(CodecConfig::Jpeg { fourcc }) if &fourcc.0 != b"mjpb" => (Codec::Jpeg, "mjpeg", "Motion JPEG".to_string()),
            Some(other) => {
                let nm = other.name();
                (Codec::Unsupported(format!("{nm} pictures are not supported yet (H.264, ProRes and Motion JPEG are)")), nm, nm.to_ascii_uppercase())
            }
            None => (Codec::Unsupported("the video track has no sample description".into()), "unknown", "unknown".into()),
        };
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(VideoError::Unsupported(format!("{width}x{height} is larger than SoundCraft decodes")));
        }
        let info = MovieInfo {
            width,
            height,
            pixel_aspect,
            duration,
            frame_rate,
            frame_count: n,
            codec: name.to_string(),
            codec_detail: detail,
            decodable: !matches!(codec, Codec::Unsupported(_)),
            has_audio,
            quicktime: mp4.is_quicktime,
        };
        Ok(Movie { src, mp4, track, info, codec, order, rank, times, h264: None, cache: Lru::new(12), thumbs: Lru::new(256), decoded: 0 })
    }

    pub fn info(&self) -> &MovieInfo {
        &self.info
    }

    /// Frames kept decoded (full size). Thumbnails have their own cache.
    pub fn set_cache_frames(&mut self, n: usize) {
        self.cache.cap = n.clamp(1, 512);
    }

    /// The frame shown at `secs` (presentation index), or `None` before the first / after the
    /// last frame.
    pub fn frame_index_at(&self, secs: f64) -> Option<usize> {
        if !secs.is_finite() || secs < 0.0 || secs >= self.info.duration {
            return None;
        }
        // Allow for rounding of `secs` computed from sample positions.
        let k = self.times.partition_point(|&t| t <= secs + 1e-7);
        Some(k.saturating_sub(1))
    }

    /// Presentation time (seconds) of frame `index`.
    pub fn frame_time(&self, index: usize) -> Option<f64> {
        self.times.get(index).copied()
    }

    /// Decode the frame shown at `secs` (full size).
    pub fn frame_at(&mut self, secs: f64) -> Result<Arc<Frame>> {
        let i = self.frame_index_at(secs).ok_or(VideoError::OutOfRange)?;
        self.frame(i)
    }

    /// True if frame `index` is in the cache (no decoding needed).
    pub fn is_cached(&self, index: usize) -> bool {
        self.cache.contains(index)
    }

    /// Decode frame `index` (presentation order), using the cache and decoding forward from the
    /// previous keyframe when needed.
    pub fn frame(&mut self, index: usize) -> Result<Arc<Frame>> {
        if index >= self.order.len() {
            return Err(VideoError::OutOfRange);
        }
        if let Some(f) = self.cache.get(index) {
            return Ok(f);
        }
        let f = match &self.codec {
            Codec::Unsupported(why) => return Err(VideoError::Unsupported(why.clone())),
            Codec::ProRes | Codec::Jpeg => {
                let s = self.order.get(index).copied().ok_or(VideoError::OutOfRange)?;
                Arc::new(self.decode_intra(s, index)?)
            }
            Codec::H264 { .. } => self.decode_h264(index)?,
        };
        self.cache.put(index, Arc::clone(&f));
        Ok(f)
    }

    /// A small picture for the timeline at `secs`, at most `max_h` pixels high. For long-GOP
    /// codecs this is the keyframe at or before `secs` (cheap: one picture is decoded).
    pub fn thumbnail_at(&mut self, secs: f64, max_h: u32) -> Result<Arc<Frame>> {
        let i = self.frame_index_at(secs).ok_or(VideoError::OutOfRange)?;
        let max_h = max_h.clamp(4, 1080);
        let key = match self.codec {
            Codec::H264 { .. } => {
                let s = self.order.get(i).copied().ok_or(VideoError::OutOfRange)?;
                let k = self.mp4.tracks.get(self.track).map_or(0, |t| t.sync_sample_before(s));
                self.rank.get(k).copied().unwrap_or(i)
            }
            _ => i,
        };
        if let Some(f) = self.thumbs.get((key, max_h)) {
            return Ok(f);
        }
        let full = if let Some(f) = self.cache.get(key) {
            f
        } else {
            match &self.codec {
                Codec::Unsupported(why) => return Err(VideoError::Unsupported(why.clone())),
                Codec::H264 { avcc } => {
                    let avcc = avcc.clone();
                    let s = self.order.get(key).copied().ok_or(VideoError::OutOfRange)?;
                    Arc::new(self.decode_h264_single(&avcc, s, key)?)
                }
                _ => {
                    let s = self.order.get(key).copied().ok_or(VideoError::OutOfRange)?;
                    Arc::new(self.decode_intra(s, key)?)
                }
            }
        };
        let w = (f64::from(max_h) * self.info.display_aspect()).round().clamp(1.0, 4096.0) as u32;
        let t = Arc::new(full.fit(w, max_h));
        self.thumbs.put((key, max_h), Arc::clone(&t));
        Ok(t)
    }

    /// `count` evenly spaced thumbnails across the movie (time, picture).
    pub fn thumbnails(&mut self, count: usize, max_h: u32) -> Vec<(f64, Result<Arc<Frame>>)> {
        let count = count.clamp(1, 1000);
        let d = self.info.duration;
        (0..count)
            .map(|k| {
                let t = d * k as f64 / count as f64;
                (t, self.thumbnail_at(t, max_h))
            })
            .collect()
    }

    fn read(&self, sample: usize) -> Result<Vec<u8>> {
        let t = self.mp4.tracks.get(self.track).ok_or(VideoError::NoVideoTrack)?;
        let size = t.samples.get(sample).map_or(0, |s| s.size);
        if size > 256 << 20 {
            return Err(VideoError::Decode("sample larger than 256 MiB".into()));
        }
        self.mp4.read_sample(&*self.src, self.track, sample).map_err(|e| VideoError::Container(e.to_string()))
    }

    fn decode_intra(&mut self, sample: usize, index: usize) -> Result<Frame> {
        let data = self.read(sample)?;
        let time = self.times.get(index).copied().unwrap_or(0.0);
        self.decoded += 1;
        match self.codec {
            Codec::ProRes => {
                let (hdr, ..) = crate::prores::FrameHeader::parse(&data).map_err(|e| VideoError::Decode(e.to_string()))?;
                let (w, h) = (u32::from(hdr.width), u32::from(hdr.height));
                if (w, h) != (self.info.width, self.info.height) {
                    return Err(VideoError::Decode(format!("{w}x{h} frame in a {}x{} track", self.info.width, self.info.height)));
                }
                let opts = crate::prores::DecodeOptions { bit_depth: Some(8), threads: false };
                let f = crate::prores::decode_frame_with(&data, &opts).map_err(|e| VideoError::Decode(e.to_string()))?;
                if u64::from(f.width) * u64::from(f.height) > MAX_PIXELS {
                    return Err(VideoError::Decode("picture too large".into()));
                }
                let (w, h) = (f.width as usize, f.height as usize);
                let cw = f.chroma_width() as usize;
                let narrow = |p: &[u16]| p.iter().map(|&v| v.min(255) as u8).collect::<Vec<u8>>();
                let (y, u, v) = (narrow(&f.y), narrow(&f.cb), narrow(&f.cr));
                let cx = if cw < w { 2 } else { 1 };
                let m = Matrix::for_code(f.color.matrix, f.height, false);
                let rgba = planar_to_rgba(&Planes { width: w, height: h, y: &y, y_stride: w, u: &u, v: &v, uv_stride: cw, cx, cy: 1 }, &m);
                Ok(Frame { width: f.width, height: f.height, rgba, index, time })
            }
            Codec::Jpeg => {
                let img = image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg).map_err(|e| VideoError::Decode(e.to_string()))?;
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();
                if u64::from(width) * u64::from(height) > MAX_PIXELS {
                    return Err(VideoError::Decode("picture too large".into()));
                }
                Ok(Frame { width, height, rgba: rgba.into_raw(), index, time })
            }
            _ => Err(VideoError::Unsupported("not an intra-frame codec".into())),
        }
    }

    fn h264_to_frame(&self, p: &crate::h264::Picture, index: usize) -> Frame {
        let m = Matrix::for_code(p.color.matrix, p.height, p.color.full_range);
        let rgba = planar_to_rgba(
            &Planes {
                width: p.width as usize,
                height: p.height as usize,
                y: &p.y,
                y_stride: p.y_stride,
                u: &p.u,
                v: &p.v,
                uv_stride: p.uv_stride,
                cx: 2,
                cy: 2,
            },
            &m,
        );
        Frame { width: p.width, height: p.height, rgba, index, time: self.times.get(index).copied().unwrap_or(0.0) }
    }

    /// Decode one sample with a fresh decoder (keyframe thumbnails).
    fn decode_h264_single(&mut self, avcc: &[u8], sample: usize, index: usize) -> Result<Frame> {
        let mut dec = crate::h264::Decoder::from_avcc(avcc).map_err(|e| VideoError::Decode(e.to_string()))?;
        let data = self.read(sample)?;
        let mut pics = dec.decode(&data, index as i64).map_err(|e| VideoError::Decode(e.to_string()))?;
        pics.extend(dec.flush());
        self.decoded += 1;
        let p = pics.iter().find(|p| p.pts == index as i64).or(pics.first()).ok_or_else(|| VideoError::Decode("no picture decoded".into()))?;
        Ok(self.h264_to_frame(p, index))
    }

    fn decode_h264(&mut self, index: usize) -> Result<Arc<Frame>> {
        let Codec::H264 { avcc } = &self.codec else { return Err(VideoError::Unsupported("not H.264".into())) };
        let target = self.order.get(index).copied().ok_or(VideoError::OutOfRange)?;
        let key = self.mp4.tracks.get(self.track).map_or(0, |t| t.sync_sample_before(target));
        let resume = self
            .h264
            .as_ref()
            .is_some_and(|st| st.ready.contains_key(&index) || (key < st.next && st.next != usize::MAX && st.last_out.is_none_or(|lo| lo < index)));
        if !resume {
            let dec = crate::h264::Decoder::from_avcc(avcc).map_err(|e| VideoError::Decode(e.to_string()))?;
            self.h264 = Some(H264State { dec, next: key, last_out: None, ready: std::collections::BTreeMap::new() });
        }
        let total = self.order.len();
        let mut last_err: Option<String> = None;
        // Bounded: each iteration feeds one sample, or flushes once at the end.
        for _ in 0..=total.saturating_add(1) {
            let Some(st) = self.h264.as_mut() else { break };
            st.ready.retain(|q, _| *q >= index);
            if let Some(p) = st.ready.remove(&index) {
                return Ok(Arc::new(self.h264_to_frame(&p, index)));
            }
            if st.next == usize::MAX {
                break;
            }
            let pics = if st.next < total {
                let s = st.next;
                st.next += 1;
                let pts = self.rank.get(s).copied().unwrap_or(0) as i64;
                let data = match self.mp4.read_sample(&*self.src, self.track, s) {
                    Ok(d) => d,
                    Err(e) => {
                        last_err = Some(e.to_string());
                        continue;
                    }
                };
                let Some(st) = self.h264.as_mut() else { break };
                match st.dec.decode(&data, pts) {
                    Ok(p) => p,
                    Err(e) => {
                        // Corrupt access unit: skip it (the decoder conceals what it can).
                        last_err = Some(e.to_string());
                        Vec::new()
                    }
                }
            } else {
                st.next = usize::MAX;
                st.dec.flush()
            };
            let Some(st) = self.h264.as_mut() else { break };
            for p in pics {
                let q = usize::try_from(p.pts).unwrap_or(usize::MAX);
                st.last_out = Some(q);
                self.decoded += 1;
                if q >= index {
                    st.ready.insert(q, p);
                }
            }
            // Pictures waiting for a later request (bounded by the reorder depth in practice).
            while st.ready.len() > 32 {
                st.ready.pop_last();
            }
        }
        Err(VideoError::Decode(last_err.unwrap_or_else(|| format!("frame {index} was not produced"))))
    }
}
