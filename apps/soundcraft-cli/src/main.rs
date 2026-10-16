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
//! soundcraft-cli mcp [--connect PORT] [--demo] [--automation-read-root DIR] [--automation-write-root DIR]
//!                                                 MCP server on stdio. The two roots confine every file the
//!                                                 agent's commands read (open, import, presets) and write
//!                                                 (save, bounce, export) to those folders, links followed;
//!                                                 a root left out grants none of its access. They run the
//!                                                 in-process server, so they conflict with --connect.
//! soundcraft-cli parity [--write PATH]            parity report (markdown)
//! soundcraft-cli plugins                          list built-in plugins
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod automation_args;

use serde_json::{Value, json};
use soundcraft_automation::{Backend, Headless, Remote, mcp::Server};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{self, AutomationRoots};
use std::io::Write;
use std::process::ExitCode;

/// Print without panicking on a closed pipe. A write error must not skip later work
/// such as `--bounce` or `--save`, and must not by itself report success.
macro_rules! outln {
    ($($t:tt)*) => {{
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, $($t)*);
    }};
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("soundcraft-cli: {msg}");
    ExitCode::FAILURE
}

/// The usage block of the module doc comment, up to its closing code fence.
fn help_text() -> String {
    include_str!("main.rs")
        .lines()
        .skip(3)
        .take_while(|l| l.trim_end() != "//! ```")
        .map(|l| l.trim_start_matches("//! "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: soundcraft-cli <info|convert|run|script|commands|describe|app|mcp|parity|plugins> …  (see --help)");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let code = match cmd.as_str() {
        "--version" | "-V" | "version" => {
            outln!("SoundCraft CLI {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            outln!("{}", help_text());
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
    };
    // The subcommand's engine and its plugin instances are gone by now.
    soundcraft_engine::shutdown_plugin_hosts();
    code
}

fn info(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("info FILE") };
    if path.ends_with(".scraft") {
        let mut e = Engine::default();
        match soundcraft_engine::io::open_session(&mut e, path) {
            Ok(missing) => warn_missing(&missing),
            Err(err) => return fail(err),
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

fn option_value(args: &[String], index: &mut usize, flag: &str, inline: Option<&str>) -> Result<String, String> {
    let value = match inline {
        Some(value) => value,
        None => {
            let value = args.get(*index + 1).ok_or_else(|| format!("{flag} needs a value"))?;
            if value.starts_with("--") {
                return Err(format!("{flag} needs a value"));
            }
            *index += 1;
            value
        }
    };
    if value.is_empty() {
        return Err(format!("{flag} needs a value"));
    }
    Ok(value.to_owned())
}

struct ConvertArgs {
    input: String,
    output: String,
    rate: Option<u32>,
    bit_depth: soundcraft_audio_io::BitDepth,
    normalize: bool,
}

fn parse_convert_args(args: &[String]) -> Result<ConvertArgs, String> {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else { return Err("convert IN OUT".into()) };
    let mut rate = None;
    let mut bit_depth = soundcraft_audio_io::BitDepth::Int24;
    let mut normalize = false;
    let mut i = 2;
    while let Some(arg) = args.get(i) {
        let (flag, inline) = arg.split_once('=').map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
        match flag {
            "--rate" => rate = parse_rate(Some(&option_value(args, &mut i, flag, inline)?))?,
            "--bit-depth" => bit_depth = parse_bit_depth(Some(&option_value(args, &mut i, flag, inline)?))?,
            "--normalize" if inline.is_none() => normalize = true,
            other if other.starts_with("--") => return Err(format!("unknown convert option `{arg}`")),
            _ => return Err(format!("unexpected convert argument `{arg}`")),
        }
        i += 1;
    }
    Ok(ConvertArgs { input: input.clone(), output: output.clone(), rate, bit_depth, normalize })
}

fn convert(args: &[String]) -> ExitCode {
    let ConvertArgs { input, output, rate, bit_depth, normalize } = match parse_convert_args(args) {
        Ok(args) => args,
        Err(err) => return fail(err),
    };
    let bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    let ext = std::path::Path::new(&input).extension().and_then(|x| x.to_str());
    let (_, mut buf) = match soundcraft_audio_io::decode(&bytes, ext) {
        Ok(x) => x,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    if let Some(rate) = rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, rate);
        buf.sample_rate = rate;
    }
    if normalize {
        soundcraft_dsp::offline::normalize(&mut buf.channels, -0.1, false);
    }
    let out_ext = std::path::Path::new(&output).extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let Some(format) = soundcraft_audio_io::encode_format_for(&out_ext) else {
        return fail(format!("{output}: cannot write `.{out_ext}` files (supported: {})", soundcraft_audio_io::ENCODE_EXTENSIONS));
    };
    let opts = soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: true, bwf: None };
    match soundcraft_audio_io::encode(&buf, &opts).map_err(|e| e.to_string()).and_then(|b| std::fs::write(&output, b).map_err(|e| e.to_string())) {
        Ok(()) => {
            outln!("wrote {output} ({} ch, {} Hz, {:.2} s)", buf.num_channels(), buf.sample_rate, buf.duration_secs());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn parse_rate(v: Option<&str>) -> Result<Option<u32>, String> {
    match v {
        None => Ok(None),
        Some(r) => match r.parse::<u32>() {
            Ok(hz) if hz > 0 => Ok(Some(hz)),
            _ => Err(format!("--rate: expected a sample rate in Hz, got `{r}`")),
        },
    }
}

fn parse_bit_depth(v: Option<&str>) -> Result<soundcraft_audio_io::BitDepth, String> {
    match v {
        None | Some("24") => Ok(soundcraft_audio_io::BitDepth::Int24),
        Some("16") => Ok(soundcraft_audio_io::BitDepth::Int16),
        Some("32") | Some("32f") => Ok(soundcraft_audio_io::BitDepth::Float32),
        Some(b) => Err(format!("--bit-depth: expected 16, 24 or 32f, got `{b}`")),
    }
}

/// One stderr line per media file a session refers to but could not load (the exit code is unchanged).
fn missing_warnings(missing: &[String]) -> Vec<String> {
    missing.iter().map(|m| format!("warning: missing media: {m}")).collect()
}

fn warn_missing(missing: &[String]) {
    for line in missing_warnings(missing) {
        eprintln!("{line}");
    }
}

fn load_engine(args: &[String]) -> Result<Engine, String> {
    if let Some(p) = arg_value(args, "--in") {
        let mut e = Engine::default();
        let missing = soundcraft_engine::io::open_session(&mut e, &p).map_err(|e| e.to_string())?;
        warn_missing(&missing);
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
        match e.execute("session.save", &json!({"path": path, "replace": true})) {
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

/// `ID` or `ID=JSON` → command id and params; malformed JSON is an error, not `{}`.
fn parse_cmd_spec(spec: &str) -> Result<(String, Value), String> {
    match spec.split_once('=') {
        Some((id, p)) => serde_json::from_str::<Value>(p).map(|v| (id.to_string(), v)).map_err(|err| format!("{id}: bad JSON: {err}")),
        None => Ok((spec.to_string(), json!({}))),
    }
}

struct RunArgs {
    shared: Vec<String>,
    commands: Vec<String>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut shared = Vec::new();
    let mut commands = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let (flag, inline) = arg.split_once('=').map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
        match flag {
            "--in" | "--bounce" | "--start" | "--end" | "--save" => {
                let value = option_value(args, &mut i, flag, inline)?;
                shared.extend([flag.to_owned(), value]);
            }
            "--cmd" => commands.push(option_value(args, &mut i, flag, inline)?),
            "--demo" | "--sample" | "--inspect" if inline.is_none() => shared.push(flag.to_owned()),
            other if other.starts_with("--") => return Err(format!("unknown run option `{arg}`")),
            _ => return Err(format!("unexpected run argument `{arg}`")),
        }
        i += 1;
    }
    Ok(RunArgs { shared, commands })
}

fn run(args: &[String]) -> ExitCode {
    let RunArgs { shared, commands } = match parse_run_args(args) {
        Ok(args) => args,
        Err(err) => return fail(err),
    };
    let mut e = match load_engine(&shared) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    for spec in commands {
        let (id, params) = match parse_cmd_spec(&spec) {
            Ok(v) => v,
            Err(err) => return fail(err),
        };
        match e.execute(&id, &params) {
            Ok(v) => outln!("{id}: {v}"),
            Err(err) => return fail(format!("{id}: {err}")),
        }
    }
    finish(&mut e, &shared)
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

/// Control-channel methods. Every other dotted id is an engine command.
fn is_control_method(method: &str) -> bool {
    matches!(
        method,
        "engine.execute"
            | "command"
            | "engine.commands"
            | "engine.parity"
            | "session.inspect"
            | "document.inspect"
            | "ui.inspect"
            | "ui.menu.list"
            | "ui.menu.invoke"
            | "ui.set"
            | "ui.move"
            | "ui.click"
            | "ui.drag"
            | "ui.key"
            | "ui.text"
            | "ui.screenshot"
            | "app.quit"
    )
}

/// A command id is shorthand for `engine.execute`. Documented control methods stay methods.
fn app_request(method: &str, params: Value) -> (String, Value) {
    if is_control_method(method) || !method.contains('.') {
        (method.to_string(), params)
    } else {
        ("engine.execute".to_string(), json!({"command": method, "params": params}))
    }
}

fn app(args: &[String]) -> ExitCode {
    let port = arg_value(args, "--port").or_else(|| std::env::var("SOUNDCRAFT_CONTROL_PORT").ok()).unwrap_or_else(|| "7979".into());
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (*i > 0 && args.get(i - 1).is_some_and(|p| p == "--port"))))
        .map(|(_, a)| a)
        .collect();
    let Some(method) = positional.first() else { return fail("app [--port P] METHOD [JSON]") };
    let params = match positional.get(1) {
        Some(p) => match serde_json::from_str::<Value>(p) {
            Ok(v) => v,
            Err(err) => return fail(format!("{method}: bad JSON: {err}")),
        },
        None => json!({}),
    };
    let mut r = Remote::new(&port);
    // A bare command id is shorthand for engine.execute. Control-channel methods stay methods.
    let (m, p) = app_request(method, params);
    match r.call(&m, p) {
        Ok(v) => {
            outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// What `mcp` was asked for.
struct McpArgs {
    connect: Option<String>,
    demo: bool,
    roots: Option<std::sync::Arc<AutomationRoots>>,
}

/// Parse the `mcp` options up front: a malformed or misspelled root flag must not leave the
/// server running unconfined.
fn parse_mcp_args(args: &[String]) -> Result<McpArgs, String> {
    let mut connect = None;
    let mut demo = false;
    let mut roots_args = automation_args::AutomationArgs::new(None, None);
    let mut it = args.iter().cloned();
    while let Some(a) = it.next() {
        if roots_args.consume(&a, &mut it)? {
            continue;
        }
        match a.as_str() {
            "--connect" => connect = Some(it.next().ok_or("--connect needs a port or host:port")?),
            "--demo" => demo = true,
            other if other.starts_with("--automation-") => return Err(format!("unknown mcp option `{other}`")),
            _ => {}
        }
    }
    let roots = AutomationRoots::new(roots_args.read_root.as_deref(), roots_args.write_root.as_deref())?;
    if roots.is_some() && connect.is_some() {
        return Err("--automation-read-root and --automation-write-root confine the in-process server; a running app is not confined by them, \
so drop --connect (or use --connect without the roots)"
            .into());
    }
    Ok(McpArgs { connect, demo, roots })
}

fn mcp(args: &[String]) -> ExitCode {
    let McpArgs { connect, demo, roots } = match parse_mcp_args(args) {
        Ok(a) => a,
        Err(err) => return fail(err),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let r = if let Some(port) = connect {
        let mut s = Server::new(Remote::new(&port));
        s.serve(stdin.lock(), stdout.lock())
    } else {
        // `--demo` is the operator's own choice, made before the roots come into force.
        let e = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
        let mut h = Headless::new(e);
        if let Some(roots) = roots {
            // Every thread of this process, not only the server's: nothing reaches past the roots.
            file_access::confine_process(roots.clone());
            let show = |root: Option<&std::path::Path>| root.map_or_else(|| "none".to_string(), |r| r.display().to_string());
            eprintln!("soundcraft-cli: files confined: read root {}, write root {}", show(roots.read_root()), show(roots.write_root()));
            h = h.with_automation_roots(Some(roots));
        }
        let mut s = Server::new(h);
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
    md.push_str("# Parity report\n\nGenerated by `soundcraft-cli parity --write docs/parity-checklist.md` (also `cargo xtask parity`).\n");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn convert_fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("soundcraft-cli-{tag}-{}", std::process::id()));
        let input = base.with_extension("input.wav");
        let output = base.with_extension("output.wav");
        let audio = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 16]] };
        let bytes = soundcraft_audio_io::encode(&audio, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
        std::fs::write(&input, bytes).unwrap();
        let _ = std::fs::remove_file(&output);
        (input, output)
    }

    fn convert_args(input: &std::path::Path, output: &std::path::Path, options: &[&str]) -> Vec<String> {
        [input.to_string_lossy().into_owned(), output.to_string_lossy().into_owned()]
            .into_iter()
            .chain(options.iter().map(|s| (*s).to_owned()))
            .collect()
    }

    #[test]
    fn command_ids_are_execute_shorthand_and_control_methods_stay_methods() {
        let (m, p) = app_request("engine.history", json!({}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["command"], "engine.history");
        let (m, p) = app_request("ui.theme", json!({"mode": "light"}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["params"]["mode"], "light");
        let (m, _) = app_request("session.new", json!({"name": "Session A"}));
        assert_eq!(m, "engine.execute");
        let (m, _) = app_request("track.new", json!({}));
        assert_eq!(m, "engine.execute");
        for method in ["session.inspect", "engine.commands", "engine.parity", "ui.screenshot", "app.quit"] {
            assert_eq!(app_request(method, json!({})).0, method);
        }
    }

    #[test]
    fn convert_rejects_invalid_rate_and_bit_depth() {
        assert_eq!(parse_rate(None), Ok(None));
        assert_eq!(parse_rate(Some("44100")), Ok(Some(44_100)));
        assert!(parse_rate(Some("4410x")).is_err());
        assert!(parse_rate(Some("0")).is_err());
        assert!(matches!(parse_bit_depth(None), Ok(soundcraft_audio_io::BitDepth::Int24)));
        assert!(matches!(parse_bit_depth(Some("16")), Ok(soundcraft_audio_io::BitDepth::Int16)));
        assert!(matches!(parse_bit_depth(Some("32f")), Ok(soundcraft_audio_io::BitDepth::Float32)));
        assert!(parse_bit_depth(Some("8")).is_err());
    }

    #[test]
    fn convert_rejects_unknown_and_misspelled_options() {
        let (input, output) = convert_fixture("unknown-options");
        let bogus = convert_args(&input, &output, &["--bogus"]);
        assert_eq!(parse_convert_args(&bogus).err().as_deref(), Some("unknown convert option `--bogus`"));
        assert_eq!(convert(&bogus), ExitCode::FAILURE);
        let normalise = convert_args(&input, &output, &["--normalise"]);
        assert_eq!(parse_convert_args(&normalise).err().as_deref(), Some("unknown convert option `--normalise`"));
        assert_eq!(convert(&normalise), ExitCode::FAILURE);
        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn convert_rejects_option_without_value() {
        let (input, output) = convert_fixture("missing-value");
        let args = convert_args(&input, &output, &["--rate"]);
        assert_eq!(parse_convert_args(&args).err().as_deref(), Some("--rate needs a value"));
        assert_eq!(convert(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn convert_accepts_equals_options() {
        let (input, output) = convert_fixture("equals-options");
        assert_eq!(convert(&convert_args(&input, &output, &["--rate=24000", "--bit-depth=16"])), ExitCode::SUCCESS);
        let bytes = std::fs::read(&output).unwrap();
        let info = soundcraft_audio_io::probe(&bytes, Some("wav")).unwrap();
        assert_eq!(info.sample_rate, 24_000);
        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn run_rejects_bad_options_and_accepts_equals_values() {
        let bogus = ["--bogus".into()];
        assert_eq!(parse_run_args(&bogus).err().as_deref(), Some("unknown run option `--bogus`"));
        assert_eq!(run(&bogus), ExitCode::FAILURE);
        let missing = ["--bounce".into()];
        assert_eq!(parse_run_args(&missing).err().as_deref(), Some("--bounce needs a value"));
        assert_eq!(run(&missing), ExitCode::FAILURE);
        let path = std::env::temp_dir().join(format!("soundcraft-cli-run-equals-{}.scraft", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(run(&[format!("--save={}", path.display())]), ExitCode::SUCCESS);
        assert!(path.exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_media_gets_one_warning_each() {
        assert!(missing_warnings(&[]).is_empty());
        let w = missing_warnings(&["a.wav".to_string(), "b.wav: decode failed".to_string()]);
        assert_eq!(w, vec!["warning: missing media: a.wav", "warning: missing media: b.wav: decode failed"]);
    }

    #[test]
    fn cmd_spec_parses_id_and_json() {
        assert_eq!(parse_cmd_spec("a.b").unwrap(), ("a.b".to_string(), json!({})));
        assert_eq!(parse_cmd_spec("a.b={\"x\":1}").unwrap(), ("a.b".to_string(), json!({"x": 1})));
    }

    #[test]
    fn cmd_spec_rejects_truncated_json() {
        let err = parse_cmd_spec("a.b={\"x\":").unwrap_err();
        assert!(err.starts_with("a.b: bad JSON"), "{err}");
    }

    #[test]
    fn help_text_is_only_the_usage_block() {
        let help = help_text();
        assert!(help.starts_with("soundcraft-cli info FILE"));
        assert!(help.ends_with("soundcraft-cli plugins                          list built-in plugins"));
        assert!(help.contains("--automation-read-root DIR") && help.contains("--automation-write-root DIR"));
        assert!(!help.contains("```"));
        assert!(!help.contains("#![") && !help.contains("use serde_json"));
    }
}
