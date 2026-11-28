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
    let mut known: BTreeMap<_, _> = saved.iter().map(|(label, id)| (id, label)).collect();
    // A device can come back under a new native ID (CoreMIDI gives a re-created port a new
    // unique ID). A port whose name was its own when it was saved follows it, as long as no
    // other port or saved label has that name: then it can only be the same device. Ports that
    // share a name keep strict ID matching, which is what tells two identical interfaces apart.
    for (label, id) in saved {
        // A WinMM binding saved by interface ID alone is still there inside the connector keys of
        // [`winmm_names`]; such a binding is chosen again by hand, not moved.
        let winmm = format!("winmm-port:{}:{id}", id.len());
        if found.iter().any(|(_, listed)| listed == id || listed.starts_with(&winmm)) {
            continue;
        }
        let mut named = found.iter().filter(|(name, _)| name.as_deref() == Some(label.as_str()));
        let (Some((_, new_id)), None) = (named.next(), named.next()) else { continue };
        let others = saved.keys().filter(|other| *other != label && numbered(other, label)).count();
        if others == 0 && !ambiguous_ids.contains(new_id) && !known.contains_key(new_id) {
            known.insert(new_id, label);
        }
    }
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

/// Whether `label` is a numbered label of the port name `name`, such as "DIN (2)" for "DIN".
fn numbered(label: &str, name: &str) -> bool {
    label
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(" ("))
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|n| n.parse::<usize>().is_ok_and(|n| n >= 2))
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
    for (name, id) in &mut ports {
        let Some(label) = name else { continue };
        let (count, fully_named) = groups[id];
        if !fully_named || id.is_empty() {
            continue;
        }
        // A lone connector is keyed by name too: the interface ID alone would let a sibling
        // that appears later on the same interface inherit its binding. A binding saved by
        // interface ID alone is kept only for a lone connector with the label it was saved under.
        if count == 1 && saved.get(label.as_str()) == Some(id) {
            continue;
        }
        *id = format!("winmm-port:{}:{id}{label}", id.len());
    }
    ports
}

/// The names the output list goes by: the system's, tidied as the MIDI input list tidies its own
/// ([`crate::midi::tidy_port_name`]). An ALSA name carries the port's sequencer address
/// (`UM-ONE:UM-ONE MIDI 1 24:0`), which changes when the device comes back as another client
/// (plugged in after another device, or after a restart); without it the device keeps its label,
/// which [`assign`] then moves to the new address as for any port back under a new ID. A CoreMIDI
/// or WinMM name normally has neither part and is left as it is, as in the input list.
fn shown_names(ports: PortNames) -> PortNames {
    ports.into_iter().map(|(name, id)| (name.map(|raw| if raw.trim().is_empty() { raw } else { crate::midi::tidy_port_name(&raw) }), id)).collect()
}

fn enumerate(midi: &MidiOutput, saved: &Bindings) -> (Vec<MidiOutputPort>, PortNames) {
    let ports = midi.ports();
    let names = shown_names(ports.iter().map(|p| (midi.port_name(p).ok(), p.id())).collect());
    #[cfg(windows)]
    let names = winmm_names(names, saved);
    #[cfg(not(windows))]
    let _ = saved;
    (ports, names)
}

pub(super) fn available(saved: &Bindings) -> Result<Bindings, String> {
    std::panic::catch_unwind(|| {
        let midi = MidiOutput::new("SoundCraft MIDI output").map_err(|e| e.to_string())?;
        Ok(assign(saved, enumerate(&midi, saved).1))
    })
    .map_err(|_| "MIDI output enumeration failed".to_owned())?
}

fn selected<'a>(available: &'a Bindings, label: &str) -> Result<&'a str, String> {
    available.get(label).map(String::as_str).ok_or_else(|| format!("MIDI output {label} unavailable"))
}

/// Open the port bound to `label`. midir panics on a few failures of the MIDI system (see
/// [`Registry::listed`]): that is an error here, so the worker retries a second later.
fn open(label: &str, saved: &Bindings) -> Result<(MidiOutputConnection, String, PortNames), String> {
    std::panic::catch_unwind(|| {
        let midi = MidiOutput::new("SoundCraft MIDI output").map_err(|e| e.to_string())?;
        let (ports, names) = enumerate(&midi, saved);
        let ids = assign(saved, names.iter().cloned());
        let id = selected(&ids, label)?;
        let index = names.iter().position(|(_, key)| key == id).ok_or_else(|| format!("MIDI output {label} unavailable"))?;
        let conn = midi.connect(&ports[index], "SoundCraft MIDI output").map_err(|e| e.to_string())?;
        Ok((conn, id.to_owned(), names))
    })
    .map_err(|_| format!("MIDI output {label} unavailable"))?
}

/// The worker also pins a legacy selection on its first successful connection, including
/// when the device was unplugged at startup. The UI saves newly learned bindings.
#[derive(Default)]
pub(super) struct Registry {
    ids: std::sync::Mutex<Bindings>,
    /// The native port each label's connection was opened with.
    open_ids: std::sync::Mutex<Bindings>,
    learned: std::sync::atomic::AtomicBool,
}

/// Bind `label` to the native port `id` it was opened on, in `saved`, together with the labels
/// of the other listed ports of the same name ("DIN (2)" beside "DIN"), so that the occurrence
/// labels stay apart after a restart: with A's twin B saved as "DIN (2)", B never takes A's
/// label when A is unplugged (see [`assign`]). A label or port already saved is left as it is.
/// Whether `saved` changed.
fn learn(saved: &mut Bindings, found: &PortNames, label: &str, id: &str) -> bool {
    let mut changed = false;
    if saved.get(label).map(String::as_str) != Some(id) {
        saved.insert(label.to_owned(), id.to_owned());
        changed = true;
    }
    let Some(name) = found.iter().find(|(_, key)| key == id).and_then(|(name, _)| name.clone()) else { return changed };
    let current = assign(saved, found.iter().cloned());
    for (sibling, sibling_id) in current {
        let same_name = found.iter().any(|(n, key)| *key == sibling_id && n.as_deref() == Some(name.as_str()));
        if same_name && sibling != label && !saved.contains_key(&sibling) && !saved.values().any(|v| *v == sibling_id) {
            saved.insert(sibling, sibling_id);
            changed = true;
        }
    }
    changed
}

/// The labels among `labels` that `current` (the assignment of the listed ports) still binds to
/// the native port they were opened with (`opened`).
fn still_bound(current: &Bindings, opened: &Bindings, labels: &BTreeSet<String>) -> BTreeSet<String> {
    labels.iter().filter(|label| current.get(*label).is_some_and(|id| opened.get(*label) == Some(id))).cloned().collect()
}

/// The labels among `labels` that the listed ports `found` still bind to the native port they
/// were opened with (`opened`), and whether `saved` changed.
/// A port of the same name listed beside a label that is still bound is saved beside it now
/// ([`learn`]): one connected after the label opened would otherwise take that label once its
/// port is unplugged.
fn check_listed(saved: &mut Bindings, found: &PortNames, opened: &Bindings, labels: &BTreeSet<String>) -> (BTreeSet<String>, bool) {
    let bound = still_bound(&assign(saved, found.iter().cloned()), opened, labels);
    let mut changed = false;
    for label in &bound {
        if let Some(id) = opened.get(label) {
            changed |= learn(saved, found, label, id);
        }
    }
    (bound, changed)
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
        let (connection, id, found) = open(label, &saved)?;
        let mut ids = self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if ids.get(label) != saved.get(label) {
            return Err(format!("MIDI output {label} changed while connecting"));
        }
        self.open_ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(label.to_owned(), id.clone());
        // A new binding, or a saved one that followed its device to a new native ID, and the
        // same-name ports beside it.
        if learn(&mut ids, &found, label, &id) {
            self.learned.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok((connection, None))
    }

    /// The labels among `labels` that are still listed and bound to the native port they were
    /// opened with: a label whose saved binding moved to another port is not, so the worker
    /// closes it and opens the port it is bound to now. A port of the same name listed beside
    /// one that is still bound is saved beside it ([`check_listed`]). `None` when the ports
    /// cannot be listed now.
    fn listed(&self, scanner: &mut Option<MidiOutput>, labels: &BTreeSet<String>) -> Option<BTreeSet<String>> {
        let saved = self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        // midir panics on a few failures of the MIDI system; a new scanner is made next time.
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if scanner.is_none() {
                *scanner = MidiOutput::new("SoundCraft MIDI output").ok();
            }
            scanner.as_ref().map(|midi| enumerate(midi, &saved).1)
        }))
        .unwrap_or_else(|_| {
            *scanner = None;
            None
        })?;
        let opened = self.open_ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut ids = self.ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (bound, learned) = check_listed(&mut ids, &found, &opened, labels);
        if learned {
            self.learned.store(true, std::sync::atomic::Ordering::Release);
        }
        Some(bound)
    }
}

/// How the output worker opens its ports and finds out which of them are still there.
pub(super) trait Connect<C> {
    fn open(&mut self, label: &str) -> Result<(C, Option<String>), String>;
    /// Whether [`Connect::listed`] can tell: the worker then wakes once a second to check.
    fn lists(&self) -> bool {
        false
    }
    /// The labels among the connected `labels` that are still listed. `None` when the ports
    /// cannot be listed, which keeps every connection.
    fn listed(&mut self, labels: &BTreeSet<String>) -> Option<BTreeSet<String>> {
        let _ = labels;
        None
    }
}

impl<C, F: FnMut(&str) -> Result<(C, Option<String>), String>> Connect<C> for F {
    fn open(&mut self, label: &str) -> Result<(C, Option<String>), String> {
        self(label)
    }
}

/// The native ports: opened through the [`Registry`] and listed with one scanner kept for the
/// worker's life, as the MIDI inputs are.
pub(super) struct Native {
    pub(super) registry: std::sync::Arc<Registry>,
    pub(super) scanner: Option<MidiOutput>,
}

impl Connect<MidiOutputConnection> for Native {
    fn open(&mut self, label: &str) -> Result<(MidiOutputConnection, Option<String>), String> {
        self.registry.connect(label)
    }

    fn lists(&self) -> bool {
        true
    }

    fn listed(&mut self, labels: &BTreeSet<String>) -> Option<BTreeSet<String>> {
        self.registry.listed(&mut self.scanner, labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(ports: &[(&str, &str)]) -> Vec<(Option<String>, String)> {
        ports.iter().map(|(name, id)| (Some(name.to_string()), id.to_string())).collect()
    }

    #[test]
    fn alsa_output_names_lose_the_sequencer_address_so_a_device_back_as_another_client_keeps_its_label() {
        // ALSA lists `Client:Port name client:port` (midir's name), and the port's ID is that
        // address: a USB interface plugged in after another device, or after a restart, comes
        // back as another client. The MIDI inputs show such a port as "UM-ONE MIDI 1"; the
        // output list must too, or the label a track saved stops matching the device.
        let alsa = |client: u8| {
            shown_names(listed(&[
                (&format!("UM-ONE:UM-ONE MIDI 1 {client}:0"), &format!("{client}:0")),
                ("Midi Through:Midi Through Port-0 14:0", "14:0"),
            ]))
        };
        let mut saved = Bindings::new();
        let first = assign(&saved, alsa(20));
        assert_eq!(selected(&first, "UM-ONE MIDI 1"), Ok("20:0"), "{first:?}");
        learn(&mut saved, &alsa(20), "UM-ONE MIDI 1", "20:0");
        let back = assign(&saved, alsa(24));
        assert_eq!(selected(&back, "UM-ONE MIDI 1"), Ok("24:0"), "the label follows the device to its new address: {back:?}");
        // CoreMIDI and WinMM names have no address: they stay as the system gives them.
        for name in ["IAC Driver Bus 1", "MIDIOUT2 (Model 12)", "UM-ONE", "Arturia MiniLab mkII", "DIN (2)"] {
            assert_eq!(shown_names(listed(&[(name, "x")])), listed(&[(name, "x")]));
        }
        assert_eq!(shown_names(vec![(None, "x".into())]), vec![(None, "x".into())]);
    }

    #[test]
    fn named_winmm_subports_keep_their_exact_connector_across_reorder_and_partial_disconnect() {
        let raw = listed(&[("DIN", "usb"), ("DAW", "usb"), ("Other", "other")]);
        let names = winmm_names(raw.clone(), &Bindings::new());
        let saved = assign(&Bindings::new(), names);
        assert_ne!(saved["DIN"], saved["DAW"]);
        // A lone connector is keyed by its name as well as its interface.
        assert!(saved["Other"] != "other" && saved["Other"].ends_with("Other"), "{saved:?}");
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
    fn a_lone_winmm_connector_binding_never_passes_to_a_sibling_on_the_same_interface() {
        let learned = assign(&Bindings::new(), winmm_names(listed(&[("DIN", "usb")]), &Bindings::new()));
        let swapped = assign(&learned, winmm_names(listed(&[("DAW", "usb")]), &learned));
        assert!(!swapped.contains_key("DIN"), "{swapped:?}");
        assert_eq!(assign(&learned, winmm_names(listed(&[("DIN", "usb")]), &learned)), learned);
        // A binding saved by interface ID alone follows only the connector it was saved for.
        let legacy = Bindings::from([("DIN".into(), "usb".into())]);
        assert_eq!(assign(&legacy, winmm_names(listed(&[("DIN", "usb")]), &legacy))["DIN"], "usb");
        assert!(!assign(&legacy, winmm_names(listed(&[("DAW", "usb")]), &legacy)).contains_key("DIN"));
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
    fn a_port_back_under_a_new_id_keeps_its_label_when_its_name_is_its_own() {
        let saved = assign(&Bindings::new(), listed(&[("UX Synth", "old"), ("IAC", "iac")]));
        let back = assign(&saved, listed(&[("UX Synth", "new"), ("IAC", "iac")]));
        assert_eq!(back, Bindings::from([("UX Synth".into(), "new".into()), ("IAC".into(), "iac".into())]));
        // Gone again: nothing else takes the label.
        assert_eq!(assign(&saved, listed(&[("IAC", "iac")])), Bindings::from([("IAC".into(), "iac".into())]));
    }

    #[test]
    fn a_port_back_under_a_new_id_is_not_rebound_when_names_repeat() {
        let saved = assign(&Bindings::new(), listed(&[("DIN", "a"), ("DIN", "b")]));
        assert_eq!(saved, Bindings::from([("DIN".into(), "a".into()), ("DIN (2)".into(), "b".into())]));
        // One of two identical interfaces back with a new ID: it could be either.
        let one = assign(&saved, listed(&[("DIN", "c")]));
        assert_eq!(one, Bindings::from([("DIN (3)".into(), "c".into())]));
        // Its twin still there under its saved ID: the newcomer is another device.
        let twin = assign(&saved, listed(&[("DIN", "b"), ("DIN", "c")]));
        assert_eq!(twin, Bindings::from([("DIN (2)".into(), "b".into()), ("DIN (3)".into(), "c".into())]));
        // Two ports of that name listed, neither saved: no guess either.
        let lone = Bindings::from([("Synth".into(), "old".into())]);
        let two = assign(&lone, listed(&[("Synth", "x"), ("Synth", "y")]));
        assert!(!two.values().any(|id| id == "old") && !two.contains_key("Synth"), "{two:?}");
    }

    #[test]
    fn a_rebound_port_is_not_taken_from_a_label_that_still_holds_its_id() {
        // "Synth" saved with an ID that another saved label now owns: strict matching wins.
        let saved = Bindings::from([("Synth".into(), "gone".into()), ("Other".into(), "x".into())]);
        assert_eq!(assign(&saved, listed(&[("Synth", "x")])), Bindings::from([("Other".into(), "x".into())]));
        assert!(numbered("DIN (2)", "DIN") && !numbered("DIN (1)", "DIN") && !numbered("DINO (2)", "DIN") && !numbered("DIN", "DIN"));
    }

    #[test]
    fn a_saved_port_unplugged_is_not_replaced_by_its_same_name_sibling_after_a_restart() {
        // Two "DIN" interfaces; a track uses only A. Binding A also saves B's label, so with A
        // unplugged (after a restart, nothing else in memory) B keeps "DIN (2)" and the route on
        // "DIN" stays unavailable instead of playing to B.
        let both = listed(&[("DIN", "a"), ("DIN", "b"), ("IAC", "iac")]);
        let mut saved = Bindings::new();
        assert!(learn(&mut saved, &both, "DIN", "a"));
        assert_eq!(saved, Bindings::from([("DIN".into(), "a".into()), ("DIN (2)".into(), "b".into())]), "IAC is no sibling");
        let restarted: Bindings = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let after = assign(&restarted, listed(&[("DIN", "b"), ("IAC", "iac")]));
        assert!(selected(&after, "DIN").is_err(), "{after:?}");
        assert_eq!(selected(&after, "DIN (2)").unwrap(), "b");
        // A lone port back under a new ID still follows its label.
        let mut lone = Bindings::new();
        learn(&mut lone, &listed(&[("UX Synth", "old")]), "UX Synth", "old");
        assert_eq!(lone, Bindings::from([("UX Synth".into(), "old".into())]));
        assert_eq!(selected(&assign(&lone, listed(&[("UX Synth", "new")])), "UX Synth").unwrap(), "new");
        // Learning again changes nothing already saved.
        assert!(!learn(&mut saved, &both, "DIN", "a"));
        // Only the twin saved (no room left for "DIN"): B still keeps "DIN (2)", and A, listed
        // again, gets "DIN" by its name.
        let twin_only = Bindings::from([("DIN (2)".into(), "b".into())]);
        let after = assign(&twin_only, listed(&[("DIN", "b"), ("IAC", "iac")]));
        assert!(selected(&after, "DIN").is_err(), "{after:?}");
        assert_eq!(selected(&assign(&twin_only, listed(&[("DIN", "a"), ("DIN", "b")])), "DIN").unwrap(), "a");
        assert_eq!(selected(&assign(&twin_only, listed(&[("DIN", "a")])), "DIN").unwrap(), "a");
    }

    #[test]
    fn a_same_name_port_that_appears_while_a_label_is_open_is_saved_beside_it() {
        // A opened and saved as "DIN" while B was absent; B is connected later, then A unplugged.
        let mut saved = Bindings::from([("DIN".into(), "a".into())]);
        let opened = saved.clone();
        let labels = BTreeSet::from(["DIN".to_owned()]);
        let (bound, learned) = check_listed(&mut saved, &listed(&[("DIN", "a"), ("DIN", "b")]), &opened, &labels);
        assert_eq!(bound, labels);
        assert!(learned);
        assert_eq!(saved, Bindings::from([("DIN".into(), "a".into()), ("DIN (2)".into(), "b".into())]));
        // A gone: B keeps its own label, and "DIN" is unavailable instead of moving to B.
        let (bound, learned) = check_listed(&mut saved, &listed(&[("DIN", "b")]), &opened, &labels);
        assert!(bound.is_empty() && !learned);
        assert!(selected(&assign(&saved, listed(&[("DIN", "b")])), "DIN").is_err());
    }

    #[test]
    fn an_open_label_whose_binding_moved_to_another_port_is_no_longer_listed() {
        let found = || listed(&[("DIN", "a"), ("DIN", "b")]);
        let opened = Bindings::from([("DIN".into(), "a".into())]);
        let labels = BTreeSet::from(["DIN".to_owned()]);
        // Still bound to the port it was opened with.
        assert_eq!(still_bound(&assign(&opened, found()), &opened, &labels), labels);
        // The saved binding now names port b, which is listed: close and reopen on b.
        let moved = Bindings::from([("DIN".into(), "b".into())]);
        assert!(still_bound(&assign(&moved, found()), &opened, &labels).is_empty());
        // Port a unplugged while the label is bound to b: the link to a is not kept either.
        assert!(still_bound(&assign(&moved, listed(&[("DIN", "b")])), &opened, &labels).is_empty());
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
