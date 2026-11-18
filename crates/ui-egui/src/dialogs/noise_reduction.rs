//! AudioSuite › Noise Reduction: learn the background noise from a selected passage, then remove
//! it from the selected clips or the selected part of a track.
use crate::SoundApp;
use egui::{Pos2, Stroke, vec2};
use serde_json::json;
use soundcraft_dsp::denoise::{BINS, FFT_SIZE, NoiseProfile};
use soundcraft_model::TrackId;
use soundcraft_time::Range;

/// Keeps the engine's profile and the one saved with the settings in step: a profile saved before
/// a restart goes back to the engine, and a profile learned or set another way (an agent, the
/// command line) is saved.
pub(crate) fn sync_profile(app: &mut SoundApp) {
    match (&app.engine.noise_profile, &app.ui.noise_profile) {
        (None, Some(saved)) if saved.is_valid() => app.engine.noise_profile = Some(saved.clone()),
        (Some(used), saved) if saved.as_ref() != Some(used) => app.ui.noise_profile = Some(used.clone()),
        _ => {}
    }
}

/// What Apply and Preview work on: the selected audio tracks over the edit selection, or else the
/// tracks of the selected clips over the span of those clips.
fn targets(app: &SoundApp) -> Option<(Vec<u64>, Range)> {
    let s = app.engine.session();
    let audio = |t: &TrackId| s.track(*t).is_some_and(|t| t.kind == soundcraft_model::TrackKind::Audio);
    let sel = s.edit.selection;
    if !sel.is_empty() {
        let tracks: Vec<u64> = s.edit.selected_tracks.iter().filter(|t| audio(t)).map(|t| t.0).collect();
        if !tracks.is_empty() {
            return Some((tracks, sel));
        }
    }
    let mut tracks = Vec::new();
    let mut span: Option<Range> = None;
    for (t, c) in s.edit.selected_clips.iter().filter_map(|id| s.find_clip(*id)) {
        if !c.is_audio() {
            continue;
        }
        if !tracks.contains(&t.0) {
            tracks.push(t.0);
        }
        let r = c.range();
        span = Some(span.map_or(r, |o| Range::new(o.start.min(r.start), o.end.max(r.end))));
    }
    span.map(|r| (tracks, r))
}

/// The recommended settings (reduction, sensitivity, smoothing): the DSP's defaults.
pub(crate) fn recommended() -> [f32; 3] {
    let d = soundcraft_dsp::denoise::ReduceSettings::default();
    [d.reduction_db, d.sensitivity_db, d.smoothing]
}

/// The settings a running preview was made with, so a change re-renders it.
#[derive(Clone, Copy, PartialEq)]
struct Preview {
    settings: [f32; 3],
}

fn settings_json(app: &SoundApp, tracks: &[u64], r: Range) -> serde_json::Value {
    let [reduction, sensitivity, smoothing] = app.ui.noise_settings;
    json!({"tracks": tracks, "start": r.start, "end": r.end, "reduction_db": reduction, "sensitivity_db": sensitivity, "smoothing": smoothing})
}

/// Starts (or, with `restart` false, refreshes) the preview of the current targets and settings.
fn preview(app: &mut SoundApp, restart: bool) -> Result<(), String> {
    let (tracks, r) =
        targets(app).ok_or_else(|| crate::localization::text("Select the clips to clean, or a range on audio tracks, first.").to_string())?;
    let most = app.engine.session().sample_rate.samples(soundcraft_engine::cmd::NOISE_PREVIEW_SECONDS);
    let r = Range::new(r.start, r.end.min(r.start.saturating_add(most)));
    let session = soundcraft_engine::cmd::noise_reduction_preview(&app.engine, &settings_json(app, &tracks, r))
        .map_err(|e| crate::localization::error_text(&e.to_string()))?;
    if restart {
        app.loop_preview(session, r);
    } else {
        app.update_preview(session);
    }
    Ok(())
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    sync_profile(app);
    let mut open = app.ui.show_noise_reduction;
    let key = egui::Id::unique("noise_reduction_result");
    let pkey = egui::Id::unique("noise_reduction_preview");
    let mut previewing: Option<Preview> = ctx.data(|d| d.get_temp(pkey)).flatten();
    if previewing.is_some() && !app.is_playing() {
        previewing = None;
    }
    // A setting changed while the preview plays: hear the new one once the slider is let go.
    if let Some(pv) = previewing
        && pv.settings != app.ui.noise_settings
        && !ctx.input(|i| i.pointer.any_down())
    {
        let msg = preview(app, false).err().unwrap_or_default();
        ctx.data_mut(|d| d.insert_temp(key, msg));
        previewing = Some(Preview { settings: app.ui.noise_settings });
    }
    if !open {
        if previewing.is_some() {
            app.stop_play();
            previewing = None;
        }
        ctx.data_mut(|d| d.insert_temp(pkey, previewing));
        return;
    }
    egui::Window::new(crate::localization::text("Noise Reduction"))
        .id(egui::Id::unique("Noise Reduction"))
        .open(&mut open)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(crate::localization::text("1. Learn the noise")).strong());
            ui.label(crate::localization::text("Select a passage that holds only the background noise (a silence between phrases), then click Learn."));
            ui.horizontal(|ui| {
                if ui.button(crate::localization::text("Learn")).clicked() {
                    let r = app.run("audiosuite.noise_learn", json!({}));
                    if r.is_ok() {
                        app.ui.noise_profile = app.engine.noise_profile.clone();
                    }
                    ui.ctx().data_mut(|d| d.insert_temp(key, r.map_or_else(|e| e, |_| String::new())));
                }
                let forget = ui.add_enabled(app.engine.noise_profile.is_some(), egui::Button::new(crate::localization::text("Forget")));
                if forget.clicked() {
                    let _ = app.run("audiosuite.noise_profile_set", json!({"profile": null}));
                    app.ui.noise_profile = None;
                }
            });
            match &app.engine.noise_profile {
                Some(p) => {
                    ui.label(crate::localized_format!(
                        "Noise profile: {secs:.2} s, {channels} channel(s), {rate} Hz",
                        secs = p.seconds(),
                        channels = p.channels(),
                        rate = p.sample_rate
                    ));
                    spectrum(ui, p);
                }
                None => {
                    ui.label(egui::RichText::new(crate::localization::text("No noise profile learned yet.")).weak());
                }
            }
            ui.separator();
            ui.label(egui::RichText::new(crate::localization::text("2. Remove it")).strong());
            ui.label(crate::localization::text(
                "Select the clips to clean, or a range on the tracks to clean only that part. The cleaned audio goes on a copy of the track; the original is kept.",
            ));
            let [reduction, sensitivity, smoothing] = &mut app.ui.noise_settings;
            let slider = |ui: &mut egui::Ui, v: &mut f32, range: std::ops::RangeInclusive<f64>, label: &str, default: f64| {
                let mut wide = f64::from(*v);
                crate::widgets::FineSlider::new(&mut wide, range).text(crate::localization::text(label)).default_value(default).show(ui);
                *v = wide as f32;
            };
            let [d_reduction, d_sensitivity, d_smoothing] = recommended().map(f64::from);
            slider(ui, reduction, 0.0..=48.0, "Reduction (dB)", d_reduction);
            slider(ui, sensitivity, 0.0..=24.0, "Sensitivity (dB)", d_sensitivity);
            slider(ui, smoothing, 0.0..=1.0, "Smoothing", d_smoothing);
            ui.label(
                egui::RichText::new(crate::localization::text(
                    "More reduction or sensitivity removes more noise; too much makes the sound hollow or watery.",
                ))
                .small(),
            );
            let has = app.engine.noise_profile.is_some();
            ui.horizontal(|ui| {
                ui.label(crate::localization::text("Listen:"));
                if ui.add_enabled(has, egui::Button::new(crate::localization::text("Preview"))).clicked() {
                    match preview(app, true) {
                        Ok(()) => {
                            previewing = Some(Preview { settings: app.ui.noise_settings });
                            ui.ctx().data_mut(|d| d.insert_temp(key, String::new()));
                        }
                        Err(e) => {
                            ui.ctx().data_mut(|d| d.insert_temp(key, e));
                        }
                    }
                }
                if ui.button(crate::localization::text("Original")).clicked() {
                    previewing = None;
                    match targets(app) {
                        Some((_, r)) => app.loop_range(r),
                        None => {
                            let msg = crate::localization::text("Select the clips to clean, or a range on audio tracks, first.").to_string();
                            ui.ctx().data_mut(|d| d.insert_temp(key, msg));
                        }
                    }
                }
                // A running preview picks the new values up like a slider move.
                let reset = ui.add_enabled(app.ui.noise_settings != recommended(), egui::Button::new(crate::localization::text("Reset")));
                if reset.on_hover_text(crate::localization::text("Back to the recommended settings")).clicked() {
                    app.ui.noise_settings = recommended();
                }
            });
            ui.label(
                egui::RichText::new(if previewing.is_some() {
                    crate::localization::text("Playing the cleaned audio. Move a slider to hear the change.")
                } else {
                    crate::localization::text("Preview loops the cleaned selection (its first 30 s); Original loops it untouched.")
                })
                .small(),
            );
            ui.separator();
            crate::widgets::checkbox(ui, &mut app.ui.noise_mute_original, crate::localization::text("Mute the original track"));
            if ui.add_enabled(has, egui::Button::new(egui::RichText::new(crate::localization::text("Apply")).strong())).clicked() {
                let [reduction, sensitivity, smoothing] = app.ui.noise_settings;
                let mut p = json!({"reduction_db": reduction, "sensitivity_db": sensitivity, "smoothing": smoothing, "mute_original": app.ui.noise_mute_original});
                // The same targets the preview played.
                if let Some((tracks, r)) = targets(app) {
                    p = settings_json(app, &tracks, r);
                    p["mute_original"] = json!(app.ui.noise_mute_original);
                }
                if previewing.take().is_some() {
                    app.stop_play();
                }
                let r = app.run("audiosuite.noise_reduce", p);
                let msg = match r {
                    Ok(v) => crate::localized_format!("Cleaned {n} clip(s) into a copy of the track.", n = v["processed"].as_u64().unwrap_or(0)),
                    Err(e) => e,
                };
                ui.ctx().data_mut(|d| d.insert_temp(key, msg));
            }
            let msg: String = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_default();
            if !msg.is_empty() {
                ui.label(egui::RichText::new(msg).small());
            }
        });
    if !open && previewing.take().is_some() {
        app.stop_play();
    }
    ctx.data_mut(|d| d.insert_temp(pkey, previewing));
    app.ui.show_noise_reduction = open;
}

/// The learned noise level over frequency, on a log scale from 20 Hz to Nyquist, −120..0 dBFS.
fn spectrum(ui: &mut egui::Ui, p: &NoiseProfile) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 80.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let db = p.spectrum_db();
    let nyquist = (p.sample_rate as f32 / 2.0).max(40.0);
    let (lo, hi) = (20f32.ln(), nyquist.ln());
    let points: Vec<Pos2> = (1..BINS)
        .filter_map(|k| {
            let hz = k as f32 * p.sample_rate as f32 / FFT_SIZE as f32;
            let x = (hz.max(20.0).ln() - lo) / (hi - lo);
            let y = (db.get(k).copied().unwrap_or(-120.0).clamp(-120.0, 0.0) + 120.0) / 120.0;
            (hz >= 20.0).then(|| Pos2::new(rect.left() + x * rect.width(), rect.bottom() - y * rect.height()))
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, ui.visuals().selection.bg_fill)));
    let dim = ui.visuals().weak_text_color();
    for (hz, label) in [(100.0f32, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        if hz < nyquist {
            let x = rect.left() + (hz.ln() - lo) / (hi - lo) * rect.width();
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(0.5, dim.gamma_multiply(0.4)));
            painter.text(Pos2::new(x + 2.0, rect.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, label, egui::FontId::proportional(9.0), dim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;

    fn profile() -> NoiseProfile {
        let hiss: Vec<f32> = (0..4096).map(|i| ((i * 7919) % 101) as f32 / 1000.0 - 0.05).collect();
        NoiseProfile::learn(&[hiss], 48_000).unwrap()
    }

    #[test]
    fn the_window_defaults_are_the_recommended_settings() {
        assert_eq!(crate::UiState::default().noise_settings, recommended());
    }

    #[test]
    fn a_saved_profile_returns_to_the_engine_after_a_restart() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.ui.noise_profile = Some(profile());
        sync_profile(&mut app);
        assert_eq!(app.engine.noise_profile, app.ui.noise_profile);
    }

    #[test]
    fn a_profile_set_by_command_is_saved_and_forgetting_clears_both() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let data = serde_json::to_value(profile()).unwrap();
        app.run("audiosuite.noise_profile_set", json!({"profile": data})).unwrap();
        sync_profile(&mut app);
        assert!(app.ui.noise_profile.is_some());
        app.run("window.noise_reduction", json!({"value": true})).unwrap();
        assert!(app.ui.show_noise_reduction);
        app.run("audiosuite.noise_profile_set", json!({"profile": null})).unwrap();
        app.ui.noise_profile = None;
        sync_profile(&mut app);
        assert!(app.engine.noise_profile.is_none());
    }

    #[test]
    fn preview_plays_without_changing_the_session() {
        use craft_audio::AudioBuffer;
        use soundcraft_model::{ChannelFormat, Clip, FileFormat, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Sax"));
        let hiss: Vec<f32> = (0..48_000).map(|i| ((i * 7919) % 101) as f32 / 1000.0 - 0.05).collect();
        let src = soundcraft_engine::io::add_source(&mut s, "take", AudioBuffer { sample_rate: 48_000, channels: vec![hiss] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "take", src, 0, 0, 48_000));
        let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
        assert!(preview(&mut app, true).is_err(), "nothing selected");
        app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": 24_000})).unwrap();
        app.run("audiosuite.noise_learn", json!({})).unwrap();
        let rev = app.engine.revision;
        preview(&mut app, true).unwrap();
        assert!(app.is_playing());
        assert_eq!(app.engine.revision, rev);
        assert_eq!(app.engine.session().tracks.len(), 1);
        assert_eq!(targets(&app).map(|(t, r)| (t.len(), r.start, r.end)), Some((1, 0, 24_000)));
    }
}
