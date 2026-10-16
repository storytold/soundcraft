//! Edit › VO Cut Pipeline — run the Python VO cut script from inside SoundCraft.
//!
//! Offline / long-running: blocks the command caller (UI / CLI / control channel), never the
//! audio callback. Future customization: more params, progress dialog, in-process ASR, or a
//! native cut planner that no longer shells out to Python.

use super::*;
use crate::cmd;
use serde_json::json;
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::process::Command;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

const CMD: &str = "edit.vo_cut";
const SCRIPT_REL: &str = "scripts/soundcraft/vo_cut_pipeline.py";
/// Default wall-clock budget for Whisper + LLM + CLI apply (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
/// Cap so a hostile `timeout_secs` cannot schedule an unbounded wait loop.
const MAX_TIMEOUT_SECS: u64 = 4 * 60 * 60;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        noundo "edit.vo_cut",
        "VO Cut Pipeline...",
        ["Edit"],
        Some("Cmd+Alt+Shift+V"),
        "{dry_run?: false, llm?: ollama|auto|fallback|gigachat|openai|lmstudio|local (default ollama), model?: string, prefer_cached_asr?: false, session?: path, wav?: path, script?: path, python?: path, timeout_secs?: 1800, force_spawn?: bool (tests)} — shells out to scripts/soundcraft/vo_cut_pipeline.py; default applies cuts (re-open session in GUI). Menu uses fresh Whisper (not stale ASR cache).",
        always,
        run
    )]
}

fn run(e: &mut Engine, p: &Value) -> Result<Value> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (e, p);
        return Err(EngineError::Unsupported(
            "edit.vo_cut needs a desktop build (Python subprocess)".into(),
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        run_native(e, p)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_native(e: &mut Engine, p: &Value) -> Result<Value> {
    // Menu clicks pass empty params: apply cuts and re-transcribe (stale ASR cache is useless
    // when the user dropped new audio into the session).
    let dry_run = bool_or(p, "dry_run", false);
    let prefer_cached = bool_or(p, "prefer_cached_asr", false);
    let llm = str_param(p, "llm").unwrap_or("ollama");
    if !matches!(
        llm,
        "auto" | "fallback" | "gigachat" | "openai" | "ollama" | "lmstudio" | "local"
    ) {
        return Err(bad(
            CMD,
            format!("unknown llm `{llm}` (use ollama|auto|fallback|gigachat|openai|lmstudio|local)"),
        ));
    }
    let model = str_param(p, "model").map(str::to_string);
    let timeout_secs = i64_or(p, "timeout_secs", DEFAULT_TIMEOUT_SECS as i64)
        .clamp(1, MAX_TIMEOUT_SECS as i64) as u64;

    let script = resolve_script(p)?;
    let python = resolve_python(p)?;
    let workspace = workspace_for(&script);

    let session = str_param(p, "session")
        .map(PathBuf::from)
        .or_else(|| e.path.as_ref().map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty());
    let wav = str_param(p, "wav").map(PathBuf::from);

    let mut args: Vec<String> = Vec::new();
    if let Some(w) = &wav {
        args.push(w.to_string_lossy().into_owned());
    }
    if prefer_cached {
        args.push("--prefer-cached-asr".into());
    }
    args.push("--llm".into());
    args.push(llm.to_string());
    if let Some(m) = &model {
        args.push("--llm-model".into());
        args.push(m.clone());
    }
    if let Some(s) = &session {
        args.push("--session".into());
        args.push(s.to_string_lossy().into_owned());
    }
    if dry_run {
        args.push("--dry-run".into());
    } else {
        args.push("--apply".into());
    }

    let summary = json!({
        "command": CMD,
        "dry_run": dry_run,
        "llm": llm,
        "model": model,
        "prefer_cached_asr": prefer_cached,
        "script": script,
        "python": python,
        "workspace": workspace,
        "session": session,
        "wav": wav,
        "timeout_secs": timeout_secs,
        "args": args,
    });

    // Survival / hostile tests must not launch Whisper+LLM. Real runs use a non-test build, or
    // pass force_spawn=true from a focused test with a stub script.
    if cfg!(test) && !bool_or(p, "force_spawn", false) {
        return Ok(json!({
            "ok": true,
            "skipped_spawn": true,
            "message": "test build: resolution only (pass force_spawn=true to spawn)",
            "plan": summary,
        }));
    }

    let output = spawn_python(&python, &script, &args, &workspace, Duration::from_secs(timeout_secs))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let code = output.status.code();
    let ok = output.status.success();
    if !ok {
        let detail = if stderr.is_empty() { stdout.clone() } else { stderr.clone() };
        let code_s = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        return Err(EngineError::Io(format!(
            "{CMD}: python exited {code_s}: {detail}"
        )));
    }
    let message = if dry_run {
        "VO cut: plan written (dry_run) → work/vo_cut_plan.json"
    } else {
        "VO cut: cuts written to session file — File › Open the same session again to reload"
    };
    // Prefer the Python summary line (keeps/drops) when present.
    let detail = stdout
        .lines()
        .rev()
        .find(|l| l.contains("Plan ->") || l.contains("Done.") || l.starts_with("WAV:"))
        .map(|l| l.trim().to_string());
    let message = match detail {
        Some(d) if !dry_run => format!("{message}. {d}"),
        Some(d) => d,
        None => message.into(),
    };
    e.messages.push(message.clone());
    Ok(json!({
        "ok": true,
        "skipped_spawn": false,
        "exit_code": code,
        "stdout_tail": tail(&stdout, 4000),
        "stderr_tail": tail(&stderr, 2000),
        "message": message,
        "plan": summary,
    }))
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len().saturating_sub(max);
    // Avoid splitting a UTF-8 codepoint.
    let start = s[start..].char_indices().next().map_or(start, |(i, _)| start + i);
    format!("…{}", &s[start..])
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_python(
    python: &Path,
    script: &Path,
    args: &[String],
    workspace: &Path,
    timeout: Duration,
) -> Result<std::process::Output> {
    use std::io::Read;
    use std::sync::mpsc;

    let mut cmd = Command::new(python);
    cmd.arg(script).args(args).current_dir(workspace);
    // Hide an extra console window on Windows when launched from the GUI.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| {
            EngineError::Io(format!(
                "{CMD}: failed to spawn `{}` with {}: {err}",
                python.display(),
                script.display()
            ))
        })?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::Io(format!("{CMD}: stdout pipe missing")))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| EngineError::Io(format!("{CMD}: stderr pipe missing")))?;
    // Drain pipes on side threads so a chatty child cannot fill the OS buffer and deadlock.
    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Io(format!(
                    "{CMD}: timed out after {}s (Whisper/LLM/apply still running? raise timeout_secs)",
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => return Err(EngineError::Io(format!("{CMD}: try_wait failed: {err}"))),
        }
    };
    let stdout = rx_out.recv().unwrap_or_default();
    let stderr = rx_err.recv().unwrap_or_default();
    Ok(std::process::Output { status, stdout, stderr })
}

fn resolve_script(p: &Value) -> Result<PathBuf> {
    if let Some(s) = str_param(p, "script") {
        let path = PathBuf::from(s);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(EngineError::NotFound(format!("{CMD}: script not found: {}", path.display())))
        };
    }
    if let Ok(s) = std::env::var("SOUNDCRAFT_VO_CUT_SCRIPT") {
        let path = PathBuf::from(s);
        if path.is_file() {
            return Ok(path);
        }
        return Err(EngineError::NotFound(format!(
            "{CMD}: SOUNDCRAFT_VO_CUT_SCRIPT not a file: {}",
            path.display()
        )));
    }
    for root in candidate_roots() {
        let cand = root.join(SCRIPT_REL);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    Err(EngineError::NotFound(format!(
        "{CMD}: `{SCRIPT_REL}` not found near the executable/cwd; set SOUNDCRAFT_VO_CUT_SCRIPT or pass script="
    )))
}

fn resolve_python(p: &Value) -> Result<PathBuf> {
    if let Some(s) = str_param(p, "python") {
        let path = PathBuf::from(s);
        return if looks_runnable(&path) {
            Ok(path)
        } else {
            Err(EngineError::NotFound(format!("{CMD}: python not found: {}", path.display())))
        };
    }
    for key in ["SOUNDCRAFT_PYTHON", "ZVUKACH_PYTHON", "PYTHON"] {
        if let Ok(s) = std::env::var(key) {
            let path = PathBuf::from(s);
            if looks_runnable(&path) {
                return Ok(path);
            }
        }
    }
    for name in ["python", "python3", "py"] {
        if let Some(found) = which(name) {
            return Ok(found);
        }
    }
    Err(EngineError::NotFound(format!(
        "{CMD}: no Python interpreter (install Python 3, or set SOUNDCRAFT_PYTHON / ZVUKACH_PYTHON)"
    )))
}

fn looks_runnable(path: &Path) -> bool {
    path.is_file() || which(&path.to_string_lossy()).is_some()
}

/// Best-effort PATH lookup without pulling in an extra crate.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "bat", "cmd"] {
                let with = dir.join(format!("{name}.{ext}"));
                if with.is_file() {
                    return Some(with);
                }
            }
        }
    }
    None
}

/// Prefer `<root>/scripts/soundcraft/vo_cut_pipeline.py` → `<root>`; else walk up for that layout.
fn workspace_for(script: &Path) -> PathBuf {
    if script.ends_with(SCRIPT_REL) {
        // scripts/soundcraft/file → two parents up is the workspace root.
        if let Some(root) = script.parent().and_then(Path::parent).and_then(Path::parent) {
            return root.to_path_buf();
        }
    }
    for a in ancestors(script) {
        if a.join(SCRIPT_REL).is_file() {
            return a;
        }
        if a.join("scripts/soundcraft").is_dir() && (a.join("work").is_dir() || a.join("input").is_dir() || a.join("sessions").is_dir())
        {
            return a;
        }
    }
    script.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn candidate_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };
    if let Ok(exe) = std::env::current_exe() {
        for a in ancestors(&exe) {
            push(a);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for a in ancestors(&cwd) {
            push(a);
        }
    }
    // crates/engine → repo root when developing in-tree.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for a in ancestors(&manifest) {
        push(a);
    }
    out
}

fn ancestors(start: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut cur = Some(start);
    // Cap depth so a weird path cannot allocate forever.
    for _ in 0..24 {
        let Some(p) = cur else { break };
        v.push(p.to_path_buf());
        cur = p.parent();
    }
    v
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registered_and_enabled() {
        let spec = find_command(CMD).expect("edit.vo_cut registered");
        assert_eq!(spec.label, "VO Cut Pipeline...");
        assert_eq!(spec.menu, &["Edit"]);
        assert_eq!(spec.shortcut, Some("Cmd+Alt+Shift+V"));
        let e = Engine::default();
        assert!((spec.enabled)(&e).is_ok());
    }

    #[test]
    fn empty_params_resolve_without_spawn() {
        let mut e = Engine::default();
        let r = e.execute(CMD, &json!({})).unwrap();
        assert_eq!(r["skipped_spawn"], true);
        assert_eq!(r["plan"]["dry_run"], false);
        assert_eq!(r["plan"]["prefer_cached_asr"], false);
        assert_eq!(r["plan"]["llm"], "ollama");
        let script = r["plan"]["script"].as_str().unwrap();
        assert!(script.replace('\\', "/").ends_with(SCRIPT_REL), "{script}");
    }

    #[test]
    fn rejects_unknown_llm() {
        let mut e = Engine::default();
        let err = e.execute(CMD, &json!({"llm": "nope"})).unwrap_err();
        assert!(matches!(err, EngineError::BadParams(..)), "{err}");
    }

    #[test]
    fn missing_script_param_errors_clearly() {
        let mut e = Engine::default();
        let err = e
            .execute(CMD, &json!({"script": "C:/definitely/missing/vo_cut_pipeline.py"}))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotFound(_)), "{err}");
    }

    #[test]
    fn force_spawn_runs_stub_script() {
        if resolve_python(&json!({})).is_err() {
            return; // machine without Python: resolution tests above still cover the command
        }
        let dir = std::env::temp_dir().join(format!("sc-vo-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts/soundcraft")).unwrap();
        let script = dir.join("scripts/soundcraft/vo_cut_pipeline.py");
        // Tiny stub: exit 0 and print argv so we never need Whisper.
        std::fs::write(
            &script,
            "import sys\nprint('vo_cut_stub', ' '.join(sys.argv[1:]))\n",
        )
        .unwrap();
        let mut e = Engine::default();
        let r = e
            .execute(
                CMD,
                &json!({
                    "script": script.to_string_lossy(),
                    "force_spawn": true,
                    "dry_run": true,
                    "prefer_cached_asr": false,
                    "timeout_secs": 30,
                }),
            )
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["skipped_spawn"], false);
        let out = r["stdout_tail"].as_str().unwrap_or("");
        assert!(out.contains("vo_cut_stub"), "{out}");
        assert!(out.contains("--dry-run"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_follows_scripts_layout() {
        let script = PathBuf::from(r"C:\proj\scripts\soundcraft\vo_cut_pipeline.py");
        assert_eq!(workspace_for(&script), PathBuf::from(r"C:\proj"));
    }
}
