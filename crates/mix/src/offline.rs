use super::clips::{ClipFx, clip_fx_params, render_clip};
use super::pan::itu_stereo_fold;
use super::{AutoParam, BusId, ChannelFormat, MixEngine, Range, Route, Samples, Session, TrackId, main_channels};

/// Ceiling on one in-memory offline render (samples summed over channels): 2^29 f32 = 2 GiB, about
/// 93 minutes of stereo at 48 kHz. Longer requests are truncated rather than exhausting memory;
/// [`RangeRender`] streams any length.
pub const MAX_RENDER_SAMPLES: usize = 1 << 29;

/// Offline render of the main mix over `range` (planar, [`main_channels`] channels in SMPTE/WAV
/// order). The mix's delay-compensation latency is removed, so the result lines up with the
/// timeline.
pub fn render_range(s: &Session, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, None, range, block)
}

/// Offline render of one bus over `range` (planar, the bus format's channel count).
/// The compensation delay written onto the bus is removed (track inserts, and the aux returns
/// when an aux returns onto it), so the bus lines up with the timeline. An unknown bus is silent.
pub fn render_bus(s: &Session, bus: BusId, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, Some(bus), range, block)
}

fn render_in_memory(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Vec<Vec<f32>> {
    let nch = source_channels(s, bus);
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / nch.max(1));
    let mut out: Vec<Vec<f32>> = (0..nch).map(|_| Vec::with_capacity(len)).collect();
    let mut render = RangeRender::new(s, bus, Range::new(range.start, range.start + len as i64), block);
    while let Some((chunk, n)) = render.next_block(s) {
        for (o, c) in out.iter_mut().zip(chunk) {
            o.extend_from_slice(c.get(..n).unwrap_or_default());
        }
    }
    out
}

/// Channels of the main mix, or of `bus` (none for an unknown bus).
pub fn source_channels(s: &Session, bus: Option<BusId>) -> usize {
    match bus {
        Some(id) => s.bus(id).map_or(0, |b| b.format.channels()),
        None => main_channels(s),
    }
}

/// [`render_range`] or [`render_bus`] of any length, a block at a time.
pub struct RangeRender {
    eng: MixEngine,
    bus: Option<BusId>,
    tmp: Vec<Vec<f32>>,
    start: Samples,
    done: usize,
    total: usize,
    latency: usize,
}

impl RangeRender {
    /// Renders the main mix over `range`, or `bus` when one is named.
    pub fn new(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Self {
        let nch = source_channels(s, bus);
        let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, block);
        eng.set_offline(true);
        let tmp = vec![vec![0.0f32; eng.max_block()]; nch];
        let len = if nch == 0 { 0 } else { usize::try_from(range.len().max(0)).unwrap_or(0) };
        RangeRender { eng, bus, tmp, start: range.start, done: 0, total: len, latency: 0 }
    }

    fn render(&mut self, s: &Session, pos: Samples, n: usize) {
        match self.bus {
            Some(id) => {
                self.eng.render(s, pos, n, &mut []);
                self.eng.copy_bus(id, n, &mut self.tmp);
            }
            None => self.eng.render(s, pos, n, &mut self.tmp),
        }
    }

    /// Renders one more block of the mix of `s` (the session given to [`RangeRender::new`]) and
    /// returns it as `(channels, n)`: the next `n` samples of the range lead each channel (none
    /// while the mix's latency is still being skipped). `None` once the range is done.
    pub fn next_block(&mut self, s: &Session) -> Option<(&[Vec<f32>], usize)> {
        if self.done >= self.total {
            return None;
        }
        let n = (self.total - self.done).min(self.eng.max_block());
        self.render(s, self.start + self.done as i64, n);
        if self.done == 0 {
            // The first block tells the mix's latency: render that much more and drop the head.
            self.latency = match self.bus {
                Some(b) => self.eng.bus_latency(b),
                None => self.eng.latency,
            };
            self.total = self.total.saturating_add(self.latency);
        }
        let skip = self.latency.saturating_sub(self.done).min(n);
        self.done += n;
        if skip > 0 && skip < n {
            for c in &mut self.tmp {
                c.copy_within(skip..n, 0);
            }
        }
        Some((&self.tmp, n - skip))
    }
}

/// ITU-R BS.775 stereo fold-down of planar audio in `fmt` (C and surrounds at -3 dB, LFE dropped;
/// Ambisonics play their W channel). Mono/stereo input is returned unchanged.
pub fn fold_down_stereo(fmt: ChannelFormat, ch: &[Vec<f32>]) -> Vec<Vec<f32>> {
    if ch.len() <= 2 {
        return ch.to_vec();
    }
    let len = ch.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![vec![0.0f32; len]; 2];
    let sp = fmt.speakers();
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (c, src) in ch.iter().enumerate() {
        let (gl, gr) = match sp.get(c) {
            Some(x) if sp.len() == ch.len() => itu_stereo_fold(*x),
            _ if c == 0 => (h, h),
            _ => (0.0, 0.0),
        };
        for (g, dst) in [(gl, 0usize), (gr, 1)] {
            if g == 0.0 {
                continue;
            }
            if let Some(d) = out.get_mut(dst) {
                for (o, x) in d.iter_mut().zip(src.iter()) {
                    *o += x * g;
                }
            }
        }
    }
    out
}

/// Block size for clip effects in [`render_clips`].
const CLIP_FX_BLOCK: usize = 4096;
/// Most a clip effect keeps rendering past its clip's end in [`render_clips`], in seconds.
const MAX_CLIP_FX_TAIL_SECONDS: f64 = 10.0;
/// Silent lead-in run before a clip with effects so smoothed parameters (EQ) settle, in seconds
/// (the smoothers settle within ~0.3 s).
const CLIP_FX_SETTLE_SECONDS: f64 = 1.0;
/// Frames an effect advances its smoothed parameters by at a time (the EQ's chunk). Processing starts on a
/// multiple of it from the selection's start, as a render of the whole selection would, so the smoothers
/// step at the same samples.
const CLIP_FX_CHUNK: usize = 32;
/// A tail block whose peak is below this (-180 dBFS) ends a clip effect's tail.
const CLIP_FX_TAIL_FLOOR: f32 = 1e-9;

/// Render only one track's clips (no mixer processing) — Consolidate.
pub fn render_clips(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / t.channels().max(1));
    let mut out = vec![vec![0.0f32; len]; t.channels().max(1)];
    let mut scratch = Vec::new();
    // One reusable block for every clip with effects.
    let mut block: Vec<Vec<f32>> = Vec::new();
    let max_tail = s.sample_rate.samples(MAX_CLIP_FX_TAIL_SECONDS).max(0);
    for c in t.clips().iter().filter(|c| !c.muted && c.range().overlaps(&range)) {
        match clip_fx_params(s, c.id.0) {
            None => render_clip(s, c, range.start, len, &mut out, &mut scratch),
            Some(params) => {
                if block.len() != out.len() {
                    block = vec![vec![0.0f32; CLIP_FX_BLOCK]; out.len()];
                }
                let mut fx = ClipFx::new(s.sample_rate.as_f64() as f32, out.len());
                fx.configure(params);
                // The effects start from silence, so nothing comes out before the clip starts:
                // process from there through the clip and a bounded tail, not the whole selection.
                let offset = |at: Samples| usize::try_from(at.saturating_sub(range.start).max(0)).unwrap_or(usize::MAX).min(len);
                let first = offset(c.start);
                let clip_end = offset(c.end());
                let tail_end = offset(c.end().saturating_add(max_tail));
                // Smoothed parameters start at their defaults; settle them on silence, as the
                // full-span render did, so the clip opens at the configured setting.
                // (the clip's own samples start inside the first chunk when `first` is not on the grid)
                let grid = first - first % CLIP_FX_CHUNK;
                let mut lead = grid.min(usize::try_from(s.sample_rate.samples(CLIP_FX_SETTLE_SECONDS).max(0)).unwrap_or(0));
                lead -= lead % CLIP_FX_CHUNK;
                // a gain-only effect has no smoother to settle
                if !fx.has_smoothed_modules() {
                    lead = 0;
                }
                while lead > 0 {
                    let n = lead.min(CLIP_FX_BLOCK);
                    for ch in &mut block {
                        ch.fill(0.0);
                    }
                    fx.process(&mut block, n);
                    lead -= n;
                }
                let mut done = grid;
                while done < len {
                    let n = (len - done).min(CLIP_FX_BLOCK);
                    for ch in &mut block {
                        ch.fill(0.0);
                    }
                    if done < clip_end {
                        render_clip(s, c, range.start.saturating_add(done as i64), n, &mut block, &mut scratch);
                    }
                    fx.process(&mut block, n);
                    let mut peak = 0.0f32;
                    for (o, b) in out.iter_mut().zip(block.iter()) {
                        if let Some(dst) = o.get_mut(done..done + n) {
                            for (x, y) in dst.iter_mut().zip(b.iter()) {
                                *x += *y;
                                peak = peak.max(y.abs());
                            }
                        }
                    }
                    done += n;
                    if done >= clip_end && (done >= tail_end || peak < CLIP_FX_TAIL_FLOOR) {
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Render one track through its inserts (pre-fader) — Commit / Freeze.
pub fn render_track_pre_fader(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    let mut solo = s.clone();
    // Multichannel tracks render into a main mix of their own format (straight through).
    let fmt = s.track(track).map_or(ChannelFormat::Stereo, |t| if t.channels() > 2 { t.format } else { ChannelFormat::Stereo });
    solo.outputs = vec![soundcraft_model::OutputPath { name: "Out".into(), first_channel: 0, format: fmt }];
    for t in &mut solo.tracks {
        if t.id == track {
            t.mixer.volume_db = 0.0;
            t.mixer.mute = false;
            t.mixer.solo = false;
            t.mixer.output = Route::Main;
            t.mixer.pan = match t.channels() {
                1 => vec![0.0],
                _ => vec![-1.0, 1.0],
            };
            t.automation.retain(|l| matches!(l.param, AutoParam::Plugin { .. }));
            t.mixer.sends.iter_mut().for_each(|x| *x = None);
            t.mixer.surround = None;
        } else {
            t.inactive = true;
        }
    }
    let mut out = render_range(&solo, range, 1024);
    // Mono tracks: fold back to one channel (undo the centre pan law).
    if s.track(track).is_some_and(|t| t.channels() == 1)
        && let Some(l) = out.first().cloned()
    {
        let g = std::f32::consts::SQRT_2;
        out = vec![l.iter().map(|x| x * g).collect()];
    }
    out
}
