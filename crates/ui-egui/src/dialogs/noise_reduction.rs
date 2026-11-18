//! AudioSuite › Noise Reduction: learn the background noise from a selected passage, then remove
//! it into a copy of the track (or of the selected part of it), after listening to it.
use crate::SoundApp;
use egui::{Pos2, Stroke, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::denoise::{BINS, FFT_SIZE, NoiseProfile};
use soundcraft_model::TrackId;
use soundcraft_time::Range;

/// Keeps the engine's profile and the one saved with the settings in step. At start-up a saved
/// profile goes back to the engine, once; after that the saved one follows the engine, so a
/// profile learned, set or forgotten another way (an agent, the command line) is saved as it is.
pub(crate) fn sync_profile(app: &mut SoundApp) {
    if !app.noise_profile_restored {
        app.noise_profile_restored = true;
        if app.engine.noise_profile.is_none()
            && let Some(saved) = app.ui.noise_profile.as_ref().filter(|p| p.is_valid())
        {
            app.engine.noise_profile = Some(saved.clone());
            return;
        }
    }
    if app.ui.noise_profile != app.engine.noise_profile {
        app.ui.noise_profile = app.engine.noise_profile.clone();
    }
}

/// What Apply and Preview work on, as `audiosuite.noise_reduce` parameters: the selected audio
/// tracks over the edit selection, or else the selected audio clips (just those, whole).
fn targets(app: &SoundApp) -> Option<Value> {
    let s = app.engine.session();
    let audio = |t: &TrackId| s.track(*t).is_some_and(|t| t.kind == soundcraft_model::TrackKind::Audio);
    let sel = s.edit.selection;
    if !sel.is_empty() {
        let tracks: Vec<u64> = s.edit.selected_tracks.iter().filter(|t| audio(t)).map(|t| t.0).collect();
        if !tracks.is_empty() {
            return Some(json!({"tracks": tracks, "start": sel.start, "end": sel.end}));
        }
    }
    let clips: Vec<u64> = s.edit.selected_clips.iter().filter(|id| s.find_clip(**id).is_some_and(|(_, c)| c.is_audio())).map(|id| id.0).collect();
    (!clips.is_empty()).then(|| json!({"clips": clips}))
}

/// The recommended settings (reduction, sensitivity, smoothing): the DSP's defaults.
pub(crate) fn recommended() -> [f32; 3] {
    let d = soundcraft_dsp::denoise::ReduceSettings::default();
    [d.reduction_db, d.sensitivity_db, d.smoothing]
}

/// A running preview: the targets it was started on (kept, so a refresh after a setting change
/// cleans the same audio the loop plays) and the settings it plays.
#[derive(Clone, PartialEq)]
struct Preview {
    targets: Value,
    settings: [f32; 3],
}

/// `targets` with the current settings.
fn with_settings(app: &SoundApp, targets: &Value) -> Value {
    let [reduction, sensitivity, smoothing] = app.ui.noise_settings;
    let mut p = targets.clone();
    if let Some(o) = p.as_object_mut() {
        o.insert("reduction_db".into(), json!(reduction));
        o.insert("sensitivity_db".into(), json!(sensitivity));
        o.insert("smoothing".into(), json!(smoothing));
    }
    p
}

fn no_targets() -> String {
    crate::localization::text("Select the clips to clean, or a range on audio tracks, first.").to_string()
}

/// Starts a preview of the current targets and settings.
fn start_preview(app: &mut SoundApp) -> Result<Preview, String> {
    let targets = targets(app).ok_or_else(no_targets)?;
    let (session, r) = soundcraft_engine::cmd::noise_reduction_preview(&app.engine, &with_settings(app, &targets))
        .map_err(|e| crate::localization::error_text(&e.to_string()))?;
    app.loop_preview(session, r)?;
    Ok(Preview { targets, settings: app.ui.noise_settings })
}

/// Re-renders a running preview with the current settings, on the targets it started with.
fn refresh_preview(app: &mut SoundApp, pv: &Preview) -> Result<Preview, String> {
    let (session, _) = soundcraft_engine::cmd::noise_reduction_preview(&app.engine, &with_settings(app, &pv.targets))
        .map_err(|e| crate::localization::error_text(&e.to_string()))?;
    app.update_preview(session);
    Ok(Preview { targets: pv.targets.clone(), settings: app.ui.noise_settings })
}

/// The loop range Original plays for the current targets: the same range Preview would loop.
fn original_range(app: &SoundApp) -> Result<Range, String> {
    let targets = targets(app).ok_or_else(no_targets)?;
    soundcraft_engine::cmd::noise_reduction_preview_range(&app.engine, &targets).map_err(|e| crate::localization::error_text(&e.to_string()))
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    sync_profile(app);
    let mut open = app.ui.show_noise_reduction;
    let key = egui::Id::unique("noise_reduction_result");
    let pkey = egui::Id::unique("noise_reduction_preview");
    // The preview's state lasts only while its own audition plays: Play, a stop or an edit ends it.
    let mut previewing: Option<Preview> = ctx.data(|d| d.get_temp(pkey)).flatten().filter(|_| app.auditioning());
    // A setting changed while the preview plays: hear the new one once the slider is let go.
    if let Some(pv) = previewing.clone()
        && pv.settings != app.ui.noise_settings
        && !ctx.input(|i| i.pointer.any_down())
    {
        match refresh_preview(app, &pv) {
            Ok(next) => previewing = Some(next),
            // The settings stay unheard, so the next frame does not retry in a loop.
            Err(e) => {
                ctx.data_mut(|d| d.insert_temp(key, e));
                previewing = Some(Preview { targets: pv.targets, settings: app.ui.noise_settings });
            }
        }
    }
    if !open {
        if previewing.take().is_some() {
            app.stop_audition();
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
                    let msg = match start_preview(app) {
                        Ok(pv) => {
                            previewing = Some(pv);
                            String::new()
                        }
                        Err(e) => e,
                    };
                    ui.ctx().data_mut(|d| d.insert_temp(key, msg));
                }
                if ui.button(crate::localization::text("Original")).clicked() {
                    // Starting the original ends the preview; failing to start leaves it as it is.
                    let msg = match original_range(app).and_then(|r| app.loop_range(r)) {
                        Ok(()) => {
                            previewing = None;
                            String::new()
                        }
                        Err(e) => e,
                    };
                    ui.ctx().data_mut(|d| d.insert_temp(key, msg));
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
                // The same targets the preview played, when it still plays.
                let on = previewing.as_ref().map(|pv| pv.targets.clone()).or_else(|| targets(app)).unwrap_or_else(|| json!({}));
                let mut p = with_settings(app, &on);
                p["mute_original"] = json!(app.ui.noise_mute_original);
                if previewing.take().is_some() {
                    app.stop_audition();
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
        app.stop_audition();
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
    use craft_audio::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Clip, FileFormat, Session, TrackKind};

    fn profile() -> NoiseProfile {
        let hiss: Vec<f32> = (0..4096).map(|i| ((i * 7919) % 101) as f32 / 1000.0 - 0.05).collect();
        NoiseProfile::learn(&[hiss], 48_000).unwrap()
    }

    /// An app with one mono track holding a one-second clip of hiss, its noise already learned.
    fn app_with_take() -> (SoundApp, TrackId, soundcraft_model::ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Sax"));
        let hiss: Vec<f32> = (0..48_000).map(|i| ((i * 7919) % 101) as f32 / 1000.0 - 0.05).collect();
        let src = soundcraft_engine::io::add_source(&mut s, "take", AudioBuffer { sample_rate: 48_000, channels: vec![hiss] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "take", src, 0, 0, 48_000));
        let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
        app.run("audiosuite.noise_learn", json!({"track": t.0, "start": 0, "end": 24_000})).unwrap();
        (app, t, id)
    }

    #[test]
    fn the_window_defaults_are_the_recommended_settings() {
        assert_eq!(crate::UiState::default().noise_settings, recommended());
    }

    #[test]
    fn a_saved_profile_returns_once_and_a_forgotten_one_stays_forgotten() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.ui.noise_profile = Some(profile());
        sync_profile(&mut app);
        assert_eq!(app.engine.noise_profile, app.ui.noise_profile, "restored at start-up");
        // Forgotten by a command, not the window: the saved copy follows instead of restoring it.
        app.run("audiosuite.noise_profile_set", json!({"profile": null})).unwrap();
        sync_profile(&mut app);
        sync_profile(&mut app);
        assert!(app.engine.noise_profile.is_none() && app.ui.noise_profile.is_none());
        // And one set by a command is saved.
        app.run("audiosuite.noise_profile_set", json!({"profile": serde_json::to_value(profile()).unwrap()})).unwrap();
        sync_profile(&mut app);
        assert!(app.ui.noise_profile.is_some());
    }

    #[test]
    fn preview_auditions_without_changing_the_session_and_play_ends_it() {
        let (mut app, t, _) = app_with_take();
        assert!(start_preview(&mut app).is_err(), "nothing selected");
        app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": 24_000})).unwrap();
        let rev = app.engine.revision;
        let pv = start_preview(&mut app).unwrap();
        assert!(app.auditioning());
        assert_eq!((app.engine.revision, app.engine.session().tracks.len()), (rev, 1));
        assert_eq!(pv.targets, json!({"tracks": [t.0], "start": 0, "end": 24_000}));
        // Original plays the document: the audition is over.
        app.loop_range(original_range(&app).unwrap()).unwrap();
        assert!(app.is_playing() && !app.auditioning());
    }

    #[test]
    fn closing_a_preview_keeps_the_selection() {
        let (mut app, t, _) = app_with_take();
        app.engine.session_mut().edit.insertion_follows_playback = true;
        app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": 24_000})).unwrap();
        start_preview(&mut app).unwrap();
        app.engine.transport.position = 12_000;
        app.stop_audition();
        assert!(!app.is_playing());
        let sel = app.engine.session().edit.selection;
        assert_eq!((sel.start, sel.end), (0, 24_000));
    }

    #[test]
    fn selected_clips_are_previewed_and_applied_by_id() {
        let (mut app, t, id) = app_with_take();
        // A second clip on the same track, not selected.
        let src = match app.engine.session().find_clip(id).unwrap().1.content {
            soundcraft_model::ClipContent::Audio { source, .. } => source,
            _ => panic!("an audio clip"),
        };
        let other = app.engine.session_mut().new_clip_id();
        soundcraft_engine::edit::place_clip(app.engine.session_mut(), t, Clip::audio(other, "other", src, 0, 60_000, 24_000));
        app.engine.session_mut().edit.selection = Range::point(0);
        app.engine.session_mut().edit.selected_clips = vec![other];
        assert_eq!(targets(&app), Some(json!({"clips": [other.0]})));
        let pv = start_preview(&mut app).unwrap();
        assert_eq!(pv.targets, json!({"clips": [other.0]}));
    }

    #[test]
    fn no_audition_while_recording() {
        let (mut app, t, _) = app_with_take();
        app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": 24_000})).unwrap();
        app.engine.transport.recording = true;
        assert!(start_preview(&mut app).is_err());
        assert!(app.loop_range(Range::new(0, 24_000)).is_err());
        assert!(!app.auditioning());
    }

    #[test]
    fn an_audition_writes_no_automation() {
        let (mut app, t, _) = app_with_take();
        app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": 24_000})).unwrap();
        app.engine.session_mut().track_mut(t).unwrap().mixer.automation_mode = soundcraft_model::AutomationMode::Write;
        start_preview(&mut app).unwrap();
        let rev = app.engine.revision;
        for pos in [0, 4_800, 9_600] {
            app.engine.transport.position = pos;
            app.write_automation();
        }
        assert_eq!(app.engine.revision, rev, "the audition wrote automation");
    }

    #[test]
    fn the_menu_opens_the_window_and_hiding_floating_windows_closes_it() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let path = "AudioSuite > Noise Reduction…";
        let id = crate::menus::command_for_path(path, &crate::menus::ui_aliases());
        assert_eq!(id.as_deref(), Some("window.noise_reduction"));
        for _ in 0..2 {
            crate::menus::invoke_menu(&mut app, "window.noise_reduction", path);
            assert!(app.ui.show_noise_reduction, "choosing the menu item again keeps the window open");
        }
        app.run("window.hide_floating", json!({})).unwrap();
        assert!(!app.ui.show_noise_reduction);
    }

    #[test]
    fn window_configurations_leave_the_profile_in_the_settings() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.ui.noise_profile = Some(profile());
        // A configuration as the window saves it, then a recall after the profile changed.
        let saved = crate::panels::windows::configuration_snapshot(&app.ui).unwrap();
        assert!(saved["noise_profile"].is_null());
        app.ui.noise_profile = None;
        crate::panels::windows::recall_configuration(&mut app, &saved).unwrap();
        assert!(app.ui.noise_profile.is_none(), "recall keeps the current profile");
        app.ui.noise_profile = Some(profile());
        crate::panels::windows::recall_configuration(&mut app, &saved).unwrap();
        assert!(app.ui.noise_profile.is_some(), "recall keeps the current profile");
    }
}
