//! The Layer III hybrid filterbank (ISO/IEC 11172-3 §C.1.3 and §C.1.5.3): the 32-band polyphase
//! analysis filterbank, an 18-line MDCT per subband (long blocks only) and the encoder's side of
//! the alias-reduction butterflies.
#![allow(clippy::needless_range_loop)]

use super::tables::{ALIAS_C, WINDOW_D};
use std::f64::consts::PI;

/// Spectral lines (and new input samples) per granule.
pub(super) const GRANULE: usize = 576;

/// The filterbank's constants, shared by every channel.
pub(super) struct Bank {
    /// The analysis window C.
    window: [f32; 512],
    /// The matrixing cosines, `cos((2k+1)(i-16)π/64)`.
    matrix: [[f32; 64]; 32],
    /// The MDCT cosines with the long-block window folded in.
    mdct: [[f32; 36]; 18],
    /// The alias-reduction butterflies `(cs, ca)`.
    alias: [(f32, f32); 8],
}

impl Bank {
    pub(super) fn new() -> Box<Self> {
        let mut bank = Box::new(Bank { window: [0.0; 512], matrix: [[0.0; 64]; 32], mdct: [[0.0; 36]; 18], alias: [(0.0, 0.0); 8] });
        for (c, d) in bank.window.iter_mut().zip(WINDOW_D) {
            *c = d / 32.0;
        }
        for k in 0..32 {
            for i in 0..64 {
                bank.matrix[k][i] = ((2 * k + 1) as f64 * (i as f64 - 16.0) * PI / 64.0).cos() as f32;
            }
        }
        for m in 0..18 {
            for k in 0..36 {
                let window = (PI / 36.0 * (k as f64 + 0.5)).sin();
                bank.mdct[m][k] = (window * (PI / 72.0 * (2 * k + 1 + 18) as f64 * (2 * m + 1) as f64).cos()) as f32;
            }
        }
        for (b, c) in bank.alias.iter_mut().zip(ALIAS_C) {
            let n = (1.0 + c * c).sqrt();
            *b = ((1.0 / n) as f32, (c / n) as f32);
        }
        bank
    }
}

/// One channel's filterbank state.
pub(super) struct Analysis {
    /// The polyphase input FIFO, newest sample first.
    fifo: [f32; 512],
    /// The previous granule's subband samples, which the MDCT overlaps.
    prev: [[f32; 18]; 32],
}

impl Analysis {
    pub(super) fn new() -> Box<Self> {
        Box::new(Analysis { fifo: [0.0; 512], prev: [[0.0; 18]; 32] })
    }

    /// The 576 spectral lines of the next granule of `input`.
    pub(super) fn granule(&mut self, bank: &Bank, input: &[f32; GRANULE], xr: &mut [f32; GRANULE]) {
        let mut cur = [[0.0f32; 18]; 32];
        for t in 0..18 {
            self.fifo.copy_within(0..480, 32);
            for i in 0..32 {
                self.fifo[31 - i] = input[t * 32 + i];
            }
            let mut y = [0.0f32; 64];
            for (i, y) in y.iter_mut().enumerate() {
                *y = (0..8).map(|j| bank.window[i + 64 * j] * self.fifo[i + 64 * j]).sum();
            }
            for k in 0..32 {
                cur[k][t] = bank.matrix[k].iter().zip(&y).map(|(m, y)| m * y).sum();
            }
        }
        // Frequency inversion: the odd time samples of the odd subbands change sign.
        for sb in (1..32).step_by(2) {
            for t in (1..18).step_by(2) {
                cur[sb][t] = -cur[sb][t];
            }
        }
        for sb in 0..32 {
            let (prev, now) = (&self.prev[sb], &cur[sb]);
            for m in 0..18 {
                let row = &bank.mdct[m];
                let a: f32 = row[..18].iter().zip(prev).map(|(c, x)| c * x).sum();
                let b: f32 = row[18..].iter().zip(now).map(|(c, x)| c * x).sum();
                xr[sb * 18 + m] = a + b;
            }
        }
        self.prev = cur;
        for sb in 1..32 {
            for (i, &(cs, ca)) in bank.alias.iter().enumerate() {
                let (lo, hi) = (xr[18 * sb - 1 - i], xr[18 * sb + i]);
                xr[18 * sb - 1 - i] = lo * cs + hi * ca;
                xr[18 * sb + i] = hi * cs - lo * ca;
            }
        }
    }
}
