//! Command plug-in runs against a real engine, with modules assembled from WebAssembly text
//! (RFC-0046).

use serde_json::{Value, json};

use super::craft_plugin::bundle::Bundle;
use crate::Engine;

/// A module that calls `calls` (command, params JSON) in order. A refused or failed call fails the
/// run when `strict`; otherwise its message becomes the output and the run goes on. `tail` is WAT
/// run after the calls (a trap, an endless loop), before returning 0.
fn module(calls: &[(&str, &str)], strict: bool, tail: &str) -> Vec<u8> {
    let mut data = String::new();
    let mut body = String::new();
    let mut at = 0usize;
    for (command, params) in calls {
        let (c, p) = (at, at + command.len());
        at = p + params.len();
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        data.push_str(&format!(r#"(data (i32.const {c}) "{}") (data (i32.const {p}) "{}")"#, esc(command), esc(params)));
        let on_error = if strict {
            "(return (i32.const 1))".to_string()
        } else {
            "(call $set_output (i32.const 8192) (call $reply_read (i32.const 8192) (call $reply_len)))".to_string()
        };
        body.push_str(&format!(
            "(if (i32.ne (call $call (i32.const {c}) (i32.const {}) (i32.const {p}) (i32.const {})) (i32.const 0)) (then {on_error}))\n",
            command.len(),
            params.len()
        ));
    }
    let wat = format!(
        r#"(module
          (import "craft" "call" (func $call (param i32 i32 i32 i32) (result i32)))
          (import "craft" "reply_len" (func $reply_len (result i32)))
          (import "craft" "reply_read" (func $reply_read (param i32 i32) (result i32)))
          (import "craft" "set_output" (func $set_output (param i32 i32)))
          (memory (export "memory") 1)
          {data}
          (func (export "craft_run") (param $n i32) (result i32) {body} {tail} (i32.const 0)))"#
    );
    wat::parse_str(&wat).unwrap_or_else(|e| panic!("{e}\n{wat}"))
}

/// The `.craftplugin` of plug-in `id` for SoundCraft, asking for `access`.
fn bundle(id: &str, access: &str, wasm: Vec<u8>) -> Vec<u8> {
    let manifest = json!({
        "id": id, "name": "Test Plug-in", "version": "1.0.0", "main": "plugin.wasm",
        "apps": {"soundcraft": {"api": 6}},
        "permissions": {"access": access},
        "commands": [{"id": "go", "label": "Go"}]
    });
    Bundle { manifest: manifest.to_string(), wasm, assets: vec![] }.encode()
}

fn base64(bytes: &[u8]) -> String {
    soundcraft_model::b64::encode(bytes)
}

/// The demo session with plug-in `id` installed for the session.
fn with_plugin(id: &str, access: &str, wasm: Vec<u8>) -> Engine {
    let mut e = crate::demo::demo_engine();
    let r = e.execute("plugin.install", &json!({"data": base64(&bundle(id, access, wasm)), "name": "test.craftplugin"})).unwrap();
    assert_eq!(r["installed"], true);
    assert_eq!(r["stored"], false, "without a store, a plug-in is kept for the session");
    e
}

const ADD_TRACK: (&str, &str) = ("track.new", r#"{"count":1,"format":"mono","name":"From Plug-in"}"#);

fn tracks(e: &Engine) -> usize {
    e.session().tracks.len()
}

#[test]
fn a_write_plugin_runs_real_commands_as_one_undo_step_named_after_it() {
    let mut e = with_plugin("io.example.tracks", "write", module(&[ADD_TRACK, ADD_TRACK], true, ""));
    let (count, steps) = (tracks(&e), e.undo_history().len());

    let r = e.execute("plugin.run", &json!({"id": "io.example.tracks"})).unwrap();

    assert_eq!(r["calls"], 2);
    assert_eq!(tracks(&e), count + 2);
    let labels = e.undo_history();
    assert_eq!(labels.len(), steps + 1, "two commands, one undo step");
    assert_eq!(labels.last().map(String::as_str), Some("Test Plug-in"));
    assert_eq!(e.journal.back().map(|(id, _)| id.as_str()), Some("plugin.run"), "the journal has the run, not its calls");
    assert!(!e.journal.iter().any(|(id, _)| id == "track.new"));
    assert!(e.undo());
    assert_eq!(tracks(&e), count, "one undo takes the whole run back");
    assert_eq!(e.undo_history().len(), steps, "the session's own steps are still there");
    assert!(e.redo());
    assert_eq!(tracks(&e), count + 2);
}

#[test]
fn a_run_that_changes_nothing_records_no_step_and_keeps_redo() {
    let mut e = with_plugin("io.example.lister", "write", module(&[("session.inspect", "{}")], true, ""));
    e.execute("track.new", &json!({"count": 1})).unwrap();
    assert!(e.undo());
    let steps = e.undo_history().len();

    e.execute("plugin.run", &json!({"id": "io.example.lister"})).unwrap();

    assert_eq!(e.undo_history().len(), steps);
    assert!(e.can_redo(), "redo survives a run that edited nothing");
}

#[test]
fn a_read_plugin_that_edits_is_put_back_and_refused() {
    let mut e = with_plugin("io.example.reader", "read", module(&[("session.inspect", "{}"), ADD_TRACK], false, ""));
    let (session, steps, dirty) = (e.session_arc(), e.undo_history().len(), e.is_dirty());

    let r = e.execute("plugin.run", &json!({"id": "io.example.reader"})).unwrap();

    let message = r["output"].as_str().unwrap();
    assert!(message.contains("may only read it"), "{message}");
    assert_eq!(*e.session(), *session, "the edit is put back");
    assert_eq!(e.undo_history().len(), steps, "nothing to undo");
    assert!(!e.can_redo());
    assert_eq!(e.is_dirty(), dirty, "the session isn't marked changed by the run");
}

#[test]
fn refused_commands_file_paths_and_hosted_audio_plugins_are_refused() {
    for (call, why) in [
        (("session.open", r#"{"path":"/etc/hosts"}"#), "isn't available to plugins"),
        (("session.save", "{}"), "isn't available to plugins"),
        (("file.import_audio", r#"{"path":"/etc/hosts"}"#), "isn't available to plugins"),
        (("file.bounce_mix", r#"{"path":"/tmp/x.wav"}"#), "isn't available to plugins"),
        (("edit.undo", "{}"), "isn't available to plugins"),
        (("engine.history", "{}"), "isn't available to plugins"),
        (("engine.plugins", "{}"), "isn't available to plugins"),
        (("transport.play", "{}"), "isn't available to plugins"),
        (("transport.record", "{}"), "isn't available to plugins"),
        (("mix.record_arm", r#"{"track":"Kick","value":true}"#), "isn't available to plugins"),
        (("setup.playback_engine", "{}"), "isn't available to plugins"),
        (("options.destructive_record", "{}"), "isn't available to plugins"),
        (("edit.copy", "{}"), "isn't available to plugins"),
        (("edit.paste", "{}"), "isn't available to plugins"),
        (("track.save_preset", r#"{"path":"/tmp/x"}"#), "isn't available to plugins"),
        (("plugin.list", "{}"), "isn't available to plugins"),
        (("window.mix", "{}"), "isn't available to plugins"),
        (("markers.add", r#"{"name":"x","dir":"/tmp"}"#), "can't name files"),
        (("mix.insert", r#"{"track":"Kick","plugin":"vst3:0123456789ABCDEF"}"#), "hosted audio plug-ins"),
        (("track.new", r#"{"rows":[{"kind":"instrument","instrument":"clap:com.example.synth"}]}"#), "hosted audio plug-ins"),
    ] {
        let mut e = with_plugin("io.example.sneaky", "write", module(&[call], false, ""));
        let (session, steps) = (e.session_arc(), e.undo_history().len());
        let r = e.execute("plugin.run", &json!({"id": "io.example.sneaky"})).unwrap();
        let message = r["output"].as_str().unwrap_or_default();
        assert!(message.contains(why), "{}: {message}", call.0);
        assert_eq!(*e.session(), *session, "{}", call.0);
        assert_eq!(e.undo_history().len(), steps, "{}", call.0);
        assert!(e.transport_requests.is_empty(), "{}", call.0);
    }
}

#[test]
fn reading_commands_stay_open_and_built_in_processors_are_allowed() {
    let m = manifest("write");
    for allowed in ["session.inspect", "transport.status", "engine.commands", "setup.session", "track.new", "mix.volume", "edit.copy_to_new_playlist"]
    {
        assert!(super::refusal(&m, allowed, &json!({})).is_none(), "{allowed}");
    }
    assert!(super::refusal(&m, "mix.insert", &json!({"plugin": "eq_7band"})).is_none(), "built-in processors are ours");
    assert!(super::refusal(&m, "mix.insert", &json!({"plugin": " AU:aufx:bpas:appl"})).is_some());
    assert!(super::refusal(&m, "markers.edit", &json!({"fields": {"nested": {"folder": "/tmp"}}})).is_some(), "at any depth");
}

fn manifest(access: &str) -> super::craft_plugin::Manifest {
    let v = json!({"id": "io.example.m", "name": "M", "version": "1.0.0", "main": "p.wasm", "apps": {"soundcraft": {"api": 6}}, "permissions": {"access": access}});
    super::craft_plugin::Manifest::from_value(&v).unwrap()
}

#[test]
fn a_call_that_reaches_the_transport_or_the_clipboard_is_put_back_and_refused() {
    // No allowed command does either today; the check after every call is the net for one that
    // starts to (a Locate added to a selection command, say).
    let m = manifest("write");
    for (what, change) in [
        ("reaches the transport", (|e: &mut Engine| e.transport_requests.push(crate::TransportRequest::Locate(48_000))) as fn(&mut Engine)),
        ("uses the clipboard", |e: &mut Engine| e.clipboard.length = 42),
        ("arms tracks", |e: &mut Engine| {
            if let Some(t) = e.runtime_session_mut().tracks.first_mut() {
                t.mixer.record_arm = true;
            }
        }),
    ] {
        let mut e = crate::demo::demo_engine();
        let (session, steps) = (e.session_arc(), e.undo_history().len());
        let probe = super::host::Probe::take(&e);
        e.execute("markers.add", &json!({"name": "Verse", "at": 48_000})).unwrap();
        change(&mut e);
        let why = probe.violation(&e, &m, "markers.add").unwrap_or_default();
        assert!(why.contains(what), "{what}: {why}");
        probe.restore(&mut e);
        assert!(e.transport_requests.is_empty() && e.clipboard.length == 0, "{what}");
        assert_eq!(*e.session(), *session, "{what}: the call's edit is put back too");
        assert_eq!(e.undo_history().len(), steps, "{what}");
    }
}

#[test]
fn every_allowed_command_exists_and_none_reaches_the_transport_files_or_the_clipboard() {
    for id in super::ALLOWED {
        assert!(crate::find_command(id).is_some(), "{id} is not a command");
        assert!(!super::craft_plugin::ALWAYS_DENIED.iter().any(|p| super::craft_plugin::permissions::pattern_matches(p, id)), "{id}");
    }
    let m = manifest("write");
    for c in crate::command_specs() {
        let refused = super::refusal(&m, c.id, &json!({})).is_some();
        let risky = ["transport.", "session.", "file.", "setup.", "options.", "plugin.", "engine.", "app."].iter().any(|p| c.id.starts_with(p))
            || (c.id.starts_with("edit.copy") && !c.id.contains("playlist"))
            || c.id.starts_with("edit.cut")
            || c.id.starts_with("edit.paste")
            || matches!(c.id, "edit.undo" | "edit.redo" | "mix.record_arm" | "mix.input_monitor" | "markers.recall" | "edit.play_timeline");
        let read_only = matches!(
            c.id,
            "session.inspect"
                | "session.format_time"
                | "session.audio_health"
                | "transport.status"
                | "engine.commands"
                | "setup.session"
                | "setup.current_timecode"
        );
        assert!(refused || !risky || read_only, "{} must not be open to command plug-ins", c.id);
        assert_eq!(refused, !super::ALLOWED.contains(&c.id), "{}: everything off the list is refused", c.id);
    }
}

#[test]
fn a_command_outside_the_manifest_allowlist_is_refused() {
    let mut e = crate::demo::demo_engine();
    let manifest = json!({"id": "io.example.narrow", "name": "Narrow", "version": "1.0.0", "main": "p.wasm",
        "apps": {"soundcraft": {"api": 6}}, "permissions": {"access": "write", "commands": ["markers.*"]}});
    let bytes = Bundle { manifest: manifest.to_string(), wasm: module(&[ADD_TRACK], false, ""), assets: vec![] }.encode();
    e.execute("plugin.install", &json!({"data": base64(&bytes)})).unwrap();
    let count = tracks(&e);

    let r = e.execute("plugin.run", &json!({"id": "io.example.narrow"})).unwrap();

    assert!(r["output"].as_str().unwrap().contains("asked permission for"), "{r}");
    assert_eq!(tracks(&e), count);
}

#[test]
fn a_trapping_or_endless_plugin_leaves_the_session_unchanged() {
    let select = ("edit.select", r#"{"tracks":["Kick"],"start":0,"end":1000}"#);
    for (tail, fuel) in [("unreachable", None), ("(loop $l (br $l))", Some(200_000))] {
        let mut e = with_plugin("io.example.crash", "write", module(&[ADD_TRACK, select, ADD_TRACK], true, tail));
        e.execute("track.new", &json!({"count": 1})).unwrap();
        let (session, steps, dirty) = (e.session_arc(), e.undo_history(), e.is_dirty());
        let mut params = json!({"id": "io.example.crash"});
        if let Some(f) = fuel {
            params["fuel"] = json!(f);
        }

        let err = e.execute("plugin.run", &params).unwrap_err().to_string();

        assert!(err.contains("Test Plug-in"), "{err}");
        assert_eq!(*e.session(), *session, "{tail}: the edits and the selection before the failure are rolled back");
        assert_eq!(e.undo_history(), steps);
        assert!(!e.can_redo());
        assert_eq!(e.is_dirty(), dirty);
        assert!(e.transport_requests.is_empty());
    }
}

#[test]
fn a_plugin_without_a_command_or_installed_does_not_run() {
    let mut e = with_plugin("io.example.tracks", "write", module(&[ADD_TRACK], true, ""));
    assert!(e.execute("plugin.run", &json!({"id": "io.example.nothing"})).unwrap_err().to_string().contains("no command plug-in"));
    assert!(e.execute("plugin.run", &json!({"id": "io.example.tracks", "command": "other"})).is_err());
    assert!(e.execute("plugin.run", &json!({})).is_err());
}

#[test]
fn a_plugin_does_not_run_while_recording() {
    let mut e = with_plugin("io.example.tracks", "write", module(&[ADD_TRACK], true, ""));
    e.transport.recording = true;
    let count = tracks(&e);
    let err = e.execute("plugin.run", &json!({"id": "io.example.tracks"})).unwrap_err().to_string();
    assert!(err.contains("stop recording"), "{err}");
    assert_eq!(tracks(&e), count);
}

#[test]
fn settings_are_bounded_and_kept_per_plugin() {
    let mut all = Default::default();
    super::set_setting(&mut all, "a", "k", Some("{\"x\": 1}")).unwrap();
    assert_eq!(all["a"]["k"], json!({"x": 1}));
    assert!(super::set_setting(&mut all, "a", &"k".repeat(129), Some("1")).is_err());
    assert!(super::set_setting(&mut all, "a", "big", Some(&format!("\"{}\"", "x".repeat(64 * 1024)))).is_err());
    assert!(super::set_setting(&mut all, "a", "bad", Some("not json")).is_err());
    super::set_setting(&mut all, "a", "k", None).unwrap();
    assert!(!all.contains_key("a"));
    // Every plug-in's settings together stay within what the preferences file holds
    let chunk = format!("\"{}\"", "x".repeat(60 * 1024));
    let mut kept = 0;
    for i in 0..super::MAX_SETTINGS_PLUGINS {
        if super::set_setting(&mut all, &format!("p{i}"), "k", Some(&chunk)).is_ok() {
            kept += 1;
        }
    }
    assert!(kept < super::MAX_SETTINGS_PLUGINS && kept * 60 * 1024 <= super::MAX_ALL_SETTINGS_BYTES, "{kept}");
}

#[test]
fn a_plugin_keeps_its_settings_and_the_change_is_reported_for_saving() {
    let wat = r#"(module
      (import "craft" "setting_set" (func $set (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "count") (data (i32.const 16) "3")
      (func (export "craft_run") (param i32) (result i32) (drop (call $set (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 1))) (i32.const 0)))"#;
    let mut e = with_plugin("io.example.counter", "read", wat::parse_str(wat).unwrap());
    assert!(!e.command_plugins.take_prefs_changed());
    e.execute("plugin.run", &json!({"id": "io.example.counter"})).unwrap();
    assert_eq!(e.command_plugins.prefs.settings["io.example.counter"]["count"], json!(3));
    assert!(e.command_plugins.take_prefs_changed(), "the desktop app saves its preferences");
    e.execute("plugin.remove", &json!({"id": "io.example.counter"})).unwrap();
    assert!(e.command_plugins.prefs.settings.is_empty(), "removing a plug-in forgets its settings");
}

#[test]
fn hostile_plugin_preferences_are_dropped_not_the_rest() {
    let v = json!({
        "settings": {"ok": {"k": 1, "": 2}, "bad": "x", "big": {"b": "x".repeat(70 * 1024)}},
        "catalogs": ["https://example.com/index.json", 3, "https://raw.githubusercontent.com/o/r/main/index.json"]
    });
    let p: super::PluginPrefs = serde_json::from_value(v).unwrap();
    assert_eq!(p.settings.len(), 1, "{:?}", p.settings);
    assert_eq!(p.settings["ok"].len(), 1);
    assert_eq!(p.catalogs, ["https://raw.githubusercontent.com/o/r/main/index.json"]);
    let p: super::PluginPrefs = serde_json::from_value(json!([1, 2])).unwrap();
    assert_eq!(p, Default::default());
}

#[test]
fn install_review_list_and_remove_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = Engine::default();
    e.command_plugins.open_store(dir.path());
    let bytes = bundle("io.example.stored", "write", module(&[], true, ""));
    let data = base64(&bytes);

    // The confirmation: described, nothing written
    let review = e.execute("plugin.install", &json!({"data": data, "review": true})).unwrap();
    assert_eq!(review["installed"], false);
    assert_eq!(review["permissions"]["access"], "write");
    assert_eq!(review["kind"], "command");
    assert_eq!(review["sha256"], super::craft_plugin::sha256_hex(&bytes));
    assert!(e.command_plugins.list().is_empty());
    let plugins_dir = super::craft_plugin::store::plugins_dir(dir.path());
    assert!(std::fs::read_dir(&plugins_dir).map(|mut d| d.next().is_none()).unwrap_or(true), "nothing is written before the install");

    // Another bundle than the one reviewed isn't installed
    let err = e.execute("plugin.install", &json!({"data": data, "sha256": "0".repeat(64)})).unwrap_err();
    assert!(err.to_string().contains("reviewed"), "{err}");
    e.execute("plugin.install", &json!({"data": data, "review": true})).unwrap();
    let r = e.execute("plugin.install", &json!({"data": data, "sha256": review["sha256"]})).unwrap();
    assert_eq!((r["installed"].clone(), r["stored"].clone()), (json!(true), json!(true)));

    // A new engine reads it back from the folder
    let mut again = Engine::default();
    again.command_plugins.open_store(dir.path());
    let list = again.execute("plugin.list", &json!({})).unwrap();
    assert_eq!(list["plugins"][0]["id"], "io.example.stored");
    assert_eq!(list["plugins"][0]["source"]["type"], "file");
    assert_eq!(list["stored"], true);

    again.execute("plugin.remove", &json!({"id": "io.example.stored"})).unwrap();
    assert_eq!(again.execute("plugin.list", &json!({})).unwrap()["plugins"], json!([]));
    let mut third = Engine::default();
    third.command_plugins.open_store(dir.path());
    assert!(third.command_plugins.list().is_empty(), "removed from the folder");
}

#[test]
fn a_file_install_goes_through_the_automation_read_roots() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let file = dir.path().join("p.craftplugin");
    std::fs::write(&file, bundle("io.example.file", "read", module(&[], true, ""))).unwrap();
    let roots = crate::file_access::AutomationRoots::new(&[other.path()], &[]).unwrap();
    let mut e = Engine::default();
    let path = file.to_string_lossy().to_string();

    let refused = crate::file_access::confine(roots.as_ref(), || e.execute("plugin.install", &json!({"path": path})));
    assert!(refused.is_err(), "outside the read roots");
    let r = e.execute("plugin.install", &json!({"path": path})).unwrap();
    assert_eq!(r["source"]["type"], "file");
}

#[test]
fn plugins_for_other_apps_or_not_bundles_are_refused() {
    let mut e = Engine::default();
    let manifest = json!({"id": "io.example.other", "name": "Other", "version": "1.0.0", "main": "p.wasm", "apps": {"figcraft": {"api": 6}}});
    let bytes = Bundle { manifest: manifest.to_string(), wasm: module(&[], true, ""), assets: vec![] }.encode();
    let err = e.execute("plugin.install", &json!({"data": base64(&bytes)})).unwrap_err().to_string();
    assert!(err.contains("figcraft"), "{err}");
    assert!(e.execute("plugin.install", &json!({"data": base64(b"not a bundle")})).is_err());
    assert!(e.execute("plugin.install", &json!({})).is_err());
    assert!(e.execute("plugin.install", &json!({"data": 7, "path": ["x"]})).is_err(), "hostile params are an error");
}

#[test]
fn a_local_catalog_installs_its_package() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = bundle("io.example.listed", "write", module(&[], true, ""));
    let file = dir.path().join("listed.craftplugin");
    std::fs::write(&file, &bytes).unwrap();
    let url = |p: &std::path::Path| format!("file://{}", p.to_string_lossy().replace('\\', "/"));
    let index = json!({"schema": 1, "packages": [{"id": "io.example.listed", "name": "Listed", "repo": "https://github.com/example/listed",
        "versions": [{"version": "1.0.0", "apps": {"soundcraft": {"api": 6}}, "url": url(&file),
            "sha256": super::craft_plugin::sha256_hex(&bytes), "size": bytes.len()}]}]});
    let catalog = dir.path().join("index.json");
    std::fs::write(&catalog, index.to_string()).unwrap();
    let mut e = Engine::default();

    e.execute("plugin.catalog.add", &json!({"url": url(&catalog)})).unwrap();
    assert!(e.command_plugins.take_prefs_changed(), "a catalog added is saved with the preferences");
    let listed = e.execute("plugin.catalog.list", &json!({})).unwrap();
    let r = e.execute("plugin.install", &json!({"catalog": url(&catalog), "id": "io.example.listed"})).unwrap();

    assert_eq!(listed["packages"][0]["id"], "io.example.listed", "{listed}");
    assert_eq!(r["inCatalog"], true);
    assert_eq!(r["source"]["type"], "catalog");
    assert_eq!(e.execute("plugin.update", &json!({})).unwrap()["updates"], json!([]), "up to date");
    assert!(e.execute("plugin.catalog.add", &json!({"url": "https://example.com/index.json"})).is_err(), "GitHub's hosts only");
}

#[test]
fn a_worker_download_is_offered_then_installed_without_downloading_again() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = bundle("io.example.fetched", "read", module(&[], true, ""));
    let file = dir.path().join("fetched.craftplugin");
    std::fs::write(&file, &bytes).unwrap();
    let url = format!("file://{}", file.to_string_lossy().replace('\\', "/"));
    let index = json!({"schema": 1, "packages": [{"id": "io.example.fetched", "name": "Fetched", "repo": "https://github.com/example/fetched",
        "versions": [{"version": "1.0.0", "apps": {"soundcraft": {"api": 6}}, "url": url, "sha256": super::craft_plugin::sha256_hex(&bytes), "size": bytes.len()}]}]});
    let catalog = dir.path().join("index.json");
    std::fs::write(&catalog, index.to_string()).unwrap();
    let catalog = format!("file://{}", catalog.to_string_lossy().replace('\\', "/"));
    let mut e = Engine::default();
    let request = json!({"catalog": catalog, "id": "io.example.fetched"});

    // What the UI's worker does, then the confirmation and Install
    let c = e.command_plugins.fetcher().with(|f| super::download(f, &[], &request)).unwrap();
    std::fs::remove_file(&file).unwrap();
    let info = super::offer(&mut e, &request, c).unwrap();
    let mut install: Value = request.clone();
    install["sha256"] = info["sha256"].clone();
    let r = e.execute("plugin.install", &install).unwrap();

    assert_eq!(r["installed"], true, "the reviewed bundle is installed; the file it came from is gone");
    assert!(e.command_plugins.get("io.example.fetched").is_some());
}

#[test]
fn a_repository_install_without_network_says_so() {
    let mut e = Engine::default();
    let err = e.execute("plugin.install", &json!({"repo": "example/tools"})).unwrap_err().to_string();
    assert!(err.contains("no network access"), "{err}");
}
