//! Bound the transient follow grid while retaining the timing of live takes and current playback.
use soundcraft_time::{Range, SampleRate, TempoEvent, TempoMap};

const HISTORY_CAP: usize = 512;

#[derive(Default)]
pub(super) struct History {
    pub(super) next_check: usize,
    /// The old and compacted time maps resume the same slope and sample at this tick.
    pub(super) last_join: Option<i64>,
}

impl History {
    pub(super) fn has_take_credit(&self) -> bool {
        self.next_check > HISTORY_CAP
    }

    pub(super) fn should_compact(&self, grid: &TempoMap) -> bool {
        grid.tempos().len() > self.next_check.max(HISTORY_CAP)
    }

    pub(super) fn compact(&mut self, grid: &mut TempoMap, sr: SampleRate, origin: i64, take_ranges: Vec<Range>) {
        // Include the interval at each take edge: a sample can fall between two integer ticks.
        let mut protected: Vec<_> = take_ranges
            .into_iter()
            .map(|r| {
                let start = grid.samples_to_ticks(r.start, sr).saturating_sub(1);
                let end = grid.samples_to_ticks(r.end, sr).saturating_add(1);
                (start, end)
            })
            .collect();
        // Only the interval containing Start/Continue must keep its slope. Exact
        // bridges reproduce its starting sample even when the SPP is hours into the score.
        protected.push((origin, origin));
        protected.sort_unstable();
        // Only compact old *complete* intervals. Keep half the cap as a recent window.
        let old_len = grid.tempos().len().saturating_sub(HISTORY_CAP / 2);
        let mut ranges = protected.as_slice();
        // Use exactly the cumulative ticks/BPM addition order of TempoMap. Independent
        // sums per run would round differently after the joins, especially on half-samples.
        let mut elapsed = 0.;
        let mut previous: Option<TempoEvent> = None;
        let segments: Vec<_> = grid
            .tempos()
            .iter()
            .take(old_len.saturating_add(1))
            .map(|&event| {
                if let Some(prev) = previous {
                    elapsed += event.tick.saturating_sub(prev.tick) as f64 / prev.bpm;
                }
                previous = Some(event);
                (event, elapsed)
            })
            .collect();
        let mut run_start = 0;
        let mut changed = false;
        for (index, pair) in segments.windows(2).enumerate() {
            let [(a, _), (b, _)] = pair else { continue };
            if references(&mut ranges, a.tick, b.tick) {
                if let Some(run) = segments.get(run_start..=index) {
                    changed |= compress_run(grid, run);
                }
                run_start = index + 1;
            }
        }
        if let Some(run) = segments.get(run_start..) {
            changed |= compress_run(grid, run);
        }
        if changed {
            self.last_join = segments.last().map(|(event, _)| event.tick);
        }
        let events = grid.tempos();
        let mut ranges = protected.as_slice();
        let ends = events.iter().skip(1).map(|next| next.tick).chain(std::iter::once(i64::MAX));
        let needed = events.iter().zip(ends).filter(|(e, end)| references(&mut ranges, e.tick, *end)).count();
        // Credit only retained take events. One position anchor is already included
        // in the 512-event allowance. An old, completed take cannot inflate the budget
        // as unrelated revisions arrive, and an open take can keep its required history.
        self.next_check = HISTORY_CAP.saturating_add(needed.saturating_sub(1));
    }
}

fn references(ranges: &mut &[(i64, i64)], start: i64, end: i64) -> bool {
    while let Some((range, rest)) = ranges.split_first() {
        if range.1 >= start {
            return range.0 < end;
        }
        *ranges = rest;
    }
    false
}

/// A bridge replaces the interior changes. Its weighted harmonic mean preserves elapsed
/// time at the far edge; rounding to samples here would accumulate error across passes.
fn compress_run(grid: &mut TempoMap, events: &[(TempoEvent, f64)]) -> bool {
    let [(first, start), middle @ .., (last, end)] = events else { return false };
    if middle.is_empty() {
        return false;
    }
    let ticks = last.tick.saturating_sub(first.tick) as f64;
    let bpm = ticks / (end - start);
    // Division is rounded too. Only drop the intervening knots if this bridge reconstructs
    // the *exact* accumulated endpoint. A different rounding pair can need two bridges.
    if let Some(exact) =
        [bpm, bpm.next_down(), bpm.next_up()].into_iter().find(|&candidate| (5.0..=1000.).contains(&candidate) && start + ticks / candidate == *end)
    {
        if grid.set_tempo(first.tick, exact).is_err() {
            return false;
        }
        for (event, _) in middle.iter().rev() {
            grid.remove_tempo(event.tick);
        }
        return true;
    }
    let mid = events.len() / 2;
    let (Some(left), Some(right)) = (events.get(..=mid), events.get(mid..)) else { return false };
    let first_changed = compress_run(grid, left);
    compress_run(grid, right) || first_changed
}
