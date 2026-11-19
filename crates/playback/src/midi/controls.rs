//! Routing on the MIDI producer and UI threads. The audio callback only sees performance MIDI.

use super::{LiveMidi, MidiFeed, PoisonError};
use std::collections::{BTreeMap, BTreeSet};

/// One batch from a single CC. Absolute knobs use the latest value, relative encoders
/// accumulate signed increments, and momentary buttons count rising edges.
#[derive(Clone, Copy)]
pub struct ControlChange {
    pub event: LiveMidi,
    steps: i32,
    range: (i32, i32),
    pub presses: u32,
}

impl ControlChange {
    /// Preserve the effect of hitting either endpoint and then reversing between UI frames.
    pub fn relative_value(&self, position: u8) -> u8 {
        // capture() shifts both endpoints by the same delta and clamps monotonically.
        (i32::from(position) + self.steps).clamp(self.range.0, self.range.1) as u8
    }
}

#[derive(Default)]
pub(super) struct ControlTap {
    sources: BTreeMap<String, BTreeSet<(u8, u8)>>,
    learning: bool,
    candidate: Option<(u8, u8, u8)>,
    first: Option<ControlChange>,
    pending: Vec<ControlChange>,
    held: BTreeSet<(u8, u8, u8)>,
}

impl ControlTap {
    pub(super) fn disconnect(&mut self, port: u8) {
        self.held.retain(|&(p, _, _)| p != port);
        self.pending.retain(|change| change.event.port != port);
        if self.first.is_some_and(|change| change.event.port == port) {
            self.first = None;
        }
        if self.candidate.is_some_and(|(p, _, _)| p == port) {
            self.candidate = None;
        }
    }

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
        let down = event.data2 >= 64;
        let pressed = if down {
            self.held.insert(source)
        } else {
            self.held.remove(&source);
            false
        };
        let delta = i32::from(event.data2) - 64;
        let mut change = ControlChange { event, steps: delta, range: (delta.max(0), (127 + delta).min(127)), presses: u32::from(pressed) };
        if self.learning && empty {
            self.first = Some(change);
        } else {
            // One latest move per source, ordered by its last movement. The first learn
            // event is kept separately so a stalled frame can both assign and apply a knob.
            if let Some(i) =
                self.pending.iter().position(|c| (c.event.port, c.event.status, c.event.data1) == (event.port, event.status, event.data1))
            {
                let old = self.pending.remove(i);
                change.range = ((old.range.0 + delta).clamp(0, 127), (old.range.1 + delta).clamp(0, 127));
                change.steps = (old.steps + delta).clamp(-127, 127);
                change.presses = old.presses.saturating_add(change.presses);
            }
            self.pending.push(change);
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
        let candidate = tap.candidate;
        tap.held.retain(|&(port, ch, cc)| {
            (learning && candidate == Some((port, ch, cc)))
                || self.ports().name(port).is_some_and(|name| table.get(name).is_some_and(|keys| keys.contains(&(ch, cc))))
        });
        tap.sources = table;
        tap.learning = learning;
        if !learning {
            tap.candidate = None;
        }
    }

    /// A new learn or document starts with fresh input.
    pub fn clear_control_capture(&self) {
        let mut tap = self.controls.lock().unwrap_or_else(PoisonError::into_inner);
        tap.held.clear();
        tap.first = None;
        tap.pending.clear();
        tap.candidate = None;
        tap.learning = false;
    }

    pub fn set_control_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        self.control_wake.get_or_init(|| Box::new(wake));
    }

    pub fn control_changes(&self) -> impl Iterator<Item = ControlChange> + use<> {
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
    fn midi_learn_relative_turn_keeps_endpoint_reversal_and_unplug_releases_buttons() {
        let feed = MidiFeed::default();
        let port = feed.ports().assign("Controller");
        feed.set_control_sources([("Controller".into(), 1, 22), ("Controller".into(), 1, 112)], false);
        feed.receive(port, &[0xb0, 112, 68]);
        feed.receive(port, &[0xb0, 112, 62]);
        let turn = feed.control_changes().next().unwrap();
        assert_eq!(turn.relative_value(126), 125);
        assert_eq!(turn.relative_value(64), 66);
        feed.receive(port, &[0xb0, 22, 127]);
        let open = std::sync::atomic::AtomicBool::new(true);
        feed.close_port(&open, port);
        assert!(feed.control_changes().next().is_none());
        feed.receive(port, &[0xb0, 22, 127]);
        assert_eq!(feed.control_changes().next().unwrap().presses, 1);
    }

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
        assert_eq!(feed.control_changes().map(|e| (e.event.data1, e.event.data2)).collect::<Vec<_>>(), [(21, 2), (71, 75), (21, 99)]);
        assert_eq!(std::iter::from_fn(|| feed.next()).map(|(_, e)| e.data1).collect::<Vec<_>>(), [99, 22]);
        feed.receive(port, &[0xb0, 71, 20]);
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn switch_pedal_release_reaches_performance_during_learn() {
        let feed = MidiFeed::default();
        let port = feed.ports().assign("Keys");
        for cc in 64..=69 {
            feed.set_control_sources([], false);
            feed.receive(port, &[0xb0, cc, 127]);
            feed.set_control_sources([], true);
            feed.receive(port, &[0xb0, cc, 0]);
            let performance: Vec<_> = std::iter::from_fn(|| feed.next()).map(|(_, e)| (e.data1, e.data2)).collect();
            assert_eq!(performance, [(cc, 127), (cc, 0)]);
            assert!(feed.control_changes().next().is_none());
        }
    }
}
