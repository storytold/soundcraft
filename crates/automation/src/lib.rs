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
    /// The reply answers another request, so it isn't returned.
    #[error("the reply does not answer this request (its id is {got}, not {expected})")]
    OutOfStep { expected: u64, got: String },
}

impl RemoteError {
    /// The connection went away, or what it carries next can't be trusted (no reply in time, a
    /// reply that isn't JSON or answers another request): it is dropped, and a fresh one may succeed.
    fn is_lost_connection(&self) -> bool {
        match self {
            RemoteError::Closed | RemoteError::Reply(_) | RemoteError::OutOfStep { .. } => true,
            RemoteError::Io(e) => matches!(
                e.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    // A read that timed out: `TimedOut` on Windows, `WouldBlock` on Unix.
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::WouldBlock
                    // A reply that isn't UTF-8: `read_line` fails before there is JSON to parse.
                    | io::ErrorKind::InvalidData
            ),
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
    /// How long a request waits for its reply.
    read_timeout: Duration,
}

/// The app answers within 90 s (it replies `timeout` itself after that), so a longer silence
/// means the reply is lost or late.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

impl Remote {
    /// A client for the app at `target`; it connects on the first request.
    pub fn new(target: &ControlTarget) -> Self {
        Remote { addr: target.to_string(), conn: None, next_id: 1, read_timeout: READ_TIMEOUT }
    }

    /// The open connection, connecting first when there is none.
    fn connection(&mut self) -> Result<&mut Connection, RemoteError> {
        match self.conn {
            Some(ref mut conn) => Ok(conn),
            None => Ok(self.conn.insert(connect(&self.addr, self.read_timeout)?)),
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
        let ok = v.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let reply_id = v.get("id").filter(|id| !id.is_null());
        // The app answers a request with its id. A reply with another id answers another request (a
        // late reply to one that timed out), and so does a success with none.
        if reply_id.is_some_and(|got| got.as_u64() != Some(id)) || (ok && reply_id.is_none()) {
            let got = reply_id.map_or_else(|| "none".to_owned(), Value::to_string);
            return Err((RemoteError::OutOfStep { expected: id, got }, true));
        }
        if ok {
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        if reply_id.is_none() {
            // A refusal with no id (too many connections, a line the app can't read) is the app's
            // last word on this connection: it closes it.
            self.conn = None;
        }
        Err((RemoteError::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string()), true))
    }

    /// Calls `method`. A request lost with its connection is sent once more on a fresh one when
    /// it only reads state, or when it never went out. A connection that is lost or out of step
    /// is never used again, so a reply that arrives late on it is not read as the answer to a
    /// later call.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, RemoteError> {
        // A call that changes state and went out before its connection was lost may have run.
        let may_have_run = |e: &RemoteError, sent: bool| sent && e.is_lost_connection() && !is_read_only(method);
        let report = |(e, sent): (RemoteError, bool)| {
            if may_have_run(&e, sent) { RemoteError::Unconfirmed { method: method.to_owned(), source: Box::new(e) } } else { e }
        };
        match self.attempt(method, &params) {
            Err((e, sent)) if e.is_lost_connection() && !may_have_run(&e, sent) => self.attempt(method, &params).map_err(report),
            first => first.map_err(report),
        }
    }

    /// One [`Self::roundtrip`]; a failure that loses the connection drops it.
    fn attempt(&mut self, method: &str, params: &Value) -> Result<Value, (RemoteError, bool)> {
        let result = self.roundtrip(method, params);
        if result.as_ref().is_err_and(|(e, _)| e.is_lost_connection()) {
            self.conn = None;
        }
        result
    }
}

/// Connects to `addr` (`host:port`), trying each address it resolves to. A read on the connection
/// gives up after `read_timeout`.
fn connect(addr: &str, read_timeout: Duration) -> Result<Connection, RemoteError> {
    let socks = addr.to_socket_addrs().map_err(|source| RemoteError::Resolve { addr: addr.to_owned(), source })?;
    let mut last = None;
    for sock in socks {
        match TcpStream::connect_timeout(&sock, Duration::from_millis(800)) {
            Ok(s) => {
                s.set_read_timeout(Some(read_timeout))?;
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
    /// then either drops the connection (`false`) or answers ok with the request's id (`true`).
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
                        let id = serde_json::from_str::<Value>(&line).map(|request| request["id"].clone()).unwrap_or(Value::Null);
                        let _ = writeln!(sock, "{}", json!({"id": id, "ok": true, "result": {"done": true}}));
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

    #[test]
    fn remote_returns_an_app_error_unchanged() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = ControlTarget { host: None, port: listener.local_addr().unwrap().port() };
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut line = String::new();
            let _ = BufReader::new(sock.try_clone().unwrap()).read_line(&mut line);
            let _ = writeln!(sock, "{}", json!({"id": 1, "ok": false, "error": "transport.locate: nothing to reset"}));
        });
        let mut r = Remote::new(&addr);
        let err = r.call("engine.execute", json!({"command": "transport.locate"})).unwrap_err();
        assert_eq!(err, "transport.locate: nothing to reset");
    }

    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// A listener on a free loopback port, and a client for it that waits 200 ms for a reply.
    fn listener_and_client() -> (TcpListener, Remote) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut remote = Remote::new(&ControlTarget { host: None, port: listener.local_addr().unwrap().port() });
        remote.read_timeout = Duration::from_millis(200);
        (listener, remote)
    }

    /// The next connection, as a write half and a line reader.
    fn accept(listener: &TcpListener) -> Option<(TcpStream, BufReader<TcpStream>)> {
        let (sock, _) = listener.accept().ok()?;
        let reader = BufReader::new(sock.try_clone().ok()?);
        Some((sock, reader))
    }

    /// The next request on a connection; `None` when the client closed it.
    fn next_request(reader: &mut BufReader<TcpStream>) -> Option<Value> {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        serde_json::from_str(&line).ok()
    }

    /// Answers `request` ok, with its id and `result`.
    fn answer(sock: &mut TcpStream, request: &Value, result: Value) {
        let _ = writeln!(sock, "{}", json!({"id": request["id"], "ok": true, "result": result}));
    }

    /// Serves every later connection: each request is counted and answered `{"call": "own"}`.
    fn serve_own_answers(listener: &TcpListener, seen: &AtomicUsize) {
        while let Some((mut sock, mut reader)) = accept(listener) {
            while let Some(request) = next_request(&mut reader) {
                seen.fetch_add(1, Ordering::SeqCst);
                answer(&mut sock, &request, json!({"call": "own"}));
            }
        }
    }

    /// The case in #894: the reply to a call that timed out arrives late, on the connection the
    /// call used. The next call must get its own answer, not that one.
    #[test]
    fn a_late_reply_is_not_taken_for_the_next_calls_answer() {
        let (listener, mut remote) = listener_and_client();
        let (release, released) = mpsc::channel::<()>();
        let (written, late_reply_written) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let Some((mut sock, mut reader)) = accept(&listener) else { return };
            let Some(first) = next_request(&mut reader) else { return };
            // Hold the reply until the client has given up on it.
            let _ = released.recv();
            answer(&mut sock, &first, json!({"call": "first"}));
            let _ = written.send(());
            serve_own_answers(&listener, &AtomicUsize::new(0));
        });
        assert!(remote.call("engine.execute", json!({"command": "file.bounce_mix"})).is_err());
        release.send(()).unwrap();
        late_reply_written.recv().unwrap();
        assert_eq!(remote.call("session.inspect", json!({})), Ok(json!({"call": "own"})));
    }

    /// A reply is only an answer when it carries the request's id: another id, or a success with
    /// no id, is refused, the call is reported as unconfirmed, and the connection is not used again.
    #[test]
    fn a_reply_that_is_not_for_the_request_is_refused() {
        for reply in [
            json!({"id": 999, "ok": true, "result": {"call": "other"}}),
            json!({"id": "1", "ok": true, "result": {"call": "other"}}),
            json!({"ok": true, "result": {"call": "other"}}),
            json!({"id": 999, "ok": false, "error": "another call failed"}),
        ] {
            let (listener, mut remote) = listener_and_client();
            let other = reply.clone();
            std::thread::spawn(move || {
                let Some((mut sock, mut reader)) = accept(&listener) else { return };
                if next_request(&mut reader).is_some() {
                    let _ = writeln!(sock, "{other}");
                }
                serve_own_answers(&listener, &AtomicUsize::new(0));
            });
            let error = remote.call("engine.execute", json!({"command": "track.new"})).unwrap_err();
            assert!(error.contains("does not answer this request"), "{reply}: {error}");
            assert!(error.contains("may or may not have been executed"), "{reply}: {error}");
            assert_eq!(remote.call("engine.execute", json!({"command": "track.new"})), Ok(json!({"call": "own"})), "{reply}");
        }
    }

    /// The app refuses a connection or a bad line without an id, and closes. That is still the
    /// app's error, word for word, and the closed connection is not used for the next call.
    #[test]
    fn a_refusal_without_an_id_is_the_apps_error_and_ends_the_connection() {
        let (listener, mut remote) = listener_and_client();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            let Some((mut sock, mut reader)) = accept(&listener) else { return };
            if next_request(&mut reader).is_some() {
                let _ = writeln!(sock, "{}", json!({"ok": false, "error": "too many control connections"}));
            }
            drop((sock, reader));
            serve_own_answers(&listener, &counter);
        });
        assert_eq!(remote.call("engine.execute", json!({"command": "track.new"})), Err("too many control connections".to_string()));
        assert!(remote.conn.is_none(), "the app closed this connection");
        assert_eq!(remote.call("engine.execute", json!({"command": "track.new"})), Ok(json!({"call": "own"})));
        assert_eq!(seen.load(Ordering::SeqCst), 1, "the second call went out once, on a new connection");
    }

    /// A reply that is not JSON leaves the stream in an unknown state: the connection is dropped.
    #[test]
    fn an_unreadable_reply_ends_the_connection() {
        let (listener, mut remote) = listener_and_client();
        std::thread::spawn(move || {
            let Some((mut sock, mut reader)) = accept(&listener) else { return };
            if next_request(&mut reader).is_some() {
                let _ = writeln!(sock, "not json");
            }
            serve_own_answers(&listener, &AtomicUsize::new(0));
            drop((sock, reader));
        });
        let error = remote.call("engine.execute", json!({"command": "track.new"})).unwrap_err();
        assert!(error.contains("may or may not have been executed"), "{error}");
        assert!(remote.conn.is_none());
        assert_eq!(remote.call("session.inspect", json!({})), Ok(json!({"call": "own"})));
    }

    /// A reply that is not UTF-8 fails the read itself (`InvalidData`), before any JSON parsing. It
    /// is as unreadable as one that is not JSON: the connection is dropped, a call that changes
    /// state is unconfirmed, and a read is asked once more on a new connection.
    #[test]
    fn a_reply_that_is_not_text_ends_the_connection() {
        for (method, params) in [("engine.execute", json!({"command": "track.new"})), ("session.inspect", json!({}))] {
            let (listener, mut remote) = listener_and_client();
            let seen = Arc::new(AtomicUsize::new(0));
            let counter = seen.clone();
            std::thread::spawn(move || {
                let Some((mut sock, mut reader)) = accept(&listener) else { return };
                if next_request(&mut reader).is_some() {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = sock.write_all(b"\xff\n");
                }
                serve_own_answers(&listener, &counter);
                drop((sock, reader));
            });
            let result = remote.call(method, params);
            if is_read_only(method) {
                assert_eq!(result, Ok(json!({"call": "own"})), "{method}: asked once more on a new connection");
                assert_eq!(seen.load(Ordering::SeqCst), 2, "{method}");
            } else {
                let error = result.unwrap_err();
                assert!(error.contains("may or may not have been executed"), "{method}: {error}");
                assert_eq!(seen.load(Ordering::SeqCst), 1, "{method}: not sent again");
                assert!(remote.conn.is_none(), "{method}");
                assert_eq!(remote.call("session.inspect", json!({})), Ok(json!({"call": "own"})), "{method}: the next call uses a new connection");
            }
        }
    }

    /// Fake app that reads the request on each of its first `silent` connections and never
    /// answers it; later connections get their own answers. Counts every request it reads.
    fn app_silent_on(silent: usize) -> (Remote, Arc<AtomicUsize>) {
        let (listener, remote) = listener_and_client();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for _ in 0..silent {
                let Some((sock, mut reader)) = accept(&listener) else { return };
                if next_request(&mut reader).is_some() {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                // Keep the connection open and silent.
                held.push((sock, reader));
            }
            serve_own_answers(&listener, &counter);
        });
        (remote, seen)
    }

    /// A call that changes state and gets no reply in time may have run: it is not sent again, the
    /// error says so, and the connection is dropped so its late reply is never read.
    #[test]
    fn a_timed_out_mutation_is_unconfirmed_and_not_sent_again() {
        let (mut remote, seen) = app_silent_on(1);
        let error = remote.call("engine.execute", json!({"command": "track.new"})).unwrap_err();
        assert!(error.contains("may or may not have been executed"), "{error}");
        assert!(remote.conn.is_none());
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    /// A call that only reads state is asked once more on a new connection after a timeout.
    #[test]
    fn a_timed_out_read_is_asked_once_more_on_a_new_connection() {
        let (mut remote, seen) = app_silent_on(1);
        assert_eq!(remote.call("session.inspect", json!({})), Ok(json!({"call": "own"})));
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }

    /// When the second attempt times out too, the call fails after exactly two requests and no
    /// connection is kept.
    #[test]
    fn a_read_that_times_out_twice_fails_and_keeps_no_connection() {
        let (mut remote, seen) = app_silent_on(2);
        assert!(remote.call("session.inspect", json!({})).is_err());
        assert!(remote.conn.is_none());
        assert_eq!(seen.load(Ordering::SeqCst), 2);
        assert_eq!(remote.call("session.inspect", json!({})), Ok(json!({"call": "own"})), "the next call starts clean");
    }

    /// A call that changes state and could not be written to a dead connection is sent on a new
    /// one. When that attempt goes out and its reply is lost, the call may have run: the error
    /// says so, as it does when the first attempt is the one that went out.
    #[test]
    fn a_mutation_sent_on_the_second_attempt_and_not_answered_is_unconfirmed() {
        use std::io::Read;
        let (listener, mut remote) = listener_and_client();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            // Closing a connection with bytes still unread resets it, so the client's next write
            // on it fails.
            let Some((sock, reader)) = accept(&listener) else { return };
            std::thread::sleep(Duration::from_millis(50));
            drop((sock, reader));
            // The second attempt arrives on a new connection: read it and close without a reply.
            let Some((sock, mut reader)) = accept(&listener) else { return };
            if next_request(&mut reader).is_some() {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            drop((sock, reader));
        });
        let (stream, _) = remote.connection().unwrap();
        writeln!(stream, "bytes the app never reads").unwrap();
        // Wait until the reset has arrived (the read stops timing out).
        let mut byte = [0u8; 1];
        for _ in 0..20 {
            match stream.read(&mut byte) {
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {}
                _ => break,
            }
        }
        let error = remote.call("engine.execute", json!({"command": "track.new"})).unwrap_err();
        assert_eq!(seen.load(Ordering::SeqCst), 1, "the call went out once, on the second connection: {error}");
        assert!(error.contains("may or may not have been executed"), "{error}");
        assert!(remote.conn.is_none());
    }

    /// Calls that are answered in turn share one connection, and each gets its own answer.
    #[test]
    fn answered_calls_share_one_connection() {
        let (listener, mut remote) = listener_and_client();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();
        std::thread::spawn(move || {
            while let Some((mut sock, mut reader)) = accept(&listener) {
                counter.fetch_add(1, Ordering::SeqCst);
                while let Some(request) = next_request(&mut reader) {
                    let echo = json!({"method": request["method"], "id": request["id"]});
                    answer(&mut sock, &request, echo);
                }
            }
        });
        for (n, method) in ["session.inspect", "engine.execute", "ui.inspect"].into_iter().enumerate() {
            assert_eq!(remote.call(method, json!({})), Ok(json!({"method": method, "id": n + 1})));
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }
}
