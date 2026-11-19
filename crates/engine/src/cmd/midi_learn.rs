use super::{CommandSpec, always, bad};
use crate::midi_learn::{Binding, PREFIX, Target, bindings, stored};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "midi.learn.bind",
            "Assign MIDI Controller",
            [],
            None,
            "{source: {device, channel, cc}, target: {track, parameter}, mode?}",
            always,
            bind
        ),
        cmd!("midi.learn.remove", "Forget MIDI Assignment", [], None, "{track, parameter}", always, remove),
    ]
}

fn bind(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "midi.learn.bind";
    let binding: Binding = serde_json::from_value(p.clone()).map_err(|err| bad(id, err.to_string()))?;
    if !binding.target.available(e.session()) {
        return Err(bad(id, "this control is unavailable"));
    }
    if !crate::midi_learn::valid_range(&binding) {
        return Err(bad(id, "expected normalized plugin parameter endpoints"));
    }
    if !binding.mode.supports(&binding.target.parameter) {
        return Err(bad(id, "unsupported MIDI controller mode for this target"));
    }
    if !binding.source.valid() {
        return Err(bad(id, "expected a device name of up to 256 bytes without control characters, channel 1–16, and an eligible mixer CC"));
    }
    if bindings(e.session()).any(|old| old.source == binding.source && old.target != binding.target) {
        return Err(bad(id, "this MIDI control is already assigned"));
    }
    if bindings(e.session()).any(|old| old == binding) {
        return Ok(json!(binding));
    }
    let encoded = serde_json::to_string(&binding).map_err(|err| bad(id, err.to_string()))?;
    remove_where(e, |old| old.target == binding.target || old.source == binding.source);
    e.session_mut().edit.flags.insert(format!("{PREFIX}{encoded}"));
    Ok(json!(binding))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let target: Target = serde_json::from_value(p.clone()).map_err(|err| bad("midi.learn.remove", err.to_string()))?;
    Ok(json!({"removed": remove_where(e, |old| old.target == target)}))
}

// Insert moves swap the two instances, including their MIDI assignments. Replacement
// discards the old instance's bindings even if the next instance has the same plugin ID.
pub(super) fn slot_changed(e: &mut Engine, track: soundcraft_model::TrackId, slot: Option<usize>, swap: Option<usize>) {
    use crate::midi_learn::Parameter;
    let changed: Vec<_> = stored(e.session())
        .filter_map(|(old, mut binding)| {
            if binding.target.track != track {
                return None;
            }
            let Parameter::Plugin { slot: current, .. } = &mut binding.target.parameter else { return None };
            if *current == slot {
                *current = swap;
            } else if swap.is_some() && *current == swap {
                *current = slot;
            } else {
                return None;
            }
            let replacement = if swap.is_some() { Some(format!("{PREFIX}{}", serde_json::to_string(&binding).ok()?)) } else { None };
            Some((old.clone(), replacement))
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    let flags = &mut e.session_mut().edit.flags;
    for (old, _) in &changed {
        flags.remove(old);
    }
    flags.extend(changed.into_iter().filter_map(|(_, new)| new));
}

pub(super) fn forget_preset_plugins(e: &mut Engine, tracks: &[soundcraft_model::TrackId], instrument: bool) {
    remove_where(e, |b| {
        tracks.contains(&b.target.track) && matches!(b.target.parameter,crate::midi_learn::Parameter::Plugin{slot,..} if slot.is_some() || instrument)
    });
}

fn remove_where(e: &mut Engine, matches: impl Fn(&Binding) -> bool) -> usize {
    let flags: Vec<_> = stored(e.session()).filter(|(_, binding)| matches(binding)).map(|(flag, _)| flag.clone()).collect();
    for flag in &flags {
        e.session_mut().edit.flags.remove(flag);
    }
    flags.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_assignment(track: soundcraft_model::TrackId, slot: Option<usize>, plugin: &str, id: &str, cc: u8) -> Value {
        json!({"source":{"device":"Keys","channel":1,"cc":cc},"target":{"track":track,"parameter":{"plugin":{"slot":slot,"plugin":plugin,"id":id}}}})
    }

    #[test]
    fn midi_learn_insert_removal_releases_stored_binding_and_undo_restores_it() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        e.execute("mix.insert", &json!({"track":t,"slot":0,"plugin":"gain"})).unwrap();
        e.execute("midi.learn.bind", &plugin_assignment(t, Some(0), "gain", "gain", 75)).unwrap();
        e.execute("mix.insert_remove", &json!({"track":t,"slot":0})).unwrap();
        assert_eq!(stored(e.session()).count(), 0);
        assert!(e.undo());
        assert_eq!(bindings(e.session()).count(), 1);
    }

    #[test]
    fn midi_learn_replacing_instrument_with_same_type_releases_binding_until_undo() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind":"instrument"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        e.execute("midi.learn.bind", &plugin_assignment(t, None, "subtractive_synth", "cutoff", 75)).unwrap();
        e.execute("mix.instrument", &json!({"track":t,"plugin":"subtractive_synth"})).unwrap();
        assert_eq!(stored(e.session()).count(), 0);
        assert!(e.undo());
        assert_eq!(bindings(e.session()).count(), 1);
    }

    #[test]
    fn midi_learn_preset_replaces_only_the_plugin_instances_it_contains() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind":"instrument"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        e.execute("mix.insert", &json!({"track":t,"slot":0,"plugin":"gain"})).unwrap();
        e.execute("midi.learn.bind", &plugin_assignment(t, None, "subtractive_synth", "cutoff", 74)).unwrap();
        e.execute("midi.learn.bind", &plugin_assignment(t, Some(0), "gain", "gain", 75)).unwrap();
        let tr = e.session().track(t).unwrap();
        let mut preset = json!({"format":"soundcraft-track-preset","mixer":tr.mixer,"instrument":tr.instrument});
        let path = std::env::temp_dir().join(format!("sc-midi-learn-preset-{}.json", std::process::id()));
        for include_instrument in [true, false] {
            if !include_instrument {
                preset.as_object_mut().unwrap().remove("instrument");
            }
            std::fs::write(&path, preset.to_string()).unwrap();
            e.execute("track.load_preset", &json!({"track":t,"path":path})).unwrap();
            let ccs: Vec<_> = stored(e.session()).map(|(_, binding)| binding.source.cc).collect();
            if include_instrument {
                assert!(ccs.is_empty());
            } else {
                assert_eq!(ccs, [74]);
            }
            assert!(e.undo());
            assert_eq!(bindings(e.session()).count(), 2);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn midi_learn_invalid_ranges_cannot_be_bound_or_restored() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        e.execute("mix.insert", &json!({"track":t,"slot":0,"plugin":"gain"})).unwrap();
        let plugin = plugin_assignment(t, Some(0), "gain", "gain", 75);
        for (param, range) in [(json!("volume"), json!([0, 1])), (plugin["target"]["parameter"].clone(), json!([0, 1.5]))] {
            let mut invalid = plugin.clone();
            invalid["target"]["parameter"] = param;
            invalid["range"] = range;
            assert!(e.execute("midi.learn.bind", &invalid).unwrap_err().to_string().contains("endpoints"));
            e.session_mut().edit.flags.insert(format!("{PREFIX}{invalid}"));
        }
        assert_eq!(stored(e.session()).count(), 2);
        assert_eq!(bindings(e.session()).count(), 0);
    }

    #[test]
    fn midi_learn_plugin_swap_carries_bindings_between_identical_effects() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        for slot in [0, 1] {
            e.execute("mix.insert", &json!({"track":t,"slot":slot,"plugin":"gain"})).unwrap();
        }
        e.execute("midi.learn.bind",&json!({"target":{"track":t,"parameter":{"plugin":{"slot":0,"plugin":"gain","id":"gain"}}},"source":{"device":"Keys","channel":1,"cc":75}})).unwrap();
        e.execute("mix.insert_move", &json!({"track":t,"from":0,"to":1})).unwrap();
        assert_eq!(json!(bindings(e.session()).next().unwrap())["target"]["parameter"]["plugin"]["slot"], 1);
        assert!(e.undo());
        assert_eq!(json!(bindings(e.session()).next().unwrap())["target"]["parameter"]["plugin"]["slot"], 0);
    }

    #[test]
    fn midi_learn_send_assignment_follows_the_bus_when_undo_restores_it() {
        let mut e = crate::demo::demo_engine();
        let track = e.session().track_by_name("Kick").unwrap().id;
        for name in ["First bus", "Second bus"] {
            e.execute("mix.new_bus", &json!({"name":name})).unwrap();
        }
        let send = e.execute("mix.send", &json!({"track":track,"slot":0,"bus":"First bus"})).unwrap();
        let binding = json!({"target":{"track":track,"parameter":{"send_pan":{"slot":0,"bus":send["bus"]}}},
            "source":{"device":"Controller","channel":1,"cc":74},"mode":"relative_64"});
        e.execute("midi.learn.bind", &binding).unwrap();
        e.execute("mix.send", &json!({"track":track,"slot":0,"bus":"Second bus"})).unwrap();
        assert_eq!(bindings(e.session()).count(), 0);
        assert!(e.undo());
        assert_eq!(bindings(e.session()).count(), 1);
    }

    #[test]
    fn midi_learn_validates_command_input_and_keeps_noops_out_of_undo() {
        let mut e = crate::demo::demo_engine();
        let track = e.session().track_by_name("Kick").unwrap().id;
        let target = json!({"track":track, "parameter":"volume"});
        let command = |device: &str, channel: u8, cc: u8| json!({"source":{"device":device,"channel":channel,"cc":cc},"target":target});
        let depth = e.undo_history().len();
        for (name, channel, cc) in
            [("Keys", 0, 71), ("Keys", 17, 71), ("Keys", 1, 120), ("Keys", 1, 99), ("Keys", 1, 0), ("", 1, 71), ("Line\nBreak", 1, 71)]
        {
            assert!(e.execute("midi.learn.bind", &command(name, channel, cc)).is_err(), "{name} {channel} {cc}");
        }
        assert!(e.execute("midi.learn.bind", &command(&"k".repeat(257), 1, 71)).is_err());
        assert_eq!(e.execute("midi.learn.remove", &target).unwrap()["removed"], 0);
        assert_eq!(e.undo_history().len(), depth);
        let first = command("Keys", 1, 71);
        for _ in 0..2 {
            e.execute("midi.learn.bind", &first).unwrap();
        }
        assert_eq!(e.undo_history().len(), depth + 1);
        let mut conflict = first;
        conflict["target"]["parameter"] = json!({"pan":0});
        assert!(e.execute("midi.learn.bind", &conflict).is_err());
        e.execute("midi.learn.bind", &command("Keys", 1, 75)).unwrap();
        assert_eq!(bindings(e.session()).count(), 1);
        assert!(e.undo());
        assert_eq!(bindings(e.session()).next().unwrap().source.cc, 71);
        e.session_mut().edit.flags.insert(format!("{PREFIX}broken"));
        e.execute("midi.learn.remove", &target).unwrap();
        assert!(!crate::midi_learn::configured(e.session()));
        assert!(e.session().edit.flags.contains(&format!("{PREFIX}broken")));
    }
}
