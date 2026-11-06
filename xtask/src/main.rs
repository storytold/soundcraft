//! `cargo xtask <ci|test|layers|assets|parity|wasm>` — repository gates.

mod ico;
mod version;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn cargo(args: &[&str]) -> Result<(), String> {
    let st = Command::new(cargo_bin()).args(args).current_dir(root()).status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(format!("cargo {} failed", args.join(" "))) }
}

/// Layer table: crate name (minus `soundcraft-`) → layer. Apps and xtask are exempt.
const LAYERS: &[(&str, u8)] = &[
    ("time", 0),
    ("audio-io", 0),
    ("midi", 0),
    ("video", 0),
    ("dsp", 1),
    ("clap-host", 1),
    ("vst3-host", 1),
    ("au-host", 1),
    ("model", 2),
    ("mix", 3),
    ("engine", 4),
    ("playback", 4),
    ("automation", 5),
    ("ui-egui", 6),
];
/// Standalone format crates: no workspace dependencies at all (publishable on their own).
const STANDALONE: &[&str] = &["time", "audio-io", "midi", "video", "dsp"];
const UI_CRATES: &[&str] = &["craft-ui", "egui", "eframe", "winit", "egui_kittest", "rfd", "egui-wgpu", "egui_glow"];
const EXEMPT: &[&str] = &["soundcraft", "soundcraft-cli", "soundcraft-web", "xtask"];

fn layers() -> Result<(), String> {
    let out =
        Command::new("cargo").args(["metadata", "--format-version", "1", "--no-deps"]).current_dir(root()).output().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    for p in v["packages"].as_array().cloned().unwrap_or_default() {
        let name = p["name"].as_str().unwrap_or("").to_string();
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        let short = name.trim_start_matches("soundcraft-");
        let Some(&(_, layer)) = LAYERS.iter().find(|(n, _)| *n == short) else {
            problems.push(format!("{name}: not registered in xtask LAYERS"));
            continue;
        };
        for d in p["dependencies"].as_array().cloned().unwrap_or_default() {
            let dn = d["name"].as_str().unwrap_or("");
            let kind = d["kind"].as_str().unwrap_or("normal");
            if UI_CRATES.contains(&dn) && layer < 6 && kind != "dev" {
                problems.push(format!("{name} (L{layer}) depends on UI crate {dn}"));
            }
            if let Some(ds) = dn.strip_prefix("soundcraft-") {
                if STANDALONE.contains(&short) && kind != "dev" {
                    problems.push(format!("{name} is standalone but depends on {dn}"));
                }
                if let Some(&(_, dl)) = LAYERS.iter().find(|(n, _)| *n == ds)
                    && dl > layer
                    && kind != "dev"
                {
                    problems.push(format!("{name} (L{layer}) depends upward on {dn} (L{dl})"));
                }
            }
        }
    }
    if problems.is_empty() {
        println!("layers: ok");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

const ASSET_EXT: &[&str] =
    &["png", "jpg", "jpeg", "svg", "ico", "icns", "ttf", "otf", "ttc", "woff", "woff2", "wav", "aif", "aiff", "flac", "mp3", "ogg", "gif", "webp"];

fn assets() -> Result<(), String> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    let files = String::from_utf8_lossy(&out.stdout).to_string();
    let attribution = std::fs::read_to_string(root().join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut missing = Vec::new();
    for f in files.lines() {
        let ext = Path::new(f).extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase();
        if !ASSET_EXT.contains(&ext.as_str()) {
            continue;
        }
        // Generated icon trees are covered by their directory row.
        let covered = attribution.contains(&format!("`{f}`"))
            || Path::new(f).ancestors().skip(1).any(|a| !a.as_os_str().is_empty() && attribution.contains(&format!("`{}/`", a.display())));
        if !covered {
            missing.push(f.to_string());
        }
    }
    if missing.is_empty() {
        println!("assets: ok");
        Ok(())
    } else {
        Err(format!("assets without an ATTRIBUTION.md row:\n  {}", missing.join("\n  ")))
    }
}

fn parity() -> Result<(), String> {
    cargo(&["run", "-q", "-p", "soundcraft-cli", "--", "parity", "--write", "docs/parity-checklist.md"])
}

/// Crates that must compile for the web.
const WASM: &[&str] = &[
    "soundcraft-time",
    "soundcraft-audio-io",
    "soundcraft-midi",
    "soundcraft-video",
    "soundcraft-dsp",
    "soundcraft-model",
    "soundcraft-mix",
    "soundcraft-engine",
    "soundcraft-playback",
    "soundcraft-automation",
    "soundcraft-ui-egui",
];

fn wasm() -> Result<(), String> {
    let mut args = vec!["check", "--target", "wasm32-unknown-unknown", "--keep-going"];
    for c in WASM {
        args.push("-p");
        args.push(c);
    }
    if root().join("apps/soundcraft-web/Cargo.toml").exists() {
        args.push("-p");
        args.push("soundcraft-web");
    }
    cargo(&args)
}

/// Test targets for the test build. Examples are left out so they aren't linked; clippy
/// `--all-targets` still compiles them.
const TEST_TARGETS: [&str; 4] = ["--workspace", "--lib", "--bins", "--tests"];

fn nextest_available() -> bool {
    Command::new(cargo_bin()).args(["nextest", "--version"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// `[[test]]` targets with `harness = false`, as (package, target). Their own `main` cannot answer
/// nextest's `--list`, so they run under `cargo test`.
fn own_main_tests() -> Result<Vec<(String, String)>, String> {
    let out =
        Command::new(cargo_bin()).args(["metadata", "--format-version", "1", "--no-deps"]).current_dir(root()).output().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for p in v["packages"].as_array().cloned().unwrap_or_default() {
        let (Some(name), Some(manifest)) = (p["name"].as_str(), p["manifest_path"].as_str()) else { continue };
        let text = std::fs::read_to_string(manifest).map_err(|e| format!("{manifest}: {e}"))?;
        found.extend(harness_false_tests(&text).into_iter().map(|t| (name.to_string(), t)));
    }
    Ok(found)
}

/// Names of the `[[test]]` tables with `harness = false` in a Cargo.toml.
fn harness_false_tests(manifest: &str) -> Vec<String> {
    // Each `[[test]]` table runs up to the next table header.
    let tables = manifest.split("\n[").filter(|t| t.starts_with("[test]]"));
    tables
        .filter_map(|table| {
            let value =
                |key: &str| table.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().trim_matches('"')));
            (value("harness") == Some("false")).then(|| value("name").map(str::to_string)).flatten()
        })
        .collect()
}

/// Workspace tests: cargo nextest when installed, else cargo test. Doctests run apart.
fn test() -> Result<(), String> {
    if !nextest_available() {
        eprintln!("note: cargo-nextest is not installed, running cargo test (cargo install cargo-nextest --locked)");
        return cargo(&[&["test"][..], &TEST_TARGETS[..]].concat());
    }
    let own = own_main_tests()?;
    let mut args = vec!["nextest", "run"];
    args.extend(TEST_TARGETS);
    let skip = format!("not ({})", own.iter().map(|(p, t)| format!("binary_id(={p}::{t})")).collect::<Vec<_>>().join(" | "));
    if !own.is_empty() {
        // A binary-level filter keeps nextest from running these binaries to list their tests.
        args.extend(["-E", &skip]);
    }
    cargo(&args)?;
    if own.is_empty() {
        return Ok(());
    }
    let packages: BTreeSet<&str> = own.iter().map(|(p, _)| p.as_str()).collect();
    let targets: BTreeSet<&str> = own.iter().map(|(_, t)| t.as_str()).collect();
    let mut args = vec!["test"];
    for p in packages {
        args.extend(["-p", p]);
    }
    for t in targets {
        args.extend(["--test", t]);
    }
    cargo(&args)
}

/// nextest does not run doctests.
fn doctest() -> Result<(), String> {
    cargo(&["test", "--workspace", "--doc"])
}

fn ci() -> Result<(), String> {
    let steps: Vec<(&str, Box<dyn Fn() -> Result<(), String>>)> = vec![
        ("fmt", Box::new(|| cargo(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(|| cargo(&["clippy", "--workspace", "--all-targets", "--keep-going", "--", "-D", "warnings"]))),
        ("test", Box::new(test)),
        ("doctest", Box::new(doctest)),
        ("assets", Box::new(assets)),
        ("layers", Box::new(layers)),
        ("wasm", Box::new(wasm)),
    ];
    let mut summary = Vec::new();
    for (name, f) in &steps {
        match f() {
            Ok(()) => summary.push(format!("ok    {name}")),
            Err(e) => {
                summary.push(format!("FAIL  {name}"));
                println!("{}", summary.join("\n"));
                return Err(e);
            }
        }
    }
    println!("{}", summary.join("\n"));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("test") => test(),
        Some("layers") => layers(),
        Some("assets") => assets(),
        Some("parity") => parity(),
        Some("wasm") => wasm(),
        Some("version") => version::run(args.get(1..).unwrap_or(&[])),
        Some("ico") => ico::run(args.get(1..).unwrap_or(&[])),
        _ => Err("usage: cargo xtask <ci|test|layers|assets|parity|wasm|version>".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn harness_false_tests_are_found_by_table() {
        let manifest = "[package]\nname = \"x\"\n\n[[test]]\nname = \"own_main\"\nharness = false\n\n[[test]]\nname = \"plain\"\n\n[[test]]\nharness = false\nname = \"second\"\n\n[dependencies]\nharness = false\n";
        assert_eq!(super::harness_false_tests(manifest), ["own_main", "second"]);
    }
}
