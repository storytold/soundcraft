use midir::{MidiOutput, MidiOutputConnection, MidiOutputPort};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Bindings = BTreeMap<String, String>;

/// Return the selection labels and the opaque native IDs to open for each.
fn assign(saved: &Bindings, found: impl IntoIterator<Item = (Option<String>, String)>) -> Bindings {
    let found: Vec<_> = found.into_iter().collect();
    // Some drivers report the same opaque ID for multiple logical ports. Opening one
    // by that ID could address its sibling, so keep such destinations unavailable.
    let mut seen_ids = BTreeSet::new();
    let ambiguous_ids: BTreeSet<_> = found.iter().filter_map(|(_, id)| (!seen_ids.insert(id)).then_some(id)).collect();
    let raw_names: BTreeSet<_> = found.iter().filter_map(|(name, _)| name.as_deref()).collect();
    let known: BTreeMap<_, _> = saved.iter().map(|(label, id)| (id, label)).collect();
    let mut available = Bindings::new();
    for (name, id) in &found {
        let Some(name) = name else { continue };
        if ambiguous_ids.contains(id) {
            continue;
        }
        let label = if let Some(label) = known.get(id) {
            (*label).clone()
        } else if !saved.contains_key(name) && !available.contains_key(name) {
            name.clone()
        } else {
            // A real device can itself be named "DIN (2)". Reserve its literal name.
            let last = saved.len().saturating_add(available.len()).saturating_add(raw_names.len()).saturating_add(2);
            let Some(label) = (2..=last)
                .map(|n| format!("{name} ({n})"))
                .find(|label| !saved.contains_key(label) && !available.contains_key(label) && !raw_names.contains(label.as_str()))
            else {
                continue;
            };
            label
        };
        available.insert(label, id.clone());
    }
    available
}

type PortNames = Vec<(Option<String>, String)>;

// WinMM gives logical ports on the same USB interface the same opaque ID.
// midir opens an enumerated WinMM object by checking both its name and
// interface ID again. Keep indistinguishable and partially named groups closed.
#[cfg(any(windows, test))]
fn winmm_names(mut ports: PortNames, saved: &Bindings) -> PortNames {
    let mut groups = BTreeMap::new();
    for (name, id) in &ports {
        let group = groups.entry(id.clone()).or_insert((0usize, true));
        group.0 += 1;
        group.1 &= name.is_some();
    }
    let pinned: BTreeSet<_> = saved.values().collect();
    for (name, id) in &mut ports {
        let Some(label) = name else { continue };
        let (count, fully_named) = groups[id];
        if !fully_named || id.is_empty() {
            continue;
        }
        let pair = format!("winmm-port:{}:{id}{label}", id.len());
        if count > 1 || pinned.contains(&pair) {
            *id = pair;
        }
    }
    ports
}

fn enumerate(midi: &MidiOutput, saved: &Bindings) -> (Vec<MidiOutputPort>, PortNames) {
    let ports = midi.ports();
    let names = ports.iter().map(|p| (midi.port_name(p).ok(), p.id())).collect();
    #[cfg(windows)]
    let names = winmm_names(names, saved);
    #[cfg(not(windows))]
    let _ = saved;
    (ports, names)
}

pub(super) fn available(saved: &Bindings) -> Result<Bindings, String> {
    std::panic::catch_unwind(|| {
        let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
        Ok(assign(saved, enumerate(&midi, saved).1))
    })
    .map_err(|_| "MIDI output enumeration failed".to_owned())?
}

fn selected<'a>(available: &'a Bindings, label: &str) -> Result<&'a str, String> {
    available.get(label).map(String::as_str).ok_or_else(|| format!("MIDI output {label} unavailable"))
}

fn open(label: &str, saved: &Bindings) -> Result<(MidiOutputConnection, String), String> {
    let midi = MidiOutput::new("SoundCraft MIDI sync").map_err(|e| e.to_string())?;
    let (ports, names) = enumerate(&midi, saved);
    let ids = assign(saved, names.iter().cloned());
    let id = selected(&ids, label)?;
    let index = names.iter().position(|(_, key)| key == id).ok_or_else(|| format!("MIDI output {label} unavailable"))?;
    let conn = midi.connect(&ports[index], "SoundCraft MIDI sync").map_err(|e| e.to_string())?;
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

    fn listed(ports: &[(&str, &str)]) -> Vec<(Option<String>, String)> {
        ports.iter().map(|(name, id)| (Some(name.to_string()), id.to_string())).collect()
    }

    #[test]
    fn named_winmm_subports_keep_their_exact_connector_across_reorder_and_partial_disconnect() {
        let raw = listed(&[("DIN", "usb"), ("DAW", "usb"), ("Other", "other")]);
        let names = winmm_names(raw.clone(), &Bindings::new());
        let saved = assign(&Bindings::new(), names);
        assert_ne!(saved["DIN"], saved["DAW"]);
        assert_eq!(saved["Other"], "other");
        let mut reversed = raw;
        reversed.reverse();
        assert_eq!(assign(&saved, winmm_names(reversed, &saved)), saved);
        let single = assign(&saved, winmm_names(listed(&[("DAW", "usb")]), &saved));
        assert_eq!(single.get("DAW"), saved.get("DAW"));
        assert!(!single.contains_key("DIN"));
        let legacy = Bindings::from([("DIN".into(), "usb".into())]);
        let changed = assign(&legacy, winmm_names(listed(&[("DIN", "usb"), ("DAW", "usb")]), &legacy));
        assert!(!changed.contains_key("DIN")); // a legacy ambiguous ID needs reselection
    }

    #[test]
    fn ambiguous_winmm_names_and_unreadable_siblings_remain_unavailable() {
        let duplicates = winmm_names(listed(&[("DIN", "usb"), ("DIN", "usb")]), &Bindings::new());
        assert!(assign(&Bindings::new(), duplicates).is_empty());
        let unreadable = vec![(Some("DIN".into()), "usb".into()), (None, "usb".into())];
        assert!(assign(&Bindings::new(), winmm_names(unreadable, &Bindings::new())).is_empty());
    }

    #[test]
    fn review_unnameable_output_with_the_same_id_never_opens_its_named_sibling() {
        let found = [(None, "shared".into()), (Some("DIN".into()), "shared".into()), (Some("Other".into()), "distinct".into())];
        assert_eq!(assign(&Bindings::new(), found), Bindings::from([("Other".into(), "distinct".into())]));
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
