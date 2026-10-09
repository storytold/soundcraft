//! The menu bar, built from the incumbent's menu catalog so every item has its place. Items
//! that map to a command are live; the rest are shown disabled (and listed in the parity report).

use crate::{MainWindow, SoundApp};
use serde_json::{Value, json};
use soundcraft_engine::catalog;

/// UI-layer commands: (id, label, catalog path or "", shortcut).
pub const UI_COMMANDS: &[(&str, &str, &str, Option<&str>)] = &[
    ("window.mix", "Mix", "Window > Mix", Some("Cmd+=")),
    ("window.edit", "Edit", "Window > Edit", Some("Cmd+=")),
    ("window.toggle_mix_edit", "Toggle Mix/Edit", "", Some("Cmd+=")),
    ("window.transport", "Transport", "Window > Transport", Some("Cmd+1")),
    ("window.big_counter", "Big Counter", "Window > Big Counter", Some("Cmd+3")),
    ("window.memory_locations", "Memory Locations", "Window > Memory Locations", Some("Cmd+5")),
    ("window.undo_history", "Undo History", "Window > Undo History", None),
    ("window.clip_list", "Clip List", "Window > Clip List", None),
    ("window.track_list", "Track List", "View > Other Displays > Track List", None),
    ("window.clip_list_view", "Clip List", "View > Other Displays > Clip List", None),
    ("window.narrow_mix", "Narrow Mix", "View > Narrow Mix", Some("Cmd+Alt+M")),
    ("window.session_info", "Session Info", "", None),
    ("window.about", "About SoundCraft", "", None),
    ("window.close", "Close Window", "Window > Close Window", Some("Cmd+W")),
    ("window.hide_floating", "Hide All Floating Windows", "Window > Hide All Floating Windows", Some("Cmd+Ctrl+W")),
    ("window.midi_editor", "MIDI Editor", "Window > MIDI Editor", None),
    ("window.universe", "Universe", "View > Other Displays > Universe", None),
    ("window.video", "Video", "Window > Video", Some("Cmd+9")),
    ("window.video_universe", "Video Universe", "Window > Video Universe", None),
    ("window.automation", "Automation", "Window > Automation", Some("Cmd+4")),
    ("window.color_palette", "Color Palette", "Window > Color Palette", None),
    ("window.disk_usage", "Disk Usage", "Window > Disk Usage", None),
    ("window.system_usage", "System Usage", "Window > System Usage", None),
    ("window.task_manager", "Task Manager", "Window > Task Manager", None),
    ("window.metadata", "Metadata Inspector", "Window > Metadata Inspector", None),
    ("window.event_list", "MIDI Event List", "Window > MIDI Event List", None),
    ("window.midi_keyboard", "MIDI Keyboard", "Window > MIDI Keyboard", None),
    ("window.workspace", "Workspace", "Window > New Workspace > Default", None),
    ("window.workspace_presets", "Workspace", "Window > New Workspace > Track Presets", None),
    ("window.workspace_sounds", "Workspace", "Window > New Workspace > Soundbase", None),
    ("window.workspace_front", "Workspaces to Front", "Window > Workspaces > Bring to Front", None),
    ("window.workspace_close", "Close All Workspaces", "Window > Workspaces > Close All Workspaces", None),
    ("window.configurations", "Window Configuration List", "Window > Configurations > Window Configuration List", None),
    ("window.config_new", "New Configuration", "Window > Configurations > New Configuration...", None),
    ("window.config_update", "Update Active Configuration", "Window > Configurations > Update Active Configuration", None),
    ("window.config_auto_update", "Auto-Update Active Configuration", "Window > Configurations > Auto-Update Active Configuration", None),
    ("window.renderer", "Renderer", "Window > Renderer", None),
    ("window.ui_customization", "UI Customization", "Window > UI Customization", None),
    ("window.arrange_tile", "Tile", "Window > Arrange > Tile", None),
    ("window.arrange_tile_h", "Tile Horizontal", "Window > Arrange > Tile Horizontal", None),
    ("window.arrange_tile_v", "Tile Vertical", "Window > Arrange > Tile Vertical", None),
    ("window.arrange_cascade", "Cascade", "Window > Arrange > Cascade", None),
    ("window.midi_front", "MIDI Editors to Front", "Window > MIDI Editors > Bring to Front", None),
    ("window.midi_back", "MIDI Editors to Back", "Window > MIDI Editors > Send to Back", None),
    ("window.clip_effects", "Clip Effects", "Window > Clip Effects", None),
    ("window.clip_effects_dock", "Clip Effects", "View > Other Displays > Lower Dock > Clip Effects", None),
    ("window.search", "Search", "Window > Pro Tools Search", Some("Cmd+Ctrl+S")),
    ("window.score", "Score Editor", "Window > Score Editor", Some("Cmd+Ctrl+=")),
    ("window.beat_detective", "Beat Detective", "", None),
    ("window.tempo_ops", "Tempo Operations", "", None),
    ("window.time_ops", "Time Operations", "", None),
    ("window.midi_ops", "MIDI Operations", "", None),
    ("window.rtp", "MIDI Real-Time Properties", "", None),
    ("window.playback_engine", "Playback Engine", "", None),
    ("window.io_setup", "I/O Setup", "", None),
    ("window.shortcuts", "Keyboard Shortcuts", "", None),
    ("view.mix_section", "Mix Window View", "", None),
    ("ui.new_tracks_dialog", "New Tracks…", "", None),
    ("ui.bounce_dialog", "Bounce Mix…", "", None),
    ("ui.set", "Set UI State", "", None),
];

/// Extra catalog mappings handled by the UI layer.
pub fn ui_aliases() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&str, &str)> = UI_COMMANDS.iter().filter(|c| !c.2.is_empty()).map(|c| (c.2, c.0)).collect();
    for sec in [
        "Inserts A-E",
        "Inserts F-J",
        "Sends A-E",
        "Sends F-J",
        "I/O",
        "Track Color",
        "Comments",
        "EQ Curve",
        "Meters and Faders",
        "All",
        "Minimal",
        "Mic Preamps",
        "Instruments",
        "Object",
    ] {
        v.push((Box::leak(format!("View > Mix Window Views > {sec}").into_boxed_str()), "view.mix_section"));
    }
    v.extend(AUDIOSUITE.iter().map(|(p, _)| (*p, "audiosuite.process")));
    v.extend(MENU_WINDOWS.iter().copied());
    v
}

/// AudioSuite catalog entries → our processes (functional equivalents, our own plugins).
pub const AUDIOSUITE: &[(&str, &str)] = &[
    ("AudioSuite > EQ > Channel Strip", "channel_strip"),
    ("AudioSuite > EQ > EQ3 1-Band", "eq_1band"),
    ("AudioSuite > EQ > EQ3 7-Band", "eq_7band"),
    ("AudioSuite > Dynamics > BF-76", "compressor"),
    ("AudioSuite > Dynamics > Channel Strip", "channel_strip"),
    ("AudioSuite > Dynamics > Dyn3 Compressor/Limiter", "compressor"),
    ("AudioSuite > Dynamics > Dyn3 De-Esser", "de_esser"),
    ("AudioSuite > Dynamics > Dyn3 Expander/Gate", "expander_gate"),
    ("AudioSuite > Dynamics > Maxim", "maximizer"),
    ("AudioSuite > Pitch Shift > Pitch II", "pitch_shift"),
    ("AudioSuite > Pitch Shift > Pitch Shift Legacy", "pitch_shift"),
    ("AudioSuite > Pitch Shift > Time Shift", "time_stretch"),
    ("AudioSuite > Pitch Shift > Vari-Fi", "varispeed"),
    ("AudioSuite > Reverb > D-Verb", "room_reverb"),
    ("AudioSuite > Delay > Mod Delay III", "mod_delay"),
    ("AudioSuite > Modulation > Sci-Fi", "flanger"),
    ("AudioSuite > Harmonic > Eleven Lite", "saturator"),
    ("AudioSuite > Harmonic > Lo-Fi", "lofi"),
    ("AudioSuite > Harmonic > Recti-Fi", "rectifier"),
    ("AudioSuite > Harmonic > SansAmp PSA-1", "saturator"),
    ("AudioSuite > Other > DC Offset Removal", "dc_offset_removal"),
    ("AudioSuite > Other > Duplicate", "duplicate"),
    ("AudioSuite > Other > Gain", "gain"),
    ("AudioSuite > Other > Invert", "invert"),
    ("AudioSuite > Other > Normalize", "normalize"),
    ("AudioSuite > Other > Reverse", "reverse"),
    ("AudioSuite > Other > Signal Generator", "signal_generator"),
    ("AudioSuite > Other > Time Compression Expansion", "time_stretch"),
];

/// Commands that open a dialog when invoked from a menu (programmatic calls never do).
const DIALOG_COMMANDS: &[&str] = &[
    "track.new",
    "file.bounce_mix",
    "session.new",
    "file.import_audio",
    "file.import_midi",
    "session.open",
    "session.save_as",
    "session.save_copy",
    "track.group",
    "edit.repeat",
    "edit.shift",
    "edit.fades_create",
    "setup.session",
    "event.quantize",
    "event.transpose",
    "edit.strip_silence",
];

struct MenuNode {
    label: String,
    path: String,
    children: Vec<MenuNode>,
}

fn tree() -> Vec<MenuNode> {
    let mut roots: Vec<MenuNode> = Vec::new();
    for line in catalog::catalog() {
        let parts: Vec<&str> = line.split(" > ").collect();
        let mut level = &mut roots;
        let mut path = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                path.push_str(" > ");
            }
            path.push_str(p);
            let idx = match level.iter().position(|n| n.label == *p) {
                Some(i) => i,
                None => {
                    level.push(MenuNode { label: p.to_string(), path: path.clone(), children: Vec::new() });
                    level.len() - 1
                }
            };
            let Some(node) = level.get_mut(idx) else { break };
            level = &mut node.children;
        }
    }
    roots
}

pub fn menu_bar(app: &mut SoundApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::current();
    egui::Panel::top("menu_bar").exact_size(24.0).frame(egui::Frame::NONE.fill(t.toolbar_bg).inner_margin(egui::Margin::symmetric(8, 2))).show(
        ui,
        |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("SoundCraft", |ui| {
                    if ui.button("About SoundCraft").clicked() {
                        app.ui.show_about = true;
                    }
                    if ui.button("Session Info").clicked() {
                        app.ui.show_session_info = true;
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        let _ = app.run("app.quit", json!({}));
                    }
                });
                let extra = ui_aliases();
                for root in tree() {
                    ui.menu_button(&root.label, |ui| {
                        for c in &root.children {
                            menu_node(app, ui, c, &extra);
                        }
                        if root.label == "AudioSuite" {
                            ui.separator();
                            ui.label(egui::RichText::new("Processes selected clips offline").small().color(t.text_dim));
                        }
                    });
                }
                let title = format!(
                    "{}{}  —  {}",
                    app.engine.session().name,
                    if app.engine.is_dirty() { " *" } else { "" },
                    if app.ui.window == MainWindow::Edit { "Edit" } else { "Mix" }
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(title).color(t.text_dim));
                });
            });
        },
    );
}

fn menu_node(app: &mut SoundApp, ui: &mut egui::Ui, n: &MenuNode, extra: &[(&str, &str)]) {
    if !n.children.is_empty() {
        ui.menu_button(&n.label, |ui| {
            for c in &n.children {
                menu_node(app, ui, c, extra);
            }
        });
        return;
    }
    let id = catalog::implemented_by(&n.path, extra).or_else(|| MENU_WINDOWS.iter().find(|(p, _)| *p == n.path).map(|(_, w)| w.to_string()));
    let enabled = match id.as_deref() {
        Some(i) if i.starts_with("window.") || i.starts_with("view.mix_section") || i == "audiosuite.process" => true,
        Some(i) => soundcraft_engine::find_command(i).is_some_and(|c| (c.enabled)(&app.engine).is_ok()),
        None => false,
    };
    let checked = checked_state(app, &n.path, id.as_deref());
    let label = if checked { format!("✔ {}", n.label) } else { n.label.clone() };
    let shortcut = id.as_deref().and_then(soundcraft_engine::find_command).and_then(|c| c.shortcut).unwrap_or("");
    let btn = egui::Button::new(label).shortcut_text(shortcut);
    if ui.add_enabled(enabled, btn).clicked()
        && let Some(id) = id
    {
        invoke_menu(app, &id, &n.path);
        ui.close();
    }
}

fn checked_state(app: &SoundApp, path: &str, id: Option<&str>) -> bool {
    let e = &app.engine.session().edit;
    match id {
        Some("options.loop_playback") => e.loop_playback,
        Some("options.loop_record") => e.loop_record,
        Some("options.quickpunch") => e.quickpunch,
        Some("options.pre_post_roll") => e.pre_post_roll,
        Some("options.link_timeline_edit") => e.link_timeline_edit,
        Some("options.link_track_edit") => e.link_track_edit,
        Some("options.insertion_follows_playback") => e.insertion_follows_playback,
        Some("options.tab_to_transient") => e.tab_to_transient,
        Some("options.automation_follows_edit") => e.automation_follows_edit,
        Some("options.markers_follow_edit") => e.markers_follow_edit,
        Some("options.click") => e.click,
        Some("options.delay_compensation") => e.delay_compensation,
        Some("options.pre_fader_metering") => e.pre_fader_metering,
        Some("view.ruler") => path.rsplit(" > ").next().map(ruler_id).is_some_and(|r| e.rulers.iter().any(|x| x == r)),
        Some("view.main_counter") => path.rsplit(" > ").next().map(ruler_id).is_some_and(|r| e.main_counter.id() == r),
        Some("window.mix") => app.ui.window == MainWindow::Mix,
        Some("window.edit") => app.ui.window == MainWindow::Edit,
        Some("window.track_list") => app.ui.show_tracks_list,
        Some("window.clip_list_view") | Some("window.clip_list") => app.ui.show_clip_list,
        Some("window.narrow_mix") => app.ui.narrow_mix,
        Some("window.renderer") => app.extra.show_renderer,
        Some("window.ui_customization") => app.extra.show_ui_customization,
        Some("window.config_auto_update") => app.extra.auto_update_config,
        Some("view.mix_section") => path.rsplit(" > ").next().map(mix_section_id).is_some_and(|s| app.ui.mix_views.iter().any(|v| v == s)),
        _ => false,
    }
}

fn ruler_id(label: &str) -> &'static str {
    match label {
        "Bars|Beats" => "bars_beats",
        "Min:Secs" => "min_secs",
        "Timecode" => "timecode",
        "Feet+Frames" => "feet_frames",
        "Samples" => "samples",
        "Tempo" => "tempo",
        "Meter" => "meter",
        "Markers" => "markers",
        "Key Signature" => "key",
        "Chord Symbols" => "chords",
        _ => "",
    }
}

fn mix_section_id(label: &str) -> &'static str {
    match label {
        "Inserts A-E" => "inserts_ae",
        "Inserts F-J" => "inserts_fj",
        "Sends A-E" => "sends_ae",
        "Sends F-J" => "sends_fj",
        "I/O" => "io",
        "Track Color" => "color",
        "Comments" => "comments",
        "EQ Curve" => "eq_curve",
        "Meters and Faders" => "meters",
        "All" => "all",
        "Minimal" => "minimal",
        "Mic Preamps" => "mic_preamps",
        "Instruments" => "instruments",
        "Object" => "object",
        _ => "",
    }
}

/// Parameters implied by the menu item a command was reached through.
fn params_for(path: &str, id: &str) -> Value {
    let leaf = path.rsplit(" > ").next().unwrap_or("");
    match id {
        "view.ruler" => json!({"ruler": ruler_id(leaf)}),
        "view.main_counter" => json!({"format": ruler_id(leaf)}),
        "options.scrolling" => {
            json!({"mode": match leaf { "No Scrolling" => "none", "After Playback" => "after_playback", "Continuous" => "continuous", "Center Playhead" => "center", _ => "page" }})
        }
        "options.solo_mode" => {
            json!({"mode": if leaf.starts_with("AFL") { "afl" } else if leaf.starts_with("PFL") { "pfl" } else { "sip" }, "xor": leaf.starts_with("X-OR")})
        }
        "clip.rating" => json!({"rating": leaf.parse::<u8>().unwrap_or(0)}),
        "track.change_width" => json!({"format": leaf}),
        "view.mix_section" => json!({"section": mix_section_id(leaf)}),
        "audiosuite.process" => json!({"process": AUDIOSUITE.iter().find(|(p, _)| *p == path).map_or("", |(_, x)| *x)}),
        _ => json!({}),
    }
}

/// Menu items that open a SoundCraft window when clicked (programmatic calls still run the command).
const MENU_WINDOWS: &[(&str, &str)] = &[
    ("Setup > Hardware...", "window.playback_engine"),
    ("Setup > Playback Engine...", "window.playback_engine"),
    ("Setup > I/O...", "window.io_setup"),
    ("Setup > Keyboard Shortcuts...", "window.shortcuts"),
    ("Setup > Session", "setup.session"),
    ("Event > Beat Detective", "window.beat_detective"),
    ("Event > Tempo Operations > Tempo Operations Window", "window.tempo_ops"),
    ("Event > Time Operations > Time Operations Window", "window.time_ops"),
    ("Event > MIDI Operations > MIDI Operations Window", "window.midi_ops"),
    ("Event > MIDI Real-Time Properties", "window.rtp"),
];

/// Menu-style invocation: may open a dialog.
pub fn invoke_menu(app: &mut SoundApp, id: &str, path: &str) {
    if let Some((_, w)) = MENU_WINDOWS.iter().find(|(p, _)| *p == path)
        && w.starts_with("window.")
    {
        let _ = app.run(w, json!({"value": true}));
        return;
    }
    if id == "audiosuite.process" {
        let p = params_for(path, id);
        app.ui.audiosuite = p.get("process").and_then(Value::as_str).map(str::to_string);
        return;
    }
    if wants_dialog(id) && app.dialogs.open_for_command(&app.engine, id) {
        return;
    }
    let params = params_for(path, id);
    let _ = app.run(id, params);
}

/// Keyboard shortcut for a dialog command: open its dialog. Returns true when handled.
pub fn invoke_shortcut_dialog(app: &mut SoundApp, id: &str) -> bool {
    wants_dialog(id) && app.dialogs.open_for_command(&app.engine, id)
}

/// Commands that open a dialog first: the listed ones, Score Setup, and anything that needs a path.
fn wants_dialog(id: &str) -> bool {
    DIALOG_COMMANDS.contains(&id) || id == "file.score_setup" || crate::dialogs::takes_path(id)
}

/// Handle UI-layer commands. Returns None when `id` is not a UI command.
pub fn run_ui_command(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if let Some(v) = crate::extra_windows::run(app, id, p) {
        return Some(Ok(v));
    }
    let toggle = |v: &mut bool| {
        *v = p.get("value").and_then(Value::as_bool).unwrap_or(!*v);
        json!({"value": *v})
    };
    let r = match id {
        "window.mix" => {
            app.ui.window = MainWindow::Mix;
            json!({"window": "mix"})
        }
        "window.edit" => {
            app.ui.window = MainWindow::Edit;
            json!({"window": "edit"})
        }
        "window.toggle_mix_edit" => {
            app.ui.window = if app.ui.window == MainWindow::Edit { MainWindow::Mix } else { MainWindow::Edit };
            json!({"window": app.ui.window})
        }
        "window.transport" => toggle(&mut app.ui.show_transport),
        "window.big_counter" => toggle(&mut app.ui.show_big_counter),
        "window.memory_locations" => toggle(&mut app.ui.show_memory_locations),
        "window.undo_history" => toggle(&mut app.ui.show_undo_history),
        "window.clip_list" | "window.clip_list_view" => toggle(&mut app.ui.show_clip_list),
        "window.track_list" => toggle(&mut app.ui.show_tracks_list),
        "window.narrow_mix" => toggle(&mut app.ui.narrow_mix),
        "window.session_info" => toggle(&mut app.ui.show_session_info),
        "window.about" => toggle(&mut app.ui.show_about),
        "window.midi_editor" | "window.midi_front" => toggle(&mut app.ui.show_midi_editor),
        "window.midi_back" => {
            app.ui.show_midi_editor = false;
            json!({})
        }
        "window.automation" => toggle(&mut app.ui.show_automation),
        "window.clip_effects" | "window.clip_effects_dock" => toggle(&mut app.ui.show_clip_effects),
        "window.search" => toggle(&mut app.ui.show_search),
        "window.score" => toggle(&mut app.ui.show_score),
        "window.beat_detective" => toggle(&mut app.ui.show_beat_detective),
        "window.tempo_ops" => toggle(&mut app.ui.show_tempo_ops),
        "window.time_ops" => toggle(&mut app.ui.show_time_ops),
        "window.midi_ops" => toggle(&mut app.ui.show_midi_ops),
        "window.rtp" => toggle(&mut app.ui.show_rtp),
        "window.playback_engine" => toggle(&mut app.ui.show_playback_engine),
        "window.io_setup" => toggle(&mut app.ui.show_io_setup),
        "window.shortcuts" => toggle(&mut app.ui.show_shortcuts),
        "window.color_palette" => toggle(&mut app.ui.show_color_palette),
        "window.disk_usage" => toggle(&mut app.ui.show_disk_usage),
        "window.system_usage" => toggle(&mut app.ui.show_system_usage),
        "window.task_manager" => toggle(&mut app.ui.show_task_manager),
        "window.metadata" => toggle(&mut app.ui.show_metadata),
        "window.event_list" => toggle(&mut app.ui.show_event_list),
        "window.midi_keyboard" => toggle(&mut app.ui.show_midi_keyboard),
        "window.workspace" | "window.workspace_presets" | "window.workspace_sounds" | "window.workspace_front" => {
            app.ui.show_workspace = true;
            json!({"value": true})
        }
        "window.workspace_close" => {
            app.ui.show_workspace = false;
            json!({"value": false})
        }
        "window.configurations" => toggle(&mut app.ui.show_configurations),
        "window.config_new" | "window.config_update" => {
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            let v = serde_json::to_value(&snap).unwrap_or_default();
            if id == "window.config_update" && !app.ui.configurations.is_empty() {
                if let Some(last) = app.ui.configurations.last_mut() {
                    last.1 = v;
                }
            } else {
                let n = app.ui.configurations.len() + 1;
                app.ui.configurations.push((format!("Configuration {n}"), v));
            }
            json!({"configurations": app.ui.configurations.len()})
        }
        "window.arrange_tile" | "window.arrange_tile_h" | "window.arrange_tile_v" | "window.arrange_cascade" => {
            app.arrange_request = true;
            json!({})
        }
        "window.universe" => toggle(&mut app.ui.show_universe),
        "window.video" => toggle(&mut app.ui.show_video),
        "window.video_universe" => toggle(&mut app.ui.show_video_universe),
        "window.close" | "window.hide_floating" => {
            app.ui.show_transport = false;
            app.ui.show_big_counter = false;
            app.ui.show_memory_locations = false;
            app.ui.show_undo_history = false;
            app.ui.show_session_info = false;
            app.ui.plugin_windows.clear();
            app.ui.audiosuite = None;
            json!({})
        }
        "view.mix_section" => {
            let sec = p.get("section").and_then(Value::as_str).unwrap_or("");
            match sec {
                "all" => {
                    app.ui.mix_views =
                        ["inserts_ae", "inserts_fj", "sends_ae", "sends_fj", "io", "comments", "color"].iter().map(|s| s.to_string()).collect()
                }
                "minimal" => app.ui.mix_views.clear(),
                "" => return Some(Err("`section` required".into())),
                s => {
                    if app.ui.mix_views.iter().any(|v| v == s) {
                        app.ui.mix_views.retain(|v| v != s);
                    } else {
                        app.ui.mix_views.push(s.to_string());
                    }
                }
            }
            json!({"mix_views": app.ui.mix_views})
        }
        "ui.new_tracks_dialog" => {
            let _ = app.dialogs.open_for_command(&app.engine, "track.new");
            json!({})
        }
        "ui.bounce_dialog" => {
            let _ = app.dialogs.open_for_command(&app.engine, "file.bounce_mix");
            json!({})
        }
        "ui.set" => match serde_json::to_value(&app.ui) {
            Ok(mut cur) => {
                if let (Some(obj), Some(patch)) = (cur.as_object_mut(), p.as_object()) {
                    for (k, v) in patch {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                match serde_json::from_value(cur) {
                    Ok(ns) => {
                        app.ui = ns;
                        json!({"ui": app.ui})
                    }
                    Err(e) => return Some(Err(format!("bad ui state: {e}"))),
                }
            }
            Err(e) => return Some(Err(e.to_string())),
        },
        _ => return None,
    };
    Some(Ok(r))
}

/// Parity including UI-layer commands.
pub fn parity() -> Value {
    let extra = ui_aliases();
    catalog::parity_with(&extra)
}
