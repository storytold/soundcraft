//! Model Context Protocol server (stdio, newline-delimited JSON-RPC 2.0), backed by
//! `craft_mcp_server`.

use crate::Backend;
use craft_mcp_server::{Contents, Info, Options, Tools};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub use craft_mcp_server::PROTOCOL_VERSION;
const MAX_BATCH_CALLS: usize = 1000;
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

const INSTRUCTIONS: &str = "SoundCraft is a digital audio workstation (Pro Tools-class). Everything a user can do is a \
command: call `list_commands` to discover ids and their params, then `execute` (or `batch`). Positions are samples \
(integers), `{\"seconds\": x}`, or time strings like \"bars_beats:5|1|000\" / \"0:12.500\". Tracks are referenced by id \
or name. Verify your work with `inspect_session` (tracks, clips, mixer, automation, markers, selection). Programmatic \
calls never open dialogs. Tools marked APP_ONLY need the desktop app (`soundcraft --control 0` + `soundcraft-cli mcp --connect PORT`).";

struct SoundcraftTools<B: Backend> {
    backend: B,
}

pub struct Server<B: Backend> {
    inner: craft_mcp_server::Server<SoundcraftTools<B>>,
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
            "Run up to 1000 commands in order; stops at the first error unless keep_going. Larger batches are rejected before any command runs.",
            json!({"calls": {"type": "array", "maxItems": MAX_BATCH_CALLS, "items": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}}}, "keep_going": {"type": "boolean"}}),
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
        tool(
            "save_session",
            "Save the session (path required the first time; replace: true to save over another existing file).",
            json!({"path": {"type": "string"}, "replace": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "import_audio",
            "Import an audio file (WAV, AIFF, FLAC, MP3, OGG, AAC/M4A, ALAC, CAF) onto a new or existing track.",
            json!({"path": {"type": "string"}, "track": {"type": ["string", "integer"]}, "at": {}}),
            &["path"],
        ),
        tool(
            "bounce_mix",
            "Render the mix to an audio file. Returns peak dBFS and integrated LUFS.",
            json!({"path": {"type": "string"}, "format": {"type": "string", "enum": craft_audio::ENCODE_EXTENSIONS}, "bit_depth": {"type": "string"}, "start": {}, "end": {}, "normalize": {"type": "boolean"}}),
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

impl<B: Backend> SoundcraftTools<B> {
    fn exec(&mut self, cmd: &str, params: Value) -> Result<Value, String> {
        self.backend.call("engine.execute", json!({"command": cmd, "params": params}))
    }

    fn call_tool(&mut self, name: &str, args: &Value) -> Value {
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
                if calls.len() > MAX_BATCH_CALLS {
                    return error_result(&format!("`calls` must contain at most {MAX_BATCH_CALLS} commands; no commands were executed"));
                }
                let mut out = Vec::new();
                let mut failed = false;
                for c in calls {
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
}

impl<B: Backend> Tools for SoundcraftTools<B> {
    fn instructions(&self) -> String {
        format!("{INSTRUCTIONS}\nBackend: {}.", self.backend.describe())
    }

    fn list(&mut self) -> Vec<Value> {
        tool_definitions()
    }

    fn call(&mut self, name: &str, args: &Value) -> Value {
        self.call_tool(name, args)
    }

    fn resources(&self) -> Vec<Value> {
        vec![
            json!({"uri": "soundcraft://session", "name": "Session", "mimeType": "application/json", "description": "The open session as JSON"}),
            json!({"uri": "soundcraft://commands", "name": "Commands", "mimeType": "application/json", "description": "All commands"}),
        ]
    }

    fn read_resource(&mut self, uri: &str) -> Option<Result<Contents, String>> {
        let r = match uri {
            "soundcraft://session" => self.backend.call("session.inspect", json!({"detail": "full"})),
            "soundcraft://commands" => self.backend.call("engine.commands", json!({})),
            _ => return None,
        };
        Some(r.map(|v| Contents::Text { mime_type: "application/json".into(), text: v.to_string() }))
    }
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Self {
        let info = Info { name: "soundcraft", title: "SoundCraft", version: env!("CARGO_PKG_VERSION"), max_line: MAX_LINE_BYTES };
        let options = Options { website_url: None, tool_errors_as_results: true, strict_request_ids: false };
        Self { inner: craft_mcp_server::Server::new(info, SoundcraftTools { backend }).with_options(options) }
    }

    /// Dispatch a tool call.
    pub fn call_tool(&mut self, name: &str, args: &Value) -> Value {
        self.inner.tools().call_tool(name, args)
    }

    /// Borrow the underlying backend.
    pub fn backend(&mut self) -> &mut B {
        &mut self.inner.tools().backend
    }

    /// Handle one JSON-RPC message; returns the reply line (None for notifications).
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        self.inner.handle(msg.clone())
    }

    /// Serve until EOF.
    pub fn serve(&mut self, input: impl BufRead, out: impl Write) -> std::io::Result<()> {
        self.inner.serve(input, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Headless;
    use soundcraft_engine::Engine;

    fn reply(line: &str) -> String {
        let mut server = Server::new(Headless::new(Engine::default()));
        let mut out = Vec::new();
        server.serve(line.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn invalid_requests_get_an_invalid_request_error() {
        for input in ["[]", "42", "[1]", "\"x\""] {
            let text = reply(input);
            assert_eq!(text.lines().count(), 1, "input {input}: reply was {text:?}");
            let v: Value = serde_json::from_str(text.trim()).unwrap();
            let e = match &v {
                Value::Array(a) => a[0].clone(),
                other => other.clone(),
            };
            assert_eq!(e["error"]["code"], -32600, "input {input}");
            assert_eq!(e["id"], Value::Null, "input {input}");
        }
    }

    #[test]
    fn notifications_still_get_no_reply() {
        assert_eq!(reply(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), "");
        let v: Value = serde_json::from_str(reply(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).trim()).unwrap();
        assert_eq!(v["id"], 1);
    }
}
