//! The graph of a fade dialog, the tool column beside it and the small controls of the dialog.

use crate::fade_dialog::{FadeEditor, Kind, Shape, Side, View};
use crate::icons;
use crate::theme::{Tokens, clip_colors, fade_colors, is_light, regular};
use crate::widgets::{key, key_fill, shade};
use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::{
    Align2, Color32, CornerRadius, Mesh, Painter, Rect, Response, Role, Sense, Shape as Draw, Stroke, StrokeKind, Ui, WidgetInfo, pos2, vec2,
};
use soundcraft_model::{ClipContent, ClipId, Fade, FadeShape, Session};
use soundcraft_time::Range;

/// Why a control is greyed while COMPARE shows the settings the dialog opened with.
pub const COMPARING: &str = "COMPARE is on and shows the settings this dialog opened with. Turn it off to edit.";
pub const COMPARE_TIP: &str = "Show the settings this dialog opened with. Turn it off to go back to your changes.";

const PLOT_HEIGHT: f32 = 176.0;
/// Vertical waveform zoom: each click is a factor of 1.4, kept between these.
const ZOOM: (f32, f32) = (0.5, 2.8);
const STEP: f32 = 1.4;
/// The width of the preset-curve thumbnail button, and the least width of the menu it opens.
const THUMB_W: f32 = 46.0;

/// The graph's colours: the outgoing side (A) and the incoming side (B), each as a fill and as a line.
struct Tones {
    fill: [Color32; 2],
    line: [Color32; 2],
}

fn tones() -> Tones {
    let [a, b] = fade_colors();
    // Lines take the waveform shade of the clip colour, as the Edit window draws it in either theme.
    let line = |c: Color32| if is_light() { shade(c, -0.3) } else { clip_colors([c.r(), c.g(), c.b()], false).2 };
    Tones { fill: [a, b], line: [line(a), line(b)] }
}

fn tool_style(radius: CornerRadius) -> IconButtonStyle {
    let t = Tokens::current();
    let visuals = |fill, stroke| ButtonVisuals { fill, stroke: Stroke::new(1.0, stroke), icon: t.text };
    IconButtonStyle {
        normal: visuals(t.button, t.button_border),
        hovered: visuals(t.button_hi, t.button_border),
        pressed: Some(visuals(t.slot_bg, t.button_border)),
        selected: visuals(t.accent_dark, t.accent),
        corner_radius: radius,
        focus_stroke: Stroke::new(1.0, t.accent),
    }
}

/// The corners a key rounds in a row or column of `n` keys, for the `i`th: only the outer ones.
fn ends(i: usize, n: usize, vertical: bool) -> CornerRadius {
    let (first, last) = (if i == 0 { 4 } else { 0 }, if i + 1 == n { 4 } else { 0 });
    if vertical { CornerRadius { nw: first, ne: first, sw: last, se: last } } else { CornerRadius { nw: first, sw: first, ne: last, se: last } }
}

/// The tool column (display modes, zoom) and the graph beside it.
pub fn work_area(ui: &mut Ui, ed: &mut FadeEditor, s: &Session) {
    let (t, tn) = (Tokens::current(), tones());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, (view, label)) in View::ALL.into_iter().enumerate() {
                if i > 0 {
                    ui.add_space(-1.0);
                }
                let style = tool_style(ends(i, View::ALL.len(), true));
                let colors = [tn.line[0], tn.line[1], t.fade_combined];
                let resp = IconButton::new(label, vec2(30.0, 28.0), &style).selected(ed.view == view).show(ui, |ui, r, _| {
                    icons::fade_view(ui.painter(), Rect::from_center_size(r.center(), vec2(22.0, 22.0)), i, colors);
                });
                if resp.on_hover_text(label).clicked() {
                    ed.view = view;
                }
            }
            ui.add_space(8.0);
            let (bigger, smaller) = if ed.kind() == Kind::Crossfade { ("chevron_up", "chevron_down") } else { ("plus", "minus") };
            for (i, (icon, label, up)) in [(bigger, "Zoom waveform in", true), (smaller, "Zoom waveform out", false)].into_iter().enumerate() {
                if i > 0 {
                    ui.add_space(1.0);
                }
                let limit = if up { ed.zoom >= ZOOM.1 } else { ed.zoom <= ZOOM.0 };
                let why = if ed.view == View::Curves {
                    Some("Curves only shows no waveform. Pick another display mode to zoom it.")
                } else {
                    limit.then_some(if up { "The waveform is as large as it goes." } else { "The waveform is as small as it goes." })
                };
                let style = tool_style(CornerRadius::same(4));
                let resp = ui
                    .add_enabled_ui(why.is_none(), |ui| {
                        IconButton::new(label, vec2(30.0, 28.0), &style).show(ui, |ui, r, tint| {
                            icons::draw(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), icon, tint);
                        })
                    })
                    .inner
                    .on_hover_text(label);
                if let Some(why) = why {
                    resp.on_disabled_hover_text(why);
                } else if resp.clicked() {
                    ed.zoom = (if up { ed.zoom * STEP } else { ed.zoom / STEP }).clamp(ZOOM.0, ZOOM.1);
                }
            }
        });
        plot(ui, ed, s, ui.available_width());
    });
}

/// Per column of the span, the loudest peak of the clip's audio there, across its channels (0..1).
/// None when the clip has no audio to show: MIDI, or a file that is not loaded.
fn column_peaks(s: &Session, clip: Option<ClipId>, span: Range, cols: usize) -> Option<Vec<f32>> {
    let (_, c) = s.find_clip(clip?)?;
    let ClipContent::Audio { source, .. } = c.content else { return None };
    let audio = s.pool.get(source)?;
    let (start, len) = (span.start as f64, span.len().max(1) as f64);
    let origin = c.source_offset() as f64 - c.start as f64;
    let at = |i: usize| (start + len * i as f64 / cols as f64 + origin).max(0.0) as usize;
    Some(
        (0..cols)
            .map(|i| {
                let (from, to) = (at(i), at(i + 1).max(at(i).saturating_add(1)));
                (0..audio.peaks.len())
                    .filter_map(|ch| {
                        let (lo, hi) = audio.peaks.get(ch)?.minmax(audio.channel(ch)?, from, to);
                        Some(lo.abs().max(hi.abs()))
                    })
                    .fold(0.0, f32::max)
                    .min(1.0)
            })
            .map(|v| if v.is_finite() { v } else { 0.0 })
            .collect(),
    )
}

/// A band between two edges, one quad per column: `(x, top, bottom)`.
fn band(p: &Painter, cols: &[(f32, f32, f32)], color: Color32) {
    let mut mesh = Mesh::default();
    for pair in cols.windows(2) {
        if let [a, b] = pair {
            let first = mesh.vertices.len() as u32;
            for (x, y) in [(a.0, a.1), (b.0, b.1), (b.0, b.2), (a.0, a.2)] {
                mesh.colored_vertex(pos2(x, y), color);
            }
            mesh.add_triangle(first, first + 1, first + 2);
            mesh.add_triangle(first, first + 2, first + 3);
        }
    }
    p.add(mesh);
}

/// One side of the graph: A goes out, B comes in.
struct Lane {
    present: bool,
    out: bool,
    fade: Fade,
    clip: Option<ClipId>,
    fill: Color32,
    line: Color32,
    /// Centre line of its waveform.
    cy: f32,
}

impl Lane {
    /// Gain at `u` (0..1) along the span.
    fn gain(&self, u: f32) -> f32 {
        self.fade.gain(if self.out { 1.0 - u } else { u })
    }
}

/// The graph: gain 0..1 up the side, the span of the fade across. Out is amber, in turquoise. Drawn from the dialog's
/// state, over the waveforms of the two clips as the display mode asks.
fn plot(ui: &mut Ui, ed: &FadeEditor, s: &Session, width: f32) {
    let (t, tn) = (Tokens::current(), tones());
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(80.0), PLOT_HEIGHT), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, "Fade curves"));
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, t.slot_bg);
    let r = rect.shrink(1.0);
    let (kind, view, set) = (ed.kind(), ed.view, ed.shown());
    let lanes = view == View::Lanes;
    let cy = |share: f32| if lanes { r.min.y + r.height() * share } else { r.center().y };
    let sides = [
        Lane { present: kind != Kind::In, out: true, fade: set.output.fade(), clip: ed.a, fill: tn.fill[0], line: tn.line[0], cy: cy(0.25) },
        Lane { present: kind != Kind::Out, out: false, fade: set.input.fade(), clip: ed.b, fill: tn.fill[1], line: tn.line[1], cy: cy(0.75) },
    ];
    let (lo, hi) = (r.max.y - 8.0, r.min.y + 8.0);
    let cols = ((r.width() / 2.0) as usize).clamp(32, 400);
    let x = |i: usize| r.min.x + r.width() * i as f32 / cols as f32;
    let y_of = |g: f32| lo + (hi - lo) * g;
    let amp = r.height() * if lanes { 0.17 } else { 0.3 } * ed.zoom;
    let half = |cy: f32, v: f32| (amp * v).clamp(0.5, (cy - r.min.y - 2.0).min(r.max.y - cy - 2.0).max(0.5));
    // ponytail: the waveforms are the files' peaks per column, scaled by the curves; the combined one adds the two
    // peaks rather than the samples, so it is an upper bound where the two cancel. Recomputed each frame: a few
    // hundred peak lookups per clip.
    let waves: Vec<Option<Vec<f32>>> =
        sides.iter().map(|l| if view == View::Curves || !l.present { None } else { column_peaks(s, l.clip, ed.span, cols) }).collect();
    let curve = |l: &Lane| -> Vec<(f32, f32)> { (0..=cols).map(|i| (x(i), l.gain(i as f32 / cols as f32))).collect() };
    if view != View::Curves {
        for l in sides.iter().filter(|l| l.present) {
            let edge: Vec<_> = curve(l).into_iter().map(|(x, g)| (x, y_of(g), r.max.y)).collect();
            band(&p, &edge, l.fill.gamma_multiply(0.16));
        }
    }
    let centred = |peak_at: &dyn Fn(usize) -> f32, cy: f32| -> Vec<(f32, f32, f32)> {
        let edges = (0..cols).map(|i| {
            let h = half(cy, peak_at(i));
            (x(i), cy - h, cy + h)
        });
        edges.chain(std::iter::once((x(cols), cy, cy))).collect()
    };
    match view {
        View::Lanes | View::Overlaid => {
            for (l, w) in sides.iter().zip(&waves) {
                if let Some(w) = w {
                    let at = |i: usize| w.get(i).copied().unwrap_or(0.0) * l.gain((i as f32 + 0.5) / cols as f32);
                    band(&p, &centred(&at, l.cy), l.fill.gamma_multiply(0.7));
                }
            }
        }
        View::Combined if waves.iter().any(Option::is_some) => {
            let at = |i: usize| -> f32 {
                let u = (i as f32 + 0.5) / cols as f32;
                sides.iter().zip(&waves).map(|(l, w)| w.as_ref().and_then(|w| w.get(i)).map_or(0.0, |peak| peak * l.gain(u))).sum()
            };
            band(&p, &centred(&at, r.center().y), t.text_dim.gamma_multiply(0.8));
        }
        View::Combined | View::Curves => {}
    }
    // A lane without a waveform keeps a flat line: Fade In has no outgoing clip, Fade Out no incoming one.
    if lanes {
        for (l, _) in sides.iter().zip(&waves).filter(|(_, w)| w.is_none()) {
            p.hline(r.x_range(), l.cy, Stroke::new(1.0, l.line));
        }
    }
    for l in sides.iter().filter(|l| l.present) {
        let points = curve(l).into_iter().map(|(x, g)| pos2(x, y_of(g))).collect();
        p.add(Draw::line(points, Stroke::new(2.0, l.line)));
    }
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
}

/// One of the five preset slots, in a row of keys. Empty ones are dim; a plain click on them does nothing.
pub fn slot_button(ui: &mut Ui, n: usize, filled: bool, selected: bool, enabled: bool) -> Response {
    let t = Tokens::current();
    let label = format!("Preset {}", n + 1);
    let mut style = tool_style(ends(n, 5, false));
    if !filled {
        style.normal.icon = t.text_dim;
        style.hovered.icon = t.text_dim;
    }
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            IconButton::new(&label, vec2(22.0, 22.0), &style).selected(selected).show(ui, |ui, r, tint| {
                ui.painter().text(r.center(), Align2::CENTER_CENTER, n + 1, regular(12.0), tint);
            })
        })
        .inner;
    let tip = if filled {
        format!("{label}. Click to recall it, Cmd-click to store the current settings here.")
    } else {
        format!("{label} is empty. Cmd-click to store the current settings here.")
    };
    if enabled { resp.on_hover_text(tip) } else { resp.on_disabled_hover_text(COMPARING) }
}

/// A text button that stays down while on.
pub fn toggle_button(ui: &mut Ui, label: &str, on: bool, tip: &str) -> Response {
    let t = Tokens::current();
    let galley = ui.painter().layout_no_wrap(label.to_string(), regular(12.0), t.text);
    let resp = ui.allocate_response(vec2(galley.size().x + 24.0, 22.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), on, label));
    let p = ui.painter();
    key(p, resp.rect, &resp, if on { t.accent_dark } else { key_fill(&resp) });
    if on {
        p.rect_stroke(resp.rect, 3.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    p.galley(resp.rect.center() - galley.size() * 0.5, galley, t.text);
    if resp.has_focus() {
        p.rect_stroke(resp.rect.shrink(2.0), 2.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    resp.on_hover_text(tip)
}

/// The curve of a preset thumbnail in `r`, as the fade would run: in rises, out falls.
fn thumb_curve(p: &Painter, r: Rect, fade: Fade, out: bool, color: Color32) {
    let points = (0..=24)
        .map(|i| {
            let u = i as f32 / 24.0;
            let g = fade.gain(if out { 1.0 - u } else { u });
            pos2(r.min.x + r.width() * (4.0 + 32.0 * u) / 40.0, r.min.y + r.height() * (22.0 - 18.0 * g) / 26.0)
        })
        .collect();
    p.add(Draw::line(points, Stroke::new(1.5, color)));
}

/// The third choice of a shape group: a radio with the preset curve's thumbnail beside it, which opens the menu of
/// preset curves. Returns the preset picked this frame, if any; `why` greys it and says why on hover.
pub fn preset_choice(ui: &mut Ui, side: Side, out: bool, why: Option<&str>) -> Option<FadeShape> {
    let t = Tokens::current();
    let on = matches!(side.shape, Shape::Preset(_));
    // Not chosen yet, the thumbnail shows the first curve of the menu.
    let fade = match side.shape {
        Shape::Preset(shape) => Fade { len: 0, shape, curvature: side.bend },
        _ => Fade { len: 0, shape: FadeShape::Exponential, curvature: 0.0 },
    };
    let (radio, thumb) = ui
        .add_enabled_ui(why.is_none(), |ui| {
            let radio = ui.add(egui::RadioButton::new(on, ""));
            radio.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), on, "Preset curve"));
            let (rect, resp) = ui.allocate_exact_size(vec2(THUMB_W, 30.0), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), "Choose a preset curve"));
            let p = ui.painter();
            p.rect_filled(rect, 4.0, if resp.is_pointer_button_down_on() { shade(t.slot_bg, -0.1) } else { t.slot_bg });
            p.rect_stroke(rect, 4.0, Stroke::new(1.0, if resp.hovered() { t.border_light } else { t.button_border }), StrokeKind::Inside);
            let [a, b] = tones().line;
            thumb_curve(p, rect, fade, out, if out { a } else { b });
            let (cx, top) = (rect.max.x - 8.0, rect.min.y + 5.0);
            p.add(Draw::convex_polygon(vec![pos2(cx - 4.0, top), pos2(cx + 4.0, top), pos2(cx, top + 4.0)], t.text_dim, Stroke::NONE));
            if resp.has_focus() {
                p.rect_stroke(rect.shrink(2.0), 3.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
            }
            (radio, resp)
        })
        .inner;
    if let Some(why) = why {
        radio.on_disabled_hover_text(why);
        thumb.on_disabled_hover_text(why);
        return None;
    }
    let mut picked = (radio.clicked() && !on).then_some(FadeShape::Exponential);
    egui::Popup::menu(&thumb.on_hover_text("Choose a preset curve")).show(|ui| {
        ui.set_min_width(THUMB_W);
        for shape in [FadeShape::Exponential, FadeShape::Logarithmic] {
            if menu_row(ui, shape, out).clicked() {
                picked = Some(shape);
            }
        }
    });
    picked
}

/// A row of the preset-curve menu: the curve's thumbnail and its name. It asks for the width of those two, never for
/// `available_width` (a popup offers the whole screen); the menu then stretches every row to the widest one.
fn menu_row(ui: &mut Ui, shape: FadeShape, out: bool) -> Response {
    let t = Tokens::current();
    let name = ui.painter().layout_no_wrap(shape.label().to_string(), regular(12.0), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(6.0 + 32.0 + 8.0 + name.size().x + 8.0, 24.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, shape.label()));
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 3.0, t.button);
    }
    let thumb = Rect::from_min_size(rect.min + vec2(6.0, 1.5), vec2(32.0, 21.0));
    let [a, b] = tones().line;
    thumb_curve(ui.painter(), thumb, Fade { len: 0, shape, curvature: 0.0 }, out, if out { a } else { b });
    ui.painter().galley(pos2(thumb.max.x + 8.0, rect.center().y - name.size().y * 0.5), name, t.text);
    resp
}
