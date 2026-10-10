//! Continuous timeline zoom for trackpads and other pointing devices.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(noundo "view.zoom_at", "Zoom at Cursor", [], None,
        "{factor?: 1, anchor_px?: 0} — continuous horizontal zoom, keeping the sample under the anchor fixed",
        always, zoom_at)]
}

fn zoom_at(e: &mut Engine, p: &Value) -> Result<Value> {
    let factor = p
        .get("factor")
        .map_or(Some(1.0), Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| bad("view.zoom_at", "`factor` must be a finite positive number"))?;
    let anchor_px = p
        .get("anchor_px")
        .map_or(Some(0.0), Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| bad("view.zoom_at", "`anchor_px` must be a finite non-negative number"))?
        .min(1_000_000.0);
    let zoom = &e.session().edit.zoom;
    let old = if zoom.samples_per_px.is_finite() { zoom.samples_per_px.clamp(0.25, 1_000_000.0) } else { 1024.0 };
    let new = (old / factor).clamp(0.25, 1_000_000.0);
    let anchor = zoom.scroll.max(0) as f64 + anchor_px * old;
    let scroll = soundcraft_time::to_samples(anchor - anchor_px * new).max(0);
    if new != zoom.samples_per_px || scroll != zoom.scroll {
        let zoom = &mut e.session_mut().edit.zoom;
        zoom.samples_per_px = new;
        zoom.scroll = scroll;
    }
    Ok(json!({"samples_per_px": new, "scroll": scroll}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_zoom_keeps_the_sample_under_the_cursor_fixed() {
        let mut e = Engine::default();
        e.execute("view.zoom_set", &json!({"samples_per_px": 1_000})).unwrap();
        e.execute("view.scroll", &json!({"to": 20_000})).unwrap();
        for factor in [1.25, 1.1, 0.9, 0.8] {
            let z = &e.session().edit.zoom;
            let before = z.scroll as f64 + 200.0 * z.samples_per_px;
            e.execute("view.zoom_at", &json!({"factor": factor, "anchor_px": 200})).unwrap();
            let z = &e.session().edit.zoom;
            assert!((before - (z.scroll as f64 + 200.0 * z.samples_per_px)).abs() <= 0.5);
        }
    }

    #[test]
    fn zoom_gestures_reject_invalid_input_and_cap_extremes() {
        let mut e = Engine::default();
        for p in [json!({"factor": 0}), json!({"factor": -1}), json!({"factor": "NaN"}), json!({"anchor_px": -1})] {
            assert!(e.execute("view.zoom_at", &p).is_err());
        }
        for factor in [f64::MAX, f64::MIN_POSITIVE, 1.0] {
            e.execute("view.zoom_at", &json!({"factor": factor, "anchor_px": f64::MAX})).unwrap();
            let z = &e.session().edit.zoom;
            assert!((0.25..=1_000_000.0).contains(&z.samples_per_px));
            assert!(z.scroll >= 0);
        }
        e.execute("view.scroll", &json!({"to": i64::MAX})).unwrap();
        e.execute("view.scroll", &json!({"by_px": f64::MAX})).unwrap();
        assert_eq!(e.session().edit.zoom.scroll, i64::MAX);
    }
}
