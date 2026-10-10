//! View menu commands that change saved view state (zoom, rulers, counters, grid/nudge).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_time::{GridValue, NoteValue, TimeFormat};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "view.zoom_in", "Zoom In", [], Some("Cmd+]"), "{}", always, |e, _| zoom(e, 0.5)),
        cmd!(noundo "view.zoom_out", "Zoom Out", [], Some("Cmd+["), "{}", always, |e, _| zoom(e, 2.0)),
        cmd!(noundo "view.zoom_set", "Set Zoom", [], None, "{samples_per_px}", always, |e, p| {
            let v = f64_or(p, "samples_per_px", 1024.0).clamp(0.25, 1_000_000.0);
            e.session_mut().edit.zoom.samples_per_px = v;
            Ok(json!({"samples_per_px": v}))
        }),
        cmd!(noundo "view.zoom_preset", "Zoom Preset", [], Some("Cmd+Ctrl+1-5"), "{preset: 1..5, store?: bool}", always, |e, p| {
            let i = (i64_or(p, "preset", 1).clamp(1, 5) - 1) as usize;
            let s = e.session_mut();
            if bool_or(p, "store", false) {
                if let Some(z) = s.edit.zoom.presets.get_mut(i) { *z = s.edit.zoom.samples_per_px }
            } else if let Some(z) = s.edit.zoom.presets.get(i) {
                s.edit.zoom.samples_per_px = *z;
            }
            Ok(json!({"samples_per_px": s.edit.zoom.samples_per_px}))
        }),
        cmd!(noundo "view.zoom_to_selection", "Zoom to Selection", [], Some("Alt+F"), "{width_px?: 1200}", always, |e, p| {
            let w = f64_or(p, "width_px", 1200.0).max(100.0);
            let s = e.session_mut();
            let r = s.edit.selection;
            if !r.is_empty() {
                s.edit.zoom.samples_per_px = (r.len() as f64 / w).max(0.25);
                s.edit.zoom.scroll = r.start;
            }
            Ok(json!({}))
        }),
        cmd!(noundo "view.zoom_fit", "Fill Window With Session", [], Some("Alt+A"), "{width_px?: 1200}", always, |e, p| {
            let w = f64_or(p, "width_px", 1200.0).max(100.0);
            let s = e.session_mut();
            let end = s.content_end().max(s.sample_rate.samples(30.0));
            s.edit.zoom.samples_per_px = (end as f64 * 1.05 / w).max(0.25);
            s.edit.zoom.scroll = 0;
            Ok(json!({}))
        }),
        cmd!(noundo "view.scroll", "Scroll Timeline", [], None, "{to?: position, by_px?: n}", always, |e, p| {
            let to = position_param(e, "view.scroll", p, "to")?;
            let by = f64_or(p, "by_px", 0.0);
            let s = e.session_mut();
            let spp = s.edit.zoom.samples_per_px;
            s.edit.zoom.scroll = to.unwrap_or_else(|| s.edit.zoom.scroll.saturating_add(soundcraft_time::to_samples(by * spp))).max(0);
            Ok(json!({"scroll": s.edit.zoom.scroll}))
        }),
        cmd!(noundo "view.waveform_zoom", "Waveform Zoom", [], Some("Cmd+Alt+[ / ]"), "{factor?: 2 | value}", always, |e, p| {
            let s = e.session_mut();
            let z = if let Some(v) = p.get("value").and_then(Value::as_f64) { v as f32 } else { s.edit.zoom.waveform_zoom * f32_or(p, "factor", 2.0) };
            s.edit.zoom.waveform_zoom = if z.is_finite() { z.clamp(0.25, 64.0) } else { 1.0 };
            Ok(json!({"waveform_zoom": s.edit.zoom.waveform_zoom}))
        }),
        cmd!(noundo "view.ruler", "Rulers", ["View", "Rulers"], None, "{ruler: bars_beats|min_secs|timecode|feet_frames|samples|tempo|meter|markers|key|chords, visible?: bool}", always, ruler),
        cmd!(noundo "view.main_counter", "Main Counter", ["View", "Main Counter"], None, "{format: bars_beats|min_secs|timecode|feet_frames|samples}", always, |e, p| {
            let f = str_param(p, "format").and_then(TimeFormat::from_id).ok_or_else(|| bad("view.main_counter", "unknown format"))?;
            e.session_mut().edit.main_counter = f;
            Ok(json!({"format": f}))
        }),
        cmd!(noundo "view.sub_counter", "Sub Counter", [], None, "{format?: … | null}", always, |e, p| {
            let f = str_param(p, "format").and_then(TimeFormat::from_id);
            e.session_mut().edit.sub_counter = f;
            Ok(json!({"format": f}))
        }),
        cmd!(noundo "view.grid", "Grid Value", [], None, "{value: '1/16' | '1 bar' | '1/8t' | '1/4.' | {seconds} | {frames} | {samples}, lines?: bool}", always, |e, p| {
            let g = grid_param(p, "value").ok_or_else(|| bad("view.grid", "unknown grid value"))?;
            let lines = p.get("lines").and_then(Value::as_bool);
            let s = e.session_mut();
            s.edit.grid = g;
            if let Some(l) = lines { s.edit.grid_lines = l }
            Ok(json!({"grid": g.label()}))
        }),
        cmd!(noundo "view.grid_lines", "Show Grid Lines", [], None, "{value?: bool}", always, |e, p| {
            let s = e.session_mut();
            s.edit.grid_lines = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.grid_lines);
            Ok(json!({"value": s.edit.grid_lines}))
        }),
        cmd!(noundo "view.nudge", "Nudge Value", [], None, "{value: like grid}", always, |e, p| {
            let g = grid_param(p, "value").ok_or_else(|| bad("view.nudge", "unknown nudge value"))?;
            e.session_mut().edit.nudge = g;
            Ok(json!({"nudge": g.label()}))
        }),
    ]
}

fn zoom(e: &mut Engine, f: f64) -> Result<Value> {
    let s = e.session_mut();
    let anchor = s.edit.selection.start;
    let old = s.edit.zoom.samples_per_px;
    let new = (old * f).clamp(0.25, 1_000_000.0);
    // Keep the insertion point at the same screen x when it is visible.
    let x = (anchor - s.edit.zoom.scroll) as f64 / old;
    if (0.0..4000.0).contains(&x) {
        s.edit.zoom.scroll = (anchor - soundcraft_time::to_samples(x * new)).max(0);
    }
    s.edit.zoom.samples_per_px = new;
    Ok(json!({"samples_per_px": new}))
}

fn ruler(e: &mut Engine, p: &Value) -> Result<Value> {
    const ALL: [&str; 10] = ["bars_beats", "min_secs", "timecode", "feet_frames", "samples", "tempo", "meter", "markers", "key", "chords"];
    let r = str_param(p, "ruler").filter(|r| ALL.contains(r)).ok_or_else(|| bad("view.ruler", "unknown ruler"))?.to_string();
    let s = e.session_mut();
    let shown = s.edit.rulers.contains(&r);
    let vis = p.get("visible").and_then(Value::as_bool).unwrap_or(!shown);
    s.edit.rulers.retain(|x| x != &r);
    if vis {
        s.edit.rulers.push(r.clone());
        s.edit.rulers.sort_by_key(|x| ALL.iter().position(|a| a == x));
    }
    Ok(json!({"ruler": r, "visible": vis}))
}

/// Parse grid/nudge values.
pub fn grid_param(p: &Value, key: &str) -> Option<GridValue> {
    let v = p.get(key)?;
    if let Some(sec) = v.get("seconds").and_then(Value::as_f64) {
        return sec.is_finite().then_some(GridValue::Seconds(sec.clamp(0.000_01, 3600.0)));
    }
    if let Some(f) = v.get("frames").and_then(Value::as_i64) {
        return Some(GridValue::Frames(f.clamp(1, 100_000)));
    }
    if let Some(n) = v.get("samples").and_then(Value::as_i64) {
        return Some(GridValue::Samples(n.clamp(1, i64::from(u32::MAX))));
    }
    let s = v.as_str()?.trim().to_ascii_lowercase();
    let triplet = s.ends_with('t') || s.contains("triplet");
    let dotted = s.ends_with('.') || s.contains("dotted");
    let core: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '/' || *c == ' ' || c.is_ascii_alphabetic() && *c != 't').collect();
    let value = match core.trim() {
        "1 bar" | "bar" | "1/1" | "1" => NoteValue::Bar,
        "1/2" => NoteValue::Half,
        "1/4" => NoteValue::Quarter,
        "1/8" => NoteValue::Eighth,
        "1/16" => NoteValue::Sixteenth,
        "1/32" => NoteValue::ThirtySecond,
        "1/64" => NoteValue::SixtyFourth,
        _ => return None,
    };
    Some(GridValue::Note { value, dotted, triplet })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_values_parse() {
        assert_eq!(grid_param(&json!({"v": "1/16"}), "v"), Some(GridValue::Note { value: NoteValue::Sixteenth, dotted: false, triplet: false }));
        assert_eq!(grid_param(&json!({"v": "1/8t"}), "v"), Some(GridValue::Note { value: NoteValue::Eighth, dotted: false, triplet: true }));
        assert_eq!(grid_param(&json!({"v": "1/4."}), "v"), Some(GridValue::Note { value: NoteValue::Quarter, dotted: true, triplet: false }));
        assert_eq!(grid_param(&json!({"v": {"seconds": 0.5}}), "v"), Some(GridValue::Seconds(0.5)));
        assert_eq!(grid_param(&json!({"v": "nonsense"}), "v"), None);
    }
}
