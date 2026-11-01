//! Editing conductor events through the shared command registry.
use super::{CommandSpec, always, bad, position_param, str_param};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{ChordEvent, ChordId, ChordSymbol};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("event.tempo_edit", "Edit Tempo Change", [], None, "{at, to, bpm}", always, |e, p| {
            let at = position_param(e, "event.tempo_edit", p, "at")?.ok_or_else(|| bad("event.tempo_edit", "at required"))?;
            let to = position_param(e, "event.tempo_edit", p, "to")?.unwrap_or(at);
            let bpm = p.get("bpm").and_then(Value::as_f64).ok_or_else(|| bad("event.tempo_edit", "bpm required"))?;
            let s = e.session_mut();
            let old = s.tempo.samples_to_ticks(at, s.sample_rate);
            let new = if old == 0 { 0 } else { s.tempo.samples_to_ticks(to, s.sample_rate) };
            let mut map = s.tempo.clone();
            if old != new {
                map.remove_tempo(old);
            }
            map.set_tempo(new, bpm).map_err(|err| bad("event.tempo_edit", err.to_string()))?;
            s.tempo = map;
            Ok(json!({}))
        }),
        cmd!(noundo "view.tempo_editor", "Tempo Editor View", [], None, "{height?, min_bpm?, max_bpm?, density?, shape?}", always, |e,p| {
            let s=e.session_mut();
            for (key,lo,hi) in [("height",56.0,400.0),("min_bpm",5.0,999.0),("max_bpm",6.0,1000.0),("density",1.0,3840.0)] {
                if let Some(value)=p.get(key).and_then(Value::as_f64).filter(|v|v.is_finite()) {s.edit.values.insert(format!("tempo_editor.{key}"),value.clamp(lo,hi));}
            }
            if let Some(shape)=str_param(p,"shape") {super::more_util::set_text(&mut s.edit,"tempo_editor.shape",Some(shape));}
            Ok(json!({}))
        }),
        cmd!("event.tempo_points", "Edit Tempo Points", [], None, "{points: [{tick, bpm}], start_tick?, end_tick?}", always, tempo_points),
        cmd!("chord.add", "Add Chord Symbol", [], None, "{at?, root, quality?, bass?}", always, |e, p| chord(e, p, false)),
        cmd!("chord.edit", "Edit Chord Symbol", [], None, "{id, at?, root, quality?, bass?}", always, |e, p| chord(e, p, true)),
        cmd!("chord.delete", "Delete Chord Symbol", [], None, "{id}", always, |e, p| {
            let id = chord_id(p)?;
            let s = e.session_mut();
            if !s.chords.iter().any(|c| c.id == id) {
                return Err(bad("chord.delete", "chord no longer exists"));
            }
            s.chords.retain(|c| c.id != id);
            Ok(json!({}))
        }),
        cmd!("event.key_edit", "Edit Key Change", [], None, "{at, to?, key}", always, key_edit),
        cmd!("event.key_remove", "Remove Key Change", [], None, "{at}", always, |e, p| {
            let at = position_param(e, "event.key_remove", p, "at")?.ok_or_else(|| bad("event.key_remove", "at required"))?;
            e.session_mut().key_signatures.retain(|(t, _)| *t != at);
            Ok(json!({}))
        }),
        cmd!("event.meter_remove", "Remove Meter Change", [], None, "{at}", always, |e, p| {
            let at = position_param(e, "event.meter_remove", p, "at")?.ok_or_else(|| bad("event.meter_remove", "at required"))?;
            let s = e.session_mut();
            let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
            Ok(json!({"removed":s.tempo.remove_meter(tick)}))
        }),
    ]
}
fn chord_id(p: &Value) -> Result<ChordId> {
    p.get("id").and_then(Value::as_u64).map(ChordId).ok_or_else(|| bad("chord.edit", "id required"))
}
fn chord(e: &mut Engine, p: &Value, editing: bool) -> Result<Value> {
    let id = if editing { Some(chord_id(p)?) } else { None };
    let previous = id.and_then(|id| e.session().chords.iter().find(|c| c.id == id));
    if editing && previous.is_none() {
        return Err(bad("chord.edit", "chord no longer exists"));
    }
    let at = position_param(e, "chord.add", p, "at")?.unwrap_or_else(|| previous.map_or(e.session().edit.selection.start, |c| c.at));
    let symbol = ChordSymbol {
        root: str_param(p, "root").unwrap_or("C").into(),
        quality: str_param(p, "quality").unwrap_or("").into(),
        bass: str_param(p, "bass").filter(|s| !s.is_empty()).map(str::to_string),
    };
    if !symbol.valid() {
        return Err(bad("chord.add", "invalid chord spelling"));
    }
    if !(0..=soundcraft_time::MAX_POSITION).contains(&at) {
        return Err(bad("chord.add", "position out of range"));
    }
    let s = e.session_mut();
    if s.chords.len() >= 10_000 && id.is_none() {
        return Err(bad("chord.add", "too many chord events"));
    }
    let id = id.unwrap_or_else(|| ChordId(s.alloc()));
    s.chords.retain(|c| c.id != id && c.at != at);
    s.chords.push(ChordEvent { id, at, symbol });
    s.chords.sort_by_key(|c| c.at);
    Ok(json!({"id":id}))
}
fn key_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let at = position_param(e, "event.key_edit", p, "at")?.ok_or_else(|| bad("event.key_edit", "at required"))?;
    let to = position_param(e, "event.key_edit", p, "to")?.unwrap_or(at);
    if !(0..=soundcraft_time::MAX_POSITION).contains(&to) {
        return Err(bad("event.key_edit", "position out of range"));
    }
    let key = str_param(p, "key").ok_or_else(|| bad("event.key_edit", "key required"))?;
    if soundcraft_model::key_fifths(key).is_none() {
        return Err(bad("event.key_edit", "choose a major or natural-minor key"));
    }
    let s = e.session_mut();
    s.key_signatures.retain(|(t, _)| *t != at && *t != to);
    s.key_signatures.push((to, key.into()));
    s.key_signatures.sort_by_key(|k| k.0);
    Ok(json!({}))
}

fn tempo_points(e: &mut Engine, p: &Value) -> Result<Value> {
    let values = p
        .get("points")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 10_000)
        .ok_or_else(|| bad("event.tempo_points", "supply 1..10000 points"))?;
    let mut points = Vec::with_capacity(values.len());
    for v in values {
        let tick = v
            .get("tick")
            .and_then(Value::as_i64)
            .filter(|t| (0..=soundcraft_time::MAX_POSITION).contains(t))
            .ok_or_else(|| bad("event.tempo_points", "invalid tick"))?;
        let bpm = v
            .get("bpm")
            .and_then(Value::as_f64)
            .filter(|b| b.is_finite() && (5.0..=1000.0).contains(b))
            .ok_or_else(|| bad("event.tempo_points", "invalid BPM"))?;
        points.push((tick, bpm));
    }
    let mut map = e.session().tempo.clone();
    if let Some(ticks) = p.get("remove_ticks").and_then(Value::as_array) {
        for tick in ticks.iter().take(10_000).filter_map(Value::as_i64) {
            map.remove_tempo(tick);
        }
    }
    if let (Some(a), Some(b)) = (p.get("start_tick").and_then(Value::as_i64), p.get("end_tick").and_then(Value::as_i64)) {
        let after = map.tempo_at_tick(b);
        let remove: Vec<_> = map.tempos().iter().filter(|t| t.tick >= a && t.tick < b).map(|t| t.tick).collect();
        for tick in remove {
            map.remove_tempo(tick);
        }
        map.set_tempo(b, after).map_err(|err| bad("event.tempo_points", err.to_string()))?;
    }
    for (tick, bpm) in &points {
        map.set_tempo(*tick, *bpm).map_err(|err| bad("event.tempo_points", err.to_string()))?;
    }
    e.session_mut().tempo = map;
    Ok(json!({"events":points.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tempo_batches_are_atomic_bounded_and_undoable() {
        let mut e = Engine::default();
        let original = e.session().tempo.clone();
        assert!(e.execute("event.tempo_points", &json!({"points":[{"tick":960,"bpm":130},{"tick":1920,"bpm":0}]})).is_err());
        assert_eq!(e.session().tempo, original);
        e.execute("event.tempo_points", &json!({"points":[{"tick":960,"bpm":130},{"tick":1920,"bpm":150}],"start_tick":960,"end_tick":2880}))
            .unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(2880), 120.0);
        assert!(e.undo());
        assert_eq!(e.session().tempo, original);
        let many = vec![json!({"tick":0,"bpm":120}); 10001];
        assert!(e.execute("event.tempo_points", &json!({"points":many})).is_err());
    }
    #[test]
    fn secondary_rate_and_chords_set_saved_state_and_survive_reopen() {
        let mut e = Engine::default();
        e.execute("setup.session", &json!({"secondary_frame_rate":"120 Drop"})).unwrap();
        assert_eq!(e.session().frame_rate, soundcraft_time::FrameRate::Fps30);
        assert_eq!(e.session().secondary_frame_rate, Some(soundcraft_time::FrameRate::Fps120Drop));
        assert!(e.is_dirty());
        e.execute("chord.add", &json!({"at":100,"root":"C#","quality":"maj9"})).unwrap();
        e.execute("event.cut_time", &json!({"start":0,"length":50})).unwrap();
        assert_eq!(e.session().chords[0].at, 50);
        let reopened = soundcraft_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(&reopened, e.session());
    }
    #[test]
    fn chords_are_independent_undoable_and_persistent() {
        let mut e = Engine::default();
        e.execute("markers.add", &json!({"name":"Verse","ruler":2})).unwrap();
        let before = e.session().markers.clone();
        let result = e.execute("chord.add", &json!({"at":100,"root":"D","quality":"m7","bass":"A"})).unwrap();
        assert_eq!(e.session().markers, before);
        assert_eq!(e.session().chords[0].symbol.label(), "Dm7/A");
        let text = e.session().to_json().unwrap();
        assert_eq!(soundcraft_model::Session::from_json(&text).unwrap().chords, e.session().chords);
        assert!(e.undo());
        assert!(e.session().chords.is_empty());
        assert!(e.redo());
        e.execute("chord.edit", &json!({"id":result["id"],"at":200,"root":"G"})).unwrap();
        e.execute("event.insert_time", &json!({"start":100,"length":50})).unwrap();
        assert_eq!(e.session().chords[0].at, 250);
        e.execute("chord.delete", &json!({"id":result["id"]})).unwrap();
        assert!(e.session().chords.is_empty());
    }
    #[test]
    fn key_and_meter_edits_validate_and_undo() {
        let mut e = Engine::default();
        e.execute("event.key_edit", &json!({"at":0,"key":"Db major"})).unwrap();
        assert!(e.execute("event.key_edit", &json!({"at":0,"key":"invalid"})).is_err());
        e.execute("event.key_remove", &json!({"at":0})).unwrap();
        assert!(e.session().key_signatures.is_empty());
        assert!(e.undo());
        assert_eq!(e.session().key_signatures[0].1, "Db major");
        e.execute("event.meter_remove", &json!({"at":0})).unwrap();
        assert_eq!(e.session().tempo.meters().len(), 1);
    }
}
