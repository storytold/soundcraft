//! Render the real UI offscreen (no window, no focus stealing) and save PNGs.
//!
//! `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png [script.jsonl] [--blank] [--size WxH] [--system-theme light|dark|none]`
//!
//! Script lines are control requests (`{"method": "...", "params": {...}}`), `{"shot": "path.png"}`
//! or `{"steps": n}`. `{"system_theme": "light"}` (also `"dark"` or `"none"`) changes the
//! simulated OS appearance for subsequent frames. The demo session is loaded unless `--blank`.

use soundcraft_ui_egui::{ControlRequest, Services, SoundApp};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

static READY: AtomicBool = AtomicBool::new(false);

fn system_theme(value: &str) -> Option<Option<egui::Theme>> {
    match value {
        "light" => Some(Some(egui::Theme::Light)),
        "dark" => Some(Some(egui::Theme::Dark)),
        "none" => Some(None),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "soundcraft.png".into());
    let script = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let blank = args.iter().any(|a| a == "--blank");
    let appearance = if let Some(i) = args.iter().position(|a| a == "--system-theme") {
        let Some(appearance) = args.get(i + 1).and_then(|value| system_theme(value)) else {
            eprintln!("--system-theme requires light, dark or none");
            return;
        };
        appearance
    } else {
        None
    };
    let (w, h) = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))
        .unwrap_or((1600.0, 1000.0));
    let engine = if blank { soundcraft_engine::Engine::default() } else { soundcraft_engine::demo::demo_engine() };
    let (tx, rx) = mpsc::channel::<ControlRequest>();
    let app = SoundApp::new(engine, None, Services::default()).with_control(rx);
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(2.0).with_max_steps(1_000_000).wgpu().build_ui_state(
            |ui, app: &mut SoundApp| {
                if !READY.load(Ordering::Relaxed) {
                    return;
                }
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.input_mut().max_texture_side = Some(8192);
    harness.input_mut().system_theme = appearance;
    READY.store(true, Ordering::Relaxed);
    let step = |h: &mut egui_kittest::Harness<SoundApp>| {
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
    };
    for _ in 0..6 {
        step(&mut harness);
    }
    let shoot = |h: &mut egui_kittest::Harness<SoundApp>, path: &str| match h.render() {
        Ok(img) => match img.save(path) {
            Ok(()) => println!("wrote {path}"),
            Err(e) => eprintln!("{path}: {e}"),
        },
        Err(e) => eprintln!("render failed: {e}"),
    };
    if let Some(script) = script {
        let text = std::fs::read_to_string(&script).unwrap_or_default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                eprintln!("bad line: {line}");
                continue;
            };
            if let Some(value) = v.get("system_theme") {
                let Some(appearance) = value.as_str().and_then(system_theme) else {
                    eprintln!("system_theme requires light, dark or none: {line}");
                    continue;
                };
                harness.input_mut().system_theme = appearance;
                step(&mut harness);
                continue;
            }
            if let Some(p) = v.get("shot").and_then(|x| x.as_str()) {
                for _ in 0..3 {
                    step(&mut harness);
                }
                shoot(&mut harness, p);
                continue;
            }
            if let Some(n) = v.get("steps").and_then(|x| x.as_u64()) {
                for _ in 0..n {
                    step(&mut harness);
                }
                continue;
            }
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
            let (req, reply) = ControlRequest::new(method, params);
            let _ = tx.send(req);
            for _ in 0..40 {
                step(&mut harness);
                if let Ok(r) = reply.try_recv() {
                    let s = r.to_string();
                    println!("{}", reply_preview(&s));
                    break;
                }
            }
            for _ in 0..12 {
                step(&mut harness);
            }
        }
    }
    for _ in 0..4 {
        step(&mut harness);
    }
    shoot(&mut harness, &out);
}

fn reply_preview(text: &str) -> String {
    text.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reply_preview_preserves_utf8_boundaries() {
        let text = format!("x{}", "เสียงร้อง".repeat(100));
        assert_eq!(super::reply_preview(&text), text.chars().take(300).collect::<String>());
        assert_eq!(super::reply_preview("เสียงร้อง"), "เสียงร้อง");
    }
}
