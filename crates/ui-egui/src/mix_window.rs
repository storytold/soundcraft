//! The Mix window: one channel strip per track.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular, rgb};
use crate::widgets::{
    PannerEdit, PannerSpeaker, db_text, fader, fader_slot, groove, inset, meter, meter_scale, multi_meter, pan_knob, pan_text, raised_key,
    rec_toggle, selector_box, selector_box_with_tooltip, shade, surround_panner, text_toggle, vgradient,
};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ChannelFormat, Route, SurroundPan, Track, TrackId, TrackKind};
use soundcraft_playback::MeterSnapshot;

pub fn strip_width(narrow: bool) -> f32 {
    if narrow { 64.0 } else { 98.0 }
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    crate::docking::show(app, ui, crate::MainWindow::Mix);
}

/// The Mix window in its own native window (Window › Mix in Separate Window), so Edit and Mix can
/// be seen at once, e.g. one per monitor. Closing that window brings Mix back into the main one.
/// Backends without multiple windows show it as a floating egui window instead.
pub fn show_separate(app: &mut SoundApp, ctx: &egui::Context) {
    let title = format!("{}  —  Mix", app.engine.session().name);
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1100.0, 720.0]).with_min_inner_size([480.0, 320.0]);
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("soundcraft.mix"), builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        if std::mem::take(&mut app.focus_mix) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        show(app, ui);
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            separate_closed(app);
        }
    });
}

/// The separate Mix window was closed: Mix goes back into the main window.
pub fn separate_closed(app: &mut SoundApp) {
    app.ui.mix_separate = false;
    app.ui.window = crate::MainWindow::Mix;
}

pub(crate) fn content(app: &mut SoundApp, ui: &mut Ui) {
    let ids: Vec<TrackId> = app.engine.session().tracks.iter().filter(|x| !x.hidden && x.kind != TrackKind::Video).map(|x| x.id).collect();
    // Per-channel peaks for multichannel meters (read once per frame).
    let snap = app.player.as_ref().filter(|_| app.meters_live()).map(|p| p.meters());
    // Taken for this frame only: a strip that isn't drawn (hidden track) must not scroll the Mix
    // whenever it comes back. A request made during this frame (the strip's menu) waits for the next.
    let scroll_to = app.mix_scroll_into_view.take();
    egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for id in ids {
                strip(app, ui, id, snap.as_ref(), scroll_to == Some(id));
            }
            let rest = Rect::from_min_max(pos2(ui.cursor().min.x, ui.clip_rect().min.y), ui.clip_rect().max);
            if rest.width() > 4.0 && ui.interact(rest, ui.id().with("past_strips"), Sense::click()).double_clicked() {
                let _ = app.dialogs.open_for_command(&app.engine, "track.new");
            }
        });
    });
}

/// Format of the destination a track's output feeds (the main mix, a bus, or stereo).
fn output_format(app: &SoundApp, track: &Track) -> ChannelFormat {
    let s = app.engine.session();
    match &track.mixer.output {
        Route::Main => s.main_format(),
        Route::Bus(b) => s.bus(*b).map_or(ChannelFormat::Stereo, |b| b.format),
        _ => ChannelFormat::Stereo,
    }
}

/// Speaker dots of a format, placed on the panner square.
fn panner_speakers(fmt: ChannelFormat) -> Vec<PannerSpeaker> {
    let sp = fmt.speakers();
    let layout: Vec<soundcraft_dsp::pan::SpeakerPos> =
        sp.iter().map(|s| soundcraft_dsp::pan::SpeakerPos { az: s.azimuth(), el: s.elevation(), lfe: s.is_lfe() }).collect();
    sp.iter()
        .filter(|s| !s.is_lfe())
        .map(|s| {
            let (x, y) = soundcraft_dsp::pan::azimuth_to_puck(&layout, s.azimuth());
            PannerSpeaker { label: s.label(), x, y, height: s.is_height() }
        })
        .collect()
}

/// Ballistic per-channel meter levels for multichannel strips, kept in egui memory.
fn channel_levels(ui: &Ui, key: (&str, u64), peaks: &[f32], n: usize) -> Vec<f32> {
    let id = egui::Id::new(key);
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.25);
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    let mut lv: Vec<f32> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    lv.resize(n, 0.0);
    for (i, l) in lv.iter_mut().enumerate() {
        let p = peaks.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        *l = if p >= *l { p } else { (*l * fall).max(p) };
        if *l < 1e-5 {
            *l = 0.0;
        }
    }
    ui.data_mut(|d| d.insert_temp(id, lv.clone()));
    lv
}

/// Pro Tools' names for sections in narrow strips, and in any strip too narrow for the full ones.
fn short_label(text: &str) -> &str {
    match text {
        "INSERTS A-E" => "I A-E",
        "INSERTS F-J" => "I F-J",
        "SENDS A-E" => "S A-E",
        "SENDS F-J" => "S F-J",
        "INSTRUMENT" => "INST",
        _ => text,
    }
}

/// A section's caption. Narrow strips always use the short names, as Pro Tools' Narrow Mix does, so
/// every narrow strip reads the same whatever the UI font: measuring alone left the full name in
/// place wherever a narrower font (egui's built-in face on a system with no fonts) happened to fit.
/// Other strips shorten a name only when it doesn't fit with 4 pt to spare on each side.
fn section_label(ui: &Ui, r: Rect, text: &str, narrow: bool) {
    let p = ui.painter();
    let color = Tokens::current().section_label;
    let text = if narrow { short_label(text) } else { text };
    let mut galley = p.layout_no_wrap(text.to_owned(), bold(10.0), color);
    if galley.size().x > r.width() - 8.0 {
        galley = p.layout_no_wrap(short_label(text).to_owned(), bold(10.0), color);
    }
    p.galley(Align2::CENTER_CENTER.anchor_size(pos2(r.center().x, r.min.y + 7.0), galley.size()).min, galley, color);
}

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => "no output".into(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| "Out 1-2".into(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
    }
}

pub(crate) fn input_name(app: &SoundApp, track: &Track) -> String {
    if track.kind.has_playlist() && track.mixer.input == Route::None { "In 1".to_string() } else { route_name(app, &track.mixer.input) }
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| route_menu(app, ui, id, input));
}

fn strip(app: &mut SoundApp, ui: &mut Ui, id: TrackId, snap: Option<&MeterSnapshot>, scroll_here: bool) {
    let t = Tokens::current();
    let session = app.engine.session_arc();
    let Some(track) = session.track(id) else { return };
    let narrow = app.ui.narrow_mix;
    let w = strip_width(narrow);
    let h = ui.available_height().max(560.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    if scroll_here {
        // egui stops `item_spacing` short of the edge (the scroll area's, not the strips' 1 pt),
        // which would leave a sliver of the previous strip showing.
        let margin = ui.ctx().global_style().spacing.item_spacing.x;
        ui.scroll_to_rect(r.translate(vec2(margin, 0.0)), Some(egui::Align::Min));
    }
    let selected = app.engine.session().edit.selected_tracks.contains(&id);
    let bg = if selected { t.strip_selected } else { t.strip_bg };
    vgradient(ui.painter(), r, shade(bg, 0.06), bg);
    ui.painter().vline(r.min.x + 0.5, r.y_range(), Stroke::new(1.0, t.edge_light));
    let mut y = r.min.y + 4.0;
    let inner_w = w - 8.0;
    let x0 = r.min.x + 4.0;
    let views = app.ui.mix_views.clone();
    let has = |v: &str| views.iter().any(|x| x == v);
    let all = has("all");
    // Preamp, instrument and object rows keep their height on every strip so sections line up;
    // their controls only appear on tracks with an input.
    let has_input = matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi);
    if all || has("mic_preamps") {
        // Input gain stage ahead of the inserts (the track's trim).
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 34.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "PREAMP", narrow);
        if has_input {
            let kr = Rect::from_min_size(pos2(x0 + 6.0, sec.min.y + 15.0), vec2(inner_w - 12.0, 16.0));
            let mut db = track.mixer.trim_db;
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let resp = c.add_sized(kr.size(), egui::DragValue::new(&mut db).range(-24.0..=24.0).speed(0.1).suffix(" dB").max_decimals(1));
            if resp.changed() {
                let _ = app.engine.execute_merged("mix.trim", &json!({"tracks": [id.0], "db": db}), &format!("trim:{}", id.0));
            }
            if resp.drag_stopped() || resp.lost_focus() {
                app.engine.end_merge();
            }
        }
        y = sec.max.y + 4.0;
    }
    if all || has("instruments") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 34.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "INSTRUMENT", narrow);
        let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 16.0), vec2(inner_w - 4.0, 15.0));
        let slot = track.mixer.inserts.iter().position(|i| i.as_ref().is_some_and(|i| plugin_info(&i.plugin).is_some_and(|p| p.is_instrument)));
        match slot {
            _ if track.kind == TrackKind::Instrument => instrument_slot(app, ui, track, sr),
            Some(k) => insert_slot(app, ui, track, k, sr),
            None if track.kind == TrackKind::Midi => {
                ui.painter().text(sr.center(), Align2::CENTER_CENTER, "MIDI in: all", regular(9.0), t.text_dim);
            }
            None => {}
        }
        y = sec.max.y + 4.0;
    }
    if all || has("object") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0));
        if has_input {
            let on = app.engine.session().edit.flag(&format!("object.{}", id.0));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(sec));
            let label = egui::RichText::new(if on { "OBJECT" } else { "BED" }).font(bold(9.0));
            if c.add_sized(sec.size(), egui::Button::new(label).selected(on)).on_hover_text("Route as an immersive object or to the bed").clicked() {
                let _ = app.run("track.object", json!({"tracks": [id.0], "object": !on}));
            }
        }
        y = sec.max.y + 4.0;
    }
    for (key, title, off, is_send) in [
        ("inserts_ae", "INSERTS A-E", 0usize, false),
        ("inserts_fj", "INSERTS F-J", 5, false),
        ("sends_ae", "SENDS A-E", 0, true),
        ("sends_fj", "SENDS F-J", 5, true),
    ] {
        if !(all || has(key)) {
            continue;
        }
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 16.0 + 5.0 * 17.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, title, narrow);
        for k in 0..5 {
            let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0 + k as f32 * 17.0), vec2(inner_w - 4.0, 15.0));
            if is_send {
                send_slot(app, ui, track, off + k, sr);
            } else {
                insert_slot(app, ui, track, off + k, sr);
            }
        }
        y = sec.max.y + 4.0;
    }
    if all || has("eq_curve") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 40.0));
        ui.painter().rect_filled(sec, 2.0, Color32::from_rgb(14, 18, 22));
        let eq = track.mixer.inserts.iter().flatten().find(|i| i.plugin == "eq_7band" || i.plugin == "eq_1band");
        if let Some(ins) = eq {
            let n = 40;
            let freqs: Vec<f32> = (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect();
            let params: Vec<(&str, f32)> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let resp = if ins.plugin == "eq_7band" {
                soundcraft_dsp::eq7_response(&params, &freqs, 48_000.0)
            } else {
                soundcraft_dsp::eq1_response(&params, &freqs, 48_000.0)
            };
            let pts: Vec<egui::Pos2> = resp
                .iter()
                .enumerate()
                .map(|(i, db)| {
                    pos2(sec.min.x + sec.width() * i as f32 / (n - 1) as f32, sec.center().y - db.clamp(-18.0, 18.0) / 18.0 * sec.height() * 0.45)
                })
                .collect();
            ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5, t.counter_text)));
        } else {
            ui.painter().line_segment(
                [pos2(sec.min.x + 2.0, sec.center().y), pos2(sec.max.x - 2.0, sec.center().y)],
                Stroke::new(1.0, Color32::from_rgb(60, 70, 80)),
            );
        }
        y = sec.max.y + 4.0;
    }
    if all || has("comments") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 44.0));
        let mut c = track.comments.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sec));
        let hint = if narrow { "" } else { "comments" };
        if child.add(egui::TextEdit::multiline(&mut c).desired_width(inner_w).desired_rows(2).hint_text(hint).font(regular(10.0))).changed() {
            let _ = app.engine.execute_merged("track.comments", &json!({"track": id.0, "comments": c}), &format!("comments:{}", id.0));
        }
        y = sec.max.y + 4.0;
    }
    if all || has("io") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 16.0 + 2.0 * 18.0 + 30.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "I / O", narrow);
        let in_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0), vec2(inner_w - 4.0, 16.0));
        let out_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 33.0), vec2(inner_w - 4.0, 16.0));
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-input", id.0)).max_rect(in_r));
        let resp = selector_box(&mut c, in_r.width(), in_r.height(), &input_name(app, track), t.text);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Input"));
        route_popup(app, &resp, id, true);
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-output", id.0)).max_rect(out_r));
        let resp = selector_box(&mut c, out_r.width(), out_r.height(), &route_name(app, &track.mixer.output), t.text);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Output"));
        route_popup(app, &resp, id, false);
        // Automation mode.
        let am_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 51.0), vec2(inner_w - 4.0, 16.0));
        ui.painter().text(pos2(am_r.center().x, am_r.min.y - 1.0), Align2::CENTER_BOTTOM, "", regular(9.0), t.text_dim);
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(am_r));
        let mode = track.mixer.automation_mode;
        let col = if mode == soundcraft_model::AutomationMode::Read {
            t.auto_read
        } else if mode == soundcraft_model::AutomationMode::Off {
            t.text_dim
        } else {
            t.auto_write
        };
        let full_label = format!("auto {}", mode.label());
        let label = if mode == soundcraft_model::AutomationMode::TouchLatch {
            if narrow { "t/l" } else { mode.label() }
        } else if narrow {
            mode.label()
        } else {
            &full_label
        };
        let resp = selector_box_with_tooltip(&mut c, am_r.width(), am_r.height(), label, col, &full_label);
        egui::Popup::menu(&resp).show(|ui| {
            for m in soundcraft_model::AutomationMode::ALL {
                if ui.selectable_label(m == mode, m.label()).clicked() {
                    let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
                }
            }
        });
        y = sec.max.y + 4.0;
    }
    // Group selector.
    let no_group = if narrow { "nogrp" } else { "no group" };
    let gname = app.engine.session().groups.iter().find(|g| g.members.contains(&id)).map_or(no_group.to_string(), |g| g.name.clone());
    let gr = Rect::from_min_size(pos2(x0 + 2.0, y), vec2(inner_w - 4.0, 16.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(gr));
    let _ = selector_box(&mut c, gr.width(), gr.height(), &gname, t.text_dim);
    y += 22.0;
    // Pan: a surround panner when the output is multichannel, else the stereo knobs.
    let out_fmt = output_format(app, track);
    let surround_out =
        out_fmt.channels() > 2 && !out_fmt.is_ambisonic() && !matches!(track.kind, TrackKind::Master | TrackKind::Vca) && track.channels() <= 2;
    if surround_out {
        let size = (inner_w - 4.0).min(88.0);
        let sp = track.mixer.surround.unwrap_or_else(|| {
            SurroundPan::from_stereo(if track.mixer.pan.len() == 1 { track.mixer.pan.first().copied().unwrap_or(0.0) } else { 0.0 })
        });
        let rows = if out_fmt.has_height() { 2.0 } else { 1.0 };
        let pr = Rect::from_min_size(pos2(x0 + (inner_w - size) * 0.5, y), vec2(size, size + rows * 11.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(pr).layout(egui::Layout::top_down(egui::Align::Min)));
        c.spacing_mut().item_spacing.y = 1.0;
        let speakers = panner_speakers(out_fmt);
        let z = out_fmt.has_height().then_some(sp.z);
        if let Some(edit) = surround_panner(&mut c, size, &speakers, (sp.x, sp.y), sp.divergence, z) {
            let params = match edit {
                PannerEdit::Position(x, y) => json!({"track": id.0, "x": x, "y": y}),
                PannerEdit::Divergence(d) => json!({"track": id.0, "divergence": d}),
                PannerEdit::Height(h) => json!({"track": id.0, "z": h}),
            };
            let _ = app.engine.execute_merged("mix.surround_pan", &params, &format!("span:{}", id.0));
        }
        y += size + rows * 11.0 + 6.0;
    } else if !track.mixer.pan.is_empty() && track.kind != TrackKind::Vca {
        let n = track.mixer.pan.len().min(2);
        let ks = if n == 2 { (inner_w / 2.0 - 6.0).min(32.0) } else { 34.0 };
        for i in 0..n {
            let cx = if n == 2 { x0 + inner_w * (0.25 + 0.5 * i as f32) } else { x0 + inner_w * 0.5 };
            let kr = Rect::from_center_size(pos2(cx, y + 17.0), vec2(ks, ks));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let mut v = track.mixer.pan.get(i).copied().unwrap_or(0.0);
            let before = v;
            let resp = pan_knob(&mut c, ks, &mut v, "Pan (drag; double-click centres)");
            if (v - before).abs() > f32::EPSILON {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": v, "index": i}), &format!("pan:{}:{i}", id.0));
            }
            let _ = resp;
            let pr = Rect::from_center_size(pos2(cx, y + 42.0), vec2(ks + 8.0, 13.0));
            inset(ui.painter(), pr, t.counter_bg);
            let (_, entered) =
                editable_readout(ui, pr, ui.id().with(("pan-value", id.0, i)), &pan_text(v), &format!("{:.0}", v * 100.0), parse_pan, 0.0);
            if let Some(v) = entered {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": v, "index": i}), &format!("pan:{}:{i}", id.0));
            }
        }
        y += 54.0;
    }
    // Rec / Input / Solo / Mute.
    let bw = (inner_w - 6.0) / 2.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if track.kind.has_playlist() {
        if text_toggle(&mut row, vec2(bw, 18.0), "I", track.mixer.input_monitor, t.input, "Input monitoring").clicked() {
            let _ = app.run("mix.input_monitor", json!({"track": id.0}));
        }
        if rec_toggle(&mut row, vec2(bw, 18.0), track.mixer.record_arm, "Record enable").clicked() {
            let _ = app.run("mix.record_arm", json!({"track": id.0}));
        }
    }
    y += 22.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if track.kind != TrackKind::Master {
        if text_toggle(&mut row, vec2(bw, 18.0), "S", track.mixer.solo, t.solo, "Solo").clicked() {
            let _ = app.run("mix.solo", json!({"track": id.0}));
        }
        if text_toggle(&mut row, vec2(bw, 18.0), "M", track.mixer.mute, t.mute, "Mute").clicked() {
            let _ = app.run("mix.mute", json!({"track": id.0}));
        }
    }
    y += 26.0;
    // Fader + meter.
    let bottom_h = 64.0;
    let fh = (r.max.y - bottom_h - y).max(120.0);
    let wide_meter = (if track.kind == TrackKind::Master { app.engine.session().main_format() } else { track.format }).channels() > 2;
    let fr = Rect::from_min_size(pos2(x0, y), vec2(inner_w * if wide_meter { 0.45 } else { 0.6 }, fh));
    if let Some(db) = fader(ui, fr, track.mixer.volume_db, ui.id().with(("fader", id.0))) {
        app.gesture = Some(crate::Gesture::Fader { track: id, start_db: track.mixer.volume_db });
        let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &format!("fader:{}", id.0));
    }
    let md = app.meters.get(&id).copied().unwrap_or_default();
    let slot = fader_slot(fr);
    let mr = Rect::from_min_max(pos2(fr.max.x + 4.0, slot.min.y - 5.0), pos2(x0 + inner_w - 2.0, slot.max.y + if wide_meter { 17.0 } else { 1.0 }));
    if !narrow && !wide_meter {
        meter_scale(ui.painter(), mr, mr.min.x - 1.0);
    }
    let strip_fmt = if track.kind == TrackKind::Master { app.engine.session().main_format() } else { track.format };
    let all_chans = strip_fmt.channels().min(soundcraft_mix::MAX_CHANNELS);
    let chans = if track.kind == TrackKind::Master { 2 } else { track.channels().min(2) };
    if all_chans > 2 {
        // One bar per channel, labelled (L R C LFE …).
        let peaks = snap.and_then(|s| s.tracks.get(&id)).map(|m| m.peaks.as_slice()).unwrap_or(&[]);
        let lv = channel_levels(ui, ("mc_meter", id.0), peaks, all_chans);
        let labels: Vec<String> = (0..all_chans).map(|c| strip_fmt.channel_label(c)).collect();
        multi_meter(ui, mr, &lv, &labels, md.clip);
    } else if chans >= 2 {
        let mw = mr.width() / 2.0 - 1.0;
        meter(ui, Rect::from_min_size(mr.min, vec2(mw, mr.height())), md.level[0], md.hold[0], md.clip);
        meter(ui, Rect::from_min_size(pos2(mr.min.x + mw + 2.0, mr.min.y), vec2(mw, mr.height())), md.level[1], md.hold[1], md.clip);
    } else {
        meter(ui, mr, md.level[0], md.hold[0], md.clip);
    }
    if md.gr > 0.1 {
        ui.painter().text(pos2(mr.center().x, mr.min.y - 3.0), Align2::CENTER_BOTTOM, format!("-{:.0}", md.gr), regular(8.0), t.meter_yellow);
    }
    // Volume readout and name.
    let vr = Rect::from_min_size(pos2(x0 + 4.0, r.max.y - bottom_h + 4.0), vec2(inner_w - 8.0, 15.0));
    inset(ui.painter(), vr, t.counter_bg);
    let (_, entered) = editable_readout(
        ui,
        vr,
        ui.id().with(("volume-value", id.0)),
        &db_text(track.mixer.volume_db),
        &format!("{:.1}", track.mixer.volume_db),
        parse_db,
        0.0,
    );
    if let Some(db) = entered {
        let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &format!("fader:{}", id.0));
    }
    let kind = match track.kind {
        TrackKind::Audio => "audio",
        TrackKind::Aux => "aux",
        TrackKind::Master => "master",
        TrackKind::Midi => "midi",
        TrackKind::Instrument => "inst",
        TrackKind::Vca => "vca",
        TrackKind::Folder => "folder",
        TrackKind::Video => "video",
    };
    ui.painter().text(pos2(vr.center().x, vr.max.y + 9.0), Align2::CENTER_CENTER, kind, regular(9.5), t.text_dim);
    let nr = Rect::from_min_size(pos2(x0, r.max.y - 26.0), vec2(inner_w, 18.0));
    ui.painter().rect(
        nr,
        CornerRadius::same(2),
        if selected { t.name_field_sel } else { t.name_field },
        Stroke::new(1.0, Color32::BLACK),
        StrokeKind::Inside,
    );
    ui.painter().with_clip_rect(nr).text(nr.center(), Align2::CENTER_CENTER, &track.name, bold(12.0), t.text_dark);
    let nresp = ui.interact(nr, ui.id().with(("strip_name", id.0)), Sense::click());
    if nresp.clicked() {
        crate::edit_window::click_track(app, id, ui.input(|i| i.modifiers), |_, t| !t.hidden && t.kind != TrackKind::Video);
    }
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    }
    nresp.context_menu(|ui| crate::edit_window::track_context_menu(app, ui, id));
    if app.ui.mix_views.iter().any(|v| v == "color") {
        ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x, r.max.y - 5.0), vec2(w, 5.0)), 0.0, rgb(track.color));
    }
    ui.painter().line_segment([pos2(r.max.x, r.min.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// An instrument track's INSTRUMENT row: click opens a hosted instrument's plugin window,
/// right-click (or click, when none is set) picks another instrument. A built-in's parameters
/// open from that menu. Dropping an audio file on it loads the sampler.
pub(crate) fn instrument_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect) {
    let t = Tokens::current();
    let (id, slot) = (track.id, soundcraft_mix::INSTRUMENT_SLOT);
    let ins = track.instrument.as_ref();
    let info = ins.and_then(|i| plugin_info(&i.plugin));
    let resp = ui.interact(r, ui.id().with(("instrument", id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Instrument"));
    match info {
        Some(info) => used_slot(ui.painter(), r, t.slot_face, Mark::Dot, info.short_name, t.text, None),
        None => empty_slot(ui.painter(), r, resp.hovered(), Mark::Dot),
    }
    let hosted = ins.is_some_and(|i| soundcraft_mix::is_third_party(&i.plugin));
    if resp.clicked() && hosted && !app.ui.plugin_windows.contains(&(id, slot)) {
        app.ui.plugin_windows.push((id, slot));
    }
    let menu_resp = if hosted { resp.clone().on_hover_text("Click: open instrument · right-click: change") } else { resp.clone() };
    let popup = if hosted { egui::Popup::context_menu(&menu_resp) } else { egui::Popup::menu(&menu_resp) };
    let popup = popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    popup.show(|ui| instrument_menu(app, ui, id));
    crate::sampler_window::drop_target(app, ui.ctx(), r, id);
}

/// What a plugin slot shows at its left: a dot for inserts, the send's letter, or an arrow for
/// a send on a short track.
#[derive(Clone, Copy)]
enum Mark {
    Dot,
    Letter(char),
    Arrow,
}

/// Slots narrower than this are a short track's squares: a mark, no name.
const COMPACT_SLOT: f32 = 30.0;
const NAME_SIZE: f32 = 9.5;

fn draw_mark(p: &egui::Painter, r: Rect, mark: Mark, color: Color32) {
    let at = r.min + vec2(4.75, 4.75);
    match mark {
        Mark::Dot => {
            p.circle_filled(at, 2.0, color);
        }
        Mark::Letter(l) => {
            p.text(pos2(r.min.x + 2.5, r.center().y), Align2::LEFT_CENTER, l, bold(NAME_SIZE), color);
        }
        Mark::Arrow => {
            let at = at + vec2(0.0, 0.5);
            p.rect_filled(Rect::from_min_max(at + vec2(-2.5, -1.5), at + vec2(0.0, 1.5)), 0.0, color);
            p.add(egui::Shape::convex_polygon(vec![at + vec2(-0.5, -3.5), at + vec2(2.75, 0.0), at + vec2(-0.5, 3.5)], color, Stroke::NONE));
        }
    }
}

fn empty_slot(p: &egui::Painter, r: Rect, hovered: bool, mark: Mark) {
    let t = Tokens::current();
    p.rect_filled(r, 2.0, if hovered { shade(t.slot_empty, 0.06) } else { t.slot_empty });
    let color = if matches!(mark, Mark::Letter(_)) { t.slot_mark.lerp_to_gamma(t.text, 0.55) } else { t.slot_mark };
    draw_mark(p, r, mark, color);
}

/// A used slot, a button a point bigger than the empty cell: the mark, a groove, the name
/// centred, and for sends a level box at the right. It doesn't brighten on hover, which would
/// cost its name contrast.
fn used_slot(p: &egui::Painter, r: Rect, faces: [Color32; 2], mark: Mark, name: &str, color: Color32, level: Option<f32>) {
    let t = Tokens::current();
    let outer = r.expand(1.0);
    let top = raised_key(p, outer, faces, false);
    draw_mark(p, r, mark, if matches!(mark, Mark::Dot) { t.slot_mark.lerp_to_gamma(color, 0.35) } else { color });
    if r.width() < COMPACT_SLOT {
        return;
    }
    let groove_x = r.min.x + 10.5;
    groove(p, groove_x, r.y_range(), top);
    let mut text = r.with_min_x(groove_x + 1.5);
    if let Some(k) = level {
        let well = level_icon(r);
        p.rect_filled(well, 1.0, Color32::BLACK);
        let inner = well.shrink(1.0);
        p.rect_filled(inner, 0.0, Color32::from_rgb(36, 38, 8));
        p.rect_filled(inner.with_min_y(inner.max.y - inner.height() * k.clamp(0.0, 1.0)), 0.0, Color32::from_rgb(51, 49, 11));
        text.max.x = well.min.x - 1.0;
    }
    let g = p.layout_no_wrap(name.to_string(), bold(NAME_SIZE), color);
    let x = if g.size().x <= text.width() { text.center().x - g.size().x / 2.0 } else { text.min.x + 1.0 };
    p.with_clip_rect(text).galley(pos2(x, text.center().y - g.size().y / 2.0), g, color);
}

/// A used send's level icon: the small fader at the right of its slot.
fn level_icon(r: Rect) -> Rect {
    let outer = r.expand(1.0);
    Rect::from_min_max(pos2(outer.max.x - 9.0, outer.min.y + 2.0), pos2(outer.max.x - 4.0, outer.max.y - 2.0))
}

pub(crate) fn insert_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let ins = track.mixer.inserts.get(slot).cloned().flatten();
    let resp = ui.interact(r, ui.id().with(("ins", id.0, slot)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Insert {}", (b'A' + slot as u8) as char)));
    let info = ins.as_ref().and_then(|i| plugin_info(&i.plugin));
    let name = ins.as_ref().map(|i| info.map_or(i.plugin.as_str(), |p| p.short_name)).unwrap_or_default();
    match &ins {
        None => empty_slot(ui.painter(), r, resp.hovered(), Mark::Dot),
        Some(i) => {
            let faces = if i.bypass { t.slot_bypassed } else { t.slot_face };
            let color = if info.is_some() { t.text } else { t.text_dim };
            used_slot(ui.painter(), r, faces, Mark::Dot, name, color, None);
        }
    }
    if resp.clicked() && ins.is_some() {
        if ui.input(|i| i.modifiers.command) {
            let _ = app.run("mix.insert_bypass", json!({"track": id.0, "slot": slot}));
        } else if !app.ui.plugin_windows.contains(&(id, slot)) {
            app.ui.plugin_windows.push((id, slot));
        }
    }
    let cmd = if ui.ctx().os().is_mac() { "⌘" } else { "Ctrl+" };
    let tip = format!("{name}: click to open · {cmd}click to bypass · right-click to change");
    let menu_resp = if ins.is_none() { resp.clone() } else { resp.clone().on_hover_text(tip) };
    let popup = if ins.is_none() { egui::Popup::menu(&menu_resp) } else { egui::Popup::context_menu(&menu_resp) };
    let popup = popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    popup.show(|ui| plugin_menu(app, ui, id, slot, ins.is_some()));
}

/// An insert's menu: a search field that takes typing at once, then (until something is typed)
/// Bypass and removal and the plugins by category and format.
pub fn plugin_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, occupied: bool) {
    let search = MenuSearch::show(ui, egui::Id::new(("plugin-search", id.0, slot)), "Search plugins…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, false) {
            pick_insert(app, ui, id, slot, &plugin);
        }
        return;
    }
    if occupied {
        if ui.button("Bypass").clicked() {
            run_pick(app, ui, "mix.insert_bypass", json!({"track": id.0, "slot": slot}));
        }
        if ui.button("no insert").clicked() {
            run_pick(app, ui, "mix.insert_remove", json!({"track": id.0, "slot": slot}));
        }
        ui.separator();
    }
    let mut cats: Vec<soundcraft_dsp::Category> = Vec::new();
    for p in soundcraft_dsp::plugins() {
        if !p.is_instrument && !cats.contains(&p.category) {
            cats.push(p.category);
        }
    }
    for c in cats {
        ui.menu_button(format!("{c:?}"), |ui| {
            for p in soundcraft_dsp::plugins().iter().filter(|p| p.category == c && !p.is_instrument) {
                if ui.button(p.name).clicked() {
                    pick_insert(app, ui, id, slot, p.id);
                }
            }
        });
    }
    if let Some(plugin) = hosted_menus(ui, false) {
        pick_insert(app, ui, id, slot, &plugin);
    }
}

/// The instrument picker of an instrument track: a search field that takes typing at once, then
/// (until something is typed) its instrument's editor and the built-in, CLAP, VST3 and Audio Units
/// instruments.
pub fn instrument_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let slot = soundcraft_mix::INSTRUMENT_SLOT;
    let search = MenuSearch::show(ui, egui::Id::new(("instrument-search", id.0)), "Search instruments…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, true) {
            run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
        }
        return;
    }
    let current = app.engine.session().track(id).and_then(|t| t.instrument.clone());
    if let Some(info) = current.as_ref().and_then(|i| plugin_info(&i.plugin)) {
        ui.label(egui::RichText::new(info.name).strong());
        if current.as_ref().is_some_and(|i| !soundcraft_mix::is_third_party(&i.plugin)) && ui.button("Edit Parameters…").clicked() {
            if !app.ui.plugin_windows.contains(&(id, slot)) {
                app.ui.plugin_windows.push((id, slot));
            }
            ui.close();
        }
        if let Some(p) = &app.player
            && p.has_editor(id, slot)
        {
            if p.editor_open(id, slot) {
                if ui.button("Close Plugin Editor").clicked() {
                    p.close_editor(id, slot);
                    ui.close();
                }
            } else if ui.button("Open Plugin Editor").clicked() {
                if let Err(e) = p.open_editor(id, slot) {
                    app.ui.status = format!("{}: {e}", info.name);
                }
                ui.close();
            }
        }
        ui.separator();
    }
    let mut picked = None;
    ui.menu_button("Built-in", |ui| {
        for p in soundcraft_dsp::plugins().iter().filter(|p| p.is_instrument) {
            if ui.button(p.name).clicked() {
                picked = Some(p.id.to_string());
            }
        }
    });
    if let Some(p) = hosted_menus(ui, true) {
        picked = Some(p);
    }
    if let Some(plugin) = picked {
        run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
    }
}

/// Inserts the plugin picked in an insert's menu and, as DAWs do, opens it once the audio engine has
/// it (`SoundApp::open_inserted`).
fn pick_insert(app: &mut SoundApp, ui: &Ui, id: TrackId, slot: usize, plugin: &str) {
    match app.run("mix.insert", json!({"track": id.0, "slot": slot, "plugin": plugin})) {
        Ok(_) => app.pending_open = Some((id, slot)),
        Err(e) => app.ui.status = e,
    }
    ui.close();
}

/// Runs a menu's pick, shows its error in the status line and closes the menu.
fn run_pick(app: &mut SoundApp, ui: &Ui, cmd: &str, params: serde_json::Value) {
    if let Err(e) = app.run(cmd, params) {
        app.ui.status = e;
    }
    ui.close();
}

/// The plugins whose name or vendor holds the search text: the built-in ones, then Audio Units
/// (listing them creates no instances) and whichever of CLAP and VST3 have been scanned, with a
/// button to scan each of the others. Returns the id picked.
fn plugin_search(ui: &mut Ui, search: &MenuSearch, instruments: bool) -> Option<String> {
    let mut built_in: Vec<&soundcraft_dsp::PluginInfo> =
        soundcraft_dsp::plugins().iter().copied().filter(|p| p.is_instrument == instruments && search.matches(p.name)).collect();
    built_in.sort_by_cached_key(|p| p.name.to_lowercase());
    let (clap, vst3) = (soundcraft_clap_host::cached(), soundcraft_vst3_host::cached());
    let mut hosted: Vec<(MenuPlugin, &str)> = Vec::new();
    for d in clap.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "CLAP"));
    }
    for d in vst3.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "VST3"));
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }, "AU"));
    }
    hosted.retain(|(p, _)| search.matches(&p.name) || search.matches(&p.vendor));
    hosted.sort_by_cached_key(|(p, format)| (p.name.to_lowercase(), *format));
    let n = built_in.len() + hosted.len();
    let picked = search.rows(ui, n, 320.0, |ui, i, highlighted| match built_in.get(i) {
        Some(p) => menu_row(ui, highlighted, p.name),
        None => {
            let (p, format) = &hosted[i - built_in.len()];
            let label = if p.vendor.is_empty() { format!("{} ({format})", p.name) } else { format!("{} — {} ({format})", p.name, p.vendor) };
            menu_row(ui, highlighted, &label)
        }
    });
    if n == 0 {
        ui.weak("No matches");
    }
    if clap.is_none() || vst3.is_none() {
        ui.separator();
        ui.horizontal(|ui| {
            if clap.is_none() && ui.small_button("Scan CLAP").clicked() {
                soundcraft_clap_host::scan();
            }
            if vst3.is_none() && ui.small_button("Scan VST3").clicked() {
                soundcraft_vst3_host::scan();
            }
        });
    }
    picked.map(|i| built_in.get(i).map_or_else(|| hosted[i - built_in.len()].0.id.clone(), |p| p.id.to_string()))
}

/// A picker menu's search field. It has the keyboard as soon as the menu opens and starts empty
/// each time; Up/Down move the highlight through the matches and Enter picks it.
struct MenuSearch {
    id: egui::Id,
    query: String,
    highlight: usize,
    keys: [bool; 3],
}

impl MenuSearch {
    fn show(ui: &mut Ui, id: egui::Id, hint: &str) -> Self {
        let pass = ui.ctx().cumulative_pass_nr();
        // Kept only while the menu stays open: one not shown last pass has just opened.
        let kept = ui.data(|d| d.get_temp::<(String, usize, u64)>(id)).filter(|s| s.2 + 1 >= pass);
        let opened = kept.is_none();
        let (mut text, mut highlight, _) = kept.unwrap_or_default();
        let field = ui.add(egui::TextEdit::singleline(&mut text).id(id.with("field")).hint_text(hint).return_key(None));
        if opened {
            field.request_focus();
        }
        if field.changed() {
            highlight = 0;
        }
        // egui's focus, as typing uses: `has_focus` also wants the window focused, which scripted input lacks.
        let focused = ui.memory(|m| m.has_focus(field.id));
        let keys = [egui::Key::ArrowUp, egui::Key::ArrowDown, egui::Key::Enter].map(|k| focused && ui.input(|i| i.key_pressed(k)));
        ui.data_mut(|d| d.insert_temp(id, (text.clone(), highlight, pass)));
        Self { id, query: text.trim().to_lowercase(), highlight, keys }
    }

    fn typing(&self) -> bool {
        !self.query.is_empty()
    }

    fn matches(&self, text: &str) -> bool {
        text.to_lowercase().contains(&self.query)
    }

    /// `n` matches in a scrolling list, each drawn by `row` (told whether it is highlighted);
    /// returns the one clicked, or the highlighted one on Enter.
    fn rows(&self, ui: &mut Ui, n: usize, max_height: f32, mut row: impl FnMut(&mut Ui, usize, bool) -> bool) -> Option<usize> {
        let [up, down, enter] = self.keys;
        let typing = self.typing();
        let highlight = match (up, down) {
            (_, true) => self.highlight + 1,
            (true, _) => self.highlight.saturating_sub(1),
            _ => self.highlight,
        }
        .min(n.saturating_sub(1));
        ui.data_mut(|d| d.get_temp_mut_or_default::<(String, usize, u64)>(self.id).1 = highlight);
        let mut picked = (typing && enter && n > 0).then_some(highlight);
        let row_h = ui.spacing().interact_size.y;
        let step = row_h + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical().id_salt(self.id.with("rows")).max_height(max_height).show_rows(ui, row_h, n, |ui, range| {
            if typing && (up || down) {
                let top = ui.max_rect().top() + (highlight as f32 - range.start as f32) * step;
                ui.scroll_to_rect(Rect::from_min_size(pos2(ui.max_rect().left(), top), vec2(1.0, row_h)), None);
            }
            for i in range {
                if row(ui, i, typing && i == highlight) {
                    picked = Some(i);
                }
            }
        });
        picked
    }
}

fn menu_row(ui: &mut Ui, selected: bool, text: &str) -> bool {
    let text = if selected { egui::RichText::new(text).color(Color32::WHITE) } else { egui::RichText::new(text) };
    ui.add(egui::Button::selectable(selected, text).min_size(vec2(0.0, ui.spacing().interact_size.y))).clicked()
}

/// A hosted plugin as the plugin menus list it.
struct MenuPlugin {
    id: String,
    name: String,
    vendor: String,
}

/// The CLAP, VST3 and (on macOS) Audio Units submenus, of effects or of instruments; returns the
/// id picked. Each format is scanned the first time its submenu opens, then cached by its host.
fn hosted_menus(ui: &mut Ui, instruments: bool) -> Option<String> {
    let mut picked = None;
    ui.menu_button("CLAP", |ui| {
        let list = soundcraft_clap_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
        if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No CLAP plugins found") {
            picked = Some(p);
        }
    });
    ui.menu_button("VST3", |ui| {
        let list = soundcraft_vst3_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
        if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No VST3 plugins found") {
            picked = Some(p);
        }
    });
    if cfg!(target_os = "macos") {
        ui.menu_button("Audio Units", |ui| {
            let list = soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments);
            if let Some(p) = vendor_menus(ui, list.map(|d| MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }).collect(), "No Audio Units found")
            {
                picked = Some(p);
            }
        });
    }
    picked
}

/// One submenu per vendor (as Pro Tools lists plug-ins by manufacturer), each list scrolling
/// within the screen so that every plugin stays reachable however many are installed.
fn vendor_menus(ui: &mut Ui, mut list: Vec<MenuPlugin>, none: &str) -> Option<String> {
    if list.is_empty() {
        ui.label(none);
        return None;
    }
    list.sort_by_cached_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
    let mut vendors: Vec<&str> = list.iter().map(|p| p.vendor.as_str()).collect();
    vendors.dedup();
    let max_h = (ui.ctx().content_rect().height() - 80.0).max(160.0);
    let mut picked = None;
    egui::ScrollArea::vertical().id_salt("vendors").max_height(max_h).show(ui, |ui| {
        for v in vendors {
            ui.menu_button(if v.is_empty() { "Unknown vendor" } else { v }, |ui| {
                egui::ScrollArea::vertical().id_salt("plugins").max_height(max_h).show(ui, |ui| {
                    for p in list.iter().filter(|p| p.vendor == v) {
                        if ui.button(&p.name).clicked() {
                            picked = Some(p.id.clone());
                        }
                    }
                });
            });
        }
    });
    picked
}

/// A built-in plugin's description, else a hosted CLAP (`clap:<id>`), VST3 (`vst3:<class id>`) or
/// Audio Unit (`au:<type>:<subtype>:<manufacturer>`) plugin's.
pub fn plugin_info(id: &str) -> Option<&'static soundcraft_dsp::PluginInfo> {
    soundcraft_dsp::plugin_info(id)
        .or_else(|| soundcraft_clap_host::plugin_info(id))
        .or_else(|| soundcraft_vst3_host::plugin_info(id))
        .or_else(|| soundcraft_au_host::plugin_info(id))
}

pub(crate) fn send_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let snd = track.mixer.sends.get(slot).cloned().flatten();
    let compact = r.width() < COMPACT_SLOT;
    // As in Pro Tools, only the fader icon opens a send's level; the rest of the slot picks its
    // destination. A short track's square has no icon, so all of it is the level.
    let level_r = if compact { r } else { level_icon(r).expand2(vec2(3.0, 0.0)).with_min_y(r.min.y).with_max_y(r.max.y) };
    let route_r = Rect::from_min_max(r.min, pos2((level_r.min.x - 1.0).max(r.min.x), r.max.y));
    let route_resp = ui.interact(if snd.is_some() { route_r } else { r }, ui.id().with(("snd-route", id.0, slot)), Sense::click());
    route_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Send {}", (b'A' + slot as u8) as char)));
    let mark = if compact { Mark::Arrow } else { Mark::Letter((b'a' + slot as u8) as char) };
    match &snd {
        Some(s) => {
            let name = route_name(app, &s.target);
            let level = soundcraft_model::fader_db_to_pos(s.level_db);
            let color = if s.mute { t.mute } else { t.text };
            used_slot(ui.painter(), r, t.slot_face, mark, &name, color, Some(level));
            let which = (b'A' + slot as u8) as char;
            let shown = app.ui.send_window == Some((id, slot));
            let level_resp = ui.interact(level_r, ui.id().with(("send-value", id.0, slot)), Sense::click_and_drag());
            level_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Send {which} level")));
            if shown {
                ui.painter().rect_stroke(level_r.shrink(0.5), 1.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
            }
            if level_resp.clicked() {
                if ui.input(|i| i.modifiers.alt) {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
                } else {
                    app.ui.send_window = (!shown).then_some((id, slot));
                }
            }
            if level_resp.dragged() {
                // Use this frame's pointer movement. `Response::drag_delta()` is cumulative, so
                // applying it to the already-updated value made trackpad adjustment accelerate.
                let (dy, fine) = ui.input(|i| (i.pointer.delta().y, i.modifiers.shift));
                let db = dragged_send_db(s.level_db, dy, fine);
                let _ =
                    app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
            }
            let _ = level_resp.clone().on_hover_text(format!(
                "Send {which} to {name}: {} dB\nClick for the send fader · drag vertically to adjust · Shift-drag for fine control · Alt-click for 0 dB · right-click for options",
                db_text(s.level_db)
            ));
            level_resp.context_menu(|ui| send_options_menu(app, ui, id, slot, s));
        }
        None => empty_slot(ui.painter(), r, ui.rect_contains_pointer(r), mark),
    }
    if let Some(send) = &snd {
        egui::Popup::menu(&route_resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(250.0);
            ui.horizontal(|ui| {
                ui.strong(format!("Send {}", (b'A' + slot as u8) as char));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(route_name(app, &send.target)).color(Tokens::current().counter_text));
                });
            });
            ui.label(egui::RichText::new("DESTINATION").size(8.0).color(Tokens::current().text_dim));
            send_route_menu(app, ui, id, slot, Some(&send.target));
            ui.separator();
            if ui.button("Remove send").clicked() {
                let _ = app.run("mix.send_remove", json!({"track": id.0, "slot": slot}));
                ui.close();
            }
        });
    } else {
        egui::Popup::menu(&route_resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(250.0);
            ui.strong(format!("Send {} · Choose destination", (b'A' + slot as u8) as char));
            send_route_menu(app, ui, id, slot, None);
        });
    }
}

/// The Send window, as in Pro Tools: one send's fader, level, pan, Mute and Pre-fader. Clicking a
/// send's level opens it, moves it to that send, or closes it, and so does Escape.
pub(crate) fn send_window(app: &mut SoundApp, ctx: &egui::Context) {
    // egui closes menus and drops text focus on Escape before this runs, so whether this Escape
    // was theirs comes from the last frame. A dialog or the search on top takes it too.
    let busy_id = egui::Id::new("send_window_escape");
    let busy = ctx.data(|d| d.get_temp(busy_id)).unwrap_or(false) || app.dialogs.open.is_some() || app.ui.show_search;
    if !busy && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.send_window = None;
    }
    send_window_contents(app, ctx);
    let busy = egui::Popup::is_any_open(ctx) || ctx.text_edit_focused();
    ctx.data_mut(|d| d.insert_temp(busy_id, busy));
}

fn send_window_contents(app: &mut SoundApp, ctx: &egui::Context) {
    let Some((id, slot)) = app.ui.send_window else { return };
    let Some((name, send)) = app.engine.session().track(id).and_then(|t| Some((t.name.clone(), t.mixer.sends.get(slot).cloned().flatten()?))) else {
        app.ui.send_window = None;
        return;
    };
    let t = Tokens::current();
    let mut open = true;
    egui::Window::new(format!("{name} · Send {}", (b'A' + slot as u8) as char))
        .id(egui::Id::new("send_window"))
        .open(&mut open)
        .auto_sized()
        .collapsible(false)
        .default_pos(ctx.pointer_latest_pos().map_or(pos2(200.0, 200.0), |p| p + vec2(24.0, -160.0)))
        .show(ctx, |ui| {
            ui.set_width(150.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(route_name(app, &send.target)).color(t.counter_text));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(((ui.available_width() - 62.0) / 2.0).max(0.0));
                    if text_toggle(ui, vec2(36.0, 18.0), "PRE", send.pre_fader, t.accent, "Pre-fader").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": !send.pre_fader}));
                    }
                    if text_toggle(ui, vec2(22.0, 18.0), "M", send.mute, t.mute, "Mute send").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": !send.mute}));
                    }
                });
                let mut pan = send.pan;
                pan_knob(ui, 32.0, &mut pan, "Send pan (drag; double-click centres)");
                if (pan - send.pan).abs() > f32::EPSILON {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "pan": pan}),
                        &format!("send-pan:{}:{slot}", id.0),
                    );
                }
                ui.label(egui::RichText::new(pan_text(send.pan)).color(t.counter_text).small());
                let (fr, fresp) = ui.allocate_exact_size(vec2(56.0, 220.0), Sense::hover());
                fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Send level"));
                if let Some(db) = fader(ui, fr, send.level_db, ui.id().with("send_fader")) {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "db": db}),
                        &format!("send:{}:{slot}", id.0),
                    );
                }
                let (rr, _) = ui.allocate_exact_size(vec2(64.0, 15.0), Sense::hover());
                ui.painter().rect_filled(rr, 1.0, t.counter_bg);
                let shown = db_text(send.level_db);
                let (_, typed) = editable_readout(ui, rr, ui.id().with("send_level_value"), &shown, &format!("{:.1}", send.level_db), parse_db, 0.0);
                if let Some(db) = typed {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
                }
            });
        });
    if !open {
        app.ui.send_window = None;
    }
}

/// Secondary controls live behind the amount readout, separate from the bus selector like Logic.
fn send_options_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, send: &soundcraft_model::SendSlot) {
    ui.set_min_width(230.0);
    ui.strong(format!("Send {} · {}", (b'A' + slot as u8) as char, route_name(app, &send.target)));
    let mut db = send.level_db.max(-60.0);
    if ui.add(egui::Slider::new(&mut db, -60.0..=12.0).text("Level").suffix(" dB")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
    }
    send_level_scale(ui, send.level_db);
    ui.horizontal(|ui| {
        for (label, delta) in [("−3", -3.0), ("−1", -1.0), ("+1", 1.0), ("+3", 3.0)] {
            if ui.small_button(label).clicked() {
                let db = (send.level_db.max(-60.0) + delta).clamp(-60.0, 12.0);
                let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
            }
        }
        if ui.small_button("0 dB").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
        }
        if ui.small_button("−∞").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": -144.0}));
        }
    });
    let mut pan = send.pan;
    if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).text("Pan")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "pan": pan}), &format!("send-pan:{}:{slot}", id.0));
    }
    ui.horizontal(|ui| {
        let mut mute = send.mute;
        if crate::widgets::checkbox(ui, &mut mute, "Mute").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": mute}));
        }
        let mut pre = send.pre_fader;
        if crate::widgets::checkbox(ui, &mut pre, "Pre-fader").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": pre}));
        }
    });
    if let Route::Bus(bus) = send.target
        && let Some(aux) =
            app.engine.session().tracks.iter().find(|t| t.kind == TrackKind::Aux && t.mixer.input == Route::Bus(bus)).map(|t| (t.id, t.name.clone()))
        && ui.button("Rename bus…").clicked()
    {
        app.dialogs.open_rename_track(aux.0, &aux.1);
        ui.close();
    }
}

/// A send level after a vertical drag of `dy` points: along the fader's taper, like the level bar,
/// so it runs smoothly down to −∞ and back. About 0.15 dB a point near 0 dB (0.02 when fine).
fn dragged_send_db(db: f32, dy: f32, fine: bool) -> f32 {
    let per_point = if fine { 0.0004 } else { 0.003 };
    soundcraft_model::fader_pos_to_db(soundcraft_model::fader_db_to_pos(db) - dy * per_point)
}

/// Paint a value until clicked, then replace it with a focused text field. Enter or clicking away
/// commits a valid value; Escape cancels, and so does the field losing focus while hidden.
/// Alt-click returns `reset`. The interaction response remains available for dragging.
fn editable_readout(
    ui: &mut Ui,
    r: Rect,
    id: egui::Id,
    display: &str,
    edit_initial: &str,
    parse: fn(&str) -> Option<f32>,
    reset: f32,
) -> (Option<egui::Response>, Option<f32>) {
    let t = Tokens::current();
    let active_id = id.with("active");
    let text_id = id.with("text");
    let mut active: bool = ui.data(|d| d.get_temp(active_id)).unwrap_or(false);
    let mut started = false;
    let mut response = None;
    let mut value = None;
    if !active {
        let resp = ui.interact(r, id, Sense::click_and_drag());
        let alt = ui.input(|i| i.modifiers.alt);
        if resp.clicked() && alt {
            value = Some(reset);
        }
        if resp.clicked() && !alt && !resp.double_clicked() {
            active = true;
            started = true;
            ui.data_mut(|d| d.insert_temp(text_id, edit_initial.to_string()));
        } else {
            ui.painter().with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, display, regular(9.0), t.counter_text);
        }
        response = Some(resp);
    }
    if active {
        let mut text: String = ui.data(|d| d.get_temp(text_id)).unwrap_or_else(|| edit_initial.to_string());
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(r));
        child.visuals_mut().override_text_color = Some(t.counter_text);
        let edit = egui::TextEdit::singleline(&mut text)
            .id(id.with("editor"))
            .frame(egui::Frame::NONE)
            .horizontal_align(egui::Align::Center)
            .font(regular(9.0))
            .desired_width(r.width());
        let edit_resp = child.add_sized(r.size(), edit);
        if started {
            edit_resp.request_focus();
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if escape {
            active = false;
        } else if (enter && edit_resp.has_focus()) || edit_resp.lost_focus() {
            if let Some(parsed) = parse(&text) {
                value = Some(parsed);
                active = false;
            } else if edit_resp.lost_focus() {
                active = false;
            }
        } else if !started && !edit_resp.has_focus() {
            active = false;
        }
        ui.data_mut(|d| d.insert_temp(text_id, text));
    }
    ui.data_mut(|d| d.insert_temp(active_id, active));
    (response, value)
}

fn parse_db(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace("db", "").replace(',', ".");
    if matches!(cleaned.trim(), "-inf" | "-infinity" | "off") {
        return Some(-144.0);
    }
    cleaned.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(-144.0, 12.0))
}

fn parse_pan(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace(',', ".");
    let (sign, number) = if let Some(v) = cleaned.strip_prefix('<').or_else(|| cleaned.strip_prefix('l')) {
        (-1.0, v)
    } else if let Some(v) = cleaned.strip_suffix('>').or_else(|| cleaned.strip_prefix('r')) {
        (1.0, v)
    } else {
        (1.0, cleaned.as_str())
    };
    let raw = number.trim().parse::<f32>().ok()? * sign;
    let value = if number.contains('.') && raw.abs() <= 1.0 { raw } else { raw / 100.0 };
    value.is_finite().then_some(value.clamp(-1.0, 1.0))
}

/// A compact calibrated scale for a send. The marks use the same taper as the strip's level bar.
fn send_level_scale(ui: &mut Ui, db: f32) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 25.0), Sense::hover());
    let line_y = r.min.y + 4.0;
    ui.painter().line_segment([pos2(r.min.x, line_y), pos2(r.max.x, line_y)], Stroke::new(1.0, t.border));
    for (i, mark) in [-60.0f32, -24.0, -10.0, 0.0, 6.0, 12.0].into_iter().enumerate() {
        let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(mark);
        ui.painter().line_segment([pos2(x, line_y - 3.0), pos2(x, line_y + 3.0)], Stroke::new(1.0, t.text_dim));
        let align = if i == 0 {
            Align2::LEFT_TOP
        } else if i == 5 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        let label = if mark > 0.0 { format!("+{mark:.0}") } else { format!("{mark:.0}") };
        ui.painter().text(pos2(x, line_y + 5.0), align, label, regular(8.0), t.text_dim);
    }
    let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(db);
    ui.painter().line_segment([pos2(x, r.min.y), pos2(x, line_y + 3.0)], Stroke::new(2.0, t.counter_text));
}

fn send_route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, current: Option<&Route>) {
    let busses = app.engine.session().busses.clone();
    let search = MenuSearch::show(ui, egui::Id::new(("send-bus-search", id.0, slot)), "Search buses…");
    let visible: Vec<_> = busses.iter().filter(|bus| search.matches(&bus.name)).collect();
    let picked = search.rows(ui, visible.len(), 260.0, |ui, i, highlighted| {
        let current = matches!(current, Some(Route::Bus(bus_id)) if *bus_id == visible[i].id);
        menu_row(ui, highlighted || (current && !search.typing()), &visible[i].name)
    });
    if let Some(i) = picked {
        let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": visible[i].name}));
        ui.close();
    }
    ui.separator();
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.iter().any(|bus| bus.name.eq_ignore_ascii_case(&format!("Bus {n}")))
            || app.engine.session().track_by_name(&format!("Bus {n}")).is_some()
        {
            n += 1;
        }
        if app.run("mix.new_bus", json!({"name": format!("Bus {n}")})).is_ok() {
            let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": format!("Bus {n}")}));
        }
        ui.close();
    }
}

fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    if ui.button(if input { "no input" } else { "no output" }).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "none"}));
        ui.close();
    }
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    if !input && ui.button(main_name).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "main"}));
        ui.close();
    }
    if input {
        for i in 1..=8 {
            if ui.button(format!("In {i}")).clicked() {
                let _ = app.run(cmd, json!({"track": id.0, key: format!("In {i}")}));
                ui.close();
            }
        }
    }
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    ui.strong("Bus");
    let search = MenuSearch::show(ui, egui::Id::new(("route-bus-search", id.0, input)), "Search buses…");
    let visible: Vec<&String> = busses.iter().filter(|b| search.matches(b)).collect();
    if let Some(i) = search.rows(ui, visible.len(), 420.0, |ui, i, highlighted| menu_row(ui, highlighted, visible[i])) {
        let _ = app.run(cmd, json!({"track": id.0, key: visible[i]}));
        ui.close();
    }
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.contains(&format!("Bus {n}")) || app.engine.session().track_by_name(&format!("Bus {n}")).is_some() {
            n += 1;
        }
        let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{dragged_send_db, editable_readout, parse_db, parse_pan};
    use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};

    #[test]
    fn typed_db_accepts_console_notation_and_clamps() {
        assert_eq!(parse_db("-6 dB"), Some(-6.0));
        assert_eq!(parse_db("+3,5"), Some(3.5));
        assert_eq!(parse_db("off"), Some(-144.0));
        assert_eq!(parse_db("99"), Some(12.0));
        assert_eq!(parse_db("loud"), None);
    }

    #[test]
    fn typed_pan_accepts_percent_console_and_fractional_values() {
        assert_eq!(parse_pan("-50"), Some(-0.5));
        assert_eq!(parse_pan("<35"), Some(-0.35));
        assert_eq!(parse_pan("35>"), Some(0.35));
        assert_eq!(parse_pan("R25"), Some(0.25));
        assert_eq!(parse_pan("0.5"), Some(0.5));
        assert_eq!(parse_pan("center"), None);
    }

    #[test]
    fn send_drags_follow_the_fader_taper_down_to_minus_infinity() {
        assert!((dragged_send_db(0.0, -10.0, false) - 1.44).abs() < 0.01);
        assert!((dragged_send_db(0.0, -10.0, true) - 0.192).abs() < 0.01);
        assert_eq!(dragged_send_db(0.0, 400.0, false), -144.0);
        let up = dragged_send_db(-144.0, -1.0, false);
        assert!(up > -144.0 && up < -100.0, "one point up leaves −∞ gently, got {up}");
        assert_eq!(dragged_send_db(12.0, -50.0, false), 12.0);
    }

    #[test]
    fn readouts_reset_on_alt_click_and_drop_a_field_that_lost_focus() {
        let ctx = egui::Context::default();
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 14.0));
        let click = |mods: Modifiers| {
            let press = |pressed| Event::PointerButton { pos: r.center(), button: PointerButton::Primary, pressed, modifiers: mods };
            vec![Event::ModifiersChanged(mods), Event::PointerMoved(r.center()), press(true), press(false)]
        };
        let mut time = 0.0;
        let mut frame = |events: Vec<Event>, shown: bool| {
            time += 0.5;
            let mut out = (false, None);
            let mut full = ctx.run_ui(egui::RawInput { events, time: Some(time), ..Default::default() }, |ui| {
                if shown {
                    let (resp, value) = editable_readout(ui, r, egui::Id::new("readout"), "-3.0", "-3.0", parse_db, 0.0);
                    out = (resp.is_some(), value);
                }
            });
            full.textures_delta.clear();
            out
        };
        assert_eq!(frame(vec![], true), (true, None));
        assert_eq!(frame(click(Modifiers::ALT), true), (true, Some(0.0)), "Alt-click resets");
        frame(vec![Event::ModifiersChanged(Modifiers::NONE)], true);
        frame(click(Modifiers::NONE), true);
        assert_eq!(frame(vec![], true), (false, None), "a click opens the text field");
        for _ in 0..3 {
            frame(vec![], false);
        }
        frame(vec![], true);
        assert_eq!(frame(vec![], true), (true, None), "hidden, the field lost focus, so the readout is back");
    }
}
