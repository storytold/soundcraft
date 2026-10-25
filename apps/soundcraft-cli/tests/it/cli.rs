//! `soundcraft-cli mcp` with the automation root flags, run as the real binary.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_soundcraft-cli"));
    cmd.env("SOUNDCRAFT_NO_PREFS", "1");
    cmd
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn tmpdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sc-cli-roots-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `path` made absolute with links followed (`/var` becomes `/private/var` on macOS), as the server
/// prints a root: without the `\\?\` prefix that `canonicalize` adds on Windows.
fn resolved(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    if !cfg!(windows) {
        return path;
    }
    let text = path.to_str().unwrap();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    text.strip_prefix(r"\\?\").map_or(path.clone(), PathBuf::from)
}

/// Send `calls` (tool name, arguments) to `soundcraft-cli mcp <args>`; returns whether it exited
/// cleanly, its stderr, and the stdout lines after `initialize`, one per call.
fn mcp_session(args: &[&str], calls: &[(&str, Value)]) -> (bool, String, Vec<Value>) {
    mcp_session_in(None, args, calls)
}

/// [`mcp_session`] with the server started in the working directory `cwd`.
fn mcp_session_in(cwd: Option<&Path>, args: &[&str], calls: &[(&str, Value)]) -> (bool, String, Vec<Value>) {
    let mut cmd = bin();
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.arg("mcp").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#).unwrap();
        for (i, (name, arguments)) in calls.iter().enumerate() {
            let call = serde_json::json!({"jsonrpc": "2.0", "id": i + 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
            writeln!(stdin, "{call}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let replies = stdout.lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned(), replies)
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap()
}

/// A short WAV made by the binary itself.
fn make_wav(path: &std::path::Path) {
    let o = run(&["run", "--demo", "--bounce", path.to_str().unwrap(), "--start", "0", "--end", "0.1"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn mcp_automation_roots_confine_the_server() {
    let base = tmpdir("roots");
    let (work, elsewhere) = (base.join("work"), base.join("elsewhere"));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    make_wav(&work.join("in.wav"));
    make_wav(&elsewhere.join("secret.wav"));
    let work_s = work.to_str().unwrap();
    let secret = elsewhere.join("secret.wav").to_str().unwrap().to_string();

    // Both spellings of the flags; inside works, outside is refused with a readable message.
    let read_flag = format!("--automation-read-root={work_s}");
    let calls = [
        ("import_audio", serde_json::json!({"path": work.join("in.wav").to_str().unwrap()})),
        ("import_audio", serde_json::json!({"path": secret})),
        ("save_session", serde_json::json!({"path": work.join("saved.scraft").to_str().unwrap()})),
        ("save_session", serde_json::json!({"path": elsewhere.join("saved.scraft").to_str().unwrap()})),
        ("bounce_mix", serde_json::json!({"path": elsewhere.join("mix.wav").to_str().unwrap()})),
    ];
    let (success, stderr, replies) = mcp_session(&[&read_flag, "--automation-write-root", work_s], &calls);
    assert!(success, "{stderr}");
    assert!(stderr.contains("files confined"), "{stderr}");
    assert_eq!(replies[0]["result"]["isError"], false, "{}", replies[0]);
    assert_eq!(replies[1]["result"]["isError"], true);
    assert!(text(&replies[1]).contains("outside the read root"), "{}", text(&replies[1]));
    assert_eq!(replies[2]["result"]["isError"], false, "{}", replies[2]);
    assert!(work.join("saved.scraft").exists());
    assert_eq!(replies[3]["result"]["isError"], true);
    assert!(text(&replies[3]).contains("outside the write root"), "{}", text(&replies[3]));
    assert_eq!(replies[4]["result"]["isError"], true);
    assert!(text(&replies[4]).contains("outside the write root"), "{}", text(&replies[4]));
    assert!(!elsewhere.join("saved.scraft").exists() && !elsewhere.join("mix.wav").exists());

    // A root left out grants none of its access.
    let (_, _, replies) = mcp_session(&[&read_flag], &calls[2..3]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(text(&replies[0]).contains("write authority is absent"), "{}", text(&replies[0]));
    let (_, _, replies) = mcp_session(&["--automation-write-root", work_s], &calls[0..1]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(text(&replies[0]).contains("read authority is absent"), "{}", text(&replies[0]));

    // No flags: nothing is confined.
    let (success, _, replies) = mcp_session(&[], &calls[1..2]);
    assert!(success);
    assert_eq!(replies[0]["result"]["isError"], false, "{}", replies[0]);
    let _ = std::fs::remove_dir_all(base);
}

/// #874: each root flag may be given more than once. A path inside any root of its kind passes;
/// the two roots' common parent, `..` out of a root and a link out of one stay refused.
#[test]
fn mcp_automation_roots_take_several_folders() {
    let base = resolved(&tmpdir("several"));
    for d in ["in", "in2", "out", "out2", "outside"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    for f in ["in/a.wav", "in2/b.wav", "outside/secret.wav", "parent.wav"] {
        make_wav(&base.join(f));
    }
    let p = |f: &str| base.join(f).to_str().unwrap().to_string();
    let sep = std::path::MAIN_SEPARATOR;
    let calls = [
        ("import_audio", serde_json::json!({"path": p("in/a.wav")})),
        ("import_audio", serde_json::json!({"path": p("in2/b.wav")})),
        // The roots' common parent is not a root.
        ("import_audio", serde_json::json!({"path": p("parent.wav")})),
        ("import_audio", serde_json::json!({"path": format!("{}{sep}..{sep}outside{sep}secret.wav", p("in2"))})),
        ("bounce_mix", serde_json::json!({"path": p("out/a.wav")})),
        ("bounce_mix", serde_json::json!({"path": p("out2/b.wav")})),
        ("bounce_mix", serde_json::json!({"path": p("x.wav")})),
        // Read and write stay separate: a read root is not a write root, nor the other way round.
        ("bounce_mix", serde_json::json!({"path": p("in2/c.wav")})),
        ("import_audio", serde_json::json!({"path": p("out2/b.wav")})),
    ];
    let read2 = format!("--automation-read-root={}", p("in2"));
    let args = ["--automation-read-root", &p("in"), &read2, "--automation-write-root", &p("out"), "--automation-write-root", &p("out2")];
    let (success, stderr, replies) = mcp_session(&args, &calls);
    assert!(success, "{stderr}");
    assert!(stderr.contains(&format!("read {}, {}", p("in"), p("in2"))), "{stderr}");
    let errors: Vec<bool> = replies.iter().map(|r| r["result"]["isError"] == true).collect();
    assert_eq!(errors, [false, false, true, true, false, false, true, true, true], "{replies:?}");
    for (i, kind) in [(2, "read"), (3, "read"), (6, "write"), (7, "write"), (8, "read")] {
        assert!(text(&replies[i]).contains(&format!("outside the {kind} roots ")), "{}", text(&replies[i]));
    }
    assert!(base.join("out/a.wav").is_file() && base.join("out2/b.wav").is_file());
    assert!(!base.join("x.wav").exists() && !base.join("in2/c.wav").exists());

    // A link in one root that leads out of all of them is outside.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(base.join("outside/secret.wav"), base.join("in2/leak.wav")).unwrap();
        let (_, _, replies) = mcp_session(&args, &[("import_audio", serde_json::json!({"path": p("in2/leak.wav")}))]);
        assert!(text(&replies[0]).contains("outside the read roots"), "{}", text(&replies[0]));
    }
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_bare_save_session_name_lands_in_the_working_directory_inside_the_roots() {
    let work = tmpdir("bare-name");
    let work_s = work.to_str().unwrap();
    let calls = [
        ("new_session", serde_json::json!({"name": "t", "sample_rate": 48000})),
        ("execute", serde_json::json!({"command": "track.new", "params": {"kind": "audio", "name": "Tone", "format": "mono"}})),
        ("save_session", serde_json::json!({"path": "t.scraft"})),
    ];
    let (success, stderr, replies) = mcp_session_in(Some(&work), &["--automation-read-root", work_s, "--automation-write-root", work_s], &calls);
    assert!(success, "{stderr}");
    for reply in &replies {
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    assert!(work.join("t.scraft").is_file());
    let _ = std::fs::remove_dir_all(work);
}

#[test]
fn mcp_automation_root_flags_are_checked_up_front() {
    let base = tmpdir("roots-args");
    let dir = base.to_str().unwrap();
    let missing = base.join("missing");
    let fails = |args: &[&str], needle: &str| {
        let mut full = vec!["mcp"];
        full.extend_from_slice(args);
        let o = run(&full);
        assert!(!o.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        assert!(err.contains(needle), "{args:?}: {err}");
    };
    // The flags confine the in-process server, so they cannot be combined with --connect.
    fails(&["--connect", "7979", "--automation-read-root", dir], "--connect");
    fails(&["--automation-write-root", dir, "--connect", "7979"], "--connect");
    // #874: a flag may be given again, but every folder it names must exist.
    fails(&["--automation-read-root", dir, "--automation-read-root", missing.to_str().unwrap()], "cannot open the automation read root");
    fails(&["--automation-write-root", dir, "--automation-write-root"], "needs a folder");
    fails(&["--automation-read-root"], "needs a folder");
    fails(&["--automation-write-root", "--demo"], "needs a folder");
    fails(&["--automation-read-root", missing.to_str().unwrap()], "cannot open the automation read root");
    fails(&["--automation-write-root="], "needs a non-empty folder");
    // A misspelled root flag must not start an unconfined server.
    fails(&["--automation-read-roots", dir], "unknown mcp option");
    // A file is not a folder.
    std::fs::write(base.join("f.txt"), "x").unwrap();
    fails(&["--automation-write-root", base.join("f.txt").to_str().unwrap()], "cannot open the automation write root");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn usage_lists_the_root_flags() {
    let o = run(&["--help"]);
    let help = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(help.contains("--automation-read-root DIR") && help.contains("--automation-write-root DIR"), "{help}");
}

/// Without `--start`/`--end`, `--bounce` covers the whole session, not a stray edit selection
/// left by the script (a bare `19.2` in command params is 19 samples).
#[test]
fn script_bounce_without_range_covers_the_session() {
    let base = tmpdir("bounce-range");
    let script = base.join("cmds.txt");
    std::fs::write(
        &script,
        concat!(
            "track.new {\"count\":1,\"kind\":\"instrument\",\"name\":\"Keys\"}\n",
            "midi.new_clip {\"track\":\"Keys\",\"start\":0,\"end\":{\"seconds\":19.2}}\n",
            "edit.select {\"start\":0,\"end\":19.2}\n",
        ),
    )
    .unwrap();
    let out = base.join("out.wav");
    let o = run(&["script", script.to_str().unwrap(), "--bounce", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let secs = last["seconds"].as_f64().unwrap();
    assert!((secs - 19.2).abs() < 0.01, "bounced {secs} s: {stdout}");
    let _ = std::fs::remove_dir_all(base);
}
