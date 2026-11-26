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
        return Err(bad(id, "the mixer control is unavailable"));
    }
    if !binding.mode.supports(binding.target.parameter) {
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
