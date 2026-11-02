//! Guitar tab: per-track tunings and the computed tablature (Window › Tab View draws it).
use super::{CommandSpec, always, bad, i64_or, position_param, track_param};

use crate::{Engine, Result, cmd, guitar_tab};
use serde_json::{Value, json};
use soundcraft_model::TrackId;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.tuning",
            "Set Guitar Tuning",
            [],
            None,
            "{track?, tuning: preset id or name | [pitches] | \"E2 A2 D3 G3 B3 E4\" | null (back to E standard)} — 4 to 8 strings, string 1 highest",
            has_midi_track,
            set_tuning
        ),
        cmd!(query "track.guitar_tab", "Guitar Tab", [], None, "{track?, at?: position, text?: bool, bars_per_line?: 1..16} — the track as tab: measures of events with string/fret per note", has_midi_track, guitar_tab_query),
        cmd!(query "track.tunings", "Guitar Tuning Presets", [], None, "{}", always, |_, _| {
            Ok(json!(guitar_tab::PRESETS.iter().map(|p| json!({"id": p.id, "name": p.name, "strings": p.strings})).collect::<Vec<_>>()))
        }),
    ]
}

fn has_midi_track(e: &Engine) -> std::result::Result<(), String> {
    guitar_tab::target_track(e.session()).map(drop).ok_or_else(|| "the session has no MIDI or instrument tracks".into())
}

/// `track`, else the tab view's target track; it must be a MIDI or instrument track.
fn midi_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = match track_param(e, id, p, "track")? {
        Some(t) => t,
        None => guitar_tab::target_track(e.session()).ok_or_else(|| bad(id, "no MIDI or instrument track"))?,
    };
    require_midi(e, id, t)
}

/// `track`, else the selected MIDI or instrument track (or the track of the selected MIDI clip).
/// Never a track the user did not pick: `track.tuning` changes the session.
fn picked_midi_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = match track_param(e, id, p, "track")? {
        Some(t) => t,
        None => guitar_tab::selected_track(e.session()).ok_or_else(|| bad(id, "no track: pass `track` or select a MIDI or instrument track"))?,
    };
    require_midi(e, id, t)
}

fn require_midi(e: &Engine, id: &str, t: TrackId) -> Result<TrackId> {
    if !e.session().track(t).is_some_and(|t| t.kind.is_midi()) {
        return Err(bad(id, "tab needs a MIDI or instrument track"));
    }
    Ok(t)
}

fn set_tuning(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = picked_midi_track(e, "track.tuning", p)?;
    let tuning = match p.get("tuning") {
        None => return Err(bad("track.tuning", "`tuning` required")),
        Some(Value::Null) => None,
        Some(v) => Some(guitar_tab::parse_tuning(v).map_err(|m| bad("track.tuning", m))?),
    };
    let shown = tuning.clone().unwrap_or_else(|| guitar_tab::DEFAULT_TUNING.to_vec());
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.guitar_tuning = tuning;
    }
    Ok(json!({"track": t.0, "tuning": guitar_tab::tuning_json(&shown)}))
}

fn guitar_tab_query(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = midi_track(e, "track.guitar_tab", p)?;
    let s = e.session();
    let tab = guitar_tab::layout(s, t).ok_or_else(|| bad("track.guitar_tab", "tab needs a MIDI or instrument track"))?;
    let mut v = guitar_tab::to_json(&tab);
    if let Some(at) = position_param(e, "track.guitar_tab", p, "at")? {
        v["cursor"] = match guitar_tab::locate(e.session(), &tab, at) {
            Some((i, unit)) => json!({"measure": i + 1, "unit": unit}),
            None => Value::Null,
        };
    }
    if p.get("text").and_then(Value::as_bool).unwrap_or(false) {
        let per = usize::try_from(i64_or(p, "bars_per_line", 4).clamp(1, 16)).unwrap_or(4);
        v["text"] = json!(guitar_tab::text(&tab, per));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::TrackId;

    /// A MIDI track with a one-bar clip of sixteenth notes, `(sixteenth, pitch, channel 0-based)`.
    fn engine(notes: &[(i64, u8, u8)]) -> (Engine, TrackId) {
        let mut e = Engine::default();
        let v = e.execute("track.new", &json!({"kind": "midi", "name": "Gtr"})).unwrap_or_default();
        let id = TrackId(v["tracks"][0].as_u64().unwrap_or(0));
        assert!(e.execute("track.rename", &json!({"track": id.0, "name": "Gtr"})).is_ok());
        let c = e.execute("midi.new_clip", &json!({"track": id.0})).unwrap_or_default();
        let q = soundcraft_time::TICKS_PER_QUARTER / 4;
        for (u, pitch, channel) in notes {
            let r =
                e.execute("midi.note_add", &json!({"clip": c["clip"], "pitch": pitch, "start_ticks": u * q, "length_ticks": q, "channel": channel}));
            assert!(r.is_ok(), "{r:?}");
        }
        (e, id)
    }

    #[test]
    fn tuning_is_set_undone_and_saved() {
        let (mut e, id) = engine(&[(0, 35, 5)]);
        let tab = e.execute("track.guitar_tab", &json!({"track": id.0})).unwrap_or_default();
        assert_eq!(tab["tuning"]["preset"], "e_standard");
        assert_eq!(tab["unplayable"], 1, "B1 is below E standard");
        let r = e.execute("track.tuning", &json!({"track": id.0, "tuning": "b_standard"})).unwrap_or_default();
        assert_eq!(r["tuning"]["name"], "B Standard");
        let tab = e.execute("track.guitar_tab", &json!({"track": id.0})).unwrap_or_default();
        assert_eq!(tab["measures"][0]["events"][0]["notes"][0], json!({"pitch": 35, "channel": 6, "string": 6, "fret": 0}));
        let saved = serde_json::to_value(e.session()).unwrap_or_default();
        let restored: soundcraft_model::Session = serde_json::from_value(saved).unwrap_or_default();
        assert_eq!(restored.track(id).and_then(|t| t.guitar_tuning.clone()), Some(vec![59, 54, 50, 45, 40, 35]));
        assert!(e.execute("edit.undo", &json!({})).is_ok());
        assert_eq!(e.session().track(id).and_then(|t| t.guitar_tuning.clone()), None);
        assert!(e.execute("track.tuning", &json!({"track": id.0, "tuning": [1, 2]})).is_err());
        assert!(e.execute("track.tuning", &json!({"track": id.0})).is_err());
    }

    #[test]
    fn tuning_without_a_track_needs_a_selection() {
        let (mut e, id) = engine(&[(0, 40, 5)]);
        // A second MIDI track first in the session: the fallback would have picked it.
        assert!(e.execute("track.new", &json!({"kind": "midi", "name": "Other"})).is_ok());
        assert!(e.execute("edit.select", &json!({"tracks": []})).is_ok());
        e.session_mut().edit.selected_tracks.clear();
        e.session_mut().edit.selected_clips.clear();
        let r = e.execute("track.tuning", &json!({"tuning": "drop_d"}));
        assert!(r.is_err(), "{r:?}");
        assert!(e.session().tracks.iter().all(|t| t.guitar_tuning.is_none()));
        // The query still falls back to the panel's track.
        assert!(e.execute("track.guitar_tab", &json!({})).is_ok());
        e.session_mut().edit.selected_tracks = vec![id];
        let r = e.execute("track.tuning", &json!({"tuning": "drop_d"})).unwrap_or_default();
        assert_eq!(r["track"], id.0);
        assert!(e.session().tracks.iter().filter(|t| t.guitar_tuning.is_some()).map(|t| t.id).eq([id]));
    }

    #[test]
    fn unisons_keep_both_strings() {
        let (e, id) = engine(&[(0, 64, 0), (0, 64, 1)]);
        let s = e.session();
        let placed: Vec<_> = s
            .track(id)
            .map(|t| crate::guitar_tab::timeline(s, t))
            .unwrap_or_default()
            .iter()
            .map(|n| (n.note.channel, n.note.string, n.note.fret))
            .collect();
        assert_eq!(placed, vec![(0, Some(1), Some(0)), (1, Some(2), Some(5))]);
        let tab = crate::guitar_tab::layout(s, id).map(|t| crate::guitar_tab::text(&t, 1)).unwrap_or_default();
        let lines: Vec<&str> = tab.lines().collect();
        assert!(lines.get(1).is_some_and(|l| l.starts_with("E|0--")) && lines.get(2).is_some_and(|l| l.starts_with("B|5--")), "{tab}");
    }

    #[test]
    fn hostile_channels_and_pitches_do_not_panic() {
        let (e, id) = engine(&[(0, 40, 5), (0, 52, 3)]);
        let mut v = serde_json::to_value(e.session()).unwrap_or_default();
        let Some(track) = v["tracks"].as_array_mut().and_then(|t| t.iter_mut().find(|t| t["id"] == id.0)) else { panic!("track") };
        let notes = &mut track["playlists"][0]["clips"][0]["content"]["Midi"]["sequence"]["notes"];
        assert!(notes.is_array(), "{notes}");
        notes[0]["channel"] = json!(255);
        notes[1]["pitch"] = json!(200);
        notes[1]["channel"] = json!(255);
        track["guitar_tuning"] = json!([64, 59, 55, 50, 45, 40]);
        let s = soundcraft_model::Session::from_json(&v.to_string()).unwrap_or_default();
        assert!(s.track(id).is_some_and(|t| t.clips().iter().any(|c| matches!(&c.content, soundcraft_model::ClipContent::Midi { sequence } if sequence.notes.iter().any(|n| n.channel == 255 && n.pitch == 200)))));
        let mut e = Engine::new(s);
        let tab = e.execute("track.guitar_tab", &json!({"track": id.0, "text": true})).unwrap_or_default();
        let notes = &tab["measures"][0]["events"][0]["notes"];
        assert!(notes.as_array().is_some_and(|n| n.iter().any(|n| n["channel"] == 256)), "{tab}");
        let s = e.session();
        assert!(s.track(id).is_some_and(|t| !crate::guitar_tab::timeline(s, t).is_empty()));
    }

    #[test]
    fn tab_query_returns_text_and_cursor() {
        let (mut e, _) = engine(&[(0, 40, 0), (4, 43, 0)]);
        let s = e.session();
        let beat2 = s.tempo.tick_to_samples(soundcraft_time::TICKS_PER_QUARTER, s.sample_rate);
        let tab = e.execute("track.guitar_tab", &json!({"track": "Gtr", "text": true, "at": beat2})).unwrap_or_default();
        assert_eq!(tab["cursor"], json!({"measure": 1, "unit": 4.0}));
        let text = tab["text"].as_str().unwrap_or_default();
        assert!(text.lines().any(|l| l.starts_with("E|0-----------3--")), "{text}");
        assert!(e.execute("track.tunings", &json!({})).is_ok_and(|v| v.as_array().is_some_and(|a| a.len() >= 6)));
    }

    #[test]
    fn tab_is_a_track_view_for_midi_tracks() {
        let (mut e, id) = engine(&[(0, 40, 5)]);
        assert!(e.execute("track.view", &json!({"tracks": [id.0], "view": "guitar_tab"})).is_ok());
        assert_eq!(e.session().track(id).map(|t| t.view.as_str()), Some("guitar_tab"));
        let saved = serde_json::to_value(e.session()).unwrap_or_default();
        let restored: soundcraft_model::Session = serde_json::from_value(saved).unwrap_or_default();
        assert_eq!(restored.track(id).map(|t| t.view.as_str()), Some("guitar_tab"));
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap_or_default();
        let audio = audio["tracks"][0].as_u64().unwrap_or(0);
        assert!(e.execute("track.view", &json!({"tracks": [audio], "view": "guitar_tab"})).is_err());
    }
}
