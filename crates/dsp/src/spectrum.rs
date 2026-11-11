//! FFT magnitude analysis for analyzers and EQ displays.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Largest FFT size accepted (larger requests are clamped).
pub const MAX_FFT: usize = 1 << 18;

fn rust_plan(n: usize) -> Arc<dyn Fft<f32>> {
    type Plans = Mutex<HashMap<usize, Arc<dyn Fft<f32>>>>;
    static PLANS: OnceLock<Plans> = OnceLock::new();
    let plans = PLANS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut plans = plans.lock().unwrap_or_else(PoisonError::into_inner);
    plans.entry(n).or_insert_with(|| FftPlanner::<f32>::new().plan_fft_forward(n)).clone()
}

fn hann_window(n: usize) -> Arc<[f32]> {
    type Windows = Mutex<HashMap<usize, Arc<[f32]>>>;
    static WINDOWS: OnceLock<Windows> = OnceLock::new();
    let windows = WINDOWS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut windows = windows.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(windows.entry(n).or_insert_with(|| (0..n).map(|i| hann(i, n)).collect::<Arc<[_]>>()))
}

#[inline]
fn hann(i: usize, n: usize) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()
}

/// Hann-windowed magnitude spectrum in dBFS, `fft_size / 2 + 1` bins (bin `k` is at
/// `k · sr / fft_size` Hz). A full-scale sine reads ≈ 0 dB. Input shorter than `fft_size` is
/// zero-padded; longer input uses the first `fft_size` samples. Floored at -144 dB.
pub fn magnitude_db(samples: &[f32], fft_size: usize) -> Vec<f32> {
    let n = fft_size.min(MAX_FFT);
    if n < 2 {
        return Vec::new();
    }
    let window = hann_window(n);
    #[cfg(target_os = "macos")]
    {
        let real: Vec<f32> =
            (0..n).map(|i| samples.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0) * window.get(i).copied().unwrap_or(0.0)).collect();
        let imag = vec![0.0; n];
        let mut output_real = vec![0.0; n];
        let mut output_imag = vec![0.0; n];
        if soundcraft_accelerate::fft_forward(&real, &imag, &mut output_real, &mut output_imag) {
            let scale = 2.0 / (n as f32 * 0.5);
            return output_real.iter().zip(&output_imag).take(n / 2 + 1).map(|(r, i)| crate::gain_to_db(r.hypot(*i) * scale)).collect();
        }
    }

    let fft = rust_plan(n);
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| {
            let x = samples.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            Complex::new(x * window.get(i).copied().unwrap_or(0.0), 0.0)
        })
        .collect();
    fft.process(&mut buf);
    // Hann coherent gain is 0.5; a sine's energy splits across ±f.
    let scale = 2.0 / (n as f32 * 0.5);
    buf.iter().take(n / 2 + 1).map(|c| crate::gain_to_db(c.norm() * scale)).collect()
}

/// Frequency in Hz of bin `k` for a given FFT size.
pub fn bin_hz(k: usize, fft_size: usize, sample_rate: f32) -> f32 {
    if fft_size == 0 { 0.0 } else { k as f32 * sample_rate / fft_size as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peak_near_zero_db() {
        let sr = 48_000.0;
        let n = 4096;
        let k = 100;
        let f = bin_hz(k, n, sr);
        let x: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * f * i as f32 / sr).sin()).collect();
        let m = magnitude_db(&x, n);
        assert_eq!(m.len(), n / 2 + 1);
        assert!(m[k].abs() < 0.1, "{}", m[k]);
        assert!(magnitude_db(&x, 0).is_empty());
    }

    #[test]
    fn rust_fft_plans_are_reused() {
        assert!(Arc::ptr_eq(&rust_plan(256), &rust_plan(256)));
        assert!(Arc::ptr_eq(&hann_window(256), &hann_window(256)));
    }
}
