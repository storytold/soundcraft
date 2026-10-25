//! The Sampler in its instrument window: a sample strip (the waveform with its start and loop
//! markers, which drag), Load Sample and Clear, and audio files dropped onto the window or onto
//! an instrument track. Markers and every other control go through `mix.instrument_param`, so
//! they undo and script like any edit.

use crate::SoundApp;
use crate::theme::{Tokens, mono, regular};
use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Insert, TrackId};
use std::ffi::OsStr;
use std::path::Path;

/// The built-in Sampler's plugin id.
pub const SAMPLER: &str = "sampler";

/// File extensions a sample can come from (what the audio decoder reads).
const AUDIO_EXTS: &[&str] = &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf"];

/// Markers on the sample strip: the parameter each one sets, and whether it needs a loop.
const MARKERS: [(&str, bool); 3] = [("start", false), ("loop_start", true), ("loop_end", true)];

/// Whether a file name looks like audio the sampler can load.
pub fn is_audio_name(name: &str) -> bool {
    Path::new(name).extension().and_then(OsStr::to_str).is_some_and(|x| AUDIO_EXTS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Sampler parameters that are MIDI notes.
const NOTE_PARAMS: [&str; 3] = ["root_note", "key_low", "key_high"];

/// Whether a built-in instrument's parameter only takes whole numbers (notes and velocities).
pub fn whole_number(plugin: &str, param: &str) -> bool {
    plugin == SAMPLER && (NOTE_PARAMS.contains(&param) || param == "vel_low" || param == "vel_high")
}

/// A built-in instrument's parameter as its window shows it: the sampler's notes by name.
pub fn format_param(plugin: &str, p: &soundcraft_dsp::ParamInfo, value: f32) -> String {
    if plugin == SAMPLER && NOTE_PARAMS.contains(&p.id) {
        let n = value.round().clamp(0.0, 127.0) as u8;
        return format!("{} ({n})", soundcraft_midi::note_name(n));
    }
    if whole_number(plugin, p.id) {
        return format!("{:.0}", value.round());
    }
    p.format(value)
}

fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(SAMPLER).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().unwrap_or(default)
}

/// Remembers `r` as a place where a dropped audio file loads into track `t`'s sampler, and
/// outlines it while files are dragged over it.
pub fn drop_target(app: &mut SoundApp, ctx: &egui::Context, r: Rect, t: TrackId) {
    app.sample_drop_targets.push((r, t));
    let over = ctx.input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p)));
    if over {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("sample_drop", t.0)))).rect_stroke(
            r,
            2.0,
            Stroke::new(2.0, Tokens::current().accent),
            StrokeKind::Inside,
        );
    }
}

/// An audio file dropped on a sampler target whose bytes the shell has to read itself (in the
/// browser, files arrive by name and are read asynchronously). Hand the bytes to
/// [`SoundApp::load_sample_bytes`].
#[derive(Debug, Clone)]
pub struct SampleRead {
    pub track: TrackId,
    pub file: egui::DroppedFileHandle,
}

impl SampleRead {
    /// The dropped file's name.
    pub fn name(&self) -> String {
        self.file.path().file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string()
    }
}

impl SoundApp {
    /// Takes this frame's dropped files: audio dropped on an instrument track or a Sampler window
    /// loads into that track's sampler (the sampler becomes its instrument). A file with a full
    /// path loads straight away; one without (the browser) waits in [`SoundApp::take_sample_reads`]
    /// for the shell to read. Everything else is returned for the shell to open or import.
    pub fn take_dropped_files(&mut self, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        let (files, pos) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        if files.is_empty() {
            return files;
        }
        let Some(track) = self.drop_track(pos) else { return files };
        let mut rest = Vec::new();
        let mut claimed = false;
        for f in files {
            if claimed || !is_audio_name(&f.path().to_string_lossy()) {
                rest.push(f);
                continue;
            }
            claimed = true;
            if f.path().is_absolute() {
                let path = f.path().to_string_lossy().into_owned();
                let r = self.run("mix.sampler_load", json!({"track": track.0, "path": path}));
                self.sample_loaded(track, &path, r);
            } else {
                self.sample_reads.push(SampleRead { track, file: f });
            }
        }
        rest
    }

    /// The track a drop at `pos` loads into. Without a position (some platforms report none
    /// during a drag), a single open Sampler window takes it.
    fn drop_track(&self, pos: Option<egui::Pos2>) -> Option<TrackId> {
        if let Some(p) = pos {
            return self.sample_drop_targets.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, t)| *t);
        }
        let s = self.engine.session();
        let mut open = self.ui.plugin_windows.iter().filter(|(t, slot)| {
            *slot == soundcraft_mix::INSTRUMENT_SLOT && s.track(*t).and_then(|x| x.instrument.as_ref()).is_some_and(|i| i.plugin == SAMPLER)
        });
        match (open.next(), open.next()) {
            (Some((t, _)), None) => Some(*t),
            _ => None,
        }
    }

    /// Dropped files waiting for the shell to read their bytes.
    pub fn take_sample_reads(&mut self) -> Vec<SampleRead> {
        std::mem::take(&mut self.sample_reads)
    }

    /// Loads a dropped file's bytes (or reports why they could not be read) into its track's
    /// sampler, as one undo step.
    pub fn load_sample_bytes(&mut self, read: &SampleRead, bytes: Result<Vec<u8>, String>) {
        let name = read.name();
        let r = bytes.and_then(|b| soundcraft_engine::io::load_sampler_bytes(&mut self.engine, read.track, &name, &b).map_err(|e| e.to_string()));
        self.sample_loaded(read.track, &name, r);
    }

    fn sample_loaded(&mut self, track: TrackId, what: &str, r: Result<serde_json::Value, String>) {
        match r {
            Ok(v) => {
                if !self.ui.plugin_windows.contains(&(track, soundcraft_mix::INSTRUMENT_SLOT)) {
                    self.ui.plugin_windows.push((track, soundcraft_mix::INSTRUMENT_SLOT));
                }
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(what);
                self.ui.status = format!("{name} loaded into the sampler");
            }
            Err(e) => self.ui.status = format!("{what}: {e}"),
        }
    }
}

/// The sample strip and its buttons, shown above the Sampler's parameters.
pub fn sample_strip(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let s = app.engine.session();
    let source = ins.sample.and_then(|id| s.source(id)).cloned();
    let audio = ins.sample.and_then(|id| s.pool.get(id)).cloned();
    ui.horizontal(|ui| {
        let can_pick = app.services.pick_open.is_some();
        let load = ui.add_enabled(can_pick, egui::Button::new("Load Sample…"));
        let load = if can_pick { load } else { load.on_disabled_hover_text("Drop an audio file on this window") };
        if load.clicked()
            && let Some(pick) = &app.services.pick_open
            && let Some(path) = pick("mix.sampler_load", AUDIO_EXTS)
        {
            let _ = app.run("mix.sampler_load", json!({"track": tid.0, "path": path}));
        }
        if ins.sample.is_some() && ui.button("Clear").clicked() {
            let _ = app.run("mix.sampler_clear", json!({"track": tid.0}));
        }
    });
    let info = match (&source, &audio) {
        (Some(src), Some(a)) => {
            let rate = a.buffer.sample_rate.max(1);
            let secs = a.frames() as f64 / f64::from(rate);
            let width = match a.buffer.num_channels() {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n} ch"),
            };
            format!("{} · {secs:.2} s · {width} · {:.1} kHz", src.name, f64::from(rate) / 1000.0)
        }
        (Some(src), None) => format!("{}: missing", src.name),
        _ => "No sample".to_string(),
    };
    ui.label(egui::RichText::new(info).font(mono(11.0)).color(t.text_dim));
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 96.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, t.playlist_bg);
    p.rect_stroke(r, 4.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let Some(audio) = audio else {
        p.text(r.center(), Align2::CENTER_CENTER, "Drop an audio file here", regular(12.0), t.text_dim);
        return;
    };
    let frames = audio.frames().max(1);
    let x_of = |pct: f32| r.min.x + r.width() * (pct / 100.0).clamp(0.0, 1.0);
    // Loop region under the waveform.
    let looping = param(ins, "loop_mode") >= 0.5;
    let (ls, le) = {
        let (a, b) = (param(ins, "loop_start"), param(ins, "loop_end"));
        (a.min(b), a.max(b))
    };
    if looping {
        p.rect_filled(Rect::from_x_y_ranges(x_of(ls)..=x_of(le), r.y_range()), 0.0, t.accent.gamma_multiply(0.18));
    }
    // One min/max column per point, a lane per channel (two at most).
    let lanes = audio.peaks.len().clamp(1, 2);
    let lane_h = r.height() / lanes as f32;
    let cols = r.width().max(1.0) as usize;
    let wave = t.text.gamma_multiply(0.8);
    for (c, peaks) in audio.peaks.iter().take(lanes).enumerate() {
        let mid = r.min.y + lane_h * (c as f32 + 0.5);
        let half = lane_h * 0.45;
        let samples = audio.channel(c).unwrap_or_default();
        for i in 0..cols {
            let a = frames.saturating_mul(i) / cols;
            let b = (frames.saturating_mul(i + 1) / cols).max(a + 1);
            let (lo, hi) = peaks.minmax(samples, a, b);
            let x = r.min.x + i as f32 + 0.5;
            p.line_segment([pos2(x, mid - hi.clamp(-1.0, 1.0) * half), pos2(x, mid - lo.clamp(-1.0, 1.0) * half)], Stroke::new(1.0, wave));
        }
    }
    // Markers: S (sample start), L and E (loop start and end, when looping).
    let shown: Vec<(&str, f32, Color32, &str)> = MARKERS
        .iter()
        .filter(|(_, needs_loop)| !needs_loop || looping)
        .map(|(id, _)| {
            let (col, tag) = match *id {
                "start" => (t.playhead, "S"),
                "loop_start" => (t.accent, "L"),
                _ => (t.accent, "E"),
            };
            (*id, param(ins, id), col, tag)
        })
        .collect();
    for (_, pct, col, tag) in &shown {
        let x = x_of(*pct);
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, *col));
        let flag = Rect::from_min_size(pos2(x, r.min.y), vec2(12.0, 12.0));
        p.rect_filled(flag, 1.0, *col);
        p.text(flag.center(), Align2::CENTER_CENTER, *tag, regular(9.0), t.text_dark);
    }
    // Dragging the nearest marker sets its parameter.
    let grab_id = ui.id().with(("sample_marker", tid.0));
    let nearest = |x: f32| {
        shown
            .iter()
            .map(|(id, pct, ..)| (*id, (x_of(*pct) - x).abs()))
            .filter(|(_, d)| *d <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    };
    if let Some(h) = resp.hover_pos()
        && nearest(h.x).is_some()
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let grabbed = nearest(pos.x).map(str::to_string);
        ui.ctx().data_mut(|d| d.insert_temp(grab_id, grabbed));
    }
    let grabbed = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab_id)).flatten();
    if resp.dragged()
        && let (Some(id), Some(pos)) = (grabbed.as_deref(), resp.interact_pointer_pos())
    {
        let pct = ((pos.x - r.min.x) / r.width().max(1.0) * 100.0).clamp(0.0, 100.0);
        let _ = app.engine.execute_merged(
            "mix.instrument_param",
            &json!({"track": tid.0, "param": id, "value": pct}),
            &format!("instrument_param:{}:{id}", tid.0),
        );
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_show_by_name() {
        let info = soundcraft_dsp::plugin_info(SAMPLER).unwrap();
        let root = info.param("root_note").unwrap();
        assert_eq!(format_param(SAMPLER, root, 60.0), format!("{} (60)", soundcraft_midi::note_name(60)));
        assert_eq!(format_param(SAMPLER, info.param("vel_high").unwrap(), 126.6), "127");
        assert_eq!(format_param(SAMPLER, info.param("attack").unwrap(), 1.0), "1.0 ms");
        assert!(whole_number(SAMPLER, "key_low") && !whole_number(SAMPLER, "attack") && !whole_number("subtractive_synth", "root_note"));
    }

    #[test]
    fn audio_names() {
        for n in ["a.wav", "B.FLAC", "x.mp3", "y.ogg", "z.aiff", "/tmp/k.Wav"] {
            assert!(is_audio_name(n), "{n}");
        }
        for n in ["song.scraft", "beat.mid", "noext", "", "pic.png"] {
            assert!(!is_audio_name(n), "{n}");
        }
    }
}
