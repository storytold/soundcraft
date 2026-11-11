//! Reproducible scalar-versus-Accelerate microbenchmarks.
//!
//! Run on a quiet machine with:
//! `cargo run --release -p soundcraft-accelerate --example benchmark`

use std::hint::black_box;
use std::time::{Duration, Instant};

use rustfft::{FftPlanner, num_complex::Complex};

const TOTAL_SAMPLES: usize = 16 * 1024 * 1024;

fn elapsed(mut operation: impl FnMut(), iterations: usize) -> Duration {
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    start.elapsed()
}

fn report(name: &str, size: usize, scalar: Duration, accelerated: Duration, iterations: usize) {
    let samples = (size * iterations) as f64;
    let scalar_ns = scalar.as_nanos() as f64 / samples;
    let accelerated_ns = accelerated.as_nanos() as f64 / samples;
    println!(
        "{name:10} n={size:5} scalar={scalar_ns:8.3} ns/sample accelerated={accelerated_ns:8.3} ns/sample speedup={:6.2}x",
        scalar_ns / accelerated_ns
    );
}

fn peak_benchmark(size: usize) {
    let iterations = (TOTAL_SAMPLES / size).max(1);
    let source: Vec<f32> = (0..size).map(|i| i as f32 * 0.000_1 - 0.5).collect();
    let scalar_time = elapsed(
        || {
            let peak = black_box(&source).iter().fold(0.0f32, |value, sample| if sample.is_finite() { value.max(sample.abs()) } else { value });
            black_box(peak);
        },
        iterations,
    );
    let accelerated_time = elapsed(
        || {
            black_box(soundcraft_accelerate::peak(black_box(&source)));
        },
        iterations,
    );
    report("peak", size, scalar_time, accelerated_time, iterations);
}

fn fft_benchmark(size: usize) {
    let iterations = ((TOTAL_SAMPLES / 8) / size).max(1);
    let real: Vec<f32> = (0..size).map(|i| (std::f32::consts::TAU * 17.0 * i as f32 / size as f32).sin()).collect();
    let imag = vec![0.0; size];

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(size);
    let mut scalar: Vec<Complex<f32>> = real.iter().map(|sample| Complex::new(*sample, 0.0)).collect();
    let scalar_time = elapsed(
        || {
            fft.process(&mut scalar);
            black_box(&scalar);
        },
        iterations,
    );

    let mut output_real = vec![0.0; size];
    let mut output_imag = vec![0.0; size];
    if !soundcraft_accelerate::fft_forward(&real, &imag, &mut output_real, &mut output_imag) {
        println!("fft        n={size:5} Accelerate unavailable");
        return;
    }
    let accelerated_time = elapsed(
        || {
            black_box(soundcraft_accelerate::fft_forward(&real, &imag, &mut output_real, &mut output_imag));
        },
        iterations,
    );
    report("fft", size, scalar_time, accelerated_time, iterations);
}

fn main() {
    println!("scalar versus soundcraft-accelerate (lower is better)");
    for size in [32, 64, 256, 1024, 4096] {
        peak_benchmark(size);
    }
    for size in [256, 1024, 4096] {
        fft_benchmark(size);
    }
}
