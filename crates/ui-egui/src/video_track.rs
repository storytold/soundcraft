//! Video track: the movie pool (opened from `Session::videos` paths, decoded off the UI thread)
//! and thumbnails drawn along video clips in the Edit window.
//!
//! Decoding runs on one background thread on native targets: the playhead frame request always
//! replaces the previous one (latest wins), thumbnails queue behind it. On the web there are no
//! threads, so the same worker runs inline on the UI thread with a per-frame budget.

use crate::SoundApp;
use crate::edit_window::{sample_at, x_of};
use crate::i18n::tr;
use crate::theme::regular;
use egui::{Color32, Painter, Rect, pos2};
use soundcraft_model::{ClipContent, Session, SourceId, Track};
use soundcraft_time::Samples;
use soundcraft_video::{Frame, Movie, MovieInfo};
use std::collections::HashMap;
use std::sync::Arc;

/// `edit.flags` key set while the Video track is offline (Options › Video Track Online).
pub const OFFLINE_FLAG: &str = "video.offline";
/// Most thumbnail textures kept; beyond that the cache is cleared and refilled on demand.
const MAX_THUMBS: usize = 600;

/// Identity of a movie: session source id plus a hash of its path (ids repeat across sessions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MovieKey(pub SourceId, pub u64);

impl MovieKey {
    pub fn new(id: SourceId, path: &str) -> MovieKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut h);
        MovieKey(id, h.finish())
    }
}

/// A thumbnail request key: movie, time (ms) and pixel height.
type ThumbKey = (MovieKey, i64, u32);

enum Job {
    Frame { key: MovieKey, path: String, secs: f64 },
    Thumb { key: MovieKey, path: String, secs: f64, h: u32, tk: ThumbKey },
}

enum Done {
    Info(MovieKey, Result<MovieInfo, String>),
    Frame(MovieKey, f64, Result<Arc<Frame>, String>),
    Thumb(ThumbKey, Result<Arc<Frame>, String>),
}

/// Owns the open movies; runs jobs (on the worker thread, or inline on wasm).
#[derive(Default)]
struct Worker {
    movies: HashMap<MovieKey, Result<Movie, String>>,
}

impl Worker {
    fn open(&mut self, key: MovieKey, path: &str, out: &mut Vec<Done>) -> Option<&mut Movie> {
        if !self.movies.contains_key(&key) {
            let m = open_movie(path);
            out.push(Done::Info(key, m.as_ref().map(|m| m.info().clone()).map_err(Clone::clone)));
            // Keep a handful of movies open.
            if self.movies.len() >= 8 {
                self.movies.clear();
            }
            self.movies.insert(key, m);
        }
        self.movies.get_mut(&key).and_then(|m| m.as_mut().ok())
    }

    fn run(&mut self, job: Job) -> Vec<Done> {
        let mut out = Vec::new();
        match job {
            Job::Frame { key, path, secs } => {
                let r = match self.open(key, &path, &mut out) {
                    Some(m) => m.frame_at(secs).map_err(|e| e.to_string()),
                    None => Err("movie offline".to_string()),
                };
                out.push(Done::Frame(key, secs, r));
            }
            Job::Thumb { key, path, secs, h, tk } => {
                let r = match self.open(key, &path, &mut out) {
                    Some(m) => m.thumbnail_at(secs, h).map_err(|e| e.to_string()),
                    None => Err("movie offline".to_string()),
                };
                out.push(Done::Thumb(tk, r));
            }
        }
        out
    }
}

fn open_movie(path: &str) -> Result<Movie, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
        Movie::open_source(Arc::new(f)).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Movie::open(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::{Done, Job, Worker};
    use std::collections::VecDeque;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Condvar, Mutex};

    #[derive(Default)]
    struct Queue {
        frame: Option<Job>,
        thumbs: VecDeque<Job>,
        quit: bool,
    }

    /// The decoding thread and its job queue.
    pub struct Backend {
        shared: Arc<(Mutex<Queue>, Condvar)>,
        rx: Receiver<Done>,
    }

    impl Backend {
        pub fn new(ctx: egui::Context) -> Option<Backend> {
            let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
            let (tx, rx) = channel();
            let s2 = Arc::clone(&shared);
            std::thread::Builder::new().name("video-decode".into()).spawn(move || run(&s2, &tx, &ctx)).ok()?;
            Some(Backend { shared, rx })
        }

        pub fn submit(&mut self, job: Job) {
            let (lock, cv) = &*self.shared;
            if let Ok(mut q) = lock.lock() {
                match job {
                    Job::Frame { .. } => q.frame = Some(job),
                    Job::Thumb { .. } => {
                        q.thumbs.push_back(job);
                        // Stale requests (scrolled away) are dropped; their slots are freed so
                        // they can be asked again.
                        while q.thumbs.len() > 96 {
                            q.thumbs.pop_front();
                        }
                    }
                }
                cv.notify_one();
            }
        }

        /// Finished jobs.
        pub fn poll(&mut self) -> Vec<Done> {
            self.rx.try_iter().collect()
        }

        /// Thumbnail jobs still waiting (pending slots not listed here were dropped or are running).
        pub fn queued_thumbs(&self) -> Vec<super::ThumbKey> {
            let (lock, _) = &*self.shared;
            lock.lock()
                .map(|q| q.thumbs.iter().filter_map(|j| if let Job::Thumb { tk, .. } = j { Some(*tk) } else { None }).collect())
                .unwrap_or_default()
        }
    }

    impl Drop for Backend {
        fn drop(&mut self) {
            let (lock, cv) = &*self.shared;
            if let Ok(mut q) = lock.lock() {
                q.quit = true;
            }
            cv.notify_all();
        }
    }

    fn run(shared: &(Mutex<Queue>, Condvar), tx: &Sender<Done>, ctx: &egui::Context) {
        let mut worker = Worker::default();
        loop {
            let job = {
                let (lock, cv) = shared;
                let Ok(mut q) = lock.lock() else { return };
                while q.frame.is_none() && q.thumbs.is_empty() && !q.quit {
                    q = match cv.wait(q) {
                        Ok(q) => q,
                        Err(_) => return,
                    };
                }
                if q.quit {
                    return;
                }
                match q.frame.take() {
                    Some(j) => j,
                    None => match q.thumbs.pop_back() {
                        // Newest first: what is on screen now.
                        Some(j) => j,
                        None => continue,
                    },
                }
            };
            // A decoder bug must not take the app down: contain it to this job.
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run(job)));
            match done {
                Ok(done) => {
                    for d in done {
                        if tx.send(d).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => worker = Worker::default(),
            }
            ctx.request_repaint();
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::{Done, Job, Worker};
    use std::collections::VecDeque;

    /// Inline decoding with a per-frame budget (no threads on the web).
    pub struct Backend {
        worker: Worker,
        frame: Option<Job>,
        thumbs: VecDeque<Job>,
        ctx: egui::Context,
    }

    impl Backend {
        pub fn new(ctx: egui::Context) -> Option<Backend> {
            Some(Backend { worker: Worker::default(), frame: None, thumbs: VecDeque::new(), ctx })
        }
        pub fn submit(&mut self, job: Job) {
            match job {
                Job::Frame { .. } => self.frame = Some(job),
                Job::Thumb { .. } => {
                    self.thumbs.push_back(job);
                    while self.thumbs.len() > 96 {
                        self.thumbs.pop_front();
                    }
                }
            }
        }
        /// Run at most one frame and two thumbnails per UI frame.
        pub fn poll(&mut self) -> Vec<Done> {
            let mut out = Vec::new();
            if let Some(j) = self.frame.take() {
                out.extend(self.worker.run(j));
            }
            for _ in 0..2 {
                let Some(j) = self.thumbs.pop_back() else { break };
                out.extend(self.worker.run(j));
            }
            if !self.thumbs.is_empty() {
                self.ctx.request_repaint();
            }
            out
        }
        pub fn queued_thumbs(&self) -> Vec<super::ThumbKey> {
            self.thumbs.iter().filter_map(|j| if let Job::Thumb { tk, .. } = j { Some(*tk) } else { None }).collect()
        }
    }
}

enum Slot {
    /// Requested at this UI time (seconds).
    Pending(f64),
    Ready(egui::TextureHandle),
    Failed,
}

/// The UI's movie cache: decoded pictures as textures (never serialised; rebuilt from paths).
#[derive(Default)]
pub struct VideoPool {
    backend: Option<backend::Backend>,
    backend_failed: bool,
    infos: HashMap<MovieKey, Result<MovieInfo, String>>,
    thumbs: HashMap<ThumbKey, Slot>,
    /// The latest playhead picture: movie, its frame index, texture.
    current: Option<(MovieKey, usize, egui::TextureHandle)>,
    current_err: Option<(MovieKey, String)>,
    /// Last frame request (to avoid re-sending identical requests).
    last_req: Option<(MovieKey, i64)>,
    /// Pictures shown in the Video window since start (diagnostics).
    pub frames_shown: u64,
}

impl VideoPool {
    fn backend(&mut self, ctx: &egui::Context) -> Option<&mut backend::Backend> {
        if self.backend.is_none() && !self.backend_failed {
            self.backend = backend::Backend::new(ctx.clone());
            self.backend_failed = self.backend.is_none();
        }
        self.backend.as_mut()
    }

    /// Collect finished work (call once per frame before drawing).
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(b) = self.backend.as_mut() else { return };
        let done = b.poll();
        let queued: std::collections::HashSet<ThumbKey> = b.queued_thumbs().into_iter().collect();
        for d in done {
            match d {
                Done::Info(k, info) => {
                    self.infos.insert(k, info);
                }
                Done::Frame(k, _secs, r) => match r {
                    Ok(f) => {
                        let img = color_image(&f);
                        match &mut self.current {
                            Some((ck, ci, tex)) => {
                                tex.set(img, egui::TextureOptions::LINEAR);
                                *ck = k;
                                *ci = f.index;
                            }
                            None => self.current = Some((k, f.index, ctx.load_texture("video-frame", img, egui::TextureOptions::LINEAR))),
                        }
                        self.current_err = None;
                        self.frames_shown += 1;
                    }
                    Err(e) => self.current_err = Some((k, e)),
                },
                Done::Thumb(tk, r) => {
                    let slot = match r {
                        Ok(f) => Slot::Ready(ctx.load_texture(format!("video-thumb-{tk:?}"), color_image(&f), egui::TextureOptions::LINEAR)),
                        Err(_) => Slot::Failed,
                    };
                    self.thumbs.insert(tk, slot);
                }
            }
        }
        // Pending slots whose job fell off the queue (scrolled away) may be asked for again; the
        // one being decoded right now is not queued either, so give it a moment.
        let now = ctx.input(|i| i.time);
        self.thumbs.retain(|k, s| !matches!(s, Slot::Pending(t) if now - *t > 2.0 && !queued.contains(k)));
    }

    pub fn info(&self, key: MovieKey) -> Option<&Result<MovieInfo, String>> {
        self.infos.get(&key)
    }

    /// Ask for the picture at `secs` of the movie (the Video window).
    pub fn request_frame(&mut self, ctx: &egui::Context, key: MovieKey, path: &str, secs: f64, fps: f64) {
        let q = (secs * fps.max(1.0) * 4.0).floor() as i64;
        if self.last_req == Some((key, q)) {
            return;
        }
        self.last_req = Some((key, q));
        let path = path.to_string();
        if let Some(b) = self.backend(ctx) {
            b.submit(Job::Frame { key, path, secs });
        }
    }

    /// The current playhead picture for `key`, if one has been decoded.
    pub fn current(&self, key: MovieKey) -> Option<&egui::TextureHandle> {
        self.current.as_ref().filter(|c| c.0 == key).map(|c| &c.2)
    }

    pub fn current_error(&self, key: MovieKey) -> Option<&str> {
        self.current_err.as_ref().filter(|c| c.0 == key).map(|c| c.1.as_str())
    }

    /// A thumbnail at `secs`, `h` pixels high; requested in the background when missing.
    pub fn thumb(&mut self, ctx: &egui::Context, key: MovieKey, path: &str, secs: f64, h: u32) -> Option<egui::TextureHandle> {
        let tk = (key, (secs * 1000.0).round() as i64, h);
        match self.thumbs.get(&tk) {
            Some(Slot::Ready(t)) => return Some(t.clone()),
            Some(Slot::Pending(_) | Slot::Failed) => return None,
            None => {}
        }
        if self.thumbs.len() >= MAX_THUMBS {
            self.thumbs.retain(|_, s| matches!(s, Slot::Pending(_)));
        }
        self.thumbs.insert(tk, Slot::Pending(ctx.input(|i| i.time)));
        let path = path.to_string();
        if let Some(b) = self.backend(ctx) {
            b.submit(Job::Thumb { key, path, secs, h, tk });
        }
        None
    }
}

fn color_image(f: &Frame) -> egui::ColorImage {
    let (w, h) = (f.width as usize, f.height as usize);
    if f.rgba.len() == w * h * 4 {
        egui::ColorImage::from_rgba_unmultiplied([w, h], &f.rgba)
    } else {
        egui::ColorImage::new([1, 1], vec![Color32::BLACK])
    }
}

/// True unless Options › Video Track Online is off.
pub fn online(s: &Session) -> bool {
    !s.edit.flag(OFFLINE_FLAG)
}

/// Movie time (seconds) shown at timeline position `at` by a video clip, including the session's
/// Video Sync Offset (positive delays the picture).
pub fn movie_secs(s: &Session, clip_start: Samples, offset: Samples, at: Samples) -> f64 {
    let sync = s.edit.value("video.sync_offset", 0.0);
    let sync = if sync.is_finite() { sync } else { 0.0 };
    (at.saturating_sub(clip_start).saturating_add(offset) as f64 - sync) / s.sample_rate.as_f64().max(1.0)
}

/// The picture at `at`: the topmost active Video track with an unmuted clip there.
pub fn picture_at(s: &Session, at: Samples) -> Option<(MovieKey, String, f64, f64)> {
    for t in s.tracks.iter().filter(|t| t.kind == soundcraft_model::TrackKind::Video && !t.inactive) {
        for c in t.clips() {
            if c.muted || !c.range().contains(at) {
                continue;
            }
            if let ClipContent::Video { source, offset } = c.content
                && let Some(v) = s.video(source)
            {
                return Some((MovieKey::new(source, &v.path), v.path.clone(), movie_secs(s, c.start, offset, at), v.frame_rate));
            }
        }
    }
    None
}

/// Draw thumbnails inside the video clips of `track` (called from the Edit window's track row,
/// after the clip blocks are painted).
pub fn draw_lane(app: &mut SoundApp, painter: &Painter, s: &Session, track: &Track, tl: Rect, lane: Rect) {
    if !track.clips().iter().any(|c| c.is_video()) {
        return;
    }
    let ctx = painter.ctx().clone();
    app.video.poll(&ctx);
    let on = online(s);
    let name_h = if lane.height() >= 40.0 && s.edit.flag("view.clip.name") { 13.0 } else { 0.0 };
    let ppp = ctx.pixels_per_point();
    for clip in track.clips() {
        let ClipContent::Video { source, offset } = clip.content else { continue };
        let x0 = x_of(s, tl, clip.start);
        let x1 = x_of(s, tl, clip.end()).max(x0 + 1.0);
        let body = Rect::from_min_max(pos2(x0 + 1.0, lane.min.y + 2.0 + name_h), pos2(x1 - 1.0, lane.max.y - 2.0));
        let vis = body.intersect(lane);
        if vis.width() < 2.0 || vis.height() < 6.0 {
            continue;
        }
        let p = painter.with_clip_rect(vis);
        let Some(v) = s.video(source) else {
            p.text(vis.center(), egui::Align2::CENTER_CENTER, tr("movie missing from session"), regular(10.0), Color32::from_rgb(220, 120, 120));
            continue;
        };
        if !on {
            p.text(vis.center(), egui::Align2::CENTER_CENTER, tr("Video Track Offline"), regular(10.0), Color32::from_gray(170));
            continue;
        }
        let key = MovieKey::new(source, &v.path);
        if let Some(Err(e)) = app.video.info(key) {
            p.text(
                vis.center(),
                egui::Align2::CENTER_CENTER,
                crate::i18n::render("media offline: {e}", &[("{e}", e.to_string())]),
                regular(10.0),
                Color32::from_rgb(220, 120, 120),
            );
            continue;
        }
        let aspect = app.video.info(key).and_then(|i| i.as_ref().ok()).map_or(v.aspect(), MovieInfo::display_aspect) as f32;
        let th = body.height();
        let tw = (th * aspect).clamp(8.0, 600.0);
        // Pixel height bucket (multiples of 16) so zooming the track does not re-decode constantly.
        let hpx = (((th * ppp) / 16.0).ceil() * 16.0).clamp(16.0, 256.0) as u32;
        let first = ((vis.min.x - body.min.x) / tw).floor().max(0.0) as i64;
        let mut k = first;
        loop {
            let tx = body.min.x + k as f32 * tw;
            if tx > vis.max.x || k > first + 400 {
                break;
            }
            let at = sample_at(s, tl, tx);
            let secs = movie_secs(s, clip.start, offset, at.max(clip.start));
            let r = Rect::from_min_max(pos2(tx, body.min.y), pos2((tx + tw).min(body.max.x), body.max.y));
            match app.video.thumb(&ctx, key, &v.path, secs.max(0.0), hpx) {
                Some(tex) => {
                    let full = Rect::from_min_size(r.min, egui::vec2(tw, th));
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2((r.width() / tw).clamp(0.0, 1.0), 1.0));
                    p.image(tex.id(), Rect::from_min_max(full.min, pos2(r.max.x, full.max.y)), uv, Color32::WHITE);
                }
                None => {
                    p.rect_filled(r.shrink(1.0), 0.0, Color32::from_rgb(24, 24, 26));
                }
            }
            p.line_segment([pos2(tx, body.min.y), pos2(tx, body.max.y)], egui::Stroke::new(1.0, Color32::from_black_alpha(160)));
            k += 1;
        }
    }
}
