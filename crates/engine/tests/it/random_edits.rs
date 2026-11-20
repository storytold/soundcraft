//! Random command sequences against the demo session: nothing panics, invariants hold, and
//! undoing everything restores the original document.

use proptest::prelude::*;
use serde_json::{Value, json};
use soundcraft_engine::Engine;

const CMDS: &[&str] = &[
    "edit.cut",
    "edit.copy",
    "edit.paste",
    "edit.clear",
    "edit.duplicate",
    "edit.repeat",
    "edit.insert_silence",
    "edit.separate",
    "edit.heal",
    "edit.trim_to_selection",
    "edit.fades_create",
    "edit.fades_delete",
    "edit.nudge",
    "edit.consolidate",
    "edit.strip_silence",
    "edit.mute_clips",
    "edit.move_clips",
    "edit.mode",
    "clip.gain",
    "clip.loop",
    "mix.volume",
    "mix.pan",
    "mix.insert",
    "automation.set_point",
    "automation.write_range",
    "event.tempo",
    "markers.add",
    "track.new",
    "track.duplicate",
    "track.delete",
    "edit.space_clips",
    "edit.snap_next",
    "edit.snap_previous",
    "edit.duplicate_extend",
    "edit.tce_to_timeline",
    "clip.unloop",
    "clip.conform_to_tempo",
    "event.tempo_linear",
    "event.insert_time",
    "event.cut_time",
    "edit.copy_to_new_playlist",
    "edit.move_to_new_playlist",
    "track.change_width",
    "clip.elastic_properties",
    "audiosuite.process",
    "event.beat_detective",
    "automation.thin",
    "clip.remove_warp",
    "clip.time_lock",
    "clip.edit_lock",
];

fn params(i: usize, a: i64, b: i64, track: &str) -> Value {
    let (s, e) = (a.min(b), a.max(b));
    match CMDS.get(i).copied().unwrap_or("") {
        "edit.mode" => {
            let m = ["slip", "shuffle", "grid", "spot"][(a.unsigned_abs() % 4) as usize];
            json!({ "mode": m })
        }
        "edit.repeat" => json!({"tracks": [track], "start": s, "end": e, "count": 2}),
        "edit.nudge" => json!({"direction": if a % 2 == 0 { 1 } else { -1 }}),
        "edit.move_clips" => json!({"by": b - a}),
        "clip.gain" => json!({"delta_db": (a % 7) as f64}),
        "clip.loop" => json!({"count": 2}),
        "mix.volume" => json!({"track": track, "db": (a % 20) as f64 - 10.0}),
        "mix.pan" => json!({"track": track, "pan": ((a % 200) as f64) / 100.0}),
        "mix.insert" => json!({"track": track, "plugin": "eq_7band"}),
        "automation.set_point" => json!({"track": track, "param": "volume", "at": s, "value": -6.0}),
        "automation.write_range" => json!({"track": track, "param": "pan", "start": s, "end": e, "value": 0.5}),
        "event.tempo" => json!({"bpm": 60.0 + (a.unsigned_abs() % 100) as f64, "at": s}),
        "markers.add" => json!({"at": s}),
        "track.new" => json!({"format": "Stereo"}),
        "track.duplicate" | "track.delete" => json!({"tracks": [track]}),
        "edit.space_clips" => json!({"tracks": [track], "start": s, "end": e, "gap": {"seconds": 0.25}}),
        "event.tempo_linear" => json!({"start": s, "end": e, "start_bpm": 90.0, "end_bpm": 130.0}),
        "event.insert_time" => json!({"start": s, "length": {"seconds": 1.0}}),
        "track.change_width" => json!({"tracks": [track], "format": if a % 2 == 0 { "Stereo" } else { "Mono" }}),
        "clip.elastic_properties" => json!({"ratio": 0.5 + (a.unsigned_abs() % 150) as f64 / 100.0}),
        "audiosuite.process" => {
            let p = ["normalize", "reverse", "invert", "gain"][(b.unsigned_abs() % 4) as usize];
            json!({ "process": p })
        }
        "event.beat_detective" => json!({"tracks": [track], "start": s, "end": e, "separate": true}),
        _ => json!({"tracks": [track], "start": s, "end": e}),
    }
}

fn check_invariants(e: &Engine) -> Result<(), String> {
    let s = e.session();
    for t in &s.tracks {
        for pl in &t.playlists {
            let mut last = i64::MIN;
            for c in &pl.clips {
                if c.length <= 0 {
                    return Err(format!("{}: clip {} has length {}", t.name, c.id, c.length));
                }
                if c.start < last {
                    return Err(format!("{}: playlist not sorted", t.name));
                }
                last = c.start;
                if c.fade_in.len > c.length || c.fade_out.len > c.length {
                    return Err(format!("{}: fade longer than clip", t.name));
                }
            }
        }
        if !t.mixer.volume_db.is_finite() {
            return Err("non-finite volume".into());
        }
    }
    serde_json::to_string(s).map(|_| ()).map_err(|err| err.to_string())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]
    #[test]
    fn random_edits_keep_the_session_sane(ops in proptest::collection::vec((0usize..CMDS.len(), -100_000i64..3_000_000, -100_000i64..3_000_000, 0usize..5), 1..25)) {
        let mut e = soundcraft_engine::demo::demo_engine();
        let original = serde_json::to_value(e.session()).unwrap();
        let names = ["Kick", "Snare", "Bass", "Pad", "Keys"];
        let mut applied = 0;
        for (i, a, b, t) in ops {
            let track = names[t % names.len()];
            let id = CMDS[i % CMDS.len()];
            // Make a selection first so range commands have something to act on.
            let _ = e.execute("edit.select", &json!({"tracks": [track], "start": a.min(b).max(0), "end": a.max(b).max(0)}));
            if let Err(soundcraft_engine::EngineError::Internal(id, msg)) = e.execute(id, &params(i % CMDS.len(), a, b, track)) {
                prop_assert!(false, "{id} panicked: {msg}");
            }
            applied += 1;
            prop_assert!(check_invariants(&e).is_ok(), "after {id}: {:?}", check_invariants(&e));
        }
        prop_assert!(applied > 0);
        while e.can_undo() {
            e.undo();
        }
        let mut back = serde_json::to_value(e.session()).unwrap();
        let mut orig = original;
        // Selection and view state are not part of the undo history.
        for v in [&mut back, &mut orig] {
            if let Some(o) = v.as_object_mut() {
                o.remove("edit");
            }
        }
        prop_assert_eq!(back, orig);
    }
}
