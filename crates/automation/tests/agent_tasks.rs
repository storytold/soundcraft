//! An agent completes realistic DAW tasks through MCP only, and verifies each through MCP.

use serde_json::{Value, json};
use soundcraft_automation::{Headless, mcp::Server};
use soundcraft_engine::Engine;

fn rpc(s: &mut Server<Headless>, id: i64, method: &str, params: Value) -> Value {
    s.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap()
}

fn tool(s: &mut Server<Headless>, name: &str, args: Value) -> Value {
    let r = rpc(s, 1, "tools/call", json!({"name": name, "arguments": args}));
    let res = &r["result"];
    assert_eq!(res["isError"], json!(false), "{name} failed: {res}");
    let text = res["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

fn session(s: &mut Server<Headless>) -> Value {
    tool(s, "inspect_session", json!({"detail": "full"}))
}

fn track<'a>(sess: &'a Value, name: &str) -> &'a Value {
    sess["tracks"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("no track {name}"))
}

#[test]
fn handshake_and_tool_list() {
    let mut s = Server::new(Headless::new(Engine::default()));
    let r = rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(r["result"]["serverInfo"]["name"], "soundcraft");
    let r = rpc(&mut s, 2, "tools/list", json!({}));
    assert!(r["result"]["tools"].as_array().unwrap().len() >= 10);
    assert!(s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    let r = rpc(&mut s, 3, "nope", json!({}));
    assert_eq!(r["error"]["code"], -32601);
}

#[test]
fn agent_builds_and_mixes_a_session() {
    let mut s = Server::new(Headless::new(Engine::default()));
    // 1. New demo session.
    tool(&mut s, "new_session", json!({"name": "Agent Mix", "template": "demo"}));
    let sess = session(&mut s);
    assert_eq!(sess["name"], "Agent Mix");
    assert!(sess["tracks"].as_array().unwrap().len() >= 9);

    // 2. Turn the kick down and pan the hats.
    tool(&mut s, "execute", json!({"command": "mix.volume", "params": {"track": "Kick", "db": -8.5}}));
    tool(&mut s, "execute", json!({"command": "mix.pan", "params": {"track": "Hats", "pan": -0.5}}));
    let sess = session(&mut s);
    assert_eq!(track(&sess, "Kick")["volume_db"], json!(-8.5));
    assert_eq!(track(&sess, "Hats")["pan"][0], json!(-0.5));

    // 3. Put an EQ on the bass with a low-shelf boost, then bypass it.
    let r = tool(&mut s, "execute", json!({"command": "mix.insert", "params": {"track": "Bass", "slot": "c", "plugin": "eq_7band"}}));
    assert_eq!(r["slot"], 2);
    let sess = session(&mut s);
    assert_eq!(track(&sess, "Bass")["inserts"].as_array().unwrap().iter().filter(|i| i["plugin"] == "eq_7band").count(), 1);
    tool(&mut s, "execute", json!({"command": "mix.insert_bypass", "params": {"track": "Bass", "slot": 2}}));
    let sess = session(&mut s);
    assert_eq!(track(&sess, "Bass")["inserts"].as_array().unwrap().iter().find(|i| i["slot"] == 2).unwrap()["bypass"], true);

    // 4. New stereo aux fed from a send.
    tool(
        &mut s,
        "batch",
        json!({"calls": [
            {"command": "track.new", "params": {"kind": "aux", "format": "stereo", "name": "Delay"}},
            {"command": "track.input", "params": {"track": "Delay", "input": "DelayBus"}},
            {"command": "mix.insert", "params": {"track": "Delay", "plugin": "mod_delay"}},
            {"command": "mix.send", "params": {"track": "Lead", "slot": 1, "bus": "DelayBus", "level_db": -6}},
        ]}),
    );
    let sess = session(&mut s);
    let lead = track(&sess, "Lead");
    assert_eq!(lead["sends"].as_array().unwrap().iter().find(|x| x["slot"] == 1).unwrap()["level_db"], json!(-6.0));

    // 5. Edit: select bars 5–9 on Snare, cut, undo, verify the clip count is restored.
    let before = track(&session(&mut s), "Snare")["clips"].as_array().unwrap().len();
    tool(
        &mut s,
        "execute",
        json!({"command": "edit.select", "params": {"tracks": ["Snare"], "start": "bars_beats:6|1|000", "end": "bars_beats:9|1|000"}}),
    );
    tool(&mut s, "execute", json!({"command": "edit.cut", "params": {}}));
    let mid = track(&session(&mut s), "Snare")["clips"].as_array().unwrap().len();
    assert_ne!(mid, before);
    tool(&mut s, "execute", json!({"command": "edit.undo", "params": {}}));
    assert_eq!(track(&session(&mut s), "Snare")["clips"].as_array().unwrap().len(), before);

    // 6. Markers and tempo.
    tool(&mut s, "execute", json!({"command": "markers.add", "params": {"name": "Bridge", "at": {"seconds": 30.0}}}));
    tool(&mut s, "execute", json!({"command": "event.tempo", "params": {"bpm": 110.0}}));
    let sess = session(&mut s);
    assert!(sess["markers"].as_array().unwrap().iter().any(|m| m["name"] == "Bridge"));
    assert_eq!(sess["tempo"][0]["bpm"], json!(110.0));

    // 7. Volume automation ride on the pad.
    tool(
        &mut s,
        "execute",
        json!({"command": "automation.write_range", "params": {"track": "Pad", "param": "volume", "start": {"seconds": 10.0}, "end": {"seconds": 12.0}, "value": -20.0}}),
    );
    let pad = track(&session(&mut s), "Pad").clone();
    assert!(pad["automation"].as_array().unwrap().iter().any(|l| l["param"] == "volume"));

    // 8. Bounce 4 seconds and check loudness is sane.
    let dir = std::env::temp_dir().join(format!("soundcraft-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bounce.wav");
    let r =
        tool(&mut s, "bounce_mix", json!({"path": path.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 14.0}, "bit_depth": "24"}));
    assert!(r["peak_db"].as_f64().unwrap() > -40.0, "{r}");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");

    // 9. Save, reopen, and the mix survives.
    let sp = dir.join("Agent Mix").join("Agent Mix.scraft");
    std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
    tool(&mut s, "save_session", json!({"path": sp.to_string_lossy()}));
    tool(&mut s, "new_session", json!({}));
    tool(&mut s, "open_session", json!({"path": sp.to_string_lossy()}));
    let sess = session(&mut s);
    assert_eq!(track(&sess, "Kick")["volume_db"], json!(-8.5));
    assert!(sess["sources"].as_array().unwrap().len() >= 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn errors_are_reported_not_panics() {
    let mut s = Server::new(Headless::new(Engine::default()));
    let r = rpc(&mut s, 1, "tools/call", json!({"name": "execute", "arguments": {"command": "mix.volume", "params": {"track": "Nope", "db": 0}}}));
    assert_eq!(r["result"]["isError"], true);
    let r = rpc(&mut s, 2, "tools/call", json!({"name": "screenshot", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
}

#[test]
fn batch_keep_going_still_sets_is_error_on_partial_failure() {
    let mut s = Server::new(Headless::new(Engine::default()));
    let r = rpc(
        &mut s,
        1,
        "tools/call",
        json!({"name": "batch", "arguments": {"keep_going": true, "calls": [
            {"command": "track.new", "params": {"count": 1, "name": "Kept"}},
            {"command": "mix.volume", "params": {"track": "Nope", "db": -6}},
            {"command": "track.new", "params": {"count": 1, "name": "After"}},
        ]}}),
    );
    assert_eq!(r["result"]["isError"], true, "{r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    let results: Value = serde_json::from_str(text).unwrap();
    let arr = results.as_array().unwrap();
    assert_eq!(arr.len(), 3, "{results}");
    assert_eq!(arr[0]["ok"], true);
    assert_eq!(arr[1]["ok"], false);
    assert_eq!(arr[2]["ok"], true);
    let sess = session(&mut s);
    assert!(sess["tracks"].as_array().unwrap().iter().any(|t| t["name"] == "Kept"));
    assert!(sess["tracks"].as_array().unwrap().iter().any(|t| t["name"] == "After"));
}

#[test]
fn batch_rejects_non_array_calls() {
    let mut s = Server::new(Headless::new(Engine::default()));
    let r = rpc(&mut s, 1, "tools/call", json!({"name": "batch", "arguments": {"calls": {"command": "track.new"}}}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("`calls` must be an array"), "{text}");
    assert_eq!(session(&mut s)["tracks"].as_array().unwrap().len(), 0);
}
