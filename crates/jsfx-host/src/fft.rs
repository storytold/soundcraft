//! In-place radix-2 complex FFT over interleaved `(re, im)` pairs, with EEL2's calling convention:
//! `fft` leaves its output in "permuted" (bit-reversed) order and `ifft` consumes that same order;
//! `fft_permute` / `fft_ipermute` convert to and from natural order. Neither direction is scaled.

use std::f64::consts::PI;

/// Largest transform EEL2 accepts, in complex points.
const MAX_POINTS: usize = 32_768;

/// Number of complex points for a requested size, `None` unless a supported power of two.
pub(crate) fn points(n: f64) -> Option<usize> {
    let n = n as usize;
    ((2..=MAX_POINTS).contains(&n) && n.is_power_of_two()).then_some(n)
}

/// Swaps every element with its bit-reversed partner (its own inverse).
pub(crate) fn permute(data: &mut [f64]) {
    let n = data.len() / 2;
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let shift = usize::BITS - n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> shift;
        if j > i {
            data.swap(2 * i, 2 * j);
            data.swap(2 * i + 1, 2 * j + 1);
        }
    }
}

/// Butterfly passes; the input must already be in bit-reversed order, the output is natural.
fn butterflies(data: &mut [f64], inverse: bool) {
    let n = data.len() / 2;
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let mut len = 2;
    while len <= n {
        let ang = if inverse { 2.0 } else { -2.0 } * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let half = len / 2;
        let mut start = 0;
        while start < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..half {
                let a = 2 * (start + k);
                let b = 2 * (start + k + half);
                let (ur, ui) = (data[a], data[a + 1]);
                let (xr, xi) = (data[b], data[b + 1]);
                let vr = xr * cr - xi * ci;
                let vi = xr * ci + xi * cr;
                data[a] = ur + vr;
                data[a + 1] = ui + vi;
                data[b] = ur - vr;
                data[b + 1] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// Forward FFT: natural-order input, permuted-order output.
pub(crate) fn fft(data: &mut [f64]) {
    permute(data);
    butterflies(data, false);
    permute(data);
}

/// Inverse FFT: permuted-order input (as left by [`fft`]), natural-order output, unscaled.
pub(crate) fn ifft(data: &mut [f64]) {
    butterflies(data, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_has_a_flat_spectrum() {
        let mut d = vec![0.0; 32];
        d[0] = 1.0;
        fft(&mut d);
        permute(&mut d);
        for k in 0..16 {
            assert!((d[2 * k] - 1.0).abs() < 1e-12 && d[2 * k + 1].abs() < 1e-12);
        }
    }

    #[test]
    fn sine_lands_in_its_bin_and_round_trips() {
        let n = 64;
        let src: Vec<f64> = (0..n).flat_map(|i| [(2.0 * PI * 5.0 * i as f64 / n as f64).cos(), 0.0]).collect();
        let mut d = src.clone();
        fft(&mut d);
        let mut nat = d.clone();
        permute(&mut nat);
        let mag = |k: usize| nat[2 * k].hypot(nat[2 * k + 1]);
        assert!((mag(5) - 32.0).abs() < 1e-9 && mag(6) < 1e-9);
        ifft(&mut d);
        for (a, b) in d.iter().zip(&src) {
            assert!((a / n as f64 - b).abs() < 1e-9);
        }
    }

    #[test]
    fn bad_sizes_are_rejected() {
        assert_eq!(points(1024.0), Some(1024));
        assert_eq!(points(1000.0), None);
        assert_eq!(points(1.0), None);
        assert_eq!(points(1e12), None);
        assert_eq!(points(f64::NAN), None);
    }
}
