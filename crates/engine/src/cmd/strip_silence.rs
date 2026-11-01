//! Strip Silence Rectangles: the ranges Strip Silence keeps, which the Strip Silence window draws.
use super::{CommandSpec, f32_or, f64_or, has_range, range_param, tracks_required};
use soundcraft_model::TrackId;
use soundcraft_time::Samples;

use crate::{Engine, Result, cmd};
use serde_json::{Value, json};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "edit.strip_silence_preview",
        "Strip Silence Rectangles",
        [],
        None,
        "{threshold_db?: -48, min_length_ms?: 50, pad_before_ms?: 5, pad_after_ms?: 20, tracks?, start?, end?} (the ranges edit.strip_silence finds with the same settings, padded, on each track: what Strip keeps and the Strip Silence window draws as rectangles; changes nothing)",
        has_range,
        strip_silence_preview
    )]
}

/// Strip Silence's tracks, range, threshold, minimum strip duration and clip start and end pads.
pub(super) type StripSettings = (Vec<TrackId>, soundcraft_time::Range, f32, Samples, (Samples, Samples));

/// The [`StripSettings`] in Strip Silence's params, with their defaults and limits. `cmd` names the
/// command in errors.
pub(super) fn strip_settings(e: &Engine, cmd: &str, p: &Value) -> Result<StripSettings> {
    let tracks = tracks_required(e, cmd, p)?;
    let r = range_param(e, cmd, p)?;
    let thr = f32_or(p, "threshold_db", -48.0).clamp(-96.0, 0.0);
    let sr = e.session().sample_rate;
    let ms = |k: &str, d: f64| sr.samples(f64_or(p, k, d).clamp(0.0, 10_000.0) / 1000.0);
    Ok((tracks, r, thr, ms("min_length_ms", 50.0), (ms("pad_before_ms", 5.0), ms("pad_after_ms", 20.0))))
}

/// The scan `edit.strip_silence_preview` runs with params `p`, on the session as it is now. The Edit
/// window runs it while the Strip Silence window or its Rename Selected Clips dialog is open: on a
/// worker thread outside the web build, and on the frame thread in the web build or when no thread
/// can be started.
pub fn strip_silence_scan(e: &Engine, p: &Value) -> Result<crate::io::StripScan> {
    let (tracks, range, threshold_db, min_len, pad) = strip_settings(e, "edit.strip_silence_preview", p)?;
    Ok(crate::io::StripScan { session: e.session_arc(), tracks, range, threshold_db, min_len, pad, stop: Default::default() })
}

fn strip_silence_preview(e: &mut Engine, p: &Value) -> Result<Value> {
    let found = strip_silence_scan(e, p)?.run()?;
    let tracks: Vec<Value> =
        found.iter().map(|(t, ranges)| json!({"track": t.0, "ranges": ranges.iter().map(|x| [x.start, x.end]).collect::<Vec<_>>()})).collect();
    Ok(json!({"tracks": tracks}))
}
