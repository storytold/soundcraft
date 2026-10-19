//! Secondary floating windows (Window menu).
use crate::SoundApp;
use crate::i18n::{tr, trf};
use crate::theme::{Tokens, mono, rgb};
use egui::{Color32, Sense, vec2};
use serde_json::json;
use soundcraft_model::{AutomationMode, ClipContent};
use soundcraft_playback::AudioConfig;
use soundcraft_playback::devices::{AudioSystem, DeviceInfo};
use std::sync::Arc;

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audio_health(app, ctx);
    automation(app, ctx);
    color_palette(app, ctx);
    disk_usage(app, ctx);
    system_usage(app, ctx);
    task_manager(app, ctx);
    metadata(app, ctx);
    event_list(app, ctx);
    midi_keyboard(app, ctx);
    workspace(app, ctx);
    configurations(app, ctx);
    playback_engine(app, ctx);
    io_setup(app, ctx);
    shortcuts_window(app, ctx);
    clip_effects(app, ctx);
}

fn audio_health(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_audio_health;
    win(ctx, &mut open, tr("Session Audio Health"), vec2(620.0, 360.0), |ui| {
        let refresh = ui.button(tr("Refresh Report")).clicked();
        if app.audio_health_report.is_none() || refresh {
            app.audio_health_report = Some(app.engine.execute("session.audio_health", &json!({})).map_err(|error| error.to_string()));
        }
        let Some(result) = &app.audio_health_report else { return };
        match result {
            Ok(report) => {
                ui.label(trf("{} audio clips checked · {} issues", &[&report["audio_clips"], &report["issue_count"]]));
                ui.label(match report["active_audio_end_seconds"].as_f64() {
                    Some(seconds) => trf("Active audio ends at {} seconds", &[&format!("{seconds:.2}")]),
                    None => tr("Active audio end time is unknown (invalid session sample rate)").into(),
                });
                ui.label(tr("Includes alternate takes and muted clips. Refresh after editing."));
                ui.separator();
                if report["healthy"] == true {
                    ui.label(tr("All audio clips have loaded media within source bounds."));
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    if let Some(issues) = report["issues"].as_array() {
                        for issue in issues {
                            ui.group(|ui| {
                                let text = |key: &str| issue[key].as_str().unwrap_or("");
                                ui.label(format!(
                                    "{} / {} / {}{}",
                                    text("track"),
                                    text("playlist"),
                                    text("clip"),
                                    if issue["active"] == true { "" } else { tr(" (alternate playlist)") }
                                ));
                                ui.label(text("message"));
                                if let Some(path) = issue["path"].as_str() {
                                    ui.label(path);
                                }
                            });
                        }
                    }
                });
                ui.separator();
                ui.label(tr("Checks loaded media only, not files on disk, signal levels, plugins or routing."));
            }
            Err(error) => {
                ui.label(error.to_string());
            }
        }
    });
    app.ui.show_audio_health = open;
    if !open {
        app.audio_health_report = None;
    }
}

fn win(ctx: &egui::Context, open: &mut bool, title: &str, size: egui::Vec2, body: impl FnOnce(&mut egui::Ui)) {
    if !*open {
        return;
    }
    egui::Window::new(title).open(open).default_size(size).show(ctx, body);
}

fn automation(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_automation;
    win(ctx, &mut open, tr("Automation"), vec2(260.0, 260.0), |ui| {
        ui.label(tr("Write automation for the selected tracks:"));
        let tracks = app.engine.session().edit.selected_tracks.clone();
        ui.horizontal_wrapped(|ui| {
            for m in [
                AutomationMode::Off,
                AutomationMode::Read,
                AutomationMode::Touch,
                AutomationMode::Latch,
                AutomationMode::TouchLatch,
                AutomationMode::Write,
                AutomationMode::Trim,
            ] {
                if ui.button(tr(m.label())).clicked() {
                    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
                    let _ = app.run("mix.automation_mode", json!({"tracks": ids, "mode": m.label()}));
                }
            }
        });
        ui.separator();
        ui.label(tr("Enabled parameters"));
        for f in ["volume", "pan", "mute", "send level", "send pan", "send mute", "plugin"] {
            let id = format!("automation.enable.{}", f.replace(' ', "_"));
            let mut on = !app.engine.session().edit.flag(&format!("{id}.off"));
            if crate::widgets::checkbox(ui, &mut on, tr(f)).changed() {
                app.engine.session_mut().edit.set_flag(&format!("{id}.off"), !on);
            }
        }
        ui.separator();
        if ui.button(tr("Write to Current")).clicked() {
            let _ = app.run("automation.write_to_current", json!({}));
        }
        if ui.button(tr("Thin All")).clicked() {
            let _ = app.run("automation.thin_all", json!({}));
        }
    });
    app.ui.show_automation = open;
}

fn color_palette(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_color_palette;
    win(ctx, &mut open, tr("Color Palette"), vec2(300.0, 140.0), |ui| {
        ui.label(tr("Apply to selected tracks (Shift: selected clips)"));
        ui.horizontal_wrapped(|ui| {
            for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                ui.painter().rect_filled(r, 3.0, rgb(*c));
                if resp.clicked() {
                    if ui.input(|x| x.modifiers.shift) {
                        let _ = app.run("clip.color", json!({"color": c}));
                    } else {
                        let _ = app.run("track.color", json!({"color": i}));
                    }
                }
            }
        });
        ui.separator();
        ui.add(egui::Slider::new(&mut app.ui.track_tint, 0.0..=1.0).show_value(false).text(tr("Track tint in the Edit window")));
    });
    app.ui.show_color_palette = open;
}

fn disk_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_disk_usage;
    win(ctx, &mut open, tr("Disk Usage"), vec2(360.0, 160.0), |ui| {
        let s = app.engine.session();
        let bytes: u64 = s.sources.iter().map(|x| x.frames * u64::from(x.channels) * 4).sum();
        ui.label(trf("Session media: {} files, {} MB in memory (32-bit float)", &[&s.sources.len(), &format!("{:.1}", bytes as f64 / 1_048_576.0)]));
        let per_min = f64::from(s.sample_rate.hz()) * 60.0 * 3.0 / 1_048_576.0;
        ui.label(trf("Recording at 24-bit uses {} MB per mono track-minute", &[&format!("{per_min:.1}")]));
        if let Some(p) = &app.engine.path {
            ui.label(trf("Session file: {}", &[p]));
        }
    });
    app.ui.show_disk_usage = open;
}

fn system_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_system_usage;
    win(ctx, &mut open, tr("System Usage"), vec2(320.0, 140.0), |ui| {
        ui.label(trf("UI frame time: {} ms", &[&format!("{:.1}", app.frame_ms)]));
        ui.label(trf(
            "Audio: {}",
            &[&app.player.as_ref().map_or(tr("no engine").to_string(), |p| format!("{} @ {} Hz", p.device_name, p.device_rate))],
        ));
        let plugins: usize = app.engine.session().tracks.iter().map(|t| t.mixer.inserts.iter().flatten().count()).sum();
        ui.label(trf("Active plugin instances: {}", &[&plugins]));
        ui.label(trf("Tracks: {}", &[&app.engine.session().tracks.len()]));
        ui.label(trf("Undo steps: {}", &[&app.engine.undo_history().len()]));
    });
    app.ui.show_system_usage = open;
}

fn task_manager(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_task_manager;
    win(ctx, &mut open, tr("Task Manager"), vec2(320.0, 120.0), |ui| {
        match &app.bounce {
            Some(b) => {
                ui.label(trf("Bouncing {} ({}%)", &[&b.path().display(), &format!("{:.0}", b.progress() * 100.0)]));
                ui.add(egui::ProgressBar::new(b.progress()));
            }
            None => {
                ui.label(tr("No background tasks are running."));
            }
        }
        ui.label(
            egui::RichText::new(tr("Renders and AudioSuite processes run to completion before returning.")).small().color(Tokens::current().text_dim),
        );
    });
    app.ui.show_task_manager = open;
}

fn metadata(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_metadata;
    win(ctx, &mut open, tr("Metadata Inspector"), vec2(380.0, 260.0), |ui| {
        let s = app.engine.session();
        ui.label(egui::RichText::new(&s.name).strong());
        egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in [
                (tr("Sample rate"), format!("{} Hz", s.sample_rate.hz())),
                (tr("Bit depth"), format!("{:?}", s.bit_depth)),
                (tr("Timecode rate"), s.frame_rate.label().to_string()),
                (tr("Tracks"), s.tracks.len().to_string()),
                (tr("Clips"), s.tracks.iter().map(|t| t.clips().len()).sum::<usize>().to_string()),
                (tr("Audio files"), s.sources.len().to_string()),
                (tr("Markers"), s.markers.len().to_string()),
                (tr("Length"), s.format_position(s.content_end(), soundcraft_time::TimeFormat::MinSecs)),
            ] {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            }
        });
        ui.separator();
        let mut c = s.comments.clone();
        ui.label(tr("Session comments"));
        if ui.text_edit_multiline(&mut c).changed() {
            app.engine.session_mut().comments = c;
        }
    });
    app.ui.show_metadata = open;
}

fn event_list(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_event_list;
    win(ctx, &mut open, tr("MIDI Event List"), vec2(420.0, 360.0), |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            ui.label(tr("Select a MIDI clip."));
            return;
        };
        let notes = match app.engine.session().find_clip(cid).map(|(_, c)| c.content.clone()) {
            Some(ClipContent::Midi { sequence }) => sequence.notes,
            _ => Vec::new(),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("evl").num_columns(5).striped(true).show(ui, |ui| {
                for h in ["Start (ticks)", "Note", "Vel", "Length", ""] {
                    ui.label(egui::RichText::new(tr(h)).strong());
                }
                ui.end_row();
                for (i, n) in notes.iter().enumerate() {
                    ui.label(egui::RichText::new(n.start.to_string()).font(mono(11.0)));
                    ui.label(soundcraft_midi::note_name(n.pitch));
                    let mut v = i64::from(n.velocity);
                    if ui.add(egui::DragValue::new(&mut v).range(1..=127)).changed() {
                        let _ = app.engine.execute("midi.note_edit", &json!({"clip": cid.0, "index": i, "velocity": v}));
                    }
                    ui.label(n.length.to_string());
                    if ui.small_button("✕").clicked() {
                        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": [i]}));
                    }
                    ui.end_row();
                }
            });
        });
    });
    app.ui.show_event_list = open;
}

fn midi_keyboard(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_keyboard;
    win(ctx, &mut open, tr("MIDI Keyboard"), vec2(640.0, 110.0), |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(400.0), 80.0), Sense::click());
        let whites = 28;
        let w = r.width() / whites as f32;
        let mut wi = 0;
        let base = 48u8;
        let mut clicked: Option<u8> = None;
        let pos = resp.interact_pointer_pos();
        for p in base..base + 48 {
            if !matches!(p % 12, 1 | 3 | 6 | 8 | 10) {
                let kr = egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w, r.min.y), vec2(w - 1.0, r.height()));
                ui.painter().rect_filled(kr, 2.0, Color32::from_rgb(235, 235, 235));
                if resp.clicked() && pos.is_some_and(|q| kr.contains(q)) {
                    clicked = Some(p);
                }
                wi += 1;
            }
        }
        wi = 0;
        for p in base..base + 48 {
            if matches!(p % 12, 1 | 3 | 6 | 8 | 10) {
                let kr = egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w - w * 0.3, r.min.y), vec2(w * 0.6, r.height() * 0.6));
                ui.painter().rect_filled(kr, 2.0, Color32::from_rgb(20, 20, 20));
                if resp.clicked() && pos.is_some_and(|q| kr.contains(q)) {
                    clicked = Some(p);
                }
            } else {
                wi += 1;
            }
        }
        if let Some(p) = clicked
            && let Some(cid) = crate::midi_editor::target_clip(app)
        {
            // Step input: add the note at the insertion point (relative to the clip) and advance.
            let s = app.engine.session();
            let (clip_start, at) = (s.find_clip(cid).map_or(0, |(_, c)| c.start), s.edit.selection.start);
            let rel = s.tempo.samples_to_ticks(at, s.sample_rate) - s.tempo.samples_to_ticks(clip_start, s.sample_rate);
            let len = soundcraft_time::TICKS_PER_QUARTER / 2;
            let _ = app.run("midi.note_add", json!({"clip": cid.0, "pitch": p, "start_ticks": rel.max(0), "length_ticks": len}));
            let s = app.engine.session();
            let next = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(at, s.sample_rate) + len, s.sample_rate);
            let _ = app.run("transport.locate", json!({"at": next}));
        }
        ui.label(egui::RichText::new(tr("Click keys to step-enter notes into the selected MIDI clip at the insertion point.")).small());
    });
    app.ui.show_midi_keyboard = open;
}

fn workspace(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_workspace;
    if !open {
        return;
    }
    egui::Window::new(tr("Workspace")).open(&mut open).default_size(vec2(460.0, 420.0)).show(ctx, |ui| {
        let dir = if app.ui.workspace_dir.is_empty() { std::env::var("HOME").unwrap_or_else(|_| ".".into()) } else { app.ui.workspace_dir.clone() };
        ui.horizontal(|ui| {
            if ui.button("⬆").on_hover_text(tr("Parent folder")).clicked()
                && let Some(p) = std::path::Path::new(&dir).parent()
            {
                app.ui.workspace_dir = p.to_string_lossy().into_owned();
            }
            ui.label(egui::RichText::new(&dir).small());
        });
        ui.separator();
        let mut entries: Vec<(String, bool)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            return None;
                        }
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        let ext = std::path::Path::new(&name).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                        (is_dir || ["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "m4b", "caf", "mid", "midi", "scraft"].contains(&ext.as_str()))
                            .then_some((name, is_dir))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (name, is_dir) in entries.into_iter().take(2000) {
                let path = std::path::Path::new(&dir).join(&name).to_string_lossy().into_owned();
                let label = if is_dir { format!("📁 {name}") } else { format!("🔊 {name}") };
                let r = ui.selectable_label(false, label);
                if r.double_clicked() {
                    if is_dir {
                        app.ui.workspace_dir = path;
                    } else if name.ends_with(".scraft") {
                        let _ = crate::dialogs::run_or_ask(app, "session.open", json!({"path": path}));
                    } else if name.ends_with(".mid") || name.ends_with(".midi") {
                        let _ = app.run("file.import_midi", json!({"path": path}));
                    } else {
                        let at = app.engine.session().edit.selection.start;
                        let _ = app.run("file.import_audio", json!({"path": path, "at": at}));
                    }
                }
            }
        });
        ui.label(egui::RichText::new(tr("Double-click to import audio/MIDI at the insertion point or open a session.")).small());
    });
    app.ui.show_workspace = open;
}

fn configurations(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_configurations;
    win(ctx, &mut open, tr("Window Configurations"), vec2(320.0, 240.0), |ui| {
        if ui.button(tr("New Configuration…")).clicked() {
            let n = app.ui.configurations.len() + 1;
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            if let Ok(v) = serde_json::to_value(&snap) {
                app.ui.configurations.push((trf("Configuration {}", &[&n]), v));
            }
        }
        ui.separator();
        let list = app.ui.configurations.clone();
        for (i, (name, v)) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button(format!("{}  {name}", i + 1)).clicked()
                    && let Ok(mut st) = serde_json::from_value::<crate::UiState>(v.clone())
                {
                    if let Err(error) = st.docking.validate().and_then(|()| st.mix_docking.validate_mix()) {
                        app.ui.status = trf("Window configuration could not be restored: {}", &[&error]);
                    } else {
                        st.configurations = app.ui.configurations.clone();
                        st.theme = app.ui.theme;
                        st.language = app.ui.language;
                        st.show_configurations = true;
                        crate::docking::cancel_replaced_state(app, &st);
                        app.ui = st;
                    }
                }
                if ui.small_button("✕").clicked() {
                    app.ui.configurations.remove(i);
                }
            });
        }
    });
    app.ui.show_configurations = open;
}

/// The audio systems as last enumerated (enumerating probes devices: done when the window opens
/// and on Refresh, not every frame).
fn audio_systems(ctx: &egui::Context, refresh: bool) -> Arc<Vec<AudioSystem>> {
    let id = egui::Id::new("audio_systems");
    if !refresh && let Some(s) = ctx.data(|d| d.get_temp::<Arc<Vec<AudioSystem>>>(id)) {
        return s;
    }
    let s = Arc::new(soundcraft_playback::devices::audio_systems());
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&s)));
    s
}

/// The system a config means (its own, else the default one) in an enumeration.
fn chosen_system<'a>(systems: &'a [AudioSystem], cfg: &AudioConfig) -> Option<&'a AudioSystem> {
    cfg.system
        .as_deref()
        .and_then(|n| systems.iter().find(|s| s.name.eq_ignore_ascii_case(n)))
        .or_else(|| systems.iter().find(|s| s.is_default))
        .or(systems.first())
}

/// The device a config means (by name, else the system default).
fn chosen_device<'a>(devices: &'a [DeviceInfo], name: Option<&str>) -> Option<&'a DeviceInfo> {
    name.and_then(|n| devices.iter().find(|d| d.name == n)).or_else(|| devices.iter().find(|d| d.is_default))
}

fn device_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<String>, devices: &[DeviceInfo]) {
    let default = devices.iter().find(|d| d.is_default).map_or(tr("none").to_string(), |d| d.name.clone());
    let shown = value.clone().unwrap_or_else(|| trf("Default ({})", &[&default]));
    egui::ComboBox::from_id_salt(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        ui.selectable_value(value, None, trf("Default ({})", &[&default]));
        for d in devices {
            ui.selectable_value(value, Some(d.name.clone()), format!("{} ({} ch)", d.name, d.max_channels));
        }
    });
}

fn number_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<u32>, options: &[u32], unit: &str) {
    let shown = value.map_or(tr("Device default").to_string(), |v| format!("{v} {unit}"));
    egui::ComboBox::from_id_salt(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        ui.selectable_value(value, None, tr("Device default"));
        for v in options {
            ui.selectable_value(value, Some(*v), format!("{v} {unit}"));
        }
    });
}

/// Setup › Playback Engine / Hardware: choose the audio system, devices, rate and buffer size.
pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_playback_engine;
    if !open {
        return;
    }
    let draft_id = egui::Id::new("audio_draft");
    let mut draft = ctx.data(|d| d.get_temp::<AudioConfig>(draft_id)).unwrap_or_else(|| app.ui.audio.clone());
    let mut refresh = false;
    let mut apply = false;
    let systems = audio_systems(ctx, false);
    win(ctx, &mut open, tr("Playback Engine"), vec2(460.0, 360.0), |ui| {
        let t = Tokens::current();
        egui::Grid::new("pe").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            ui.label(tr("Audio system"));
            let def = systems.iter().find(|s| s.is_default).map_or(tr("none").to_string(), |s| s.name.clone());
            let shown = draft.system.clone().unwrap_or_else(|| trf("Default ({})", &[&def]));
            egui::ComboBox::from_id_salt("pe-system").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                let before = draft.system.clone();
                ui.selectable_value(&mut draft.system, None, trf("Default ({})", &[&def]));
                for s in systems.iter() {
                    ui.selectable_value(&mut draft.system, Some(s.name.clone()), &s.name);
                }
                if draft.system != before {
                    // Device names belong to a system.
                    draft.output = None;
                    draft.input = None;
                }
            });
            ui.end_row();
            let sys = chosen_system(&systems, &draft);
            ui.label(tr("Output device"));
            device_combo(ui, "pe-out", &mut draft.output, sys.map_or(&[][..], |s| &s.outputs));
            ui.end_row();
            ui.label(tr("Input device"));
            device_combo(ui, "pe-in", &mut draft.input, sys.map_or(&[][..], |s| &s.inputs));
            ui.end_row();
            let out = sys.and_then(|s| chosen_device(&s.outputs, draft.output.as_deref()));
            ui.label(tr("Sample rate"));
            number_combo(ui, "pe-rate", &mut draft.sample_rate, out.map_or(&[][..], |d| &d.sample_rates), "Hz");
            ui.end_row();
            ui.label(tr("Buffer size"));
            let buffers = out.map(DeviceInfo::buffer_sizes).unwrap_or_else(|| soundcraft_playback::devices::STANDARD_BUFFERS.to_vec());
            number_combo(ui, "pe-buffer", &mut draft.buffer_size, &buffers, tr("samples"));
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr("Refresh devices")).clicked() {
                refresh = true;
            }
            let changed = draft != app.ui.audio;
            if ui.add_enabled(changed, egui::Button::new(tr("Apply"))).on_hover_text(tr("Reopen the audio devices with these settings")).clicked() {
                apply = true;
            }
            if changed && ui.button(tr("Revert")).clicked() {
                draft = app.ui.audio.clone();
            }
        });
        ui.separator();
        egui::Grid::new("pe-state").num_columns(2).show(ui, |ui| {
            ui.label(tr("Output"));
            ui.label(app.player.as_ref().map_or(tr("none").to_string(), |p| {
                if p.silent {
                    tr("no device (silent clock)").to_string()
                } else {
                    let buf = p.device_buffer.map_or(tr("default buffer").to_string(), |b| trf("{} samples", &[&b]));
                    format!("{} · {} Hz · {} ch · {buf}", p.device_name, p.device_rate, p.device_channels)
                }
            }));
            ui.end_row();
            ui.label(tr("Input"));
            ui.label(app.recorder.as_ref().map_or(tr("opened on first record or monitor").to_string(), |r| {
                format!("{} · {} Hz · {} ch", r.device_name, r.sample_rate, r.channels)
            }));
            ui.end_row();
            ui.label(tr("Session rate"));
            let sr = app.engine.session().sample_rate.hz();
            let note = app
                .player
                .as_ref()
                .filter(|p| !p.silent && p.device_rate != sr)
                .map_or(String::new(), |_| tr(" (resampled to the device rate)").to_string());
            ui.label(format!("{sr} Hz{note}"));
            ui.end_row();
        });
        ui.label(
            egui::RichText::new(tr("The mix runs in 512-sample blocks; the buffer size sets the device's own latency.")).small().color(t.text_dim),
        );
        let mut dc = app.engine.session().edit.delay_compensation;
        if crate::widgets::checkbox(ui, &mut dc, tr("Delay compensation")).changed() {
            let _ = app.run("options.delay_compensation", json!({"value": dc}));
        }
        if ui.button(tr("Reconnect audio device")).clicked() {
            app.reconnect_audio();
        }
    });
    if refresh {
        audio_systems(ctx, true);
    }
    if apply && let Err(e) = app.apply_audio_config(draft.clone()) {
        app.ui.status = e;
    }
    if open {
        ctx.data_mut(|d| d.insert_temp(draft_id, draft));
    } else {
        // Closing discards unapplied edits and the device list (re-read on the next open).
        ctx.data_mut(|d| {
            d.remove_temp::<AudioConfig>(draft_id);
            d.remove_temp::<Arc<Vec<AudioSystem>>>(egui::Id::new("audio_systems"));
        });
    }
    app.ui.show_playback_engine = open;
}

/// `ui.audio_device`: `{list?: bool}` returns the audio systems and their devices;
/// `{system?, output?, input?, sample_rate?, buffer_size?}` (a `null` value means "default") sets
/// the choice and reopens the devices. Always returns the choice and the open device.
pub fn audio_device_command(app: &mut SoundApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let mut changed = false;
    let mut cfg = app.ui.audio.clone();
    for (key, slot) in [("system", &mut cfg.system), ("output", &mut cfg.output), ("input", &mut cfg.input)] {
        if let Some(v) = p.get(key) {
            *slot = v.as_str().map(str::to_string).filter(|s| !s.is_empty());
            changed = true;
        }
    }
    for (key, slot, range) in [("sample_rate", &mut cfg.sample_rate, 8_000..=768_000u64), ("buffer_size", &mut cfg.buffer_size, 16..=8192u64)] {
        if let Some(v) = p.get(key) {
            *slot = match v {
                Value::Null => None,
                v => Some(v.as_u64().filter(|n| range.contains(n)).ok_or_else(|| format!("`{key}` must be a number in {range:?} or null"))? as u32),
            };
            changed = true;
        }
    }
    if changed {
        app.apply_audio_config(cfg)?;
    }
    let device = app.player.as_ref().map(|pl| {
        json!({"name": pl.device_name, "sample_rate": pl.device_rate, "channels": pl.device_channels, "buffer_size": pl.device_buffer, "silent": pl.silent})
    });
    let mut out = json!({"config": app.ui.audio, "device": device});
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        out["systems"] = json!(soundcraft_playback::devices::audio_systems());
    }
    Ok(out)
}

/// Setup › I/O: busses and output paths.
pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_io_setup;
    win(ctx, &mut open, tr("I/O Setup"), vec2(420.0, 300.0), |ui| {
        ui.label(egui::RichText::new(tr("Busses")).strong());
        let busses: Vec<(String, String)> = app.engine.session().busses.iter().map(|b| (b.name.clone(), tr(b.format.label()).to_string())).collect();
        egui::Grid::new("busses").num_columns(3).striped(true).show(ui, |ui| {
            for (name, fmt) in busses {
                let key = egui::Id::new(("bus_name", name.clone()));
                let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.clone());
                let r = ui.text_edit_singleline(&mut edit);
                ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit.clone()));
                if r.lost_focus() && edit != name {
                    let _ = app.run("setup.io", json!({"action": "rename_bus", "name": name, "new_name": edit}));
                }
                ui.label(fmt);
                if ui.small_button(tr("Delete")).clicked() {
                    let _ = app.run("setup.io", json!({"action": "delete_bus", "name": name}));
                }
                ui.end_row();
            }
        });
        if ui.button(tr("New Bus")).clicked() {
            let n = app.engine.session().busses.len() + 1;
            let _ = app.run("mix.new_bus", json!({"name": format!("Bus {n}")}));
        }
        ui.separator();
        ui.label(egui::RichText::new(tr("Outputs")).strong());
        // Main output format: stereo, or a surround format (the mix and master faders follow).
        let main = app.engine.session().main_format();
        ui.horizontal(|ui| {
            ui.label(tr("Main output"));
            egui::ComboBox::from_id_salt("main_output_format").selected_text(tr(main.label())).show_ui(ui, |ui| {
                for f in soundcraft_model::ChannelFormat::ALL.into_iter().filter(|f| (2..=16).contains(&f.channels())) {
                    if ui.selectable_label(f == main, tr(f.label())).clicked() && f != main {
                        let _ = app.run("setup.main_format", json!({"format": f.label()}));
                    }
                }
            });
            ui.label(egui::RichText::new(format!("{} ch", main.channels())).small());
        });
        for o in app.engine.session().outputs.clone() {
            ui.label(trf("{} — {} from channel {}", &[&o.name, &tr(o.format.label()), &(o.first_channel + 1)]));
        }
    });
    app.ui.show_io_setup = open;
}

/// Setup › Keyboard Shortcuts.
pub fn shortcuts_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_shortcuts;
    let mac = ctx.os().is_mac();
    win(ctx, &mut open, tr("Keyboard Shortcuts"), vec2(520.0, 480.0), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sc").num_columns(3).striped(true).show(ui, |ui| {
                let engine = soundcraft_engine::command_specs()
                    .iter()
                    .map(|c| (c.shortcut, c.label, c.menu.iter().map(|m| tr(m)).collect::<Vec<_>>().join(" › ")));
                let windows = crate::menus::UI_COMMANDS.iter().map(|c| (c.3, c.1, c.2.split(" > ").map(tr).collect::<Vec<_>>().join(" › ")));
                for (shortcut, label, menu) in engine.chain(windows) {
                    let Some(shortcut) = shortcut else { continue };
                    ui.label(egui::RichText::new(crate::shortcuts::shortcut_label(shortcut, mac)).font(mono(11.0)));
                    ui.label(tr(label));
                    ui.label(egui::RichText::new(menu).small());
                    ui.end_row();
                }
            });
        });
    });
    app.ui.show_shortcuts = open;
}

/// Clip Effects: per-clip EQ, dynamics and gain (stored as `clip_fx.<clip>.*` values).
pub fn clip_effects(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_clip_effects;
    win(ctx, &mut open, tr("Clip Effects"), vec2(560.0, 420.0), |ui| {
        let clip = app.engine.session().edit.selected_clips.first().copied().or_else(|| {
            let s = app.engine.session();
            s.tracks
                .iter()
                .filter(|t| s.edit.selected_tracks.contains(&t.id))
                .flat_map(|t| t.clips().iter())
                .find(|c| c.range().contains(s.edit.selection.start) && c.is_audio())
                .map(|c| c.id)
        });
        let Some(cid) = clip else {
            ui.label(tr("Select an audio clip."));
            return;
        };
        let name = app.engine.session().find_clip(cid).map(|(_, c)| c.name.clone()).unwrap_or_default();
        let bypass = app.engine.session().edit.flag(&format!("clip_fx.bypass.{}", cid.0));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).strong());
            if ui.selectable_label(bypass, tr("Bypass")).clicked() {
                let _ = app.run("clip.effects_bypass", json!({"clips": [cid.0]}));
            }
            if ui.button(tr("Render")).clicked() {
                let _ = app.run("clip.effects_render", json!({"clips": [cid.0]}));
            }
            if ui.button(tr("Clear")).clicked() {
                let _ = app.run("edit.clear_clip_effects", json!({"clips": [cid.0]}));
            }
        });
        let get = |app: &SoundApp, k: &str, d: f32| app.engine.session().edit.values.get(&format!("clip_fx.{}.{k}", cid.0)).map_or(d, |v| *v as f32);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                for (col, (title, plugin, prefix)) in cols.iter_mut().zip([(tr("EQ"), "eq_7band", "eq."), (tr("Dynamics"), "compressor", "comp.")]) {
                    col.label(egui::RichText::new(title).strong());
                    let Some(info) = soundcraft_dsp::plugin_info(plugin) else { continue };
                    for p in info.params {
                        let key = format!("{prefix}{}", p.id);
                        let mut v = get(app, &key, p.default);
                        let before = v;
                        col.add(egui::Slider::new(&mut v, p.min..=p.max).text(tr(p.name)));
                        if (v - before).abs() > f32::EPSILON {
                            let mut m = serde_json::Map::new();
                            m.insert(key.clone(), json!(v));
                            let _ = app.engine.execute_merged(
                                "clip.effects_set",
                                &json!({"clips": [cid.0], "params": m}),
                                &format!("cfx:{}:{key}", cid.0),
                            );
                        }
                    }
                }
            });
            let mut g = get(app, "gain", 0.0);
            let before = g;
            ui.add(egui::Slider::new(&mut g, -36.0..=24.0).text(tr("Clip effects gain (dB)")));
            if (g - before).abs() > f32::EPSILON {
                let _ =
                    app.engine.execute_merged("clip.effects_set", &json!({"clips": [cid.0], "params": {"gain": g}}), &format!("cfx:{}:gain", cid.0));
            }
        });
    });
    app.ui.show_clip_effects = open;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod audio_health_tests {
    use super::*;

    #[test]
    fn window_command_renders_report_and_close_clears_snapshot() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(app.audio_health_report.as_ref().unwrap().as_ref().unwrap()["healthy"], true);
        assert!(!app.engine.is_dirty());
        app.run("window.hide_floating", json!({})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(!app.ui.show_audio_health);
        assert!(app.audio_health_report.is_none());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.audio_health_report.is_some());
    }
}
