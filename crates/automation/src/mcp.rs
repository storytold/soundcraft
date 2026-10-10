//! Model Context Protocol server (stdio, newline-delimited JSON-RPC 2.0).

use crate::Backend;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "SoundCraft is a digital audio workstation (Pro Tools-class). Everything a user can do is a \
command: call `list_commands` to discover ids and their params, then `execute` (or `batch`). Positions are samples \
(integers), `{\"seconds\": x}`, or time strings like \"bars_beats:5|1|000\" / \"0:12.500\". Tracks are referenced by id \
or name. Verify your work with `inspect_session` (tracks, clips, mixer, automation, markers, selection). Programmatic \
calls never open dialogs. Tools marked APP_ONLY need the desktop app (`soundcraft --control 0` + `soundcraft-cli mcp --connect PORT`).";

pub struct Server<B: Backend> {
    pub backend: B,
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": required}})
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "list_commands",
            "List SoundCraft commands (id, label, menu path, shortcut, params doc, enabled).",
            json!({"filter": {"type": "string", "description": "substring of id or label"}}),
            &[],
        ),
        tool(
            "execute",
            "Run one command by id with JSON params. Returns the command's result.",
            json!({"command": {"type": "string"}, "params": {"type": "object"}}),
            &["command"],
        ),
        tool(
            "batch",
            "Run several commands in order; stops at the first error unless keep_going.",
            json!({"calls": {"type": "array", "items": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}}}, "keep_going": {"type": "boolean"}}),
            &["calls"],
        ),
        tool(
            "inspect_session",
            "The whole session as JSON: tracks (clips, inserts, sends, automation), busses, markers, groups, selection, transport.",
            json!({"detail": {"type": "string", "enum": ["summary", "full"]}}),
            &[],
        ),
        tool(
            "new_session",
            "Start a new session (optionally the built-in demo).",
            json!({"name": {"type": "string"}, "sample_rate": {"type": "integer"}, "template": {"type": "string", "enum": ["blank", "demo"]}}),
            &[],
        ),
        tool("open_session", "Open a .scraft session file.", json!({"path": {"type": "string"}}), &["path"]),
        tool("save_session", "Save the session (path required the first time).", json!({"path": {"type": "string"}}), &[]),
        tool(
            "import_audio",
            "Import an audio file (WAV, AIFF, FLAC, MP3, OGG, AAC/M4A, ALAC, CAF) onto a new or existing track.",
            json!({"path": {"type": "string"}, "track": {"type": ["string", "integer"]}, "at": {}}),
            &["path"],
        ),
        tool(
            "bounce_mix",
            "Render the mix to an audio file. Returns peak dBFS and integrated LUFS.",
            json!({"path": {"type": "string"}, "format": {"type": "string", "enum": ["wav", "aiff", "flac"]}, "bit_depth": {"type": "string"}, "start": {}, "end": {}, "normalize": {"type": "boolean"}}),
            &["path"],
        ),
        tool("parity", "Feature-parity report against the incumbent's menu catalog.", json!({}), &[]),
        tool("screenshot", "APP_ONLY. Screenshot the app window (PNG).", json!({"path": {"type": "string"}}), &[]),
        tool("ui_inspect", "APP_ONLY. UI state: front window, panels, layout rectangles of tracks, dialogs.", json!({}), &[]),
        tool("ui_set", "APP_ONLY. Set UI state fields, e.g. {\"window\": \"Mix\"}.", json!({"state": {"type": "object"}}), &["state"]),
        tool(
            "menu_invoke",
            "APP_ONLY. Invoke a menu item like a click (may open its dialog), e.g. \"Track > New...\".",
            json!({"path": {"type": "string"}}),
            &["path"],
        ),
        tool(
            "click",
            "APP_ONLY. Click at window coordinates (points).",
            json!({"x": {"type": "number"}, "y": {"type": "number"}, "button": {"type": "string"}, "count": {"type": "integer"}}),
            &["x", "y"],
        ),
        tool(
            "drag",
            "APP_ONLY. Drag with the primary button.",
            json!({"x": {"type": "number"}, "y": {"type": "number"}, "to_x": {"type": "number"}, "to_y": {"type": "number"}}),
            &["x", "y", "to_x", "to_y"],
        ),
        tool(
            "key",
            "APP_ONLY. Press a key with modifiers.",
            json!({"key": {"type": "string"}, "cmd": {"type": "boolean"}, "shift": {"type": "boolean"}, "alt": {"type": "boolean"}, "ctrl": {"type": "boolean"}}),
            &["key"],
        ),
    ]
}

fn text_result(v: &Value) -> Value {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    json!({"content": [{"type": "text", "text": text}], "isError": false})
}

fn error_result(e: &str) -> Value {
    json!({"content": [{"type": "text", "text": e}], "isError": true})
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Self {
        Server { backend }
    }

    fn exec(&mut self, cmd: &str, params: Value) -> Result<Value, String> {
        self.backend.call("engine.execute", json!({"command": cmd, "params": params}))
    }

    /// Dispatch a tool call.
    pub fn call_tool(&mut self, name: &str, args: &Value) -> Value {
        let a = |k: &str| args.get(k).cloned();
        let r: Result<Value, String> = match name {
            "list_commands" => self.backend.call("engine.commands", json!({"filter": a("filter")})),
            "execute" => {
                let Some(cmd) = args.get("command").and_then(Value::as_str) else { return error_result("`command` required") };
                self.exec(cmd, a("params").unwrap_or(json!({})))
            }
            "batch" => {
                let keep = args.get("keep_going").and_then(Value::as_bool).unwrap_or(false);
                let Some(calls) = args.get("calls").and_then(Value::as_array) else {
                    return error_result("`calls` must be an array");
                };
                let mut out = Vec::new();
                let mut failed = false;
                for c in calls.iter().take(1000) {
                    let Some(cmd) = c.get("command").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
                        out.push(json!({"command": "", "ok": false, "error": "`command` required"}));
                        failed = true;
                        if !keep {
                            break;
                        }
                        continue;
                    };
                    let r = self.exec(cmd, c.get("params").cloned().unwrap_or(json!({})));
                    match r {
                        Ok(v) => out.push(json!({"command": cmd, "ok": true, "result": v})),
                        Err(e) => {
                            out.push(json!({"command": cmd, "ok": false, "error": e}));
                            failed = true;
                            if !keep {
                                break;
                            }
                        }
                    }
                }
                // Any failure sets isError so agents that only check that flag notice;
                // keep_going still runs the rest and the payload lists every call.
                if failed { Err(serde_json::to_string(&out).unwrap_or_default()) } else { Ok(json!(out)) }
            }
            "inspect_session" => self.backend.call("session.inspect", json!({"detail": a("detail")})),
            "new_session" => self.exec("session.new", args.clone()),
            "open_session" => self.exec("session.open", args.clone()),
            "save_session" => self.exec("session.save", args.clone()),
            "import_audio" => self.exec("file.import_audio", args.clone()),
            "bounce_mix" => self.exec("file.bounce_mix", args.clone()),
            "parity" => self.backend.call("engine.parity", json!({})),
            "screenshot" => {
                let r = self.backend.call("ui.screenshot", args.clone());
                if let Ok(v) = &r
                    && let Some(b64) = v.get("png_base64").and_then(Value::as_str)
                {
                    return json!({"content": [{"type": "image", "data": b64, "mimeType": "image/png"}], "isError": false});
                }
                r
            }
            "ui_inspect" => self.backend.call("ui.inspect", json!({})),
            "ui_set" => self.backend.call("ui.set", a("state").unwrap_or(json!({}))),
            "menu_invoke" => self.backend.call("ui.menu.invoke", args.clone()),
            "click" => self.backend.call("ui.click", args.clone()),
            "drag" => self.backend.call("ui.drag", args.clone()),
            "key" => self.backend.call("ui.key", args.clone()),
            other => Err(format!("unknown tool `{other}`")),
        };
        match r {
            Ok(v) => text_result(&v),
            Err(e) => error_result(&e),
        }
    }

    /// Handle one JSON-RPC message; returns the reply line (None for notifications).
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let id = id?;
        let result: Result<Value, (i64, String)> = match method {
            "initialize" => {
                let want = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                let ver = if SUPPORTED.contains(&want) { want } else { PROTOCOL_VERSION };
                Ok(json!({
                    "protocolVersion": ver,
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": {"name": "soundcraft", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": format!("{INSTRUCTIONS}\nBackend: {}.", self.backend.describe()),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(self.call_tool(name, &args))
            }
            "resources/list" => Ok(json!({"resources": [
                {"uri": "soundcraft://session", "name": "Session", "mimeType": "application/json", "description": "The open session as JSON"},
                {"uri": "soundcraft://commands", "name": "Commands", "mimeType": "application/json", "description": "All commands"},
            ]})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                let r = match uri {
                    "soundcraft://session" => self.backend.call("session.inspect", json!({"detail": "full"})),
                    "soundcraft://commands" => self.backend.call("engine.commands", json!({})),
                    _ => Err(format!("unknown resource {uri}")),
                };
                match r {
                    Ok(v) => Ok(json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": v.to_string()}]})),
                    Err(e) => Err((-32002, e)),
                }
            }
            "" => Err((-32600, "invalid request".into())),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, m)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}}),
        })
    }

    /// Serve until EOF.
    pub fn serve(&mut self, input: impl BufRead, mut out: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(Value::Array(batch)) => {
                    let replies: Vec<Value> = batch.iter().filter_map(|m| self.handle(m)).collect();
                    if replies.is_empty() { None } else { Some(Value::Array(replies)) }
                }
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
            };
            if let Some(r) = reply {
                writeln!(out, "{r}")?;
                out.flush()?;
            }
        }
        Ok(())
    }
}
