//! `Headless::with_automation_roots` through the real MCP protocol: every file a command reads
//! stays inside the read root, every file it writes inside the write root.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).

use serde_json::{Value, json};
use soundcraft_automation::{Headless, mcp::Server};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::AutomationRoots;
use std::path::{Path, PathBuf};

/// A marker that must never come back from a file outside the read root.
const SECRET: &str = "TOP-SECRET-MARKER-5f3a";

struct Fixture {
    base: PathBuf,
    read: PathBuf,
    write: PathBuf,
    outside: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn s(p: impl AsRef<Path>) -> String {
    p.as_ref().to_string_lossy().into_owned()
}

/// `in/` (read root), `out/` (write root) and `outside/`, each holding the same real files:
/// a session with audio, a WAV, a MIDI file, a track preset and a clip group file. `outside/`
/// also holds text files holding [`SECRET`] under the names a command might open.
fn fixture(name: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!("sc-roots-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (read, write, outside) = (base.join("in"), base.join("out"), base.join("outside"));
    for d in [&read, &write, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    // Real files, made by an unconfined engine.
    let mut e = soundcraft_engine::demo::demo_engine();
    for dir in [&read, &outside] {
        let run = |e: &mut Engine, id: &str, p: Value| e.execute(id, &p).unwrap_or_else(|err| panic!("{id}: {err}"));
        run(&mut e, "file.bounce_mix", json!({"path": s(dir.join("tone.wav")), "start": 0, "end": 4800}));
        run(&mut e, "file.export_midi", json!({"path": s(dir.join("keys.mid")), "tracks": ["Keys"]}));
        run(&mut e, "edit.select", json!({"tracks": ["Bass"]}));
        run(&mut e, "track.save_preset", json!({"path": s(dir.join("bass.json"))}));
        run(&mut e, "session.save_as", json!({"path": s(dir.join("song.scraft")), "replace": true}));
        let clips = e.session().track_by_name("Kick").unwrap().clips().iter().map(|c| c.id.0).collect::<Vec<_>>();
        run(&mut e, "file.export_clip_groups", json!({"path": s(dir.join("group.scgrp")), "clips": clips}));
    }
    for f in ["secret.wav", "secret.mid", "secret.scraft", "secret.json", "secret.scgrp", "secret.mp4"] {
        std::fs::write(outside.join(f), SECRET).unwrap();
    }
    Fixture { base, read, write, outside }
}

fn server(read: Option<&Path>, write: Option<&Path>) -> Server<Headless> {
    let roots = AutomationRoots::new(read.as_slice(), write.as_slice()).unwrap();
    Server::new(Headless::new(Engine::default()).with_automation_roots(roots))
}

/// One `execute` call: whether it failed and the text of the result.
fn run(s: &mut Server<Headless>, command: &str, params: Value) -> (bool, String) {
    let r = s
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "execute", "arguments": {"command": command, "params": params}}}))
        .unwrap();
    let res = &r["result"];
    (res["isError"].as_bool().unwrap(), res["content"][0]["text"].as_str().unwrap().to_string())
}

fn ok(s: &mut Server<Headless>, command: &str, params: Value) -> String {
    let (failed, text) = run(s, command, params);
    assert!(!failed, "{command} failed: {text}");
    text
}

fn refused(s: &mut Server<Headless>, command: &str, params: Value, needle: &str) {
    let (failed, text) = run(s, command, params.clone());
    assert!(failed, "{command} {params} must be refused, got {text}");
    assert!(text.contains(needle), "{command}: {text}");
    assert!(!text.contains(SECRET), "{command} leaked file contents: {text}");
}

/// Every file under `dir`, to show nothing was written.
fn listing(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(listing(&p));
        }
        out.push(s(&p));
    }
    out.sort();
    out
}

fn inspect(s: &mut Server<Headless>) -> Value {
    let r = s
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "inspect_session", "arguments": {"detail": "full"}}}))
        .unwrap();
    serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// Every command that reads a user-named file, with params for a file in `dir`.
fn reads(dir: &Path, name: &dyn Fn(&str) -> String) -> Vec<(&'static str, Value)> {
    let p = |f: &str| s(dir.join(name(f)));
    vec![
        ("session.open", json!({"path": p("song.scraft")})),
        ("file.import_audio", json!({"path": p("tone.wav")})),
        ("file.import_audio", json!({"paths": [p("tone.wav")]})),
        ("file.import_midi", json!({"path": p("keys.mid")})),
        ("file.import_video", json!({"path": p("movie.mp4")})),
        ("file.import_clip_groups", json!({"path": p("group.scgrp")})),
        ("file.import_session_data", json!({"path": p("song.scraft")})),
        ("track.load_preset", json!({"path": p("bass.json"), "tracks": ["Bass"]})),
    ]
}

#[test]
fn reads_outside_the_read_root_are_refused_and_nothing_reaches_the_result() {
    let f = fixture("reads");
    let mut srv = server(Some(&f.read), Some(&f.write));
    ok(&mut srv, "session.new", json!({"name": "Work"}));
    ok(&mut srv, "track.new", json!({"name": "Bass", "count": 1}));
    ok(&mut srv, "track.new", json!({"name": "Pads", "count": 1, "kind": "instrument"}));
    let before = inspect(&mut srv);
    // The secret files hold the marker under the names the commands read, so a read would show.
    let secret_name = |f: &str| format!("secret.{}", f.rsplit('.').next().unwrap());
    for (command, params) in reads(&f.outside, &secret_name) {
        refused(&mut srv, command, params, "outside the read root");
    }
    refused(&mut srv, "mix.sampler_load", json!({"track": "Pads", "path": s(f.outside.join("secret.wav"))}), "outside the read root");
    // The open session is exactly as it was: nothing was imported or opened.
    assert_eq!(inspect(&mut srv), before);
    // The same commands work on files inside the root.
    ok(&mut srv, "file.import_audio", json!({"path": s(f.read.join("tone.wav"))}));
    ok(&mut srv, "file.import_midi", json!({"path": s(f.read.join("keys.mid"))}));
    ok(&mut srv, "file.import_clip_groups", json!({"path": s(f.read.join("group.scgrp"))}));
    ok(&mut srv, "file.import_session_data", json!({"path": s(f.read.join("song.scraft")), "tracks": ["Kick"]}));
    ok(&mut srv, "track.load_preset", json!({"path": s(f.read.join("bass.json")), "tracks": ["Bass"]}));
    ok(&mut srv, "mix.sampler_load", json!({"track": "Pads", "path": s(f.read.join("tone.wav"))}));
    // A missing movie inside the root is an ordinary error, not a refusal.
    let (failed, text) = run(&mut srv, "file.import_video", json!({"path": s(f.read.join("movie.mp4"))}));
    assert!(failed && !text.contains("read root"), "{text}");
    ok(&mut srv, "session.open", json!({"path": s(f.read.join("song.scraft"))}));
    // The typed tools take the same path through the same checks.
    let r = srv
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "open_session", "arguments": {"path": s(f.outside.join("secret.scraft"))}}}))
        .unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("outside the read root"));
}

/// A clip of the demo session.
fn kick_clip() -> u64 {
    soundcraft_engine::demo::demo_engine().session().track_by_name("Kick").unwrap().clips()[0].id.0
}

/// Every command that writes a user-named file or folder, with params for `dir`.
fn writes(dir: &Path) -> Vec<(&'static str, Value)> {
    let p = |f: &str| s(dir.join(f));
    let clip = kick_clip();
    vec![
        ("session.save", json!({"path": p("a.scraft")})),
        ("session.save_as", json!({"path": p("b.scraft")})),
        ("session.save_copy", json!({"path": p("c.scraft")})),
        ("session.save_template", json!({"path": p("d.scraft")})),
        ("file.bounce_mix", json!({"path": p("e.wav"), "start": 0, "end": 4800})),
        ("file.bounce_stems", json!({"dir": p("stems"), "start": 0, "end": 4800})),
        ("file.export_midi", json!({"path": p("f.mid")})),
        ("file.export_session_text", json!({"path": p("g.txt")})),
        ("file.export_clips", json!({"dir": p("clips"), "clips": [clip]})),
        ("file.export_selected_tracks_as_session", json!({"path": p("h.scraft"), "tracks": ["Kick"]})),
        ("file.export_sibelius", json!({"path": p("i.musicxml")})),
        ("file.send_to_sibelius", json!({"path": p("j.musicxml")})),
        ("file.print_score", json!({"path": p("k.svg")})),
        ("file.export_clip_groups", json!({"path": p("l.scgrp"), "clips": [clip]})),
        ("track.save_preset", json!({"path": p("m.json"), "tracks": ["Kick"]})),
    ]
}

fn demo_server(read: Option<&Path>, write: Option<&Path>) -> Server<Headless> {
    let roots = AutomationRoots::new(read.as_slice(), write.as_slice()).unwrap();
    Server::new(Headless::new(soundcraft_engine::demo::demo_engine()).with_automation_roots(roots))
}

#[test]
fn every_writing_command_is_refused_outside_the_write_root_and_writes_nothing() {
    let f = fixture("writes");
    let mut srv = demo_server(Some(&f.read), Some(&f.write));
    let before = listing(&f.base);
    let outside_before = listing(&f.outside);
    for (command, params) in writes(&f.outside) {
        refused(&mut srv, command, params, "outside the write root");
    }
    // Not even the temporary file a save goes through, nor a folder.
    assert_eq!(listing(&f.base), before, "nothing may appear anywhere");
    // With no path, the score goes to the temp folder: outside the write root, so refused too.
    refused(&mut srv, "file.send_to_sibelius", json!({}), "outside the write root");
    refused(&mut srv, "file.print_score", json!({}), "outside the write root");
    assert_eq!(listing(&f.base), before);
    // Inside the write root every one of them works.
    for (command, params) in writes(&f.write) {
        let text = ok(&mut srv, command, params);
        assert!(!text.is_empty(), "{command}");
    }
    assert!(f.write.join("e.wav").is_file() && f.write.join("stems").is_dir() && f.write.join("l.scgrp").is_file());
    // A save puts the audio in `Audio Files/` beside it, inside the root as well.
    assert!(f.write.join("Audio Files").is_dir());
    assert_eq!(listing(&f.outside), outside_before, "outside is as it was");
}

#[test]
fn saving_over_an_existing_file_outside_does_not_report_that_it_exists() {
    let f = fixture("exists");
    let mut srv = demo_server(Some(&f.read), Some(&f.write));
    // Without `replace`, an existing target is reported as existing: not for a path outside the root.
    refused(&mut srv, "session.save_as", json!({"path": s(f.outside.join("song.scraft"))}), "outside the write root");
}

#[test]
fn the_typed_bounce_and_save_tools_are_confined() {
    let f = fixture("typed");
    let mut srv = demo_server(Some(&f.read), Some(&f.write));
    for (tool, args) in [
        ("bounce_mix", json!({"path": s(f.outside.join("x.wav")), "start": 0, "end": 4800})),
        ("save_session", json!({"path": s(f.outside.join("x.scraft"))})),
    ] {
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}})).unwrap();
        assert_eq!(r["result"]["isError"], true, "{tool}");
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("outside the write root"), "{tool}");
    }
    assert!(!f.outside.join("x.wav").exists() && !f.outside.join("x.scraft").exists());
    let r = srv
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "bounce_mix", "arguments": {"path": s(f.write.join("x.wav")), "start": 0, "end": 4800}}}))
        .unwrap();
    assert_eq!(r["result"]["isError"], false);
}

#[test]
fn one_root_grants_only_its_own_access() {
    let f = fixture("one-root");
    // Read root only: reads work, nothing can be written anywhere.
    let mut srv = demo_server(Some(&f.read), None);
    ok(&mut srv, "file.import_audio", json!({"path": s(f.read.join("tone.wav"))}));
    let before = listing(&f.base);
    for (command, params) in writes(&f.write) {
        refused(&mut srv, command, params, "write authority is absent");
    }
    for (command, params) in writes(&f.read) {
        refused(&mut srv, command, params, "write authority is absent");
    }
    assert_eq!(listing(&f.base), before);
    // Write root only: writes work, nothing can be read anywhere.
    let mut srv = demo_server(None, Some(&f.write));
    for (command, params) in writes(&f.write) {
        ok(&mut srv, command, params);
    }
    let name = |f: &str| f.to_string();
    for (command, params) in reads(&f.read, &name) {
        refused(&mut srv, command, params, "read authority is absent");
    }
    refused(&mut srv, "file.import_audio", json!({"path": s(f.write.join("e.wav"))}), "read authority is absent");
}

#[test]
fn the_roots_may_be_the_same_folder_and_are_compared_by_whole_names() {
    let f = fixture("same");
    let mut srv = demo_server(Some(&f.read), Some(&f.read));
    ok(&mut srv, "file.import_audio", json!({"path": s(f.read.join("tone.wav"))}));
    ok(&mut srv, "file.bounce_mix", json!({"path": s(f.read.join("again.wav")), "start": 0, "end": 4800}));
    // `in2` shares the prefix of `in` and is a different folder.
    let sibling = f.base.join("in2");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::copy(f.read.join("tone.wav"), sibling.join("tone.wav")).unwrap();
    refused(&mut srv, "file.import_audio", json!({"path": s(sibling.join("tone.wav"))}), "outside the read root");
    refused(&mut srv, "file.bounce_mix", json!({"path": s(sibling.join("x.wav"))}), "outside the write root");
    // `..` out of the root is outside it.
    let up = format!("{}/../outside/tone.wav", s(&f.read));
    refused(&mut srv, "file.import_audio", json!({"path": up}), "outside the read root");
}

#[cfg(unix)]
#[test]
fn links_out_of_the_roots_are_refused() {
    use std::os::unix::fs::symlink;
    let f = fixture("links");
    symlink(f.outside.join("tone.wav"), f.read.join("leak.wav")).unwrap();
    symlink(&f.outside, f.write.join("away")).unwrap();
    symlink(f.outside.join("not-yet.wav"), f.write.join("dangling.wav")).unwrap();
    let mut srv = demo_server(Some(&f.read), Some(&f.write));
    refused(&mut srv, "file.import_audio", json!({"path": s(f.read.join("leak.wav"))}), "outside the read root");
    let before = listing(&f.outside);
    refused(&mut srv, "file.bounce_mix", json!({"path": s(f.write.join("away/x.wav"))}), "outside the write root");
    refused(&mut srv, "file.bounce_stems", json!({"dir": s(f.write.join("away/stems"))}), "outside the write root");
    refused(&mut srv, "session.save_as", json!({"path": s(f.write.join("away/x.scraft"))}), "outside the write root");
    refused(&mut srv, "file.bounce_mix", json!({"path": s(f.write.join("dangling.wav"))}), "a link that leads nowhere");
    assert_eq!(listing(&f.outside), before, "nothing was written through a link");
    assert!(!f.outside.join("not-yet.wav").exists());
    // A link to a folder inside the root stays inside it.
    symlink(f.read.join("."), f.base.join("in-alias")).unwrap();
    let mut via_alias = demo_server(Some(&f.base.join("in-alias")), None);
    ok(&mut via_alias, "file.import_audio", json!({"path": s(f.read.join("tone.wav"))}));
}

/// A session file is hostile input too: media it names outside the read root is not loaded,
/// and the refusal is listed with the other missing media.
#[test]
fn media_a_session_names_outside_the_read_root_is_not_loaded() {
    let f = fixture("media");
    let session = f.read.join("song.scraft");
    let text = std::fs::read_to_string(&session).unwrap();
    let outside_wav = s(f.outside.join("tone.wav"));
    let swapped = {
        let v: Value = serde_json::from_str(&text).unwrap();
        let mut rel = vec![];
        for src in v["sources"].as_array().unwrap() {
            rel.push(src["path"].as_str().unwrap().to_string());
        }
        assert!(!rel.is_empty());
        rel.iter().fold(text.clone(), |t, r| t.replace(&format!("\"{r}\""), &format!("{}", json!(outside_wav))))
    };
    std::fs::write(&session, swapped).unwrap();
    let mut srv = server(Some(&f.read), None);
    let opened = ok(&mut srv, "session.open", json!({"path": s(&session)}));
    let opened: Value = serde_json::from_str(&opened).unwrap();
    let missing = opened["missing"].as_array().unwrap();
    assert!(!missing.is_empty() && missing.iter().all(|m| m.as_str().unwrap().contains("outside the read root")), "{opened}");
    let sources = inspect(&mut srv)["sources"].clone();
    assert!(sources.as_array().unwrap().iter().all(|x| x["loaded"] == false), "{sources}");
    // The same session opened without roots does load the file it names.
    let mut free = server(None, None);
    ok(&mut free, "session.open", json!({"path": s(&session)}));
    assert!(inspect(&mut free)["sources"].as_array().unwrap().iter().all(|x| x["loaded"] == true));
}

#[test]
fn without_roots_nothing_is_confined() {
    let f = fixture("free");
    let mut srv = server(None, None);
    ok(&mut srv, "file.import_audio", json!({"path": s(f.outside.join("tone.wav"))}));
    ok(&mut srv, "file.bounce_mix", json!({"path": s(f.outside.join("free.wav")), "start": 0, "end": 4800}));
    assert!(f.outside.join("free.wav").is_file());
}
