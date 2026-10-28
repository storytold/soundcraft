//! Agent control for SoundCraft.
//!
//! - [`Backend`]: something that answers control-protocol methods (`engine.execute`,
//!   `session.inspect`, `ui.*`…). [`Headless`] runs an in-process engine; [`Remote`] talks to a
//!   running app's `--control` port.
//! - [`mcp::Server`]: a Model Context Protocol server over stdio (newline-delimited JSON-RPC 2.0)
//!   exposing SoundCraft as tools and resources.
//! - [`args`]: the control and automation launch flags and their environment variables, as clap
//!   types.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
pub mod args;
pub mod mcp;

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{self, AutomationRoots};
use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// Answers control-protocol methods.
pub trait Backend {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// True when a UI is attached (screenshots, clicks).
    fn has_ui(&self) -> bool;
    fn describe(&self) -> String;
}

/// An in-process engine (no UI).
pub struct Headless {
    pub engine: Engine,
    roots: Option<Arc<AutomationRoots>>,
}

impl Headless {
    pub fn new(engine: Engine) -> Self {
        Headless { engine, roots: None }
    }

    /// Confine every file the session's commands read and write to `roots`
    /// (`soundcraft-cli mcp --automation-read-root … --automation-write-root …`).
    pub fn with_automation_roots(mut self, roots: Option<Arc<AutomationRoots>>) -> Self {
        self.roots = roots;
        self
    }

    fn answer(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "engine.execute" | "command" => {
                let id = params.get("command").or_else(|| params.get("id")).and_then(Value::as_str).ok_or("`command` required")?;
                let p = params.get("params").cloned().unwrap_or(json!({}));
                let r = self.engine.execute(id, &p).map_err(|e| e.to_string());
                // Headless has no audio engine: drop transport requests but keep the playhead.
                self.engine.transport_requests.clear();
                r
            }
            "engine.commands" => self.engine.execute("engine.commands", &params).map_err(|e| e.to_string()),
            "engine.parity" => Ok(soundcraft_engine::catalog::parity_json()),
            "session.inspect" | "document.inspect" => {
                Ok(soundcraft_engine::inspect::session(&self.engine, params.get("detail").and_then(Value::as_str) == Some("full")))
            }
            m if m.starts_with("ui.") => {
                Err(format!("`{m}` needs the desktop app: start `soundcraft --control 0` and use `soundcraft-cli mcp --connect PORT`"))
            }
            other => Err(format!("unknown method `{other}`")),
        }
    }
}

impl Backend for Headless {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let roots = self.roots.clone();
        file_access::confine(roots.as_ref(), || self.answer(method, params))
    }
    fn has_ui(&self) -> bool {
        false
    }
    fn describe(&self) -> String {
        format!("headless engine, session `{}`", self.engine.session().name)
    }
}

/// Where a running app's control channel listens: what `soundcraft-cli app --port`,
/// `soundcraft-cli mcp --connect`, `soundcraft --control` and SOUNDCRAFT_CONTROL_PORT give, parsed
/// once (`args::control_target`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlTarget {
    /// As given: a name, an IPv4 address, or an IPv6 address in brackets (`[::1]`). `None`:
    /// 127.0.0.1.
    pub host: Option<String>,
    pub port: u16,
}

impl ControlTarget {
    /// Can the desktop app's control server, which listens on 127.0.0.1 only, serve this target?
    /// True for no host, `127.0.0.1` and `localhost`; `[::1]` is another address.
    pub fn is_loopback(&self) -> bool {
        self.host.as_deref().is_none_or(|host| ["127.0.0.1", "localhost"].contains(&host))
    }
}

/// `host:port`, as a socket address is written.
impl fmt::Display for ControlTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host.as_deref().unwrap_or("127.0.0.1"), self.port)
    }
}

/// Why a [`Remote`] request failed.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The host name didn't resolve.
    #[error("{addr}: {source}")]
    Resolve { addr: String, source: io::Error },
    #[error("cannot reach SoundCraft at {addr}: no addresses found")]
    NoAddress { addr: String },
    #[error("cannot reach SoundCraft at {addr}: {source}")]
    Unreachable { addr: String, source: io::Error },
    /// Reading or writing the connection failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The app closed the connection before it replied.
    #[error("connection closed")]
    Closed,
    /// The reply isn't JSON.
    #[error(transparent)]
    Reply(#[from] serde_json::Error),
    /// The app's reply to a request it refused.
    #[error("{0}")]
    App(String),
    /// The connection was lost after a request that changes state went out: it isn't repeated,
    /// since the app may already have executed it.
    #[error("{source}; `{method}` may or may not have been executed, check the session before repeating it")]
    Unconfirmed { method: String, source: Box<RemoteError> },
}

impl RemoteError {
    /// The connection went away, so a fresh one may succeed.
    fn is_lost_connection(&self) -> bool {
        match self {
            RemoteError::Closed => true,
            RemoteError::Io(e) => matches!(e.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted),
            _ => false,
        }
    }
}

type Connection = (TcpStream, BufReader<TcpStream>);

/// A TCP client for a running app's control channel.
pub struct Remote {
    /// The target as `host:port`, for connecting and for messages.
    addr: String,
    conn: Option<Connection>,
    next_id: u64,
}

impl Remote {
    /// A client for the app at `target`; it connects on the first request.
    pub fn new(target: &ControlTarget) -> Self {
        Remote { addr: target.to_string(), conn: None, next_id: 1 }
    }

    /// The open connection, connecting first when there is none.
    fn connection(&mut self) -> Result<&mut Connection, RemoteError> {
        match self.conn {
            Some(ref mut conn) => Ok(conn),
            None => Ok(self.conn.insert(connect(&self.addr)?)),
        }
    }

    /// One request/reply exchange. On failure the flag says whether the request line was already
    /// written to the socket (so the app may have executed it).
    fn roundtrip(&mut self, method: &str, params: &Value) -> Result<Value, (RemoteError, bool)> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"id": id, "method": method, "params": params}).to_string();
        let (w, r) = self.connection().map_err(|e| (e, false))?;
        writeln!(w, "{line}").map_err(|e| (e.into(), false))?;
        let mut buf = String::new();
        r.read_line(&mut buf).map_err(|e| (e.into(), true))?;
        if buf.is_empty() {
            return Err((RemoteError::Closed, true));
        }
        let v: Value = serde_json::from_str(&buf).map_err(|e| (e.into(), true))?;
        if v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err((RemoteError::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string()), true))
        }
    }

    /// Calls `method`. A request lost with its connection is sent once more on a fresh one when
    /// it only reads state.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, RemoteError> {
        match self.roundtrip(method, &params) {
            Ok(v) => Ok(v),
            Err((e, sent)) if e.is_lost_connection() => {
                self.conn = None;
                if sent && !is_read_only(method) {
                    return Err(RemoteError::Unconfirmed { method: method.to_owned(), source: Box::new(e) });
                }
                self.roundtrip(method, &params).map_err(|(e, _)| e)
            }
            Err((e, _)) => Err(e),
        }
    }
}

/// Connects to `addr` (`host:port`), trying each address it resolves to.
fn connect(addr: &str) -> Result<Connection, RemoteError> {
    let socks = addr.to_socket_addrs().map_err(|source| RemoteError::Resolve { addr: addr.to_owned(), source })?;
    let mut last = None;
    for sock in socks {
        match TcpStream::connect_timeout(&sock, Duration::from_millis(800)) {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_secs(120)))?;
                let r = s.try_clone()?;
                return Ok((s, BufReader::new(r)));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(source) => RemoteError::Unreachable { addr: addr.to_owned(), source },
        None => RemoteError::NoAddress { addr: addr.to_owned() },
    })
}

/// Methods that only read state, so repeating them after a lost reply is harmless.
fn is_read_only(method: &str) -> bool {
    matches!(method, "session.inspect" | "document.inspect" | "engine.commands" | "engine.parity" | "ui.inspect" | "ui.menu.list")
}

impl Backend for Remote {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request(method, params).map_err(|e| e.to_string())
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        format!("SoundCraft app at {}", self.addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_executes_and_inspects() {
        let mut h = Headless::new(Engine::default());
        let r = h.call("engine.execute", json!({"command": "track.new", "params": {"count": 2, "format": "stereo"}}));
        assert!(r.is_ok(), "{r:?}");
        let s = h.call("session.inspect", json!({})).unwrap();
        assert_eq!(s["tracks"].as_array().map(Vec::len), Some(2));
        assert!(h.call("ui.click", json!({})).is_err());
        assert!(h.call("nope", json!({})).is_err());
    }

    #[test]
    fn remote_reports_unreachable() {
        let mut r = Remote::new(&ControlTarget { host: Some("127.0.0.1".into()), port: 1 });
        let e = r.request("session.inspect", json!({})).unwrap_err();
        assert!(matches!(e, RemoteError::Unreachable { .. }), "{e:?}");
        assert!(e.to_string().starts_with("cannot reach SoundCraft at 127.0.0.1:1: "), "{e}");
    }

    #[test]
    fn remote_resolves_hostnames() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut r = Remote::new(&ControlTarget { host: Some("localhost".into()), port });
        assert!(r.connection().is_ok());
    }

    /// Fake app: accepts one connection per entry in `replies`, counts the request line it reads,
    /// then either drops the connection (`false`) or answers ok (`true`).
    fn fake_app(replies: Vec<bool>) -> (ControlTarget, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ControlTarget { host: None, port: listener.local_addr().unwrap().port() };
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            for answer in replies {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut line = String::new();
                if BufReader::new(sock.try_clone().unwrap()).read_line(&mut line).unwrap_or(0) > 0 {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if answer {
                        let _ = writeln!(sock, "{}", json!({"id": 1, "ok": true, "result": {"done": true}}));
                    }
                }
            }
        });
        (addr, seen)
    }

    #[test]
    fn remote_does_not_repeat_a_mutation_after_a_lost_reply() {
        let (addr, seen) = fake_app(vec![false, true]);
        let mut r = Remote::new(&addr);
        let err = r.call("engine.execute", json!({"command": "track.new"})).unwrap_err();
        assert!(err.contains("may or may not have been executed"), "{err}");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn remote_retries_a_read_after_a_lost_reply() {
        let (addr, seen) = fake_app(vec![false, true]);
        let mut r = Remote::new(&addr);
        assert!(r.call("session.inspect", json!({})).is_ok());
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
