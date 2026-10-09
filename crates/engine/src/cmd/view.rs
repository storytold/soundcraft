//! View menu commands that change saved view state (zoom, rulers, counters, grid/nudge).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_time::{GridValue, NoteValue, TimeFormat};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(view "view.zoom_in", "Zoom In", [], Some("Cmd+]"), "{}", always, |e, _| zoom(e, 0.5, None)),
        cmd!(view "view.zoom_out", "Zoom Out", [], Some("Cmd+["), "{}", always, |e, _| zoom(e, 2.0, None)),
        cmd!(view "view.zoom_by", "Zoom By", [], None, "{factor: >1 zooms in, x?: px into the timeline to hold (else the insertion point if on screen)}", always, |e, p| {
            let f = p.get("factor").and_then(Value::as_f64).filter(|f| *f > 0.0).ok_or_else(|| bad("view.zoom_by", "factor must be a positive number"))?;
            zoom(e, 1.0 / f, p.get("x").and_then(Value::as_f64))
        }),
        cmd!(view "view.zoom_set", "Set Zoom", [], None, "{samples_per_px}", always, |e, p| {
            let v = f64_or(p, "samples_per_px", 1024.0).clamp(MIN_SPP, MAX_SPP);
            e.zoom_mut().samples_per_px = v;
            Ok(json!({"samples_per_px": v}))
        }),
        cmd!(view "view.zoom_preset", "Zoom Preset", [], Some("Cmd+Ctrl+1-5"), "{preset: 1..5, store?: bool}", always, |e, p| {
            let i = (i64_or(p, "preset", 1).clamp(1, 5) - 1) as usize;
            let z = e.zoom_mut();
            if bool_or(p, "store", false) {
                if let Some(slot) = z.presets.get_mut(i) { *slot = z.samples_per_px }
            } else if let Some(v) = z.presets.get(i) {
                z.samples_per_px = v.clamp(MIN_SPP, MAX_SPP);
            }
            Ok(json!({"samples_per_px": z.samples_per_px}))
        }),
        cmd!(view "view.zoom_to_selection", "Zoom to Selection", [], Some("Alt+F"), "{width_px?: 1200}", always, |e, p| {
            let w = f64_or(p, "width_px", 1200.0).max(100.0);
            let r = e.session().edit.selection;
            if !r.is_empty() {
                let z = e.zoom_mut();
                z.samples_per_px = (r.len() as f64 / w).clamp(MIN_SPP, MAX_SPP);
                z.scroll = r.start;
            }
            Ok(json!({}))
        }),
        cmd!(view "view.zoom_fit", "Fill Window With Session", [], Some("Alt+A"), "{width_px?: 1200}", always, |e, p| {
            let w = f64_or(p, "width_px", 1200.0).max(100.0);
            let s = e.session();
            let end = s.content_end().max(s.sample_rate.samples(30.0));
            let z = e.zoom_mut();
            z.samples_per_px = (end as f64 * 1.05 / w).clamp(MIN_SPP, MAX_SPP);
            z.scroll = 0;
            Ok(json!({}))
        }),
        cmd!(view "view.scroll", "Scroll Timeline", [], None, "{to?: position, by_px?: n}", always, |e, p| {
            let to = position_param(e, "view.scroll", p, "to")?;
            let by = f64_or(p, "by_px", 0.0);
            let z = e.zoom_mut();
            let by = soundcraft_time::to_samples(by * z.samples_per_px);
            z.scroll = to.unwrap_or(z.scroll.saturating_add(by)).clamp(0, MAX_POSITION);
            Ok(json!({"scroll": z.scroll}))
        }),
        cmd!(view "view.waveform_zoom", "Waveform Zoom", [], Some("Cmd+Alt+[ / ]"), "{factor?: 2 | value}", always, |e, p| {
            let z = e.zoom_mut();
            let w = if let Some(v) = p.get("value").and_then(Value::as_f64) { v as f32 } else { z.waveform_zoom * f32_or(p, "factor", 2.0) };
            z.waveform_zoom = if w.is_finite() { w.clamp(0.25, 64.0) } else { 1.0 };
            Ok(json!({"waveform_zoom": z.waveform_zoom}))
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

const MIN_SPP: f64 = 0.25;
const MAX_SPP: f64 = 1_000_000.0;

fn zoom(e: &mut Engine, f: f64, x: Option<f64>) -> Result<Value> {
    let insertion = e.session().edit.selection.start as f64;
    let z = e.zoom_mut();
    let (old, scroll) = (z.samples_per_px, z.scroll as f64);
    let new = (old * f).clamp(MIN_SPP, MAX_SPP);
    if let Some(x) = x.or_else(|| Some((insertion - scroll) / old).filter(|x| (0.0..4000.0).contains(x))) {
        z.scroll = soundcraft_time::to_samples(scroll + x * (old - new)).clamp(0, MAX_POSITION);
    }
    z.samples_per_px = new;
    Ok(json!({"samples_per_px": new, "scroll": z.scroll}))
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

    fn view(e: &Engine) -> (f64, Samples) {
        (e.session().edit.zoom.samples_per_px, e.session().edit.zoom.scroll)
    }

    #[test]
    fn zoom_by_holds_the_position_under_the_pointer() {
        let mut e = Engine::default();
        e.execute("view.zoom_set", &json!({"samples_per_px": 100.0})).unwrap();
        e.execute("view.scroll", &json!({"to": 48_000})).unwrap();
        e.execute("view.zoom_by", &json!({"factor": 2.0, "x": 300.0})).unwrap();
        assert_eq!(view(&e), (50.0, 78_000 - 300 * 50));
        e.execute("view.zoom_by", &json!({"factor": 0.5, "x": 300.0})).unwrap();
        assert_eq!(view(&e), (100.0, 48_000));
    }

    #[test]
    fn zoom_and_scroll_stay_in_range() {
        let mut e = Engine::default();
        e.execute("edit.select", &json!({"start": 0, "end": 4_000_000_000_i64})).unwrap();
        e.execute("view.zoom_to_selection", &json!({})).unwrap();
        assert_eq!(view(&e).0, MAX_SPP);
        e.execute("view.zoom_by", &json!({"factor": 1e12, "x": 0.0})).unwrap();
        assert_eq!(view(&e).0, MIN_SPP);
        e.execute("view.zoom_by", &json!({"factor": 1e-300, "x": 1e300})).unwrap();
        assert_eq!(view(&e), (MAX_SPP, 0));
        for bad in [json!(0.0), json!(-2.0), json!("2"), json!(null)] {
            assert!(e.execute("view.zoom_by", &json!({"factor": bad})).is_err(), "{bad}");
        }
        e.execute("view.scroll", &json!({"by_px": 1e300})).unwrap();
        assert_eq!(view(&e).1, MAX_POSITION);
        e.execute("view.scroll", &json!({"by_px": -1e300})).unwrap();
        assert_eq!(view(&e).1, 0);
    }

    #[test]
    fn view_commands_skip_undo_the_journal_and_session_copies() {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({})).unwrap()["tracks"][0].clone();
        let steps = e.undo_history().len();
        for db in [-1.0, -2.0] {
            e.execute_merged("mix.volume", &json!({"track": t, "db": db}), "fader").unwrap();
            e.execute("view.scroll", &json!({"by_px": 5.0})).unwrap();
        }
        assert_eq!(e.undo_history().len(), steps + 1);
        let state = |e: &Engine| (std::ptr::from_ref(e.session()), e.revision, e.journal.len());
        let before = state(&e);
        e.execute("view.zoom_by", &json!({"factor": 2.0})).unwrap();
        e.execute("view.scroll", &json!({"by_px": 1.0})).unwrap();
        assert_eq!(state(&e), before);
    }
}
