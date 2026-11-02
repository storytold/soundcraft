use super::input::snap;
use super::layout::{COLUMN_TITLES_H, COLUMNS, Column, HEADER_W, RULER_H, edit_columns, header_width, sample_at, x_of};
use super::{
    Align2, Color32, GridValue, NoteValue, Range, Rect, Samples, Sense, Session, Shape, SoundApp, Stroke, TimeFormat, Tokens, Ui, bold,
    format_position, json, pos2, regular, rgb, vec2,
};

// ---- rulers ----------------------------------------------------------------------------------

pub(crate) fn ruler_label(id: &str) -> &'static str {
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
    if matches!(id, "tempo_editor" | "key_staff") { 56.0 } else { RULER_H }
}

/// Main time ruler, or the first visible timebase when the main one is hidden.
pub(super) fn main_ruler_row(s: &Session, top: f32, tl: Rect) -> Option<Rect> {
    let id = s
        .edit
        .rulers
        .iter()
        .find(|id| id.as_str() == s.edit.main_counter.id())
        .or_else(|| s.edit.rulers.iter().find(|id| TimeFormat::from_id(id).is_some() || id.as_str() == "timecode2"))?;
    let mut y = top + 3.0;
    for ruler in &s.edit.rulers {
        if ruler == id {
            return Some(Rect::from_min_max(pos2(tl.min.x, y), pos2(tl.max.x, y + ruler_height(ruler))));
        }
        y += ruler_height_for(s, ruler);
        if ruler == "key" && s.edit.flag("view.ruler.key_staff") {
            y += ruler_height("key_staff");
        }
        if ruler == "tempo" && s.edit.flag("view.ruler.tempo_editor") {
            y += ruler_height_for(s, "tempo_editor");
        }
    }
    None
}

fn icon_button(ui: &mut Ui, rect: Rect, id: &str, icon: &str, label: &str) -> egui::Response {
    let t = Tokens::current();
    let response = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect.shrink(1.0), 2.0, t.button_hi);
    }
    crate::icons::draw(ui.painter(), rect.shrink(3.0), icon, t.ruler_text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response.on_hover_text(label)
}

#[derive(Clone)]
enum RulerMenu {
    Rulers,
    MarkerDisplays,
    SecondaryTimecode,
    Conductor { kind: String, at: i64, id: Option<u64>, existing: bool, session: std::sync::Arc<Session> },
}
fn menu_contents(app: &mut SoundApp, ui: &mut Ui, target: &RulerMenu) {
    match target {
        RulerMenu::Rulers => {
            for id in [
                "bars_beats",
                "min_secs",
                "timecode",
                "timecode2",
                "feet_frames",
                "samples",
                "tempo",
                "meter",
                "key",
                "chords",
                "markers",
                "markers2",
                "markers3",
                "markers4",
                "markers5",
            ] {
                let checked = app.engine.session().edit.rulers.iter().any(|r| r == id);
                if ui.selectable_label(checked, ruler_label(id)).clicked() {
                    let command = match id {
                        "timecode2" | "markers2" | "markers3" | "markers4" | "markers5" => format!("view.ruler_{id}"),
                        _ => "view.ruler".into(),
                    };
                    let _ = app.run(&command, json!({"ruler":id}));
                }
            }
            ui.separator();
            for (label, command, flag) in [
                ("Tempo Editor", "view.ruler_tempo_editor", Some("view.ruler.tempo_editor")),
                ("Key Signature Staff", "view.ruler_key_staff", Some("view.ruler.key_staff")),
                ("All Marker Rulers", "view.ruler_all_markers", None),
                ("All", "view.ruler_all", None),
                ("Minimal", "view.ruler_minimal", None),
            ] {
                if ui.selectable_label(flag.is_some_and(|f| app.engine.session().edit.flag(f)), label).clicked() {
                    let _ = app.run(command, json!({}));
                }
            }
        }
        RulerMenu::MarkerDisplays => {
            for (label, command, flag) in [
                ("Ruler Lane Color", "view.marker_ruler_lane_color", "view.marker.ruler_lane_color"),
                ("Ruler Lines", "view.marker_ruler_lines", "view.marker.ruler_lines"),
            ] {
                if ui.selectable_label(app.engine.session().edit.flag(flag), label).clicked() {
                    let _ = app.run(command, json!({}));
                }
            }
        }
        RulerMenu::SecondaryTimecode => {
            for rate in soundcraft_time::FrameRate::ALL {
                if ui.selectable_label(app.engine.session().secondary_frame_rate == Some(rate), rate.label()).clicked() {
                    let _ = app.run("setup.session", json!({"secondary_frame_rate":rate.label()}));
                    ui.close();
                }
            }
        }
        RulerMenu::Conductor { kind, at, id, existing, session } => {
            if !std::sync::Arc::ptr_eq(session, &app.engine.session_arc()) {
                ui.close();
                return;
            }
            if ui.button(if *existing { "Edit…" } else { "Add…" }).clicked() {
                let form = if *existing {
                    crate::dialogs::conductor::Conductor::edit(session, kind, *at, *id)
                } else {
                    crate::dialogs::conductor::Conductor::new(session, kind, *at, *id)
                };
                app.dialogs.open = Some(crate::dialogs::Dialog::Conductor(form));
                ui.close();
            }
            if *existing && ui.button("Delete").clicked() {
                let command = match kind.as_str() {
                    "key" => "event.key_remove",
                    "meter" => "event.meter_remove",
                    _ => "chord.delete",
                };
                let _ = app.run(command, json!({"at":at,"id":id}));
                ui.close();
            }
        }
    }
}
fn dropdown(app: &mut SoundApp, response: &egui::Response, target: RulerMenu) {
    egui::Popup::menu(response).show(|ui| menu_contents(app, ui, &target));
}

fn ruler_header(app: &mut SoundApp, ui: &mut Ui, lab: Rect, id: &str, active: bool) {
    let t = Tokens::current();
    ui.painter().rect_filled(lab, 0.0, if active { t.ruler_active_bg } else { t.ruler_label_bg });
    let marker_lane = match id {
        "markers" => Some(1),
        "markers2" => Some(2),
        "markers3" => Some(3),
        "markers4" => Some(4),
        "markers5" => Some(5),
        _ => None,
    };
    let label_end = if marker_lane.is_some() { lab.max.x - 40.0 } else { lab.max.x - 4.0 };
    let label = Rect::from_min_max(pos2(lab.min.x + 32.0, lab.min.y), pos2(label_end, lab.max.y));
    if label.width() > 0.0 {
        ui.painter().with_clip_rect(label).text(pos2(label.min.x, label.center().y), Align2::LEFT_CENTER, ruler_label(id), bold(11.0), t.ruler_text);
        if TimeFormat::from_id(id).is_some() {
            let response = ui.interact(label, ui.scope_id().with(("main_ruler", id)), Sense::click()).on_hover_text("Use as main counter");
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), format!("Use {} as main counter", ruler_label(id))));
            if response.clicked() {
                let _ = app.run("view.main_counter", json!({"format": id}));
            }
        }
    }
    if active {
        let button = Rect::from_min_size(lab.min, vec2(30.0, lab.height()));
        let response = icon_button(ui, button, "rulers_menu", "rulers", "Show or hide rulers");
        dropdown(app, &response, RulerMenu::Rulers);
        ui.painter().hline(lab.min.x..=lab.min.x + 28.0, lab.max.y - 2.0, Stroke::new(2.0, t.accent));
    }
    if matches!(id, "tempo" | "meter" | "key" | "chords") {
        let add = Rect::from_min_size(pos2(lab.max.x - 20.0, lab.min.y), vec2(20.0, lab.height()));
        if icon_button(ui, add, &format!("add_{id}"), "plus", &format!("Add {id} change")).clicked() {
            let at = app.engine.session().edit.selection.start;
            if id == "tempo" {
                app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange {
                    at,
                    original: None,
                    bpm: app.engine.session().tempo.tempo_at_tick(app.engine.session().tempo.samples_to_ticks(at, app.engine.session().sample_rate)),
                });
            } else {
                app.dialogs.open =
                    Some(crate::dialogs::Dialog::Conductor(crate::dialogs::conductor::Conductor::new(app.engine.session(), id, at, None)));
            }
        }
        if matches!(id, "tempo" | "key") {
            let rect = Rect::from_min_size(lab.min, vec2(26.0, lab.height()));
            let command = if id == "tempo" { "view.ruler_tempo_editor" } else { "view.ruler_key_staff" };
            if icon_button(ui, rect, &format!("expand_{id}"), "rulers", &format!("Expand {id}")).clicked() {
                let _ = app.run(command, json!({}));
            }
        }
    }
    if id == "timecode2" {
        let rect = Rect::from_min_size(pos2(lab.max.x - 20.0, lab.min.y), vec2(20.0, lab.height()));
        let response = icon_button(ui, rect, "secondary_rate", "gear", "Timecode 2 frame rate");
        dropdown(app, &response, RulerMenu::SecondaryTimecode);
    }
    if let Some(lane) = marker_lane {
        let button = Rect::from_min_size(lab.min, vec2(26.0, lab.height()));
        if icon_button(ui, button, &format!("memory_{id}"), "marker", "Open Memory Locations").clicked() {
            let _ = app.run("window.memory_locations", json!({}));
        }
        let settings = Rect::from_min_size(pos2(lab.max.x - 40.0, lab.min.y), vec2(20.0, lab.height()));
        let response = icon_button(ui, settings, &format!("marker_settings_{id}"), "gear", "Marker display settings");
        dropdown(app, &response, RulerMenu::MarkerDisplays);
        let add = Rect::from_min_size(pos2(lab.max.x - 20.0, lab.min.y), vec2(20.0, lab.height()));
        if icon_button(ui, add, &format!("add_marker_{id}"), "plus", &format!("Add marker to lane {lane}")).clicked() {
            let _ = app.run("markers.add", json!({"ruler": lane}));
        }
    }
}

/// The tempo editor: a graph of tempo over time. Click adds a change, Alt-click removes the nearest.
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
        let h = ruler_height_for(&s, id);
        let row = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + h));
        let lab = Rect::from_min_max(pos2(area.min.x, y0), pos2(tl.min.x, y0 + h));
        y0 += h;
        let painter = ui.painter().with_clip_rect(row);
        let active = main_ruler_row(&s, area.min.y, tl).is_some_and(|r| (r.min.y - row.min.y).abs() < 0.5);
        ruler_header(app, ui, lab, id, active);
        if active {
            painter.rect_filled(row, 0.0, t.ruler_active_bg);
        }
        let fmt = TimeFormat::from_id(id);
        match id.as_str() {
            "tempo" => {
                painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.tempo_ruler);
                let tresp = ui.interact(row, ui.scope_id().with("tempo_ruler"), Sense::click());
                if tresp.double_clicked()
                    && let Some(p) = tresp.interact_pointer_pos()
                {
                    let original = s
                        .tempo
                        .tempos()
                        .iter()
                        .find(|event| {
                            let x = x_of(&s, tl, s.tempo.tick_to_samples(event.tick, sr));
                            p.x >= x - 5.0 && p.x < x + 50.0
                        })
                        .map(|event| s.tempo.tick_to_samples(event.tick, sr));
                    let at = original.unwrap_or_else(|| snap(&s, sample_at(&s, tl, p.x).max(0)));
                    let bpm = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(at, sr));
                    app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange { at, original, bpm });
                }
                if tresp.clicked()
                    && ui.input(|i| i.modifiers.alt)
                    && let Some(p) = tresp.interact_pointer_pos()
                    && let Some(event) = s.tempo.tempos().iter().find(|e| (x_of(&s, tl, s.tempo.tick_to_samples(e.tick, sr)) - p.x).abs() < 10.0)
                {
                    let _ = app.run("event.tempo_remove", json!({"at":s.tempo.tick_to_samples(event.tick,sr)}));
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
            "tempo_editor" => {
                super::tempo_editor::draw(app, ui, &s, tl, row);
            }
            "key" | "chords" | "meter" => {
                musical_ruler(app, ui, &s, tl, row, id);
            }
            "key_staff" => {
                key_staff(&painter, &s, tl, row);
            }
            "timecode2" => {
                let alt = s.secondary_frame_rate.unwrap_or(soundcraft_time::FrameRate::Fps25);
                timebase_ruler(&painter, &s, &RulerRow { tl, row, view, bottom: i + 1 == rulers.len() }, TimeFormat::Timecode, alt);
            }
            "markers" | "markers2" | "markers3" | "markers4" | "markers5" => {
                let lane: u8 = match id.as_str() {
                    "markers2" => 2,
                    "markers3" => 3,
                    "markers4" => 4,
                    "markers5" => 5,
                    _ => 1,
                };
                painter.rect_filled(row, 0.0, if id == "chords" { Color32::from_rgb(40, 56, 60) } else { t.marker_ruler });
                let mut markers: Vec<_> = s.markers.iter().filter(|m| m.ruler == lane || (lane == 1 && m.ruler == 0)).collect();
                markers.sort_by_key(|m| m.start);
                if s.edit.flag("view.marker.ruler_lane_color") {
                    let points: Vec<_> = markers.iter().copied().filter(|m| m.kind == soundcraft_model::MarkerKind::Marker).collect();
                    for (i, m) in points.iter().enumerate() {
                        let x = x_of(&s, tl, m.start).max(row.min.x);
                        let end = points.get(i + 1).map_or(row.max.x, |n| x_of(&s, tl, n.start)).min(row.max.x);
                        if end > x {
                            painter.rect_filled(
                                Rect::from_min_max(pos2(x, row.min.y), pos2(end, row.max.y)),
                                0.0,
                                m.color.map_or(Color32::from_rgb(232, 200, 64), rgb).gamma_multiply(0.35),
                            );
                        }
                    }
                }
                for m in markers {
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
        ui.painter().hline(area.min.x..=tl.max.x, row.max.y - 0.5, Stroke::new(1.0, t.border));
        ui.painter().vline(tl.min.x - 0.5, row.y_range(), Stroke::new(1.0, t.border));
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
            painter.line_segment([pos2(x, row.max.y - 8.0), pos2(x, row.max.y)], Stroke::new(1.0, t.ruler_tick));
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
    let r = Rect::from_min_size(pos2(rulers.min.x + 6.0, rulers.max.y - COLUMN_TITLES_H - 2.0), vec2(50.0, COLUMN_TITLES_H));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let resp = crate::widgets::selector_box(&mut child, r.width(), r.height(), "Views", Tokens::current().text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, child.is_enabled(), "Views"));
    let resp = resp.on_hover_text("Columns beside the track headers: inserts, sends, I/O…");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let shown = edit_columns(app.engine.session());
        for (id, label, _, _) in COLUMNS {
            if ui.selectable_label(shown.iter().any(|c| c.0 == id), label).clicked() {
                let _ = app.run(&format!("view.edit_{id}"), json!({}));
            }
        }
        ui.separator();
        for (label, command) in [("All", "view.edit_all"), ("Minimal", "view.edit_minimal")] {
            if ui.button(label).clicked() {
                let _ = app.run(command, json!({}));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_handles_follow_the_visible_main_ruler_and_tempo_editor_height() {
        let mut session = Session::default();
        session.edit.rulers = vec!["tempo".into(), "min_secs".into(), "bars_beats".into()];
        session.edit.set_flag("view.ruler.tempo_editor", true);
        let tl = Rect::from_min_max(pos2(200.0, 300.0), pos2(900.0, 800.0));
        let row = main_ruler_row(&session, 100.0, tl).unwrap();
        assert_eq!(row.min.y, 103.0 + RULER_H + ruler_height_for(&session, "tempo_editor"));
        session.edit.main_counter = TimeFormat::BarsBeats;
        assert_eq!(main_ruler_row(&session, 100.0, tl).unwrap().min.y, row.min.y + RULER_H);
        session.edit.rulers.retain(|id| id != "bars_beats");
        assert_eq!(main_ruler_row(&session, 100.0, tl), Some(row));
        session.edit.rulers = vec!["markers".into()];
        assert_eq!(main_ruler_row(&session, 100.0, tl), None);
    }
}

#[derive(Clone)]
struct MusicalDrag {
    session: std::sync::Arc<Session>,
    original: Samples,
    to: Samples,
    id: Option<u64>,
}

fn musical_ruler(app: &mut SoundApp, ui: &mut Ui, s: &std::sync::Arc<Session>, tl: Rect, row: Rect, kind: &str) {
    let t = Tokens::current();
    let ink = Color32::from_rgb(236, 236, 242);
    ui.painter().rect_filled(
        row,
        0.0,
        match kind {
            "meter" => t.meter_ruler,
            "key" => Color32::from_rgb(64, 52, 88),
            _ => Color32::from_rgb(40, 56, 60),
        },
    );
    let events: Vec<(Samples, String, Option<u64>)> = match kind {
        "key" => s.key_signatures.iter().map(|(at, key)| (*at, key.clone(), None)).collect(),
        "meter" => s
            .tempo
            .meters()
            .iter()
            .map(|m| (s.tempo.tick_to_samples(m.tick, s.sample_rate), format!("{}/{}", m.numerator, m.denominator), None))
            .collect(),
        _ => s.chords.iter().map(|c| (c.at, c.symbol.label(), Some(c.id.0))).collect(),
    };
    let resp = ui.interact(row, ui.scope_id().with(("musical_ruler", kind)), Sense::click_and_drag());
    let drag_id = ui.scope_id().with(("musical_drag", kind));
    let mut drag = ui.ctx().data_mut(|d| d.get_temp::<MusicalDrag>(drag_id));
    if drag.as_ref().is_some_and(|d| !std::sync::Arc::ptr_eq(&d.session, s)) || ui.input(|i| !i.focused || i.key_pressed(egui::Key::Escape)) {
        drag = None;
    }
    let hit = |p: egui::Pos2| {
        events.iter().rev().find(|(at, label, _)| {
            let x = x_of(s, tl, *at);
            p.x >= x - 5.0 && p.x < x + 8.0 + label.chars().count() as f32 * 7.0
        })
    };
    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        && let Some((at, _, id)) = hit(origin)
    {
        drag = Some(MusicalDrag { session: s.clone(), original: *at, to: *at, id: *id });
    }
    if let Some(d) = drag.as_mut()
        && resp.dragged()
    {
        d.to = snap(
            s,
            d.original
                .saturating_add(soundcraft_time::to_samples(f64::from(resp.total_drag_delta().unwrap_or_default().x) * s.edit.zoom.samples_per_px)),
        )
        .clamp(0, soundcraft_time::MAX_POSITION);
    }
    if resp.drag_stopped()
        && let Some(d) = drag.take()
        && d.original != d.to
    {
        let mut form = crate::dialogs::conductor::Conductor::edit(s, kind, d.original, d.id);
        form.at = d.to;
        let (command, params) = form.command();
        let _ = app.run(&command, params);
    }
    let menu_id = ui.scope_id().with(("musical_menu", kind));
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let event = hit(p);
        let (at, id) = event.map_or((snap(s, sample_at(s, tl, p.x).max(0)), None), |(at, _, id)| (*at, *id));
        ui.ctx()
            .data_mut(|d| d.insert_temp(menu_id, RulerMenu::Conductor { kind: kind.into(), at, id, existing: event.is_some(), session: s.clone() }));
    }
    // Draw every frame, including when the pointer leaves the ruler for the popup.
    // Keep the right-click target fixed until another right-click or document change.
    let menu = ui.ctx().data_mut(|d| d.get_temp::<RulerMenu>(menu_id));
    resp.context_menu(|ui| {
        if let Some(target) = &menu {
            menu_contents(app, ui, target);
        } else {
            ui.close();
        }
    });
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.double_clicked() {
            let form = if let Some((at, _, id)) = hit(p) {
                crate::dialogs::conductor::Conductor::edit(s, kind, *at, *id)
            } else {
                crate::dialogs::conductor::Conductor::new(s, kind, snap(s, sample_at(s, tl, p.x).max(0)), None)
            };
            app.dialogs.open = Some(crate::dialogs::Dialog::Conductor(form));
        } else if resp.clicked()
            && ui.input(|i| i.modifiers.alt)
            && let Some((at, _, id)) = hit(p)
        {
            let command = match kind {
                "meter" => "event.meter_remove",
                "key" => "event.key_remove",
                _ => "chord.delete",
            };
            let _ = app.run(command, json!({"at":at,"id":id}));
        }
    }
    ui.ctx().data_mut(|d| {
        d.remove::<MusicalDrag>(drag_id);
        if let Some(drag) = drag.clone() {
            d.insert_temp(drag_id, drag);
        }
    });
    let painter = ui.painter().with_clip_rect(row);
    if kind == "key" && events.is_empty() {
        painter.text(pos2(row.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, "C major", bold(11.0), ink);
    }
    for (index, (at, label, id)) in events.iter().enumerate() {
        let at = drag.as_ref().filter(|d| d.original == *at && d.id == *id).map_or(*at, |d| d.to);
        let x = x_of(s, tl, at);
        painter.add(Shape::convex_polygon(
            vec![pos2(x, row.min.y + 3.0), pos2(x + 5.0, row.center().y), pos2(x, row.max.y - 3.0)],
            ink,
            Stroke::NONE,
        ));
        let next = events.get(index + 1).map_or(row.max.x, |(at, _, _)| x_of(s, tl, *at) - 4.0);
        let label_rect = Rect::from_min_max(pos2((x + 8.0).max(row.min.x), row.min.y), pos2(next.min(row.max.x), row.max.y));
        if label_rect.width() > 0.0 {
            painter.with_clip_rect(label_rect).text(pos2(x + 8.0, row.center().y), Align2::LEFT_CENTER, label, bold(11.0), ink);
        }
    }
}

fn key_staff(painter: &egui::Painter, s: &Session, tl: Rect, row: Rect) {
    let ink = Tokens::current().ruler_text;
    painter.rect_filled(row, 0.0, Tokens::current().ruler_bg);
    for line in 0..5 {
        let y = row.min.y + 14.0 + line as f32 * 7.0;
        painter.line_segment([pos2(row.min.x, y), pos2(row.max.x, y)], Stroke::new(0.7, ink.gamma_multiply(0.6)));
    }
    let first = sample_at(s, tl, tl.min.x);
    let active = s.key_signatures.iter().rev().find(|(at, _)| *at <= first).map_or("C major", |(_, k)| k.as_str());
    let changes = std::iter::once((first, active)).chain(s.key_signatures.iter().filter(|(at, _)| *at > first).map(|(at, k)| (*at, k.as_str())));
    for (at, key) in changes {
        let Some(fifths) = soundcraft_model::key_fifths(key) else { continue };
        let x = x_of(s, tl, at) + 10.0;
        let order = if fifths >= 0 { [0, 3, -1, 2, 5, 1, 4] } else { [4, 1, 5, 2, 6, 3, 7] };
        for (i, step) in order.iter().take(usize::from(fifths.unsigned_abs())).enumerate() {
            let cx = x + i as f32 * 10.0;
            let y = row.min.y + 14.0 + *step as f32 * 3.5;
            if fifths > 0 {
                for dx in [-2.0, 2.0] {
                    painter.line_segment([pos2(cx + dx, y - 7.0), pos2(cx + dx, y + 7.0)], Stroke::new(1.0, ink));
                }
                for dy in [-2.5, 2.5] {
                    painter.line_segment([pos2(cx - 4.0, y + dy + 1.0), pos2(cx + 4.0, y + dy - 1.0)], Stroke::new(1.7, ink));
                }
            } else {
                painter.line_segment([pos2(cx - 2.0, y - 10.0), pos2(cx - 2.0, y + 4.0)], Stroke::new(1.3, ink));
                painter.add(Shape::line(
                    vec![pos2(cx - 2.0, y), pos2(cx + 2.0, y - 2.0), pos2(cx + 4.0, y), pos2(cx + 3.0, y + 2.0), pos2(cx - 2.0, y + 5.0)],
                    Stroke::new(1.3, ink),
                ));
            }
        }
    }
}

pub fn ruler_height_for(s: &Session, id: &str) -> f32 {
    if id == "tempo_editor" { s.edit.value("tempo_editor.height", 112.0).clamp(56.0, 400.0) as f32 } else { ruler_height(id) }
}
