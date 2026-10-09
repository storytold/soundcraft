//! `soundcraft-cli`: SoundCraft without a window.
//!
//! ```text
//! soundcraft-cli info FILE                       audio file or .scraft summary
//! soundcraft-cli convert IN OUT [--bit-depth 16|24|32f] [--rate HZ] [--normalize]
//! soundcraft-cli run [--in S.scraft | --demo] [--cmd ID[=JSON]]… [--bounce OUT] [--start T --end T] [--save S.scraft] [--inspect]
//! soundcraft-cli script FILE [--in S | --demo] [--save S] [--bounce OUT]   (one `ID JSON` per line)
//! soundcraft-cli commands [FILTER]               list commands
//! soundcraft-cli describe ID                     one command's details
//! soundcraft-cli app [--port P] METHOD [JSON]     call a running app's control channel
//! soundcraft-cli mcp [--connect PORT] [--demo]    MCP server on stdio
//! soundcraft-cli parity [--write PATH]            parity report (markdown)
//! soundcraft-cli plugins                          list built-in plugins
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde_json::{Value, json};
use soundcraft_automation::{Backend, Headless, Remote, mcp::Server};
use soundcraft_engine::Engine;
use std::io::Write;
use std::process::ExitCode;

/// Print without panicking on a closed pipe.
macro_rules! outln {
    ($($t:tt)*) => {{
        let mut o = std::io::stdout().lock();
        if writeln!(o, $($t)*).is_err() { return ExitCode::SUCCESS; }
    }};
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("soundcraft-cli: {msg}");
    ExitCode::FAILURE
}

/// Parse a JSON parameter string. Malformed JSON is an error (never silently `{}`), while an
/// omitted argument is fine — the caller decides the default.
fn parse_params(p: &str) -> Result<Value, ExitCode> {
    serde_json::from_str::<Value>(p).map_err(|err| fail(format!("bad JSON: {err}")))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: soundcraft-cli <info|convert|run|script|commands|describe|app|mcp|parity|plugins> …  (see --help)");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    match cmd.as_str() {
        "--version" | "-V" | "version" => {
            outln!("SoundCraft CLI {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            // The usage is the doc block's fenced region: skip the first doc lines (module title +
            // opening ```text), then take lines up to (not including) the closing fence.
            let usage = include_str!("main.rs").lines().skip_while(|l| !l.starts_with("//! ```text")).skip(1).take_while(|l| !l.starts_with("//! ```")).map(|l| l.trim_start_matches("//! ")).collect::<Vec<_>>().join("\n");
            outln!("{usage}");
            ExitCode::SUCCESS
        }
        "info" => info(&rest),
        "convert" => convert(&rest),
        "run" => run(&rest),
        "script" => script(&rest),
        "commands" => commands(&rest),
        "describe" => describe(&rest),
        "app" => app(&rest),
        "mcp" => mcp(&rest),
        "parity" => parity(&rest),
        "plugins" => {
            for p in soundcraft_dsp::plugins() {
                outln!("{:<20} {:<22} {:?}{}", p.id, p.name, p.category, if p.is_instrument { " (instrument)" } else { "" });
            }
            ExitCode::SUCCESS
        }
        other => fail(format!("unknown subcommand `{other}`")),
    }
}

fn info(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("info FILE") };
    if path.ends_with(".scraft") {
        let mut e = Engine::default();
        if let Err(err) = soundcraft_engine::io::open_session(&mut e, path) {
            return fail(err);
        }
        outln!("{}", soundcraft_engine::inspect::session_text(&e));
        return ExitCode::SUCCESS;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let ext = std::path::Path::new(path).extension().and_then(|x| x.to_str());
    match soundcraft_audio_io::probe(&bytes, ext) {
        Ok(i) => {
            outln!("{}", serde_json::to_string_pretty(&json!({"format": i.format, "sample_format": i.sample_format, "sample_rate": i.sample_rate, "channels": i.channels, "frames": i.frames, "seconds": i.duration_secs(), "bwf": i.bwf})).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(format!("{path}: {e}")),
    }
}

fn convert(args: &[String]) -> ExitCode {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else { return fail("convert IN OUT") };
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    let ext = std::path::Path::new(input).extension().and_then(|x| x.to_str());
    let (_, mut buf) = match soundcraft_audio_io::decode(&bytes, ext) {
        Ok(x) => x,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    if let Some(rate) = arg_value(args, "--rate").and_then(|r| r.parse::<u32>().ok()) {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, rate);
        buf.sample_rate = rate;
    }
    if args.iter().any(|a| a == "--normalize") {
        soundcraft_dsp::offline::normalize(&mut buf.channels, -0.1, false);
    }
    let out_ext = std::path::Path::new(output).extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let format = match out_ext.as_str() {
        "aif" | "aiff" => soundcraft_audio_io::FileFormat::Aiff,
        "flac" => soundcraft_audio_io::FileFormat::Flac,
        "wav" => soundcraft_audio_io::FileFormat::Wav,
        other => return fail(format!("unsupported output format: {other} (supported: wav, aiff, flac)")),
    };
    let bit_depth = match arg_value(args, "--bit-depth").as_deref() {
        Some("16") => soundcraft_audio_io::BitDepth::Int16,
        Some("32") | Some("32f") => soundcraft_audio_io::BitDepth::Float32,
        _ => soundcraft_audio_io::BitDepth::Int24,
    };
    let opts = soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: true, bwf: None };
    match soundcraft_audio_io::encode(&buf, &opts).map_err(|e| e.to_string()).and_then(|b| std::fs::write(output, b).map_err(|e| e.to_string())) {
        Ok(()) => {
            outln!("wrote {output} ({} ch, {} Hz, {:.2} s)", buf.num_channels(), buf.sample_rate, buf.duration_secs());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn load_engine(args: &[String]) -> Result<Engine, String> {
    if let Some(p) = arg_value(args, "--in") {
        let mut e = Engine::default();
        soundcraft_engine::io::open_session(&mut e, &p).map_err(|e| e.to_string())?;
        Ok(e)
    } else if args.iter().any(|a| a == "--demo" || a == "--sample") {
        Ok(soundcraft_engine::demo::demo_engine())
    } else {
        Ok(Engine::default())
    }
}

fn finish(e: &mut Engine, args: &[String]) -> ExitCode {
    if let Some(out) = arg_value(args, "--bounce") {
        let mut p = json!({"path": out});
        if let Some(s) = arg_value(args, "--start") {
            p["start"] = time_arg(&s);
        }
        if let Some(s) = arg_value(args, "--end") {
            p["end"] = time_arg(&s);
        }
        match e.execute("file.bounce_mix", &p) {
            Ok(v) => outln!("{v}"),
            Err(err) => return fail(err),
        }
    }
    if let Some(path) = arg_value(args, "--save") {
        match e.execute("session.save", &json!({"path": path})) {
            Ok(v) => outln!("{v}"),
            Err(err) => return fail(err),
        }
    }
    if args.iter().any(|a| a == "--inspect") {
        outln!("{}", serde_json::to_string_pretty(&soundcraft_engine::inspect::session(e, false)).unwrap_or_default());
    }
    ExitCode::SUCCESS
}

/// `12.5` → seconds; anything else is passed through as a time string.
fn time_arg(s: &str) -> Value {
    s.parse::<f64>().map_or_else(|_| json!(s), |f| json!({"seconds": f}))
}

fn run(args: &[String]) -> ExitCode {
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let mut i = 0;
    while i < args.len() {
        if args.get(i).is_some_and(|a| a == "--cmd")
            && let Some(spec) = args.get(i + 1)
        {
            let (id, params) = match spec.split_once('=') {
                Some((id, p)) => (id.to_string(), parse_params(p)?),
                None => (spec.clone(), json!({})),
            };
            match e.execute(&id, &params) {
                Ok(v) => outln!("{id}: {v}"),
                Err(err) => return fail(format!("{id}: {err}")),
            }
            i += 1;
        }
        i += 1;
    }
    finish(&mut e, args)
}

fn script(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("script FILE") };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let keep = args.iter().any(|a| a == "--keep-going");
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, p) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
        let params = match serde_json::from_str::<Value>(p.trim()) {
            Ok(v) => v,
            Err(err) => return fail(format!("{path}:{}: bad JSON: {err}", n + 1)),
        };
        match e.execute(id, &params) {
            Ok(v) => outln!("{id}: {v}"),
            Err(err) => {
                eprintln!("{path}:{}: {id}: {err}", n + 1);
                if !keep {
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    finish(&mut e, args)
}

fn commands(args: &[String]) -> ExitCode {
    let filter = args.first().cloned().unwrap_or_default();
    let e = Engine::default();
    for c in soundcraft_engine::command_specs() {
        if !filter.is_empty() && !c.id.contains(&filter) && !c.label.to_ascii_lowercase().contains(&filter.to_ascii_lowercase()) {
            continue;
        }
        let i = c.info(&e);
        outln!("{:<36} {:<34} {}", i.id, i.label, i.menu.join(" > "));
    }
    ExitCode::SUCCESS
}

fn describe(args: &[String]) -> ExitCode {
    let Some(id) = args.first() else { return fail("describe ID") };
    match soundcraft_engine::find_command(id) {
        Some(c) => {
            outln!("{}", serde_json::to_string_pretty(&c.info(&Engine::default())).unwrap_or_default());
            ExitCode::SUCCESS
        }
        None => fail(format!("unknown command `{id}`")),
    }
}

fn app(args: &[String]) -> ExitCode {
    let port = arg_value(args, "--port").or_else(|| std::env::var("SOUNDCRAFT_CONTROL_PORT").ok()).unwrap_or_else(|| "7979".into());
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && args.get(i - 1).is_some_and(|p| p == "--port")))
        .map(|(_, a)| a)
        .collect();
    let Some(method) = positional.first() else { return fail("app [--port P] METHOD [JSON]") };
    let params = match positional.get(1) {
        Some(p) => parse_params(p)?,
        None => json!({}),
    };
    let mut r = Remote::new(&port);
    // A bare command id is shorthand for engine.execute.
    let (m, p) = if method.contains('.')
        && !method.starts_with("ui.")
        && !method.starts_with("engine.")
        && !method.starts_with("session.")
        && !method.starts_with("app.")
    {
        ("engine.execute".to_string(), json!({"command": method, "params": params}))
    } else {
        (method.to_string(), params)
    };
    match r.call(&m, p) {
        Ok(v) => {
            outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn mcp(args: &[String]) -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let r = if let Some(port) = arg_value(args, "--connect") {
        let mut s = Server::new(Remote::new(&port));
        s.serve(stdin.lock(), stdout.lock())
    } else {
        let e = if args.iter().any(|a| a == "--demo") { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
        let mut s = Server::new(Headless::new(e));
        s.serve(stdin.lock(), stdout.lock())
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

fn parity(args: &[String]) -> ExitCode {
    let p = soundcraft_engine::catalog::parity_json();
    let mut md = String::new();
    md.push_str("# Parity report\n\nGenerated by `soundcraft-cli parity --write docs/parity.md` (also `cargo xtask parity`).\n");
    md.push_str("It compares the incumbent's menu tree (feature names observed black-box, `crates/engine/catalog/menus.txt`) with SoundCraft's command registry.\n");
    md.push_str("UI-layer commands (windows, AudioSuite) are counted by the app's own report (`engine.parity` over the control channel).\n\n");
    md.push_str(&format!(
        "**Menu items implemented by the engine: {} / {} ({}%)**\n\n| Menu | Implemented | Total |\n|---|---:|---:|\n",
        p["implemented"], p["total"], p["percent"]
    ));
    for m in p["per_menu"].as_array().cloned().unwrap_or_default() {
        md.push_str(&format!("| {} | {} | {} |\n", m["menu"].as_str().unwrap_or(""), m["implemented"], m["total"]));
    }
    md.push_str("\n## Not yet implemented\n\n");
    for m in p["missing"].as_array().cloned().unwrap_or_default() {
        md.push_str(&format!("- {}\n", m.as_str().unwrap_or("")));
    }
    if let Some(path) = arg_value(args, "--write") {
        if let Err(e) = std::fs::write(&path, &md) {
            return fail(format!("{path}: {e}"));
        }
        outln!("wrote {path}: {}% ({}/{})", p["percent"], p["implemented"], p["total"]);
    } else {
        outln!("{md}");
    }
    ExitCode::SUCCESS
}
