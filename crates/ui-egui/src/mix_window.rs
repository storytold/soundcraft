//! The Mix window: one channel strip per track.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular, rgb};
use crate::widgets::{
    PannerEdit, PannerSpeaker, SelectorLabels, db_text, fader, fader_slot, groove, inset, meter, meter_scale, multi_meter, pan_knob, pan_text,
    raised_key, rec_toggle, selector_box_named, shade, surround_panner, text_toggle, vgradient,
};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ChannelFormat, Route, SurroundPan, Track, TrackId, TrackKind};
use soundcraft_playback::MeterSnapshot;

mod plugin_menu;
mod routing;
mod sends;
mod slots;

pub use plugin_menu::{instrument_menu, plugin_info, plugin_menu};
pub(crate) use routing::{input_name, route_name, route_popup};
#[cfg(test)]
use sends::dragged_send_db;
use sends::{Readout, editable_readout, parse_db, parse_pan};
pub(crate) use sends::{send_slot, send_window};
pub(crate) use slots::{insert_slot, instrument_slot};

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

/// Draw the horizontal Mix window strips for visible non-video tracks, scroll one requested strip into view and open
/// New Tracks when the space past the last strip is double-clicked.
pub(crate) fn content(app: &mut SoundApp, ui: &mut Ui) {
    let ids: Vec<TrackId> = app.engine.session().tracks.iter().filter(|x| !x.hidden && x.kind != TrackKind::Video).map(|x| x.id).collect();
    // Per-channel peaks for multichannel meters, from the snapshot `logic` takes from the player
    // (taking them here too would split each period's peaks between two readers).
    let snap = (app.player.is_some() && app.meters_live()).then(|| app.meter_snapshot.clone());
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
            if rest.width() > 4.0 && ui.interact(rest, ui.scope_id().with("past_strips"), Sense::click()).double_clicked() {
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
    let id = egui::Id::unique(key);
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

/// Draw one mixer channel strip: preamp, instrument, object, insert and send sections, EQ curve, routing, fader and
/// meters, with its interactions.
/// The height of the strip's footer: volume readout and name plate.
const STRIP_FOOT_H: f32 = 64.0;

/// One mixer strip's geometry and settings, shared by its sections.
struct StripCtx<'a> {
    track: &'a Track,
    id: TrackId,
    r: Rect,
    w: f32,
    x0: f32,
    inner_w: f32,
    narrow: bool,
    selected: bool,
    views: Vec<String>,
    all: bool,
}

impl StripCtx<'_> {
    /// Whether the mixer shows the section named `view`.
    fn shows(&self, view: &str) -> bool {
        self.all || self.views.iter().any(|x| x == view)
    }
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
    let all = views.iter().any(|x| x == "all");
    let cx = StripCtx { track, id, r, w, x0, inner_w, narrow, selected, views, all };
    // Preamp, instrument and object rows keep their height on every strip so sections line up;
    // their controls only appear on tracks with an input.
    y = preamp_section(app, ui, &cx, y);
    y = instrument_section(app, ui, &cx, y);
    y = object_section(app, ui, &cx, y);
    y = slot_sections(app, ui, &cx, y);
    y = eq_section(ui, &cx, y);
    y = comments_section(app, ui, &cx, y);
    y = io_section(app, ui, &cx, y);
    y = group_selector(app, ui, &cx, y);
    y = pan_section(app, ui, &cx, y);
    y = toggle_rows(app, ui, &cx, y);
    fader_and_meter(app, ui, &cx, y, snap);
    name_and_readout(app, ui, &cx);
}

/// Input gain stage ahead of the inserts (the track's trim).
fn preamp_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, narrow, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    let has_input = matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi);
    if cx.shows("mic_preamps") {
        // Input gain stage ahead of the inserts (the track's trim).
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 34.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "PREAMP", narrow);
        if has_input {
            let kr = Rect::from_min_size(pos2(x0 + 6.0, sec.min.y + 15.0), vec2(inner_w - 12.0, 16.0));
            let mut db = track.mixer.trim_db;
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let resp = c
                .add_sized(kr.size(), egui::DragValue::new(&mut db).range(-24.0..=24.0).speed(0.1).suffix(" dB").max_decimals(1))
                .accessible_name("Input trim (dB)");
            if resp.changed() {
                let _ = app.engine.execute_merged("mix.trim", &json!({"tracks": [id.0], "db": db}), &format!("trim:{}", id.0));
            }
            if resp.drag_stopped() || resp.lost_focus() {
                app.engine.end_merge();
            }
        }
        y = sec.max.y + 4.0;
    }
    y
}

/// The instrument slot.
fn instrument_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, narrow, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    if cx.shows("instruments") {
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
    y
}

/// The object/bed switch.
fn object_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, x0, inner_w, .. } = *cx;
    let has_input = matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi);
    if cx.shows("object") {
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
    y
}

/// The insert and send sections.
fn slot_sections(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, narrow, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    for (key, title, off, is_send) in [
        ("inserts_ae", "INSERTS A-E", 0usize, false),
        ("inserts_fj", "INSERTS F-J", 5, false),
        ("sends_ae", "SENDS A-E", 0, true),
        ("sends_fj", "SENDS F-J", 5, true),
    ] {
        if !cx.shows(key) {
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
    y
}

/// The EQ curve.
fn eq_section(ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    if cx.shows("eq_curve") {
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
    y
}

/// The comments field.
fn comments_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, narrow, x0, inner_w, .. } = *cx;
    if cx.shows("comments") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 44.0));
        let mut c = track.comments.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sec));
        let hint = if narrow { "" } else { "comments" };
        let field = crate::widgets::named_text_field(
            child.add(egui::TextEdit::multiline(&mut c).desired_width(inner_w).desired_rows(2).hint_text(hint).font(regular(10.0))),
            &format!("Comments for {}", track.name),
        );
        if field.changed() {
            let _ = app.engine.execute_merged("track.comments", &json!({"track": id.0, "comments": c}), &format!("comments:{}", id.0));
        }
        y = sec.max.y + 4.0;
    }
    y
}

/// Input, output and automation mode selectors.
fn io_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, narrow, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    if cx.shows("io") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 16.0 + 2.0 * 18.0 + 30.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "I / O", narrow);
        let in_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0), vec2(inner_w - 4.0, 16.0));
        let out_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 33.0), vec2(inner_w - 4.0, 16.0));
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-input", id.0)).max_rect(in_r));
        let input = input_name(app, track);
        let resp = selector_box_named(
            &mut c,
            in_r.width(),
            in_r.height(),
            &input,
            t.text,
            SelectorLabels { name: &format!("Input for {}", track.name), value: &input, tooltip: "Input" },
        );
        route_popup(app, &resp, id, true);
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-output", id.0)).max_rect(out_r));
        let output = route_name(app, &track.mixer.output);
        let resp = selector_box_named(
            &mut c,
            out_r.width(),
            out_r.height(),
            &output,
            t.text,
            SelectorLabels { name: &format!("Output for {}", track.name), value: &output, tooltip: "Output" },
        );
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
        let resp = selector_box_named(
            &mut c,
            am_r.width(),
            am_r.height(),
            label,
            col,
            SelectorLabels { name: &format!("Automation mode for {}", track.name), value: &full_label, tooltip: &full_label },
        );
        egui::Popup::menu(&resp).show(|ui| {
            for m in soundcraft_model::AutomationMode::ALL {
                if ui.selectable_label(m == mode, m.label()).clicked() {
                    let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
                }
            }
        });
        y = sec.max.y + 4.0;
    }
    y
}

/// The group selector.
fn group_selector(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, narrow, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    // Group selector.
    let no_group = if narrow { "nogrp" } else { "no group" };
    let gname = app.engine.session().groups.iter().find(|g| g.members.contains(&id)).map_or(no_group.to_string(), |g| g.name.clone());
    let gr = Rect::from_min_size(pos2(x0 + 2.0, y), vec2(inner_w - 4.0, 16.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(gr));
    let _ = selector_box_named(
        &mut c,
        gr.width(),
        gr.height(),
        &gname,
        t.text_dim,
        SelectorLabels { name: &format!("Group for {}", track.name), value: &gname, tooltip: "Group" },
    );
    y += 22.0;
    y
}

/// Pan: a surround panner when the output is multichannel, else the stereo knobs.
fn pan_section(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
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
            let (_, entered) = editable_readout(
                ui,
                pr,
                ui.scope_id().with(("pan-value", id.0, i)),
                Readout {
                    name: &format!("{} pan {}", track.name, i + 1),
                    display: &pan_text(v),
                    edit_initial: &format!("{:.0}", v * 100.0),
                    parse: parse_pan,
                    reset: 0.0,
                },
            );
            if let Some(v) = entered {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": v, "index": i}), &format!("pan:{}:{i}", id.0));
            }
        }
        y += 54.0;
    }
    y
}

/// Rec / Input / Solo / Mute.
fn toggle_rows(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, mut y: f32) -> f32 {
    let StripCtx { track, id, x0, inner_w, .. } = *cx;
    let t = Tokens::current();
    // Rec / Input / Solo / Mute.
    let bw = (inner_w - 6.0) / 2.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if track.kind.has_playlist() {
        let input_label = format!("Input monitoring for {}", track.name);
        if text_toggle(&mut row, vec2(bw, 18.0), "I", track.mixer.input_monitor, t.input, &input_label).clicked() {
            let _ = app.run("mix.input_monitor", json!({"track": id.0}));
        }
        let record_label = format!("Record enable for {}", track.name);
        if rec_toggle(&mut row, vec2(bw, 18.0), track.mixer.record_arm, &record_label).clicked() {
            let _ = app.run("mix.record_arm", json!({"track": id.0}));
        }
    }
    y += 22.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if track.kind != TrackKind::Master {
        let solo_label = format!("Solo for {}", track.name);
        if text_toggle(&mut row, vec2(bw, 18.0), "S", track.mixer.solo, t.solo, &solo_label).clicked() {
            let _ = app.run("mix.solo", json!({"track": id.0}));
        }
        let mute_label = format!("Mute for {}", track.name);
        if text_toggle(&mut row, vec2(bw, 18.0), "M", track.mixer.mute, t.mute, &mute_label).clicked() {
            let _ = app.run("mix.mute", json!({"track": id.0}));
        }
    }
    y += 26.0;
    y
}

/// The fader and its meter.
fn fader_and_meter(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>, y: f32, snap: Option<&MeterSnapshot>) {
    let StripCtx { track, id, narrow, x0, inner_w, r, .. } = *cx;
    let t = Tokens::current();
    // Fader + meter.
    let fh = (r.max.y - STRIP_FOOT_H - y).max(120.0);
    let wide_meter = (if track.kind == TrackKind::Master { app.engine.session().main_format() } else { track.format }).channels() > 2;
    let fr = Rect::from_min_size(pos2(x0, y), vec2(inner_w * if wide_meter { 0.45 } else { 0.6 }, fh));
    if let Some(db) = fader(ui, fr, track.mixer.volume_db, ui.scope_id().with(("fader", id.0)), &format!("{} level", track.name)) {
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
}

/// Volume readout, kind label and name plate.
fn name_and_readout(app: &mut SoundApp, ui: &mut Ui, cx: &StripCtx<'_>) {
    let StripCtx { track, id, x0, inner_w, r, w, selected, .. } = *cx;
    let t = Tokens::current();
    // Volume readout and name.
    let vr = Rect::from_min_size(pos2(x0 + 4.0, r.max.y - STRIP_FOOT_H + 4.0), vec2(inner_w - 8.0, 15.0));
    inset(ui.painter(), vr, t.counter_bg);
    let (_, entered) = editable_readout(
        ui,
        vr,
        ui.scope_id().with(("volume-value", id.0)),
        Readout {
            name: &format!("{} volume", track.name),
            display: &db_text(track.mixer.volume_db),
            edit_initial: &format!("{:.1}", track.mixer.volume_db),
            parse: parse_db,
            reset: 0.0,
        },
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
    let nresp = ui.interact(nr, ui.scope_id().with(("strip_name", id.0)), Sense::click());
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

#[cfg(test)]
mod tests {
    use super::{Readout, dragged_send_db, editable_readout, parse_db, parse_pan};
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

    /// Alt-click resets a readout; a click opens its text field, and hiding the widget drops focus so the formatted
    /// display returns.
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
                    let (resp, value) = editable_readout(
                        ui,
                        r,
                        egui::Id::unique("readout"),
                        Readout { name: "Readout", display: "-3.0", edit_initial: "-3.0", parse: parse_db, reset: 0.0 },
                    );
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
