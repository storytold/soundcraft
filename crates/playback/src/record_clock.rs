//! Capture and playback positions in the shared ASIO callback sequence.
//!
//! CPAL's ASIO backend dispatches the already-open output before the input added to the same
//! driver. Both see the same hardware cycle. Counting it avoids rounding short buffers to the
//! driver's millisecond callback timestamps; endpoint timestamps still supply its latencies.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct OutputClock {
    pub(crate) cycle: AtomicU64,
    pub(crate) first_play: Stamp,
}

#[derive(Default)]
pub(crate) struct Stamp {
    cycle: AtomicU64,
    latency_ns: AtomicU64,
    frames: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) cycle: u64,
    pub(crate) latency_ns: u64,
    pub(crate) frames: u64,
}

impl Stamp {
    pub(crate) fn clear(&self) {
        self.cycle.store(0, Ordering::Release);
    }

    pub(crate) fn write(&self, cycle: u64, latency: Duration, frames: usize) {
        self.latency_ns.store(latency.as_nanos() as u64, Ordering::Relaxed);
        self.frames.store(frames as u64, Ordering::Relaxed);
        self.cycle.store(cycle, Ordering::Release);
    }

    pub(crate) fn read(&self) -> Option<Block> {
        let cycle = self.cycle.load(Ordering::Acquire);
        (cycle != 0).then(|| Block { cycle, latency_ns: self.latency_ns.load(Ordering::Relaxed), frames: self.frames.load(Ordering::Relaxed) })
    }
}

/// Number of samples by which a raw take's first sample predates the audible play origin.
pub(crate) fn placement_offset(input: Block, output: Block, rate: u32) -> Option<i64> {
    if input.frames == 0 || input.frames != output.frames || rate == 0 {
        return None;
    }
    let phase = (i128::from(output.cycle) - i128::from(input.cycle)) * i128::from(input.frames);
    let latency_ns = i128::from(input.latency_ns) + i128::from(output.latency_ns);
    let latency = (latency_ns * i128::from(rate) + 500_000_000) / 1_000_000_000;
    let offset = i64::try_from(phase + latency).ok()?;
    // Reject discontinuous or stale pairs instead of shifting a take an implausible amount.
    (offset.unsigned_abs() <= u64::from(rate) / 2).then_some(offset)
}

#[cfg(test)]
mod tests {
    use super::{Block, placement_offset};

    #[test]
    fn uses_the_signed_callback_difference_and_driver_latency() {
        let input = Block { cycle: 3103, latency_ns: 6_333_333, frames: 256 };
        let output = Block { cycle: 3104, latency_ns: 15_166_667, frames: 256 };
        assert_eq!(placement_offset(input, output, 48_000), Some(1288));
        assert_eq!(placement_offset(Block { cycle: 3104, ..input }, output, 48_000), Some(1032));
        assert_eq!(placement_offset(Block { cycle: 3105, ..input }, output, 48_000), Some(776));
        assert_eq!(placement_offset(input, Block { frames: 512, ..output }, 48_000), None);
        assert_eq!(placement_offset(input, Block { cycle: 4000, ..output }, 48_000), None);
    }
}
