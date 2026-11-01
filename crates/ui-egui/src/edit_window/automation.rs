use super::layout::{sample_at, x_of};
use super::{Align2, AutoParam, Color32, Pos2, Rect, Samples, Session, Shape, Stroke, Tokens, Track, TrackKind, pos2, regular};

pub(super) fn draw_automation(painter: &egui::Painter, s: &Session, track: &Track, p: &AutoParam, tl: Rect, lane: Rect) {
    let t = Tokens::current();
    let range = lane_range(track, p);
    let def = match p {
        AutoParam::Volume => track.mixer.volume_db,
        AutoParam::Pan(i) => track.mixer.pan.get(usize::from(*i)).copied().unwrap_or(0.0),
        _ => 0.0,
    };
    let yv = |v: f32| auto_y(lane, range, v);
    let lanes = track.lane(p);
    let pts: Vec<(f32, f32)> = match lanes {
        Some(l) if !l.points.is_empty() => {
            let mut v: Vec<(f32, f32)> = Vec::new();
            v.push((lane.min.x, yv(l.value_at(sample_at(s, tl, lane.min.x), def))));
            for pt in &l.points {
                let x = x_of(s, tl, pt.at);
                if x > lane.min.x && x < lane.max.x {
                    v.push((x, yv(pt.value)));
                }
            }
            v.push((lane.max.x, yv(l.value_at(sample_at(s, tl, lane.max.x), def))));
            v
        }
        _ => vec![(lane.min.x, yv(def)), (lane.max.x, yv(def))],
    };
    let line: Vec<Pos2> = pts.iter().map(|(x, y)| pos2(*x, *y)).collect();
    painter.add(Shape::line(line, Stroke::new(1.5, t.automation_line)));
    if let Some(l) = lanes {
        for pt in &l.points {
            let x = x_of(s, tl, pt.at);
            if x > lane.min.x - 4.0 && x < lane.max.x + 4.0 {
                painter.circle(pos2(x, yv(pt.value)), 3.0, Color32::from_rgb(40, 40, 40), Stroke::new(1.5, t.automation_line));
            }
        }
    }
    painter.text(pos2(lane.min.x + 6.0, lane.min.y + 9.0), Align2::LEFT_CENTER, p.label(), regular(10.0), t.text_dim);
}

/// The values an automation lane spans, bottom to top.
pub(super) fn lane_range(track: &Track, p: &AutoParam) -> (f32, f32) {
    if matches!(p, AutoParam::Volume | AutoParam::SendLevel(_)) {
        (-60.0, 12.0)
    } else if matches!(p, AutoParam::Plugin { .. }) {
        plugin_range(track, p)
    } else {
        let (lo, hi, _) = p.range();
        (lo, hi)
    }
}

fn auto_y(lane: Rect, (lo, hi): (f32, f32), v: f32) -> f32 {
    let k = ((v.clamp(lo, hi) - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0);
    lane.max.y - 4.0 - k * (lane.height() - 8.0)
}

pub(super) fn auto_value(lane: Rect, (lo, hi): (f32, f32), y: f32) -> f32 {
    let k = ((lane.max.y - 4.0 - y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
    lo + (hi - lo) * k
}

/// The automation a track's lane draws: its view's, or volume on the tracks that always show it.
pub(super) fn shown_automation(track: &Track) -> Option<AutoParam> {
    AutoParam::parse(&track.view).or_else(|| matches!(track.kind, TrackKind::Master | TrackKind::Aux | TrackKind::Vca).then_some(AutoParam::Volume))
}

/// The drawn breakpoint nearest `pos`, within grabbing distance.
pub(super) fn breakpoint_at(s: &Session, tl: Rect, lane: Rect, track: &Track, p: &AutoParam, pos: Pos2) -> Option<Samples> {
    let range = lane_range(track, p);
    let dist = |at: Samples, v: f32| pos2(x_of(s, tl, at), auto_y(lane, range, v)).distance(pos);
    track
        .lane(p)?
        .points
        .iter()
        .map(|pt| (pt.at, dist(pt.at, pt.value)))
        .filter(|(_, d)| *d <= 6.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(at, _)| at)
}

pub(super) fn plugin_range(track: &Track, p: &AutoParam) -> (f32, f32) {
    if let AutoParam::Plugin { slot, param } = p
        && let Some(Some(ins)) = track.mixer.inserts.get(usize::from(*slot))
        && let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin)
        && let Some(pi) = info.param(param)
    {
        return (pi.min, pi.max);
    }
    (0.0, 1.0)
}

pub(super) fn automation_id(p: &AutoParam) -> String {
    match p {
        AutoParam::Volume => "volume".into(),
        AutoParam::Pan(0) => "pan".into(),
        AutoParam::Pan(_) => "pan2".into(),
        AutoParam::Mute => "mute".into(),
        AutoParam::SendLevel(i) => format!("send_{}_level", slot_id(*i)),
        AutoParam::SendPan(i) => format!("send_{}_pan", slot_id(*i)),
        AutoParam::SendMute(i) => format!("send_{}_mute", slot_id(*i)),
        AutoParam::Plugin { slot, param } => format!("plugin:{}:{param}", slot_id(*slot)),
    }
}

/// A slot as `AutoParam::parse` reads it: a letter for 0..=25, the number beyond that.
fn slot_id(i: u8) -> String {
    if i < 26 { char::from(b'a' + i).to_string() } else { i.to_string() }
}
