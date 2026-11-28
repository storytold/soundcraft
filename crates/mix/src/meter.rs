use super::instances::PluginFailures;

/// Peak meter values for one strip (linear, per channel). [`MixEngine`] measures one block; a
/// realtime host folds blocks together with [`StripMeter::accumulate`] so a reader polling less
/// often than blocks arrive sees the highest peak since it last looked, not just the latest block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StripMeter {
    /// First two channels (mono strips repeat channel 0), for stereo meters.
    pub peak: [f32; 2],
    /// Every channel of the strip (or of the main mix).
    pub peaks: Vec<f32>,
    /// Gain reduction of the first dynamics insert, dB.
    pub gain_reduction_db: f32,
    pub plugin_failures: PluginFailures,
}

impl StripMeter {
    /// Copy another meter without reallocating once the channel count is stable.
    pub fn copy_from(&mut self, o: &StripMeter) {
        self.peak = o.peak;
        self.peaks.clone_from(&o.peaks);
        self.gain_reduction_db = o.gain_reduction_db;
        self.plugin_failures = o.plugin_failures;
    }

    /// Fold another (later) measurement in: per-channel peaks and the gain reduction keep their
    /// maximum, plugin failures take the later value (the count only grows). Allocation-free once
    /// the channel count is stable, so it can run on the audio thread every block.
    pub fn accumulate(&mut self, o: &StripMeter) {
        if self.peaks.len() != o.peaks.len() {
            // The strip's format changed (a document edit): earlier channels no longer line up.
            self.peaks.clone_from(&o.peaks);
            self.peak = o.peak;
        } else {
            for (p, q) in self.peaks.iter_mut().zip(&o.peaks) {
                *p = max_level(*p, *q);
            }
            for (p, q) in self.peak.iter_mut().zip(o.peak) {
                *p = max_level(*p, q);
            }
        }
        self.gain_reduction_db = max_level(self.gain_reduction_db, o.gain_reduction_db);
        self.plugin_failures = o.plugin_failures;
    }

    /// Zero the levels (peaks and gain reduction) after a reader took them, keeping the channel
    /// count and the plugin failure record. Never allocates.
    pub fn reset_levels(&mut self) {
        self.peak = [0.0; 2];
        self.peaks.iter_mut().for_each(|p| *p = 0.0);
        self.gain_reduction_db = 0.0;
    }

    pub(crate) fn measure(&mut self, bufs: &[Vec<f32>], frames: usize, gr: f32) {
        if self.peaks.len() != bufs.len() {
            self.peaks.resize(bufs.len(), 0.0);
        }
        for (p, b) in self.peaks.iter_mut().zip(bufs.iter()) {
            *p = peak(b.get(..frames).unwrap_or(&[]));
        }
        let last = self.peaks.len().saturating_sub(1);
        self.peak = [self.peaks.first().copied().unwrap_or(0.0), self.peaks.get(1.min(last)).copied().unwrap_or(0.0)];
        self.gain_reduction_db = gr;
    }
}

/// The larger of two meter levels; a NaN (from a misbehaving plugin) never wins or sticks.
fn max_level(a: f32, b: f32) -> f32 {
    match (a.is_finite(), b.is_finite()) {
        (true, true) => a.max(b),
        (true, false) => a,
        (false, true) => b,
        (false, false) => 0.0,
    }
}

fn peak(x: &[f32]) -> f32 {
    soundcraft_dsp::offline::channel_peak(x)
}
