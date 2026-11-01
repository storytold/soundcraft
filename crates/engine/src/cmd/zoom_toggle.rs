//! Zoom Toggle: zoom in on the edit selection, then back out to the view before. Cancel Zoom Toggle turns it off
//! and keeps the current view.
use super::{CommandSpec, always, f64_or};
use soundcraft_model::{TrackHeight, ZoomToggle};
use soundcraft_time::MAX_POSITION;

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "view.zoom_toggle", "Zoom Toggle", [], None, "{width_px?: 1200, height_px?: 600}", always, toggle),
        cmd!(noundo "view.zoom_toggle_cancel", "Cancel Zoom Toggle", [], Some("Alt+Shift+E"), "{}", always, cancel),
    ]
}

/// Forget the view Zoom Toggle would go back to, leaving the zoom, scroll and track heights as they are.
fn cancel(e: &mut Engine, _: &Value) -> Result<Value> {
    let was_on = e.session_mut().edit.zoom_toggle.take().is_some();
    Ok(json!({"on": false, "canceled": was_on}))
}

fn toggle(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(back) = s.edit.zoom_toggle.take() {
        s.edit.zoom.samples_per_px = back.samples_per_px.clamp(0.25, 1_000_000.0);
        s.edit.zoom.scroll = back.scroll.clamp(0, MAX_POSITION);
        for t in &mut s.tracks {
            t.height = back.heights.iter().find(|(id, _)| *id == t.id).map_or(t.height, |(_, h)| *h);
        }
        return Ok(json!({"on": false}));
    }
    let heights: Vec<_> = s.tracks.iter().filter(|t| !t.hidden && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.height)).collect();
    // Rows sit 1 point apart.
    let each = f64_or(p, "height_px", 600.0) / heights.len().max(1) as f64 - 1.0;
    let fit = TrackHeight::Fit(each.clamp(16.0, f64::from(u16::MAX)) as u16);
    for t in s.tracks.iter_mut().filter(|t| heights.iter().any(|(id, _)| *id == t.id)) {
        t.height = fit;
    }
    let (r, z) = (s.edit.selection, &mut s.edit.zoom);
    s.edit.zoom_toggle = Some(ZoomToggle { samples_per_px: z.samples_per_px, scroll: z.scroll, heights });
    if !r.is_empty() {
        z.samples_per_px = (r.len() as f64 / f64_or(p, "width_px", 1200.0).max(100.0)).clamp(0.25, 1_000_000.0);
        z.scroll = r.start;
    }
    Ok(json!({"on": true}))
}

#[cfg(test)]
mod tests {
    use super::TrackHeight::{Fit, Large, Medium};
    use super::*;
    use soundcraft_model::TrackId;
    use soundcraft_time::Samples;

    fn engine(select: Value) -> Engine {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 3})).unwrap();
        e.execute("view.zoom_set", &json!({"samples_per_px": 500.0})).unwrap();
        e.execute("view.scroll", &json!({"to": 1000})).unwrap();
        e.execute("edit.select", &select).unwrap();
        e
    }

    fn view(e: &Engine) -> (f64, Samples, Vec<TrackHeight>) {
        let s = e.session();
        (s.edit.zoom.samples_per_px, s.edit.zoom.scroll, s.tracks.iter().map(|t| t.height).collect())
    }

    #[test]
    fn the_selection_fills_the_window_and_toggling_again_goes_back() {
        let mut e = engine(json!({"tracks": ["Audio 1", "Audio 3"], "start": 48_000, "end": 96_000}));
        let steps = e.undo_history().len();
        assert_eq!(e.execute("view.zoom_toggle", &json!({"width_px": 1000.0, "height_px": 500.0})).unwrap(), json!({"on": true}));
        assert_eq!(view(&e), (48.0, 48_000, vec![Fit(249), Medium, Fit(249)]));
        e.execute("view.zoom_in", &json!({})).unwrap();
        e.execute("track.height", &json!({"tracks": ["Audio 2"], "height": "large"})).unwrap();
        assert_eq!(e.execute("view.zoom_toggle", &json!({})).unwrap(), json!({"on": false}));
        assert_eq!(view(&e), (500.0, 1000, vec![Medium, Large, Medium]));
        assert_eq!(e.undo_history().len(), steps + 1);
        e.execute("edit.select", &json!({"tracks": ["Audio 2"], "start": 5000})).unwrap();
        e.execute("view.zoom_toggle", &json!({"height_px": 300.0})).unwrap();
        assert_eq!(view(&e), (500.0, 1000, vec![Medium, Fit(299), Medium]));
    }

    #[test]
    fn cancel_keeps_the_zoomed_view_and_the_next_toggle_zooms_in_again() {
        let mut e = engine(json!({"tracks": ["Audio 1", "Audio 3"], "start": 48_000, "end": 96_000}));
        e.execute("view.zoom_toggle", &json!({"width_px": 1000.0, "height_px": 500.0})).unwrap();
        let zoomed = view(&e);
        assert_eq!(e.execute("view.zoom_toggle_cancel", &json!({})).unwrap(), json!({"on": false, "canceled": true}));
        assert_eq!(e.session().edit.zoom_toggle, None);
        assert_eq!(view(&e), zoomed, "cancel keeps the view Zoom Toggle zoomed in to");
        assert_eq!(e.execute("view.zoom_toggle_cancel", &json!({})).unwrap(), json!({"on": false, "canceled": false}));
        assert_eq!(view(&e), zoomed);
        e.execute("view.zoom_set", &json!({"samples_per_px": 200.0})).unwrap();
        assert_eq!(e.execute("view.zoom_toggle", &json!({"width_px": 1000.0, "height_px": 500.0})).unwrap(), json!({"on": true}));
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        assert_eq!(view(&e), (200.0, 48_000, vec![Fit(249), Medium, Fit(249)]), "the toggle goes back to the view it saw after the cancel");
        let spec = crate::cmd::command_specs().iter().find(|c| c.id == "view.zoom_toggle_cancel").unwrap();
        assert_eq!(spec.shortcut, Some("Alt+Shift+E"));
    }

    #[test]
    fn hostile_sizes_and_stored_views_stay_in_range() {
        let mut e = engine(json!({"tracks": ["Audio 1"], "start": 0, "end": MAX_POSITION}));
        e.execute("view.zoom_toggle", &json!({"width_px": -5.0, "height_px": 1e300})).unwrap();
        assert_eq!((view(&e).0, view(&e).2[0]), (1_000_000.0, Fit(u16::MAX)));
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        e.execute("view.zoom_toggle", &json!({"width_px": "wide", "height_px": -1.0})).unwrap();
        assert_eq!(view(&e).2[0], Fit(16));
        let audio_1 = e.session().tracks[0].id;
        let heights = vec![(TrackId(9999), TrackHeight::Jumbo), (audio_1, Fit(0))];
        e.session_mut().edit.zoom_toggle = Some(ZoomToggle { samples_per_px: -1.0, scroll: Samples::MIN, heights });
        e.execute("view.zoom_toggle", &json!({})).unwrap();
        assert_eq!(view(&e), (0.25, 0, vec![Fit(0), Medium, Medium]));
        assert_eq!(Fit(0).points(), 16.0);
    }
}
