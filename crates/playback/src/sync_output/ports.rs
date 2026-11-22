use midir::{MidiOutput, MidiOutputConnection};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Bindings = BTreeMap<String, String>;

/// Return the selection labels and the opaque native IDs to open for each.
fn assign(saved: &Bindings, found: impl IntoIterator<Item = (String, String)>) -> Bindings {
    let found: Vec<_> = found.into_iter().collect();
    // Some drivers report the same opaque ID for multiple logical ports. Opening one
    // by that ID could address its sibling, so keep such destinations unavailable.
    let mut seen_ids = BTreeSet::new();
    let ambiguous_ids: BTreeSet<_> = found.iter().filter_map(|(_, id)| (!seen_ids.insert(id)).then_some(id)).collect();
    let raw_names: BTreeSet<_> = found.iter().map(|(name, _)| name.as_str()).collect();
    let known: BTreeMap<_, _> = saved.iter().map(|(label, id)| (id, label)).collect();
    let mut available = Bindings::new();
    for (name, id) in &found {
        if ambiguous_ids.contains(id) {
            continue;
        }
        let label = if let Some(label) = known.get(id) {
            (*label).clone()
        } else if !saved.contains_key(name) && !available.contains_key(name) {
            name.clone()
        } else {
            // A real device can itself be named "DIN (2)". Reserve its literal name.
            (2..)
                .map(|n| format!("{name} ({n})"))
                .find(|label| !saved.contains_key(label) && !available.contains_key(label) && !raw_names.contains(label.as_str()))
                .expect("a finite port list leaves an available suffix")
        };
        available.insert(label, id.clone());
    }
    available
}

fn scan(midi: &MidiOutput, saved: &Bindings) -> Bindings {
    assign(saved, midi.ports().iter().filter_map(|port| Some((midi.port_name(port).ok()?, port.id()))))
}

pub(super) fn available(saved: &Bindings) -> Result<Bindings, String> {
    std::panic::catch_unwind(|| {
        let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
        Ok(scan(&midi, saved))
    })
    .map_err(|_| "MIDI output enumeration failed".to_owned())?
}

fn selected<'a>(available: &'a Bindings, label: &str) -> Result<&'a str, String> {
    available.get(label).map(String::as_str).ok_or_else(|| format!("MIDI output {label} unavailable"))
}

fn open(label: &str, saved: &Bindings) -> Result<(MidiOutputConnection, String), String> {
    let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
    let ids = scan(&midi, saved);
    let id = selected(&ids, label)?;
    let port = midi.find_port_by_id(id).ok_or_else(|| format!("MIDI output {label} unavailable"))?;
    let conn = midi.connect(&port, "SoundCraft MIDI sync").map_err(|e| e.to_string())?;
    Ok((conn, id.to_owned()))
}

/// The worker also pins a legacy selection on its first successful connection, including
/// when the device was unplugged at startup. The UI saves newly learned bindings.
#[derive(Default)]
pub(super) struct Registry {
    ids: std::sync::Mutex<Bindings>,
    learned: std::sync::atomic::AtomicBool,
}

impl Registry {
    pub(super) fn restore(&self, saved: &Bindings) {
        self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone_from(saved);
    }

    pub(super) fn opened(&self) -> Option<Bindings> {
        self.learned
            .swap(false, std::sync::atomic::Ordering::Acquire)
            .then(|| self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
    }

    pub(super) fn connect(&self, label: &str) -> Result<(MidiOutputConnection, Option<String>), String> {
        let saved = self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let (connection, id) = open(label, &saved)?;
        let mut ids = self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = ids.get(label) {
            if existing != &id {
                return Err(format!("MIDI output {label} changed while connecting"));
            }
        } else {
            ids.insert(label.to_owned(), id);
            self.learned.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok((connection, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(ports: &[(&str, &str)]) -> Vec<(String, String)> {
        ports.iter().map(|(name, id)| (name.to_string(), id.to_string())).collect()
    }

    #[test]
    fn duplicate_calibrations_reserve_literal_names_and_reject_indistinguishable_ids() {
        let found = listed(&[("DIN", "a"), ("DIN", "b"), ("DIN (2)", "c"), ("unstable", "d"), ("unstable", "d")]);
        let available = assign(&Bindings::new(), found);
        assert_eq!(available, Bindings::from([("DIN".into(), "a".into()), ("DIN (2)".into(), "c".into()), ("DIN (3)".into(), "b".into())]));
    }

    #[test]
    fn duplicate_calibrations_follow_the_saved_endpoints_after_reopen_and_unplug() {
        let both = listed(&[("DIN", "device-a"), ("DIN", "device-b")]);
        let saved = assign(&Bindings::new(), both);
        let offsets = BTreeMap::from([("DIN", 701), ("DIN (2)", 302)]);
        let reopened: Bindings = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let now = assign(&reopened, listed(&[("DIN", "device-b"), ("DIN", "device-a")]));
        let routed: BTreeMap<_, _> = offsets.iter().map(|(label, offset)| (selected(&now, label).unwrap(), *offset)).collect();
        assert_eq!(routed, BTreeMap::from([("device-a", 701), ("device-b", 302)]));
        let now = assign(&reopened, listed(&[("DIN", "device-b")]));
        assert!(selected(&now, "DIN").unwrap_err().contains("unavailable"));
        assert_eq!(selected(&now, "DIN (2)").unwrap(), "device-b");
    }
}
