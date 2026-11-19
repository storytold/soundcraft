//! Routing on the MIDI producer and UI threads. The audio callback only sees performance MIDI.

use super::{LiveMidi, MidiFeed, PoisonError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct ControlTap {
    sources: BTreeMap<String, BTreeSet<(u8, u8)>>,
    learning: bool,
    candidate: Option<(u8, u8, u8)>,
    first: Option<LiveMidi>,
    pending: Vec<LiveMidi>,
}

impl ControlTap {
    /// None passes through to instruments and recording. A captured event wakes the UI
    /// only on the transition from an empty batch to pending work.
    pub(super) fn capture(&mut self, device: &str, event: LiveMidi) -> Option<bool> {
        let channel = (event.status & 15) + 1;
        let source = (event.port, channel, event.data1);
        if !self.sources.get(device).is_some_and(|ccs| ccs.contains(&(channel, event.data1)))
            && (!self.learning || *self.candidate.get_or_insert(source) != source)
        {
            return None;
        }
        let empty = self.first.is_none() && self.pending.is_empty();
        if self.learning && empty {
            self.first = Some(event);
        } else {
            // One latest move per source, ordered by its last movement. The first learn
            // event is kept separately so a stalled frame can both assign and apply a knob.
            self.pending.retain(|e| (e.port, e.status, e.data1) != (event.port, event.status, event.data1));
            self.pending.push(event);
        }
        Some(empty)
    }
}

impl MidiFeed {
    /// Publish all valid project assignments by device name. A newly arriving port uses
    /// this table on its first message, without waiting for the next UI rescan.
    pub fn set_control_sources(&self, sources: impl IntoIterator<Item = (String, u8, u8)>, learning: bool) {
        let mut table = BTreeMap::<_, BTreeSet<_>>::new();
        for (device, channel, cc) in sources {
            table.entry(device).or_default().insert((channel, cc));
        }
        let mut tap = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        // Reserve on the UI thread for each mapped source and one learn candidate.
        let capacity = table.values().map(BTreeSet::len).sum::<usize>() + 1;
        let extra = capacity.saturating_sub(tap.pending.len());
        tap.pending.reserve(extra);
        tap.sources = table;
        tap.learning = learning;
        if !learning {
            tap.candidate = None;
        }
    }

    /// A new learn or document starts with fresh input.
    pub fn clear_control_capture(&self) {
        let mut tap = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        tap.first = None;
        tap.pending.clear();
        tap.candidate = None;
        tap.learning = false;
    }

    pub fn set_control_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        self.control_wake.get_or_init(|| Box::new(wake));
    }

    pub fn control_changes(&self) -> impl Iterator<Item = LiveMidi> + use<> {
        let mut tap = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        let first = tap.first.take();
        let changes: Vec<_> = first.into_iter().chain(tap.pending.drain(..)).collect();
        changes.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn midi_learn_wakes_once_per_batch_and_keeps_the_final_value() {
        let feed = MidiFeed::default();
        let port = feed.ports().assign("Keys");
        let wakes = Arc::new(AtomicUsize::default());
        let count = Arc::clone(&wakes);
        feed.set_control_wake(move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
        feed.set_control_sources([("Keys".into(), 1, 71)], true);
        feed.receive(port, &[0xb0, 99, 0]); // NRPN must reach the instrument intact.
        feed.receive(port, &[0xb0, 21, 2]); // First unassigned control starts learning.
        feed.receive(port, &[0xb0, 22, 3]); // Another unassigned control still plays through.
        for v in 0..500 {
            feed.receive(port, &[0xb0, 21, (v % 128) as u8]);
        }
        feed.receive(port, &[0xb0, 71, 75]);
        feed.receive(port, &[0xb0, 21, 99]);
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert_eq!(feed.control_changes().map(|e| (e.data1, e.data2)).collect::<Vec<_>>(), [(21, 2), (71, 75), (21, 99)]);
        assert_eq!(std::iter::from_fn(|| feed.next()).map(|(_, e)| e.data1).collect::<Vec<_>>(), [99, 22]);
        feed.receive(port, &[0xb0, 71, 20]);
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
    }
}
