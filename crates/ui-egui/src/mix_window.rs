//! The Mix window: one channel strip per track.

use crate::theme::{Tokens, bold, regular, rgb};
use crate::widgets::{
    PannerEdit, PannerSpeaker, db_text, fader, meter, multi_meter, pan_knob, pan_text, rec_toggle, selector_box, surround_panner, text_toggle,
};
use crate::{SoundApp, panels};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ChannelFormat, Route, SurroundPan, Track, TrackId, TrackKind};
use soundcraft_playback::MeterSnapshot;

pub fn strip_width(narrow: bool) -> f32 {
    if narrow { 64.0 } else { 98.0 }
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    if app.ui.show_tracks_list {
        egui::Panel::left("mix_tracks_list")
            .exact_size(168.0)
            .frame(egui::Frame::NONE.fill(t.panel_bg))
            .show(ui, |ui| panels::tracks_and_groups(app, ui));
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.window_bg)).show(ui, |ui| {
        let ids: Vec<TrackId> = app.engine.session().tracks.iter().filter(|x| !x.hidden && x.kind != TrackKind::Video).map(|x| x.id).collect();
        // Per-channel peaks for multichannel meters (read once per frame).
        let snap = app
            .player
            .as_ref()
            .filter(|_| app.engine.transport.playing || app.engine.session().tracks.iter().any(|t| t.mixer.input_monitor))
            .map(|p| p.meters());
        egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                for id in ids {
                    strip(app, ui, id, snap.as_ref());
                }
            });
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

fn section_label(ui: &Ui, r: Rect, text: &str) {
    ui.painter().text(pos2(r.center().x, r.min.y + 7.0), Align2::CENTER_CENTER, text, bold(10.0), Tokens::current().section_label);
}

fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => "no output".into(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| "Out 1-2".into(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
    }
}

fn strip(app: &mut SoundApp, ui: &mut Ui, id: TrackId, snap: Option<&MeterSnapshot>) {
    let t = Tokens::current();
    let Some(track) = app.engine.session().track(id).cloned() else { return };
    let narrow = app.ui.narrow_mix;
    let w = strip_width(narrow);
    let h = ui.available_height().max(560.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let selected = app.engine.session().edit.selected_tracks.contains(&id);
    ui.painter().rect_filled(r, 0.0, if selected { t.strip_selected } else { t.strip_bg });
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
        ui.painter().rect_filled(sec, 2.0, t.strip_section);
        section_label(ui, sec, "PREAMP");
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
        ui.painter().rect_filled(sec, 2.0, t.strip_section);
        section_label(ui, sec, "INSTRUMENT");
        let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 16.0), vec2(inner_w - 4.0, 15.0));
        let slot = track.mixer.inserts.iter().position(|i| i.as_ref().is_some_and(|i| plugin_info(&i.plugin).is_some_and(|p| p.is_instrument)));
        match slot {
            _ if track.kind == TrackKind::Instrument => instrument_slot(app, ui, &track, sr),
            Some(k) => insert_slot(app, ui, &track, k, sr),
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
        ui.painter().rect_filled(sec, 2.0, t.strip_section);
        section_label(ui, sec, title);
        for k in 0..5 {
            let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0 + k as f32 * 17.0), vec2(inner_w - 4.0, 15.0));
            if is_send {
                send_slot(app, ui, &track, off + k, sr);
            } else {
                insert_slot(app, ui, &track, off + k, sr);
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
        if child.add(egui::TextEdit::multiline(&mut c).desired_width(inner_w).desired_rows(2).hint_text("comments").font(regular(10.0))).changed() {
            let _ = app.engine.execute_merged("track.comments", &json!({"track": id.0, "comments": c}), &format!("comments:{}", id.0));
        }
        y = sec.max.y + 4.0;
    }
    if all || has("io") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 16.0 + 2.0 * 18.0 + 30.0));
        ui.painter().rect_filled(sec, 2.0, t.strip_section);
        section_label(ui, sec, "I / O");
        let in_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0), vec2(inner_w - 4.0, 16.0));
        let out_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 33.0), vec2(inner_w - 4.0, 16.0));
        let input =
            if track.kind.has_playlist() && track.mixer.input == Route::None { "In 1".to_string() } else { route_name(app, &track.mixer.input) };
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(in_r));
        let resp = selector_box(&mut c, in_r.width(), in_r.height(), &input, t.text);
        egui::Popup::menu(&resp).show(|ui| route_menu(app, ui, id, true));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(out_r));
        let resp = selector_box(&mut c, out_r.width(), out_r.height(), &route_name(app, &track.mixer.output), t.text);
        egui::Popup::menu(&resp).show(|ui| route_menu(app, ui, id, false));
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
        let resp = selector_box(&mut c, am_r.width(), am_r.height(), &format!("auto {}", mode.label()), col);
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
    let gname = app.engine.session().groups.iter().find(|g| g.members.contains(&id)).map_or("no group".to_string(), |g| g.name.clone());
    let gr = Rect::from_min_size(pos2(x0 + 2.0, y), vec2(inner_w - 4.0, 16.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(gr));
    let _ = selector_box(&mut c, gr.width(), gr.height(), &gname, t.text_dim);
    y += 22.0;
    // Pan: a surround panner when the output is multichannel, else the stereo knobs.
    let out_fmt = output_format(app, &track);
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
            let kr = Rect::from_center_size(pos2(cx, y + ks * 0.5), vec2(ks, ks));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let mut v = track.mixer.pan.get(i).copied().unwrap_or(0.0);
            let before = v;
            let resp = pan_knob(&mut c, ks, &mut v, "Pan (drag; double-click centres)");
            if (v - before).abs() > f32::EPSILON {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": v, "index": i}), &format!("pan:{}:{i}", id.0));
            }
            let _ = resp;
            let pr = Rect::from_center_size(pos2(cx, y + ks + 8.0), vec2(ks + 8.0, 13.0));
            ui.painter().rect_filled(pr, 1.0, t.counter_bg);
            ui.painter().text(pr.center(), Align2::CENTER_CENTER, pan_text(v), regular(10.0), t.counter_text);
        }
        y += ks + 20.0;
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
    let mr = Rect::from_min_max(pos2(fr.max.x + 4.0, fr.min.y + 6.0), pos2(x0 + inner_w - 2.0, fr.max.y - 6.0));
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
    ui.painter().rect_filled(vr, 1.0, t.counter_bg);
    ui.painter().text(vr.center(), Align2::CENTER_CENTER, db_text(track.mixer.volume_db), regular(11.0), t.counter_text);
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
        let _ = app.run("edit.select", json!({"tracks": [id.0]}));
    }
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    }
    if app.ui.mix_views.iter().any(|v| v == "color") {
        ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x, r.max.y - 5.0), vec2(w, 5.0)), 0.0, rgb(track.color));
    }
    ui.painter().line_segment([pos2(r.max.x, r.min.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// An instrument track's INSTRUMENT row: click opens a hosted instrument's plugin window,
/// right-click (or click, when none is set) picks another instrument.
fn instrument_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect) {
    let t = Tokens::DARK;
    let (id, slot) = (track.id, soundcraft_mix::INSTRUMENT_SLOT);
    let ins = track.instrument.as_ref();
    let info = ins.and_then(|i| plugin_info(&i.plugin));
    let resp = ui.interact(r, ui.id().with(("instrument", id.0)), Sense::click());
    let fill = if info.is_some() { Color32::from_rgb(52, 62, 80) } else { t.slot_bg };
    ui.painter().rect(
        r,
        CornerRadius::same(2),
        if resp.hovered() { fill.gamma_multiply(1.3) } else { fill },
        Stroke::new(1.0, Color32::from_rgb(16, 16, 16)),
        StrokeKind::Inside,
    );
    ui.painter().with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, info.map_or("no instrument", |p| p.short_name), regular(10.5), t.text);
    let hosted = ins.is_some_and(|i| soundcraft_mix::is_third_party(&i.plugin));
    if resp.clicked() && hosted && !app.ui.plugin_windows.contains(&(id, slot)) {
        app.ui.plugin_windows.push((id, slot));
    }
    let menu_resp = if hosted { resp.clone().on_hover_text("Click: open instrument · right-click: change") } else { resp.clone() };
    let popup = if hosted { egui::Popup::context_menu(&menu_resp) } else { egui::Popup::menu(&menu_resp) };
    popup.show(|ui| instrument_menu(app, ui, id));
}

fn insert_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let ins = track.mixer.inserts.get(slot).cloned().flatten();
    let resp = ui.interact(r, ui.id().with(("ins", id.0, slot)), Sense::click());
    let fill = match &ins {
        Some(i) if i.bypass => t.insert_bypass,
        Some(_) => t.insert_on,
        None => t.slot_bg,
    };
    ui.painter().rect(
        r,
        CornerRadius::same(2),
        if resp.hovered() { fill.gamma_multiply(1.3) } else { fill },
        Stroke::new(1.0, t.slot_border),
        StrokeKind::Inside,
    );
    let label = ins.as_ref().and_then(|i| plugin_info(&i.plugin)).map_or("", |p| p.short_name);
    if label.is_empty() {
        ui.painter().circle_filled(pos2(r.min.x + 6.0, r.center().y), 1.5, t.text_dim);
    } else {
        ui.painter().with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, label, regular(10.5), t.text);
    }
    if resp.clicked() && ins.is_some() && !app.ui.plugin_windows.contains(&(id, slot)) {
        app.ui.plugin_windows.push((id, slot));
    }
    let menu_resp = if ins.is_none() { resp.clone() } else { resp.clone().on_hover_text("Click: open plugin · right-click: change") };
    let popup = if ins.is_none() { egui::Popup::menu(&menu_resp) } else { egui::Popup::context_menu(&menu_resp) };
    popup.show(|ui| plugin_menu(app, ui, id, slot, ins.is_some()));
}

pub fn plugin_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, occupied: bool) {
    if occupied {
        if ui.button("Bypass").clicked() {
            let _ = app.run("mix.insert_bypass", json!({"track": id.0, "slot": slot}));
        }
        if ui.button("no insert").clicked() {
            let _ = app.run("mix.insert_remove", json!({"track": id.0, "slot": slot}));
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
                    let _ = app.run("mix.insert", json!({"track": id.0, "slot": slot, "plugin": p.id}));
                }
            }
        });
    }
    if let Some(plugin) = hosted_menus(ui, false)
        && let Err(e) = app.run("mix.insert", json!({"track": id.0, "slot": slot, "plugin": plugin}))
    {
        app.ui.status = e;
    }
}

/// The instrument picker of an instrument track: its instrument's editor, then the built-in,
/// CLAP, VST3 and Audio Units instruments.
pub fn instrument_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let slot = soundcraft_mix::INSTRUMENT_SLOT;
    let current = app.engine.session().track(id).and_then(|t| t.instrument.clone());
    if let Some(info) = current.as_ref().and_then(|i| plugin_info(&i.plugin)) {
        ui.label(egui::RichText::new(info.name).strong());
        if let Some(p) = &app.player
            && p.has_editor(id, slot)
        {
            if p.editor_open(id, slot) {
                if ui.button("Close Plugin Editor").clicked() {
                    p.close_editor(id, slot);
                }
            } else if ui.button("Open Plugin Editor").clicked()
                && let Err(e) = p.open_editor(id, slot)
            {
                app.ui.status = format!("{}: {e}", info.name);
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
    if let Some(plugin) = picked
        && let Err(e) = app.run("mix.instrument", json!({"track": id.0, "plugin": plugin}))
    {
        app.ui.status = e;
    }
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

fn send_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let snd = track.mixer.sends.get(slot).cloned().flatten();
    let resp = ui.interact(r, ui.id().with(("snd", id.0, slot)), Sense::click_and_drag());
    let fill = if snd.is_some() { t.send_on } else { t.slot_bg };
    ui.painter().rect(r, CornerRadius::same(2), fill, Stroke::new(1.0, t.slot_border), StrokeKind::Inside);
    match &snd {
        Some(s) => {
            let name = route_name(app, &s.target);
            // Level bar.
            let k = soundcraft_model::fader_db_to_pos(s.level_db);
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(r.min.x + 1.0, r.max.y - 3.0), pos2(r.min.x + 1.0 + (r.width() - 2.0) * k, r.max.y - 1.0)),
                0.0,
                t.counter_text,
            );
            ui.painter().with_clip_rect(r).text(
                r.center() - vec2(0.0, 1.0),
                Align2::CENTER_CENTER,
                name,
                regular(10.0),
                if s.mute { t.mute } else { t.text },
            );
            if resp.dragged() {
                let db = (s.level_db.max(-60.0) - resp.drag_delta().y * 0.3).clamp(-144.0, 12.0);
                let _ =
                    app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
            }
            if resp.double_clicked() {
                let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
            }
            let _ = resp.clone().on_hover_text(format!("Send {}: {} dB (drag to change)", (b'A' + slot as u8) as char, db_text(s.level_db)));
        }
        None => {
            ui.painter().circle_filled(pos2(r.min.x + 6.0, r.center().y), 1.5, t.text_dim);
        }
    }
    let popup = if snd.is_none() { egui::Popup::menu(&resp) } else { egui::Popup::context_menu(&resp) };
    popup.show(|ui| {
        if snd.is_some() && ui.button("no send").clicked() {
            let _ = app.run("mix.send_remove", json!({"track": id.0, "slot": slot}));
        }
        let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
        for b in busses {
            if ui.button(&b).clicked() {
                let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": b}));
            }
        }
        ui.separator();
        if ui.button("new bus...").clicked() {
            let n = app.engine.session().busses.len() + 1;
            let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": format!("Bus {n}")}));
        }
    });
}

fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    if ui.button(if input { "no input" } else { "no output" }).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "none"}));
    }
    let main_name = app.engine.session().outputs.first().map_or_else(|| "Out 1-2".to_string(), |o| o.name.clone());
    if !input && ui.button(main_name).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "main"}));
    }
    if input {
        for i in 1..=8 {
            if ui.button(format!("In {i}")).clicked() {
                let _ = app.run(cmd, json!({"track": id.0, key: format!("In {i}")}));
            }
        }
    }
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    ui.menu_button("bus", |ui| {
        for b in &busses {
            if ui.button(b).clicked() {
                let _ = app.run(cmd, json!({"track": id.0, key: b}));
            }
        }
        if ui.button("new bus...").clicked() {
            let n = busses.len() + 1;
            let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        }
    });
}
