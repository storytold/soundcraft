//! Route a MIDI track's clips to a shared instrument.
use super::{CommandSpec, always, bad, track_param};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_model::{MidiDestination, TrackKind};

const ID: &str = "track.midi_destination";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "track.midi_destination",
        "MIDI Destination",
        [],
        None,
        "{track?, to: instrument track id|name, channel: 1..16} or {track?, to: null} to disconnect",
        always,
        set
    )]
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    let source = track_param(e, ID, p, "track")?
        .or_else(|| e.session().edit.selected_tracks.first().copied())
        .ok_or_else(|| bad(ID, "select a MIDI track or supply its id"))?;
    let track = e.session().track(source).ok_or_else(|| bad(ID, "no source track"))?;
    if track.kind != TrackKind::Midi {
        return Err(bad(ID, "only a MIDI track can use this destination"));
    }
    let destination = if p.get("to").is_some_and(Value::is_null) {
        None
    } else {
        let target = track_param(e, ID, p, "to")?.ok_or_else(|| bad(ID, "supply a destination track or null"))?;
        if !e.session().track(target).is_some_and(|t| t.kind == TrackKind::Instrument && !t.inactive && t.instrument.is_some()) {
            return Err(bad(ID, "the destination must have an instrument"));
        }
        let channel = p
            .get("channel")
            .and_then(Value::as_u64)
            .filter(|ch| (1..=16).contains(ch))
            .ok_or_else(|| bad(ID, "MIDI channel must be an integer from 1 to 16"))?;
        let route = MidiDestination { track: target, channel: channel as u8 };
        if track.midi_destination != Some(route)
            && e.session().tracks.iter().any(|other| other.kind == TrackKind::Midi && other.id != source && other.midi_destination == Some(route))
        {
            return Err(bad(ID, "the destination channel is already assigned to another MIDI track"));
        }
        Some(route)
    };
    if track.midi_destination != destination
        && let Some(t) = e.session_mut().track_mut(source)
    {
        t.midi_destination = destination;
    }
    Ok(json!({ "track": source, "midi_destination": destination }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Insert, Session};

    #[test]
    fn midi_destination_is_saved_and_removed_with_target() {
        let mut session = Session::default();
        let source = session.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Bass MIDI"));
        let target = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Shared"));
        session.track_mut(target).unwrap().instrument = Some(Insert::new("subtractive_synth"));
        let mut engine = Engine::new(session);
        assert!(engine.execute(ID, &json!({"track": source, "to": target, "channel": 17})).is_err());
        assert!(engine.execute(ID, &json!({"track": source, "to": source, "channel": 2})).is_err());
        engine.execute(ID, &json!({"track": source, "to": "Shared", "channel": 2})).unwrap();
        let other = engine.session_mut().add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Other MIDI"));
        assert!(engine.execute(ID, &json!({"track": other, "to": target, "channel": 2})).is_err());
        let saved = Session::from_json(&engine.session().to_json().unwrap()).unwrap();
        assert_eq!(saved.track(source).unwrap().midi_destination, Some(MidiDestination { track: target, channel: 2 }));
        engine.execute("track.delete", &json!({"tracks": [target]})).unwrap();
        assert_eq!(engine.session().track(source).unwrap().midi_destination, None);
    }
}
