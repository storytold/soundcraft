//! Safe, allocation-free vector DSP operations.
//!
//! macOS uses Accelerate/vDSP for normal audio blocks; other platforms and short slices use the
//! same scalar operations. The public API is safe and supports in-place realtime processing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod ffi;

#[cfg(target_os = "macos")]
use std::{cell::RefCell, collections::HashMap};

/// Below this length, the function-call overhead is usually greater than vectorization's gain.
const MIN_VECTOR_LEN: usize = 64;

#[cfg(target_os = "macos")]
thread_local! {
    // Apple states that a DFT setup must not execute concurrently. A cache per worker thread
    // both reuses the expensive setup and makes that constraint structural, without a mutex.
    static FFT_PLANS: RefCell<HashMap<usize, ffi::DftPlan>> = RefCell::new(HashMap::new());
}

/// Largest absolute finite sample, or zero for an empty/non-finite slice.
#[inline]
pub fn peak(values: &[f32]) -> f32 {
    #[cfg(target_os = "macos")]
    if values.len() >= MIN_VECTOR_LEN {
        let peak = ffi::peak(values);
        if peak.is_finite() {
            return peak;
        }
    }
    values.iter().fold(0.0, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
}

/// Performs a forward complex FFT with a cached vDSP plan on macOS.
///
/// The four slices must have the same power-of-two length. Plan creation can allocate on the
/// first call for a size, so this operation is intended for analyzers and offline work rather
/// than a realtime audio callback. Returns `false` when Accelerate is unavailable or rejects the
/// transform size.
pub fn fft_forward(real: &[f32], imag: &[f32], output_real: &mut [f32], output_imag: &mut [f32]) -> bool {
    let n = real.len();
    if n < 2 || !n.is_power_of_two() || imag.len() != n || output_real.len() != n || output_imag.len() != n {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        FFT_PLANS.with(|plans| {
            let mut plans = plans.borrow_mut();
            if let std::collections::hash_map::Entry::Vacant(entry) = plans.entry(n) {
                let Some(plan) = ffi::DftPlan::new(n) else { return false };
                entry.insert(plan);
            }
            let Some(plan) = plans.get(&n) else { return false };
            plan.forward(real, imag, output_real, output_imag);
            true
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (real, imag, output_real, output_imag);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_matches_scalar_results_for_short_and_vector_blocks() {
        for len in [0, 1, 32, 64, 257, 4096] {
            let values: Vec<f32> = (0..len).map(|i| i as f32 * 0.01 - 3.0).collect();
            let expected = values.iter().fold(0.0f32, |value, sample| value.max(sample.abs()));
            assert_eq!(peak(&values), expected);
        }
    }

    #[test]
    fn peak_ignores_non_finite_values() {
        assert_eq!(peak(&[f32::NAN, -2.0, f32::INFINITY]), 2.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn vdsp_fft_finds_a_bin_and_reuses_its_plan() {
        let n = 256;
        let real: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * 7.0 * i as f32 / n as f32).sin()).collect();
        let imag = vec![0.0; n];
        let mut out_r = vec![0.0; n];
        let mut out_i = vec![0.0; n];
        assert!(fft_forward(&real, &imag, &mut out_r, &mut out_i));
        assert!(out_r[7].hypot(out_i[7]) > 127.0);
        assert!(fft_forward(&real, &imag, &mut out_r, &mut out_i));
        FFT_PLANS.with(|plans| assert_eq!(plans.borrow().len(), 1));
    }
}
