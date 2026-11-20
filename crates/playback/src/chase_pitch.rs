//! Pitch correction for external timecode. The transport still consumes every source block at
//! the chase rate. Read short overlapping grains of the device mix at the inverse rate so that
//! oscillator periods retain their original duration. Channel offsets always move together.
use super::MAX_CHANNELS;

pub(crate) struct ChasePitch {
    history: Vec<[f32; MAX_CHANNELS]>,
    inverse_rates: Vec<f64>,
    hop: usize,
    search: i64,
    lookback: i64,
    written: i64,
    phase: usize,
    outgoing: f64,
    incoming: f64,
}

impl ChasePitch {
    pub(crate) fn new(device_rate: f64) -> Self {
        let hop = (device_rate * 0.010).round().clamp(32.0, 2048.0) as usize;
        let search = (hop as i64 * 2 / 5).max(1);
        let lookback = hop as i64 + 2 * search + 4;
        let capacity = (lookback as usize + 2 * hop + 8).next_power_of_two();
        let mut processor = Self {
            history: vec![[0.0; MAX_CHANNELS]; capacity],
            inverse_rates: vec![1.0; capacity],
            hop,
            search,
            lookback,
            written: 0,
            phase: 0,
            outgoing: 0.0,
            incoming: 0.0,
        };
        processor.reset();
        processor
    }

    pub(crate) fn reset(&mut self) {
        self.written = 0;
        self.phase = 0;
        self.incoming = -self.lookback as f64;
        self.outgoing = self.incoming;
    }

    /// Device frames of source history behind the last output. The timecode servo needs
    /// the matched grain position, which can differ from the nominal lookback by a period.
    pub(crate) fn latency(&self) -> f64 {
        let incoming_weight = if self.phase == 0 { 1.0 } else { self.phase as f64 / self.hop as f64 };
        let read = self.outgoing + incoming_weight * (self.incoming - self.outgoing);
        (self.written as f64 - read).max(0.0)
    }

    fn at(&self, frame: i64, channel: usize) -> f32 {
        if frame < 0 || frame > self.written || self.written - frame >= self.history.len() as i64 {
            0.0
        } else {
            self.history[frame as usize & (self.history.len() - 1)][channel]
        }
    }

    fn sample(&self, frame: f64, channel: usize) -> f32 {
        let i = frame.floor() as i64;
        let a = self.at(i, channel);
        a + (frame - i as f64) as f32 * (self.at(i + 1, channel) - a)
    }

    // The two sides of a join can belong to different transport speeds. Advance each
    // head using the rate at which its own input samples were rendered.
    fn read_step(&self, head: f64) -> f64 {
        if head < 0.0 {
            return 1.0;
        }
        self.inverse_rates[head as usize & (self.inverse_rates.len() - 1)]
    }

    fn next_grain(&mut self, channels: usize, speed: f64) {
        self.outgoing = self.incoming;
        let target = self.written - self.lookback;
        let natural = self.outgoing.round() as i64;
        if target < self.search || (self.outgoing - target as f64).abs() < 0.01 {
            self.incoming = target as f64;
            return;
        }
        // Use the strongest channel as guide (including anti-phase stereo). Digital silence
        // carries no phase information, so it can return to the nominal delay immediately.
        let mut best = 0.0;
        let mut guide = 0;
        for ch in 0..channels {
            let energy: f32 = (0..self.hop).step_by(8).map(|i| self.at(natural + i as i64, ch).powi(2)).sum();
            if energy > best {
                best = energy;
                guide = ch;
            }
        }
        self.incoming = if best < 1e-12 {
            target as f64
        } else if (target - natural) as f64 * (speed - 1.0).signum() <= self.search as f64 {
            self.outgoing
        } else {
            use soundcraft_dsp::offline::best_overlap_start;
            let matched = best_overlap_start(&|i| self.at(i, guide), target, natural, self.search, self.hop);
            if (matched - natural).abs() >= (self.hop / 10) as i64 {
                matched as f64
            } else {
                // At bass frequencies the narrow search can run into its boundary before it
                // finds another period. Match the preceding waveform on the far side of the
                // target; both windows are already available without increasing output latency.
                let radius = (self.lookback - self.hop as i64 / 8) / 2;
                let center = target + (target - natural).signum() * radius;
                best_overlap_start(&|i| self.at(i - self.hop as i64, guide), center, natural, radius, self.hop) as f64
            }
        };
    }

    pub(crate) fn process(&mut self, out: &mut [f32], channels: usize, speed: f64) {
        let active = channels.min(MAX_CHANNELS);
        let step = speed.recip();
        for frame in out.chunks_exact_mut(channels) {
            let slot = self.written as usize & (self.history.len() - 1);
            self.history[slot][..active].copy_from_slice(&frame[..active]);
            self.inverse_rates[slot] = step;
            if self.phase == 0 {
                self.next_grain(active, speed);
            }
            let blend = self.phase as f32 / self.hop as f32;
            for (ch, dst) in frame[..active].iter_mut().enumerate() {
                let a = self.sample(self.outgoing, ch);
                *dst = a + blend * (self.sample(self.incoming, ch) - a);
            }
            self.incoming += self.read_step(self.incoming);
            self.outgoing += self.read_step(self.outgoing);
            self.written += 1;
            self.phase = (self.phase + 1) % self.hop;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g4_grain_joins_preserve_short_window_pitch_during_chase() {
        // The transport has already sped up the source. Exercise every join, including
        // the ones between the points a whole-note average would sample.
        let hz = 391.99543598174927;
        for (initial, corrected) in [(1.025, 1.013), (0.98, 0.993), (1.025, 1.025)] {
            let mut pitch = ChasePitch::new(48_000.0);
            let mut wave = Vec::new();
            let mut phase = 0.0_f64;
            for block in 0..50 {
                let speed = if block < 24 { initial } else { corrected };
                let mut samples = [0.0; 480];
                for sample in &mut samples {
                    *sample = phase.sin() as f32;
                    phase += std::f64::consts::TAU * hz * speed / 48_000.0;
                }
                pitch.process(&mut samples, 1, speed);
                wave.extend(samples);
            }
            let crossings: Vec<f64> = wave[2400..]
                .windows(2)
                .enumerate()
                .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
                .map(|(i, w)| i as f64 - f64::from(w[0]) / f64::from(w[1] - w[0]))
                .collect();
            // Sixteen cycles of G4 span 40.8 ms, comparable to the hardware pitch window.
            let worst = crossings
                .windows(17)
                .map(|w| {
                    let measured = 16.0 * 48_000.0 / (w[16] - w[0]);
                    (1200.0 * (measured / hz).log2()).abs()
                })
                .fold(0.0_f64, f64::max);
            assert!(worst < 2.0, "G4 at {initial}x to {corrected}x deviated by {worst:.3} cents");
        }
    }

    #[test]
    fn unity_keeps_the_first_transient_and_reset_discards_the_previous_take() {
        let mut pitch = ChasePitch::new(48_000.0);
        let delay = pitch.latency() as usize;
        let mut samples = vec![0.0; (delay + 700) * 2];
        samples[0] = 1.0;
        samples[1] = -0.5;
        for chunk in samples.chunks_mut(178) {
            pitch.process(chunk, 2, 1.0);
        }
        assert_eq!(samples[delay * 2..delay * 2 + 2], [1.0, -0.5]);
        assert_eq!(samples.iter().filter(|v| **v != 0.0).count(), 2);
        pitch.process(&mut vec![1.0; 2400], 2, 1.04);
        pitch.reset();
        let mut silent = vec![0.0; 4800];
        pitch.process(&mut silent, 2, 0.98);
        assert!(silent.iter().all(|v| *v == 0.0));
    }
}
