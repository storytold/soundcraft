use super::input::snap;
use super::layout::{COLUMN_TITLES_H, COLUMNS, Column, HEADER_W, RULER_H, edit_columns, header_width, sample_at, x_of};
use super::{
    Align2, Color32, GridValue, NoteValue, Range, Rect, Samples, Sense, Session, Shape, SoundApp, Stroke, TimeFormat, Tokens, Ui, bold,
    format_position, json, pos2, regular, rgb, vec2,
};

// ---- rulers ----------------------------------------------------------------------------------

fn ruler_label(id: &str) -> &'static str {
    match id {
        "bars_beats" => "Bars|Beats",
        "min_secs" => "Min:Secs",
        "timecode" => "Timecode",
        "feet_frames" => "Feet+Frames",
        "samples" => "Samples",
        "tempo" => "Tempo",
        "meter" => "Meter",
        "markers" => "Markers",
        "key" => "Key",
        "chords" => "Chords",
        "timecode2" => "Timecode 2",
        "markers2" => "Markers 2",
        "markers3" => "Markers 3",
        "markers4" => "Markers 4",
        "markers5" => "Markers 5",
        "tempo_editor" => "",
        _ => "",
    }
}

pub fn ruler_height(id: &str) -> f32 {
    if id == "tempo_editor" { 56.0 } else { RULER_H }
}

/// The tempo editor: a graph of tempo over time. Click adds a change, Alt-click removes the nearest.
fn tempo_editor(app: &mut SoundApp, ui: &mut Ui, painter: &egui::Painter, s: &Session, tl: Rect, row: Rect) {
    let sr = s.sample_rate;
    painter.rect_filled(row, 0.0, Color32::from_rgb(24, 40, 32));
    let (lo, hi) = s.tempo.tempos().iter().fold((f64::MAX, f64::MIN), |(a, b), e| (a.min(e.bpm), b.max(e.bpm)));
    let (lo, hi) = ((lo - 20.0).max(20.0), (hi + 20.0).min(400.0));
    let y_of = |bpm: f64| row.max.y - 4.0 - ((bpm - lo) / (hi - lo).max(1.0)) as f32 * (row.height() - 8.0);
    let mut pts = Vec::new();
    let evs = s.tempo.tempos();
    for (k, e) in evs.iter().enumerate() {
        let x = x_of(s, tl, s.tempo.tick_to_samples(e.tick, sr)).max(tl.min.x);
        let x_next = evs.get(k + 1).map_or(tl.max.x, |n| x_of(s, tl, s.tempo.tick_to_samples(n.tick, sr)));
        pts.push(pos2(x, y_of(e.bpm)));
        pts.push(pos2(x_next.min(tl.max.x), y_of(e.bpm)));
        painter.circle_filled(pos2(x, y_of(e.bpm)), 3.0, Color32::from_rgb(140, 230, 160));
    }
    painter.add(Shape::line(pts, Stroke::new(1.5, Color32::from_rgb(140, 230, 160))));
    painter.text(pos2(tl.min.x + 4.0, row.min.y + 8.0), Align2::LEFT_CENTER, format!("{hi:.0}"), regular(9.0), Color32::from_rgb(120, 160, 130));
    painter.text(pos2(tl.min.x + 4.0, row.max.y - 8.0), Align2::LEFT_CENTER, format!("{lo:.0}"), regular(9.0), Color32::from_rgb(120, 160, 130));
    let resp = ui.interact(row, ui.scope_id().with("tempo_editor"), Sense::click());
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let at = snap(s, sample_at(s, tl, p.x).max(0));
        if ui.input(|i| i.modifiers.alt) {
            let near = evs
                .iter()
                .filter(|e| e.tick > 0)
                .min_by_key(|e| ((x_of(s, tl, s.tempo.tick_to_samples(e.tick, sr)) - p.x).abs() * 10.0) as i64)
                .map(|e| s.tempo.tick_to_samples(e.tick, sr));
            if let Some(n) = near {
                let _ = app.run("event.tempo_remove", json!({"at": n}));
            }
        } else {
            let bpm = lo + f64::from((row.max.y - 4.0 - p.y) / (row.height() - 8.0)) * (hi - lo);
            let _ = app.run("event.tempo", json!({"bpm": (bpm * 10.0).round() / 10.0, "at": at}));
        }
    }
}

/// A "nice" step in samples for ruler labels at the current zoom.
fn nice_step(spp: f64, sr: f64, min_px: f64) -> i64 {
    let min = spp * min_px;
    let steps_s = [0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0];
    for st in steps_s {
        if st * sr >= min {
            return (st * sr) as i64;
        }
    }
    (3600.0 * sr) as i64
}

/// Draw each enabled ruler and handle its clicks and double-clicks against the visible sample range.
pub(super) fn draw_rulers(app: &mut SoundApp, ui: &mut Ui, area: Rect, tl: Rect, rulers: &[String]) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    ui.painter().rect_filled(area, 0.0, t.ruler_bg);
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let sr = s.sample_rate;
    let mut y0 = area.min.y + 3.0;
    for (i, id) in rulers.iter().enumerate() {
        let h = ruler_height(id);
        let row = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + h));
        let lab = Rect::from_min_max(pos2(area.min.x, y0), pos2(area.min.x + HEADER_W, y0 + h));
        y0 += h;
        let painter = ui.painter().with_clip_rect(row);
        ui.painter().text(pos2(lab.max.x - 12.0, lab.center().y), Align2::RIGHT_CENTER, ruler_label(id), bold(11.0), t.ruler_text);
        let fmt = TimeFormat::from_id(id);
        match id.as_str() {
            "tempo" => {
                painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.tempo_ruler);
                let tresp = ui.interact(row, ui.scope_id().with("tempo_ruler"), Sense::click());
                if tresp.double_clicked()
                    && let Some(p) = tresp.interact_pointer_pos()
                {
                    let at = snap(&s, sample_at(&s, tl, p.x).max(0));
                    let bpm = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(at, sr));
                    app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange { at, bpm });
                }
                let mut last_label = f32::MIN;
                for ev in s.tempo.tempos() {
                    let x = x_of(&s, tl, s.tempo.tick_to_samples(ev.tick, sr));
                    painter.add(Shape::convex_polygon(
                        vec![pos2(x, row.min.y + 3.0), pos2(x + 7.0, row.center().y), pos2(x, row.max.y - 3.0)],
                        Color32::from_rgb(220, 60, 50),
                        Stroke::NONE,
                    ));
                    if x - last_label > 46.0 {
                        last_label = x;
                        painter.text(
                            pos2(x + 10.0, row.center().y),
                            Align2::LEFT_CENTER,
                            format!("{}", (ev.bpm * 100.0).round() / 100.0),
                            bold(11.0),
                            Color32::from_rgb(230, 240, 230),
                        );
                    }
                }
            }
            "meter" => {
                painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.meter_ruler);
                for ev in s.tempo.meters() {
                    let x = x_of(&s, tl, s.tempo.tick_to_samples(ev.tick, sr));
                    painter.text(
                        pos2(x + 6.0, row.center().y),
                        Align2::LEFT_CENTER,
                        format!("{}/{}", ev.numerator, ev.denominator),
                        bold(11.0),
                        Color32::from_rgb(230, 236, 240),
                    );
                }
            }
            "tempo_editor" => {
                tempo_editor(app, ui, &painter, &s, tl, row);
            }
            "key" => {
                painter.rect_filled(row, 0.0, Color32::from_rgb(64, 52, 88));
                if s.key_signatures.is_empty() {
                    painter.text(pos2(tl.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, "C major", bold(11.0), Color32::from_rgb(220, 214, 236));
                }
                for (at, k) in &s.key_signatures {
                    let x = x_of(&s, tl, *at);
                    painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y - 2.0)], Stroke::new(2.0, Color32::from_rgb(200, 180, 240)));
                    painter.text(pos2(x + 5.0, row.center().y), Align2::LEFT_CENTER, k, bold(11.0), Color32::from_rgb(236, 230, 250));
                }
                let resp = ui.interact(row, ui.scope_id().with("key_ruler"), Sense::click());
                if resp.double_clicked()
                    && let Some(p) = resp.interact_pointer_pos()
                {
                    let at = snap(&s, sample_at(&s, tl, p.x).max(0));
                    let _ = app.run("event.add_key_change", json!({"at": at, "key": "G major"}));
                }
            }
            "timecode2" => {
                // A second timecode ruler at 25 fps when the session runs at another rate (or 30 otherwise).
                let alt = if s.frame_rate == soundcraft_time::FrameRate::Fps25 {
                    soundcraft_time::FrameRate::Fps30
                } else {
                    soundcraft_time::FrameRate::Fps25
                };
                timebase_ruler(&painter, &s, &RulerRow { tl, row, view, bottom: i + 1 == rulers.len() }, TimeFormat::Timecode, alt);
            }
            "markers" | "markers2" | "markers3" | "markers4" | "markers5" | "chords" => {
                let lane: u8 = match id.as_str() {
                    "markers2" | "chords" => 2,
                    "markers3" => 3,
                    "markers4" => 4,
                    "markers5" => 5,
                    _ => 1,
                };
                painter.rect_filled(row, 0.0, if id == "chords" { Color32::from_rgb(40, 56, 60) } else { t.marker_ruler });
                for m in s.markers.iter().filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0)) {
                    let x = x_of(&s, tl, m.start);
                    let col = m.color.map_or(Color32::from_rgb(232, 200, 64), rgb);
                    if m.kind == soundcraft_model::MarkerKind::Selection {
                        let x1 = x_of(&s, tl, m.end);
                        painter.rect_filled(Rect::from_min_max(pos2(x, row.min.y + 3.0), pos2(x1, row.max.y - 3.0)), 2.0, col.gamma_multiply(0.5));
                    }
                    painter.add(Shape::convex_polygon(
                        vec![
                            pos2(x - 5.0, row.min.y + 2.0),
                            pos2(x + 5.0, row.min.y + 2.0),
                            pos2(x + 5.0, row.max.y - 6.0),
                            pos2(x, row.max.y - 2.0),
                            pos2(x - 5.0, row.max.y - 6.0),
                        ],
                        col,
                        Stroke::NONE,
                    ));
                    painter.text(pos2(x + 8.0, row.center().y), Align2::LEFT_CENTER, &m.name, bold(11.0), t.marker_text);
                }
                // Click on the marker ruler: recall a marker; double-click renames it, or adds one.
                let resp = ui.interact(row, ui.scope_id().with(("marker_ruler", lane)), Sense::click());
                if let Some(p) = resp.interact_pointer_pos() {
                    let hit = s
                        .markers
                        .iter()
                        .filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0))
                        .find(|m| (x_of(&s, tl, m.start) - p.x).abs() < 8.0);
                    if resp.double_clicked() {
                        match hit {
                            Some(m) => app.dialogs.open_rename_marker(m.id, &m.name),
                            None => {
                                let _ = app.run("markers.add", json!({"at": sample_at(&s, tl, p.x).max(0), "ruler": lane}));
                            }
                        }
                    } else if resp.clicked()
                        && let Some(m) = hit
                    {
                        let _ = app.run("markers.recall", json!({"number": m.number}));
                    }
                }
            }
            _ => {
                if let Some(f) = fmt {
                    timebase_ruler(&painter, &s, &RulerRow { tl, row, view, bottom: i + 1 == rulers.len() }, f, s.frame_rate);
                    // Clicking a timebase ruler locates.
                    let resp = ui.interact(row, ui.scope_id().with(("ruler", id.as_str())), Sense::click_and_drag());
                    if (resp.clicked() || resp.dragged())
                        && let Some(p) = resp.interact_pointer_pos()
                    {
                        let at = sample_at(&s, tl, p.x).max(0);
                        let _ = app.run("transport.locate", json!({"at": at}));
                    }
                }
            }
        }
        if id == "min_secs" || id == "bars_beats" || id == "timecode" || id == "samples" || id == "feet_frames" {
            // Main timebase marker (blue tick like a selected ruler).
            if Some(s.edit.main_counter) == fmt {
                ui.painter().rect_filled(Rect::from_min_size(pos2(lab.min.x + 6.0, lab.center().y - 3.0), vec2(6.0, 6.0)), 1.0, t.accent);
            }
        }
    }
    ui.painter().line_segment([pos2(area.min.x, area.max.y - 1.0), pos2(area.max.x, area.max.y - 1.0)], Stroke::new(1.0, t.border));
}

/// Where a timebase ruler sits: the timeline, its row and the visible range, and whether it is the bottom ruler.
#[derive(Clone, Copy)]
struct RulerRow {
    tl: Rect,
    row: Rect,
    view: Range,
    bottom: bool,
}

fn timebase_ruler(painter: &egui::Painter, s: &Session, place: &RulerRow, f: TimeFormat, rate: soundcraft_time::FrameRate) {
    let RulerRow { tl, row, view, bottom } = *place;
    let t = Tokens::current();
    let sr = s.sample_rate;
    let spp = s.edit.zoom.samples_per_px;
    let mut marks: Vec<(Samples, bool)> = Vec::new();
    match f {
        TimeFormat::BarsBeats => {
            // Bars every n so labels are ≥ 60 px apart.
            let bar_px = {
                let b = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
                b.step_samples(view.start.max(0), sr, &s.tempo, rate) as f64 / spp
            };
            let every = [1i64, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f64 >= 60.0).unwrap_or(256);
            let first = s.tempo.bar_beat_at(view.start.max(0), sr).bar;
            let mut bar = first - (first - 1).rem_euclid(every);
            let mut guard = 0;
            loop {
                let at = s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat: 1, tick: 0 }, sr);
                if at > view.end || guard > 400 {
                    break;
                }
                marks.push((at, true));
                if bar_px * every as f64 >= 240.0 {
                    let m = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(at, sr));
                    for beat in 2..=i64::from(m.numerator) {
                        marks.push((s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar, beat, tick: 0 }, sr), false));
                    }
                }
                bar += every;
                guard += 1;
            }
        }
        TimeFormat::Samples => {
            let step = {
                let min = spp * 90.0;
                let mut st = 1i64;
                while (st as f64) < min {
                    st *= if st.to_string().starts_with('1') {
                        2
                    } else if st.to_string().starts_with('2') {
                        5
                    } else {
                        2
                    };
                    if st > 10_000_000_000 {
                        break;
                    }
                }
                st
            };
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                at += step;
            }
        }
        _ => {
            let step = nice_step(spp, sr.as_f64(), 70.0).max(1);
            let mut at = view.start.max(0).div_euclid(step) * step;
            while at <= view.end && marks.len() < 400 {
                marks.push((at, true));
                let sub = step / 4;
                if sub > 0 && (sub as f64 / spp) > 8.0 {
                    for k in 1..4 {
                        marks.push((at + sub * k, false));
                    }
                }
                at += step;
            }
        }
    }
    for (at, major) in marks {
        let x = x_of(s, tl, at);
        if major {
            painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
            let label = match f {
                TimeFormat::BarsBeats => s.tempo.bar_beat_at(at, sr).bar.to_string(),
                TimeFormat::MinSecs => {
                    let secs = sr.seconds(at);
                    if secs.fract().abs() < 1e-9 {
                        format!("{}:{:02}", (secs / 60.0) as i64, (secs as i64) % 60)
                    } else {
                        format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start())
                    }
                }
                _ => format_position(at, f, sr, &s.tempo, rate, s.timecode_start, s.feet_frames_start()),
            };
            painter.text(pos2(x + 3.0, row.center().y), Align2::LEFT_CENTER, label, regular(11.0), t.ruler_text);
        } else {
            painter.line_segment([pos2(x, row.max.y - 4.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
        }
    }
    if bottom {
        painter.line_segment([pos2(tl.min.x, row.max.y), pos2(tl.max.x, row.max.y)], Stroke::new(1.0, t.border));
    }
}

/// Captions over the header columns, boxed in their own row under the rulers.
pub(super) fn column_titles(ui: &Ui, rulers: Rect, cols: &[Column]) {
    if cols.is_empty() {
        return;
    }
    let t = Tokens::current();
    let line = Stroke::new(1.0, t.border);
    let mut x = rulers.min.x + HEADER_W;
    let end = rulers.min.x + header_width(cols);
    let row = Rect::from_min_max(pos2(x, rulers.max.y - COLUMN_TITLES_H - 3.0), pos2(end, rulers.max.y));
    ui.painter().rect_filled(row, 0.0, t.panel_bg2);
    ui.painter().hline(rulers.min.x..=end, row.min.y + 0.5, line);
    ui.painter().hline(rulers.min.x..=end, row.max.y - 0.5, line);
    ui.painter().vline(end - 0.5, row.y_range(), line);
    for &(_, _, title, w) in cols {
        ui.painter().vline(x + 0.5, row.y_range(), line);
        ui.painter().text(pos2(x + w / 2.0, row.center().y), Align2::CENTER_CENTER, title, bold(10.0), t.section_label);
        x += w;
    }
}

/// The Edit window view selector: which columns sit beside the track headers.
pub(super) fn views_button(app: &mut SoundApp, ui: &mut Ui, rulers: Rect) {
    let r = Rect::from_min_size(rulers.min + vec2(6.0, 5.0), vec2(50.0, 18.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let resp = crate::widgets::selector_box(&mut child, r.width(), r.height(), "Views", Tokens::current().text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, child.is_enabled(), "Views"));
    let resp = resp.on_hover_text("Columns beside the track headers: inserts, sends, I/O…");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let shown = edit_columns(app.engine.session());
        for (id, label, _, _) in COLUMNS {
            if ui.add(egui::Button::new(label).selected(shown.iter().any(|c| c.0 == id))).clicked() {
                let _ = app.run(&format!("view.edit_{id}"), json!({}));
            }
        }
        ui.separator();
        if ui.button("All").clicked() {
            let _ = app.run("view.edit_all", json!({}));
        }
        if ui.button("Minimal").clicked() {
            let _ = app.run("view.edit_minimal", json!({}));
        }
    });
}
