//! Secondary floating windows (Window menu).

use crate::SoundApp;
use crate::theme::{Tokens, mono, rgb};
use egui::{Color32, Sense, vec2};
use serde_json::json;
use soundcraft_model::{AutomationMode, ClipContent};

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
    win(ctx, &mut open, "Session Audio Health", vec2(620.0, 360.0), |ui| {
        let refresh = ui.button("Refresh Report").clicked();
        if app.audio_health_report.is_none() || refresh {
            app.audio_health_report = Some(app.engine.execute("session.audio_health", &json!({})).map_err(|error| error.to_string()));
        }
        let Some(result) = &app.audio_health_report else { return };
        match result {
            Ok(report) => {
                ui.label(format!("{} audio clips checked · {} issues", report["audio_clips"], report["issue_count"]));
                ui.label(match report["active_audio_end_seconds"].as_f64() {
                    Some(seconds) => format!("Active audio ends at {seconds:.2} seconds"),
                    None => "Active audio end time is unknown (invalid session sample rate)".into(),
                });
                ui.label("Includes alternate takes and muted clips. Refresh after editing.");
                ui.separator();
                if report["healthy"] == true {
                    ui.label("All audio clips have loaded media within source bounds.");
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
                                    if issue["active"] == true { "" } else { " (alternate playlist)" }
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
                ui.label("Checks loaded media only, not files on disk, signal levels, plugins or routing.");
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
    win(ctx, &mut open, "Automation", vec2(260.0, 260.0), |ui| {
        ui.label("Write automation for the selected tracks:");
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
                if ui.button(m.label()).clicked() {
                    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
                    let _ = app.run("mix.automation_mode", json!({"tracks": ids, "mode": m.label()}));
                }
            }
        });
        ui.separator();
        ui.label("Enabled parameters");
        for f in ["volume", "pan", "mute", "send level", "send pan", "send mute", "plugin"] {
            let id = format!("automation.enable.{}", f.replace(' ', "_"));
            let mut on = !app.engine.session().edit.flag(&format!("{id}.off"));
            if crate::widgets::checkbox(ui, &mut on, f).changed() {
                app.engine.session_mut().edit.set_flag(&format!("{id}.off"), !on);
            }
        }
        ui.separator();
        if ui.button("Write to Current").clicked() {
            let _ = app.run("automation.write_to_current", json!({}));
        }
        if ui.button("Thin All").clicked() {
            let _ = app.run("automation.thin_all", json!({}));
        }
    });
    app.ui.show_automation = open;
}

fn color_palette(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_color_palette;
    win(ctx, &mut open, "Color Palette", vec2(300.0, 140.0), |ui| {
        ui.label("Apply to selected tracks (Shift: selected clips)");
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
        ui.add(egui::Slider::new(&mut app.ui.track_tint, 0.0..=1.0).show_value(false).text("Track tint in the Edit window"));
    });
    app.ui.show_color_palette = open;
}

fn disk_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_disk_usage;
    win(ctx, &mut open, "Disk Usage", vec2(360.0, 160.0), |ui| {
        let s = app.engine.session();
        let bytes: u64 = s.sources.iter().map(|x| x.frames * u64::from(x.channels) * 4).sum();
        ui.label(format!("Session media: {} files, {:.1} MB in memory (32-bit float)", s.sources.len(), bytes as f64 / 1_048_576.0));
        let per_min = f64::from(s.sample_rate.hz()) * 60.0 * 3.0 / 1_048_576.0;
        ui.label(format!("Recording at 24-bit uses {per_min:.1} MB per mono track-minute"));
        if let Some(p) = &app.engine.path {
            ui.label(format!("Session file: {p}"));
        }
    });
    app.ui.show_disk_usage = open;
}

fn system_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_system_usage;
    win(ctx, &mut open, "System Usage", vec2(320.0, 140.0), |ui| {
        ui.label(format!("UI frame time: {:.1} ms", app.frame_ms));
        ui.label(format!("Audio: {}", app.player.as_ref().map_or("no engine".to_string(), |p| format!("{} @ {} Hz", p.device_name, p.device_rate))));
        let plugins: usize = app.engine.session().tracks.iter().map(|t| t.mixer.inserts.iter().flatten().count()).sum();
        ui.label(format!("Active plugin instances: {plugins}"));
        ui.label(format!("Tracks: {}", app.engine.session().tracks.len()));
        ui.label(format!("Undo steps: {}", app.engine.undo_history().len()));
    });
    app.ui.show_system_usage = open;
}

fn task_manager(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_task_manager;
    win(ctx, &mut open, "Task Manager", vec2(320.0, 120.0), |ui| {
        match &app.bounce {
            Some(b) => {
                ui.label(format!("Bouncing {} ({:.0}%)", b.path().display(), b.progress() * 100.0));
                ui.add(egui::ProgressBar::new(b.progress()));
            }
            None => {
                ui.label("No background tasks are running.");
            }
        }
        ui.label(
            egui::RichText::new("Renders and AudioSuite processes run to completion before returning.").small().color(Tokens::current().text_dim),
        );
    });
    app.ui.show_task_manager = open;
}

fn metadata(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_metadata;
    win(ctx, &mut open, "Metadata Inspector", vec2(380.0, 260.0), |ui| {
        let s = app.engine.session();
        ui.label(egui::RichText::new(&s.name).strong());
        egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in [
                ("Sample rate", format!("{} Hz", s.sample_rate.hz())),
                ("Bit depth", format!("{:?}", s.bit_depth)),
                ("Timecode rate", s.frame_rate.label().to_string()),
                ("Tracks", s.tracks.len().to_string()),
                ("Clips", s.tracks.iter().map(|t| t.clips().len()).sum::<usize>().to_string()),
                ("Audio files", s.sources.len().to_string()),
                ("Markers", s.markers.len().to_string()),
                ("Length", s.format_position(s.content_end(), soundcraft_time::TimeFormat::MinSecs)),
            ] {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            }
        });
        ui.separator();
        let mut c = s.comments.clone();
        ui.label("Session comments");
        if ui.text_edit_multiline(&mut c).changed() {
            app.engine.session_mut().comments = c;
        }
    });
    app.ui.show_metadata = open;
}

fn event_list(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_event_list;
    win(ctx, &mut open, "MIDI Event List", vec2(420.0, 360.0), |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            ui.label("Select a MIDI clip.");
            return;
        };
        let notes = match app.engine.session().find_clip(cid).map(|(_, c)| c.content.clone()) {
            Some(ClipContent::Midi { sequence }) => sequence.notes,
            _ => Vec::new(),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("evl").num_columns(5).striped(true).show(ui, |ui| {
                for h in ["Start (ticks)", "Note", "Vel", "Length", ""] {
                    ui.label(egui::RichText::new(h).strong());
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
    win(ctx, &mut open, "MIDI Keyboard", vec2(640.0, 110.0), |ui| {
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
        ui.label(egui::RichText::new("Click keys to step-enter notes into the selected MIDI clip at the insertion point.").small());
    });
    app.ui.show_midi_keyboard = open;
}

fn workspace(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_workspace;
    if !open {
        return;
    }
    egui::Window::new("Workspace").open(&mut open).default_size(vec2(460.0, 420.0)).show(ctx, |ui| {
        let dir = if app.ui.workspace_dir.is_empty() { std::env::var("HOME").unwrap_or_else(|_| ".".into()) } else { app.ui.workspace_dir.clone() };
        ui.horizontal(|ui| {
            if ui.button("⬆").on_hover_text("Parent folder").clicked()
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
        ui.label(egui::RichText::new("Double-click to import audio/MIDI at the insertion point or open a session.").small());
    });
    app.ui.show_workspace = open;
}

fn configurations(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_configurations;
    win(ctx, &mut open, "Window Configurations", vec2(320.0, 240.0), |ui| {
        if ui.button("New Configuration…").clicked() {
            let n = app.ui.configurations.len() + 1;
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            if let Ok(v) = serde_json::to_value(&snap) {
                app.ui.configurations.push((format!("Configuration {n}"), v));
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
                        app.ui.status = format!("Window configuration could not be restored: {error}");
                    } else {
                        st.configurations = app.ui.configurations.clone();
                        st.theme = app.ui.theme;
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

/// What a MIDI message is, for the "last received" readout.
fn midi_event_text(e: &soundcraft_mix::LiveMidi) -> String {
    let ch = e.channel() + 1;
    match e.status & 0xF0 {
        0x90 if e.data2 > 0 => format!("note {} on, velocity {}, channel {ch}", soundcraft_midi::note_name(e.data1), e.data2),
        0x80 | 0x90 => format!("note {} off, channel {ch}", soundcraft_midi::note_name(e.data1)),
        0xA0 => format!("aftertouch {} on note {}, channel {ch}", e.data2, soundcraft_midi::note_name(e.data1)),
        0xB0 => format!("controller {} = {}, channel {ch}", e.data1, e.data2),
        0xC0 => format!("program {}, channel {ch}", u16::from(e.data1) + 1),
        0xD0 => format!("aftertouch {}, channel {ch}", e.data1),
        0xE0 => format!("pitch bend {}, channel {ch}", (i32::from(e.data2) << 7 | i32::from(e.data1)) - 8192),
        _ => format!("message {:#04x}", e.status),
    }
}

/// The MIDI tab of Setup › Playback Engine: every input port with a switch, and on request what
/// arrives from it, to see that a keyboard gets through.
fn midi_inputs(app: &mut SoundApp, ui: &mut egui::Ui) {
    ui.strong("MIDI inputs");
    let Some(m) = &app.midi_inputs else {
        ui.weak("No audio engine, so MIDI devices are not opened.");
        return;
    };
    if let Some(e) = m.error() {
        ui.colored_label(ui.visuals().warn_fg_color, e);
        return;
    }
    let ports = m.ports().to_vec();
    if ports.is_empty() {
        ui.weak("No MIDI input device found. Plug one in: it shows up here by itself.");
    }
    // Per port name: whether its debug readout is shown, and the last message seen from it.
    let debug_id = egui::Id::new("playback-engine-midi-debug");
    let mut debug: std::collections::BTreeMap<String, (bool, Option<String>)> = ui.data(|d| d.get_temp(debug_id)).unwrap_or_default();
    let (count, last) = m.feed().activity();
    if let Some(e) = last
        && let Some(from) = m.feed().ports().name(e.port)
    {
        debug.entry(from.to_string()).or_default().1 = Some(format!("{} (message {count} of all devices)", midi_event_text(&e)));
    }
    let mut toggle = None;
    egui::Grid::new("pe-midi").num_columns(3).show(ui, |ui| {
        for p in &ports {
            let mut on = p.enabled;
            if crate::widgets::checkbox(ui, &mut on, &p.name).on_hover_text("Play and record from this device").changed() {
                toggle = Some((p.name.clone(), on));
            }
            match (p.enabled, p.open) {
                (false, _) => ui.weak("off"),
                (true, true) => ui.label("connected"),
                (true, false) => ui.colored_label(ui.visuals().warn_fg_color, "could not be opened"),
            };
            let shown = &mut debug.entry(p.name.clone()).or_default().0;
            if ui.selectable_label(*shown, "Test").on_hover_text("Show what arrives from this device").clicked() {
                *shown = !*shown;
            }
            ui.end_row();
        }
    });
    if let Some((name, on)) = toggle {
        app.set_midi_input_enabled(&name, on);
    }
    for p in &ports {
        let Some((true, seen)) = debug.get(&p.name) else { continue };
        ui.separator();
        ui.label(&p.name);
        ui.weak(format!("Enabled: {}, open: {}", if p.enabled { "yes" } else { "no" }, if p.open { "yes" } else { "no" }));
        ui.weak(match seen {
            Some(text) => format!("Last received: {text}"),
            None => "Last received: nothing yet".to_string(),
        });
        // Keep the readout live while it is shown.
        ui.ctx().request_repaint_after_secs(0.1);
    }
    ui.data_mut(|d| d.insert_temp(debug_id, debug));
}

/// Setup › Playback Engine / Hardware.
pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {
    use soundcraft_playback::host::{self, DeviceInfo};
    let mut open = app.ui.show_playback_engine;
    // Input channels (one bit each) listened to while the Audio tab shows them.
    let listen_id = egui::Id::new("playback-engine-listen");
    let mut listen = 0u64;
    win(ctx, &mut open, "Playback Engine", vec2(460.0, 420.0), |ui| {
        // Audio devices on one tab, MIDI devices on the other; the buttons below serve both.
        let mut tab = app.ui.playback_engine_tab.min(1);
        ui.horizontal(|ui| {
            for (i, l) in (0u8..).zip(["Audio", "MIDI"]) {
                if ui.selectable_label(tab == i, l).clicked() {
                    tab = i;
                }
            }
        });
        app.ui.playback_engine_tab = tab;
        ui.separator();
        // Probing every device takes a moment (ALSA opens each one): listed once, then on request.
        let cache = egui::Id::new("playback-engine-devices");
        let devices: std::sync::Arc<(Vec<DeviceInfo>, Vec<DeviceInfo>, Vec<u32>)> = match ui.data(|d| d.get_temp(cache)) {
            Some(d) => d,
            None => {
                let rates = soundcraft_playback::devices::describe_chosen(true).map(|d| d.sample_rates).unwrap_or_default();
                let d = std::sync::Arc::new((host::output_devices(), host::input_devices(), rates));
                ui.data_mut(|x| x.insert_temp(cache, std::sync::Arc::clone(&d)));
                d
            }
        };
        let mut rescan = false;
        let top = ui.cursor().top();
        if tab == 0 {
            // "Default" (naming the device it is now, when one is open) plus the host's devices; a
            // remembered device that is gone stays visible. `open` is the device in use and what it
            // runs at, for the tooltip.
            let device_combo = |ui: &mut egui::Ui,
                                salt: &str,
                                list: &[DeviceInfo],
                                current: &Option<String>,
                                open: Option<(&str, String)>|
             -> Option<Option<String>> {
                let mut choice = current.clone();
                let default = match (current, &open) {
                    (None, Some((name, _))) => format!("Default ({name})"),
                    _ => "Default".to_string(),
                };
                let label = match current {
                    None => default.clone(),
                    Some(id) => list.iter().find(|d| &d.id == id).map_or_else(|| format!("{id} (unavailable)"), |d| d.name.clone()),
                };
                let combo = egui::ComboBox::from_id_salt(salt).selected_text(label).width(260.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, default).on_hover_text("Whatever the system uses, following it when it changes");
                    for d in list {
                        // The name, then what tells it apart from its neighbours, dimmer.
                        let mut row = egui::text::LayoutJob::default();
                        let font = egui::TextStyle::Button.resolve(ui.style());
                        row.append(&d.name, 0.0, egui::TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() });
                        if !d.detail.is_empty() {
                            let small = egui::FontId::new(font.size * 0.85, font.family.clone());
                            row.append(
                                &d.detail,
                                10.0,
                                egui::TextFormat { font_id: small, color: ui.visuals().weak_text_color(), ..Default::default() },
                            );
                        }
                        ui.selectable_value(&mut choice, Some(d.id.clone()), row).on_hover_text(&d.id);
                    }
                });
                if let Some((_, runs)) = open {
                    combo.response.on_hover_text(runs);
                }
                (&choice != current).then_some(choice)
            };
            egui::Grid::new("pe").num_columns(2).show(ui, |ui| {
                let hosts = host::available_hosts();
                let missing = host::unavailable_hosts();
                ui.label("Audio host");
                let mut choice = app.ui.audio.system.clone();
                // Which host "Default" turned out to be, once the output is open on it.
                let default = match app.player.as_ref().filter(|p| choice.is_none() && !p.host_name.is_empty()) {
                    Some(p) => format!("Default ({})", p.host_name),
                    None => "Default".to_string(),
                };
                let label = match &choice {
                    None => default.clone(),
                    Some(id) => hosts.iter().find(|h| &h.id == id).map_or_else(|| format!("{id} (unavailable)"), |h| h.name.clone()),
                };
                egui::ComboBox::from_id_salt("audio_host").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, default).on_hover_text("The first host that has a device");
                    for h in &hosts {
                        ui.selectable_value(&mut choice, Some(h.id.clone()), &h.name);
                    }
                    // Hosts that exist but cannot be chosen: shown with the reason, not left out.
                    for (name, why) in &missing {
                        ui.add_enabled(false, egui::Button::selectable(false, format!("{name}  ({why})")));
                    }
                });
                if choice != app.ui.audio.system {
                    app.set_audio_host(choice);
                    rescan = true;
                }
                ui.end_row();
                // The chosen host did not open (JACK without a running server, …): say what is used.
                let chosen = app.ui.audio.system.as_ref().and_then(|id| hosts.iter().find(|h| &h.id == id)).map(|h| h.name.clone());
                if let (Some(chosen), Some(p)) = (chosen, app.player.as_ref())
                    && p.host_name != chosen
                {
                    ui.label("");
                    let used = if p.host_name.is_empty() { "no device".to_string() } else { format!("using {}", p.host_name) };
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{chosen} could not be opened (is its server running?); {used}"));
                    ui.end_row();
                }
                ui.label("Output device");
                let out_open = app
                    .player
                    .as_ref()
                    .filter(|p| !p.silent)
                    .map(|p| (p.device_name.clone(), format!("{} ch @ {} Hz", p.device_channels, p.device_rate)));
                if let Some(choice) =
                    device_combo(ui, "audio_output_device", &devices.0, &app.ui.audio.output, out_open.as_ref().map(|(n, r)| (n.as_str(), r.clone())))
                {
                    app.set_output_device(choice);
                    // Its rates are another device's now.
                    rescan = true;
                }
                ui.end_row();
                ui.label("Input device");
                let in_open = app.recorder.as_ref().map(|r| (r.device_name.clone(), format!("{} ch @ {} Hz", r.channels, r.sample_rate)));
                if let Some(choice) =
                    device_combo(ui, "audio_input_device", &devices.1, &app.ui.audio.input, in_open.as_ref().map(|(n, r)| (n.as_str(), r.clone())))
                {
                    app.set_input_device(choice);
                }
                ui.end_row();
                if app.recorder.is_none() && app.recorder_failed {
                    ui.label("");
                    ui.colored_label(ui.visuals().warn_fg_color, "The input device could not be opened");
                    ui.end_row();
                }
                // A level per input channel: which socket of the interface is which input, at a glance.
                // "Test" plays them, to try them out without a track.
                if let Some(r) = &app.recorder {
                    listen = ui.data(|d| d.get_temp(listen_id)).unwrap_or(0);
                    ui.label("Inputs");
                    let peaks = r.monitor.take_peaks();
                    let held = egui::Id::new("playback-engine-input-levels");
                    let mut levels: Vec<f32> = ui.data(|d| d.get_temp(held)).unwrap_or_default();
                    levels.resize(peaks.len(), 0.0);
                    ui.horizontal_wrapped(|ui| {
                        for (i, (level, peak)) in levels.iter_mut().zip(peaks.iter()).enumerate() {
                            // Fall back slowly, jump up at once.
                            *level = (*level * 0.8).max(*peak);
                            let db = 20.0 * level.max(1e-6).log10();
                            ui.label(format!("Input {}", i + 1));
                            ui.add(egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0)).desired_width(70.0).desired_height(8.0))
                                .on_hover_text(format!("{db:.0} dB"));
                        }
                        // All of them at once: the levels tell which is which.
                        if ui.selectable_label(listen != 0, "Test").on_hover_text("Hear the inputs through the output").clicked() {
                            listen = if listen == 0 { u64::MAX } else { 0 };
                        }
                    });
                    ui.data_mut(|d| d.insert_temp(held, levels));
                    ui.data_mut(|d| d.insert_temp(listen_id, listen));
                    ui.ctx().request_repaint_after_secs(0.05);
                    ui.end_row();
                }
                ui.label("Sample rate");
                let mut choice = app.ui.audio.sample_rate;
                let session = app.engine.session().sample_rate.hz();
                // What the output runs at: a device may not offer the rate asked for.
                let running = app.player.as_ref().filter(|p| !p.silent && p.device_rate > 0).map(|p| p.device_rate);
                let default = match running.filter(|_| choice.is_none()) {
                    Some(r) => format!("Default ({r} Hz)"),
                    None => "Default".to_string(),
                };
                let label = choice.map_or_else(|| default.clone(), |r| format!("{r} Hz"));
                let combo = egui::ComboBox::from_id_salt("audio_sample_rate").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, default).on_hover_text("The session's rate when the device offers it");
                    for r in &devices.2 {
                        ui.selectable_value(&mut choice, Some(*r), format!("{r} Hz"));
                    }
                });
                if let Some(r) = running.filter(|r| *r != session) {
                    combo.response.on_hover_text(format!("The session is {session} Hz: the mix is resampled to the device's {r} Hz"));
                }
                if choice != app.ui.audio.sample_rate {
                    app.set_sample_rate(choice);
                }
                ui.end_row();
                ui.label("Session rate");
                ui.label(format!("{} Hz", app.engine.session().sample_rate.hz()));
                ui.end_row();
                ui.label("Buffer size");
                let mut choice = app.ui.audio.buffer_size;
                // The delay of a buffer, at the rate the output runs at (the session's until it is open).
                let rate =
                    app.player.as_ref().filter(|p| !p.silent && p.device_rate > 0).map_or(app.engine.session().sample_rate.hz(), |p| p.device_rate);
                let ms = |frames: u32| f64::from(frames) * 1000.0 / f64::from(rate.max(1));
                // What the output really runs at: a host may give another size than asked for.
                let running = app.player.as_ref().filter(|p| !p.silent && p.buffer_frames() > 0).map(|p| p.buffer_frames());
                let default = match running.filter(|_| choice.is_none()) {
                    Some(f) => format!("Default ({f} frames, {:.1} ms)", ms(f)),
                    None => "Default".to_string(),
                };
                let label = choice.map_or_else(|| default.clone(), |n| format!("{n} frames ({:.1} ms)", ms(n)));
                let combo = egui::ComboBox::from_id_salt("audio_buffer_size").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut choice, None, default).on_hover_text("Whatever the audio host runs at");
                    for n in host::BUFFER_SIZES {
                        ui.selectable_value(&mut choice, Some(n), format!("{n} frames ({:.1} ms)", ms(n)));
                    }
                });
                if let Some(f) = running {
                    combo.response.on_hover_text(format!("The output runs at {f} frames ({:.1} ms)", ms(f)));
                    ui.ctx().request_repaint_after_secs(0.5);
                }
                if choice != app.ui.audio.buffer_size {
                    app.set_buffer_size(choice);
                }
                ui.end_row();
                if let Some(gaps) = app.recorder.as_ref().map(|r| r.monitor.underruns()).filter(|g| *g > 0) {
                    ui.label("");
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{gaps} gap(s) in the monitored input so far: try a larger buffer"));
                    ui.end_row();
                }
            });
            let mut dc = app.engine.session().edit.delay_compensation;
            if crate::widgets::checkbox(ui, &mut dc, "Delay compensation").changed() {
                let _ = app.run("options.delay_compensation", json!({"value": dc}));
            }
        } else {
            midi_inputs(app, ui);
        }
        // Both tabs take the height of the taller one, so the window keeps its size when switching.
        let heights_id = egui::Id::new("playback-engine-tab-heights");
        let mut heights: [f32; 2] = ui.data(|d| d.get_temp(heights_id)).unwrap_or_default();
        let height = ui.cursor().top() - top;
        if let Some(h) = heights.get_mut(usize::from(tab)) {
            *h = height;
        }
        ui.data_mut(|d| d.insert_temp(heights_id, heights));
        ui.add_space((heights[0].max(heights[1]) - height).max(0.0));
        ui.separator();
        let mut io = app.engine.session().edit.flag("edit_view.io");
        if crate::widgets::checkbox(ui, &mut io, "Show input selectors on tracks in the Edit window")
            .on_hover_text("The same as View > Edit Window Views > I/O")
            .changed()
        {
            let _ = app.run("view.edit_io", json!({"value": io}));
        }
        ui.horizontal(|ui| {
            if ui.button("Reconnect audio device").clicked() {
                app.reconnect_audio();
            }
            if ui.button("Rescan devices").clicked() {
                rescan = true;
                if let Some(m) = &mut app.midi_inputs {
                    m.reconnect();
                }
            }
        });
        if rescan {
            ui.data_mut(|d| d.remove::<std::sync::Arc<(Vec<DeviceInfo>, Vec<DeviceInfo>, Vec<u32>)>>(cache));
        }
    });
    // Only while the dialog shows the inputs: closing it, or leaving the tab, ends the listening.
    if !open {
        ctx.data_mut(|d| d.remove::<u64>(listen_id));
    }
    if let Some(p) = &app.player {
        p.set_input_test(listen);
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
    win(ctx, &mut open, "I/O Setup", vec2(420.0, 300.0), |ui| {
        ui.label(egui::RichText::new("Busses").strong());
        let busses: Vec<(String, String)> = app.engine.session().busses.iter().map(|b| (b.name.clone(), b.format.label().to_string())).collect();
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
                if ui.small_button("Delete").clicked() {
                    let _ = app.run("setup.io", json!({"action": "delete_bus", "name": name}));
                }
                ui.end_row();
            }
        });
        if ui.button("New Bus").clicked() {
            let n = app.engine.session().busses.len() + 1;
            let _ = app.run("mix.new_bus", json!({"name": format!("Bus {n}")}));
        }
        ui.separator();
        ui.label(egui::RichText::new("Outputs").strong());
        // Main output format: stereo, or a surround format (the mix and master faders follow).
        let main = app.engine.session().main_format();
        ui.horizontal(|ui| {
            ui.label("Main output");
            egui::ComboBox::from_id_salt("main_output_format").selected_text(main.label()).show_ui(ui, |ui| {
                for f in soundcraft_model::ChannelFormat::ALL.into_iter().filter(|f| (2..=16).contains(&f.channels())) {
                    if ui.selectable_label(f == main, f.label()).clicked() && f != main {
                        let _ = app.run("setup.main_format", json!({"format": f.label()}));
                    }
                }
            });
            ui.label(egui::RichText::new(format!("{} ch", main.channels())).small());
        });
        for o in app.engine.session().outputs.clone() {
            ui.label(format!("{} — {} from channel {}", o.name, o.format.label(), o.first_channel + 1));
        }
    });
    app.ui.show_io_setup = open;
}

/// Setup › Keyboard Shortcuts.
pub fn shortcuts_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_shortcuts;
    let mac = ctx.os().is_mac();
    win(ctx, &mut open, "Keyboard Shortcuts", vec2(520.0, 480.0), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sc").num_columns(3).striped(true).show(ui, |ui| {
                let engine = soundcraft_engine::command_specs().iter().map(|c| (c.shortcut, c.label, c.menu.join(" › ")));
                let windows = crate::menus::UI_COMMANDS.iter().map(|c| (c.3, c.1, c.2.replace(" > ", " › ")));
                for (shortcut, label, menu) in engine.chain(windows) {
                    let Some(shortcut) = shortcut else { continue };
                    ui.label(egui::RichText::new(crate::shortcuts::shortcut_label(shortcut, mac)).font(mono(11.0)));
                    ui.label(label);
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
    win(ctx, &mut open, "Clip Effects", vec2(560.0, 420.0), |ui| {
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
            ui.label("Select an audio clip.");
            return;
        };
        let name = app.engine.session().find_clip(cid).map(|(_, c)| c.name.clone()).unwrap_or_default();
        let bypass = app.engine.session().edit.flag(&format!("clip_fx.bypass.{}", cid.0));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).strong());
            if ui.selectable_label(bypass, "Bypass").clicked() {
                let _ = app.run("clip.effects_bypass", json!({"clips": [cid.0]}));
            }
            if ui.button("Render").clicked() {
                let _ = app.run("clip.effects_render", json!({"clips": [cid.0]}));
            }
            if ui.button("Clear").clicked() {
                let _ = app.run("edit.clear_clip_effects", json!({"clips": [cid.0]}));
            }
        });
        let get = |app: &SoundApp, k: &str, d: f32| app.engine.session().edit.values.get(&format!("clip_fx.{}.{k}", cid.0)).map_or(d, |v| *v as f32);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                for (col, (title, plugin, prefix)) in cols.iter_mut().zip([("EQ", "eq_7band", "eq."), ("Dynamics", "compressor", "comp.")]) {
                    col.label(egui::RichText::new(title).strong());
                    let Some(info) = soundcraft_dsp::plugin_info(plugin) else { continue };
                    for p in info.params {
                        let key = format!("{prefix}{}", p.id);
                        let mut v = get(app, &key, p.default);
                        let before = v;
                        col.add(egui::Slider::new(&mut v, p.min..=p.max).text(p.name));
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
            ui.add(egui::Slider::new(&mut g, -36.0..=24.0).text("Clip effects gain (dB)"));
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
