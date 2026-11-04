use serde_json::{Value, json};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first() else {
        eprintln!("usage: bounce_bench OUT.wav [reps] [JSON params]");
        std::process::exit(2);
    };
    let reps = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(5usize);
    let mut params = args.get(2).and_then(|p| serde_json::from_str::<Value>(p).ok()).unwrap_or_else(|| json!({}));
    if let Some(m) = params.as_object_mut() {
        m.insert("path".into(), json!(out));
    }
    let mut e = soundcraft_engine::demo::demo_engine();
    let mut times = Vec::new();
    let mut last = Value::Null;
    for _ in 0..reps {
        let t0 = Instant::now();
        match e.execute("file.bounce_mix", &params) {
            Ok(v) => last = v,
            Err(err) => {
                eprintln!("bounce failed: {err}");
                std::process::exit(1);
            }
        }
        times.push(t0.elapsed().as_secs_f64());
    }
    let total: f64 = times.iter().sum();
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    println!("{last}");
    println!("total {total:.4} s, min {min:.4} s over {reps} bounces");
}
