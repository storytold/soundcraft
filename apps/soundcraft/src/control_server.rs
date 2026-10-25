//! Loopback JSON-lines control server: one request per line, one reply per line.
//! This is the transport the MCP server (`soundcraft-cli mcp --connect PORT`) wraps.
//!
//! Lines are bounded ([`MAX_LINE`]): a longer one is refused and the connection closed, without
//! buffering the rest. So is the number of connections served at once ([`MAX_CONNECTIONS`]).

use serde_json::{Value, json};
use soundcraft_ui_egui::ControlRequest;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

/// Longest accepted request line (bytes, without the newline). The largest payload a request
/// carries is a plugin state for `mix.insert_state`: base64 of at most `b64::MAX_DECODED_BYTES`
/// (64 MiB, so about 85 MiB of text). Room for that and 1 MiB of JSON around it; replies
/// (screenshots) aren't bounded by this.
const MAX_LINE: usize = soundcraft_engine::model::b64::MAX_DECODED_BYTES.div_ceil(3) * 4 + (1 << 20);
// With MAX_CONNECTIONS, at most 1.5 GiB of request lines are buffered at once.
const _: () = assert!(MAX_LINE <= 96 << 20);
/// Connections served at once; further ones get an error line and are closed.
const MAX_CONNECTIONS: usize = 16;
/// Requests that may wait for the UI thread. Each connection handles one request at a time, so
/// this matches [`MAX_CONNECTIONS`] and makes the transport's bound explicit instead of relying
/// on an unbounded channel.
const MAX_QUEUED_REQUESTS: usize = MAX_CONNECTIONS;

/// Bind the loopback control listener and return the request receiver. Port 0 picks a free one and the actual port is
/// printed; bind or spawn failures are logged instead of panicking.
pub fn start(port: u16, ctx: egui::Context) -> Receiver<ControlRequest> {
    let (tx, rx) = sync_channel::<ControlRequest>(MAX_QUEUED_REQUESTS);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return rx;
        }
    };
    let actual = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    log::info!("control server listening on 127.0.0.1:{actual}");
    println!("SOUNDCRAFT_CONTROL_PORT={actual}");
    let spawned = std::thread::Builder::new().name("soundcraft-control".into()).spawn(move || accept_loop(listener, tx, ctx));
    if let Err(e) = spawned {
        log::error!("control thread failed: {e}");
    }
    rx
}

/// A connection's place among [`MAX_CONNECTIONS`], given back when the connection ends (also when
/// its thread can't be started, or panics).
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accept until the listener closes, admitting at most [`MAX_CONNECTIONS`] peers; excess peers get one error line.
/// Each admitted connection is served on its own thread, releasing its slot on exit.
fn accept_loop(listener: TcpListener, tx: SyncSender<ControlRequest>, ctx: egui::Context) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let slot = Slot(Arc::clone(&open));
        let tx = tx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("soundcraft-control-conn".into()).spawn(move || {
            let _slot = slot;
            serve(stream, tx, ctx);
        });
    }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err(format!("request line longer than {max} bytes; closing the connection"));
    }
    // Last line without a newline, then end of stream.
    Ok(true)
}

/// Answer newline-delimited JSON requests for one connection: bound each line, forward it to the UI thread with a 90
/// s reply deadline, and stop at EOF, invalid UTF-8 or a closed request queue. Bad JSON still gets an error reply.
fn serve(stream: TcpStream, tx: SyncSender<ControlRequest>, ctx: egui::Context) {
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        match read_bounded_line(&mut reader, &mut buf, MAX_LINE) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"ok": false, "error": e}));
                break;
            }
        }
        // Bytes that aren't UTF-8 end the connection, as `BufRead::lines` did.
        let Ok(line) = std::str::from_utf8(&buf) else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                let (req, rrx) = ControlRequest::new(method, params);
                match tx.try_send(req) {
                    Ok(()) => {
                        ctx.request_repaint();
                        let mut r = rrx.recv_timeout(Duration::from_secs(90)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}));
                        if let Some(o) = r.as_object_mut() {
                            o.insert("id".into(), id);
                        }
                        r
                    }
                    // Refuse rather than grow or block: the UI drains a bounded batch each tick.
                    Err(TrySendError::Full(_)) => json!({"ok": false, "id": id, "error": "control queue full"}),
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
            Err(e) => json!({"ok": false, "error": format!("bad JSON: {e}")}),
        };
        if writeln!(out, "{reply}").is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_up_to_the_limit() {
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(false), "then end of stream");
    }

    #[test]
    fn an_overlong_line_is_refused_without_reading_the_rest() {
        let mut buf = Vec::new();
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert!(read_bounded_line(&mut input, &mut buf, 10).is_err());
        assert!(buf.len() <= 11, "at most limit + 1 bytes are buffered, got {}", buf.len());
        assert_eq!(input.len(), 100 - 11, "the rest of the line is left unread");
    }

    #[test]
    fn the_largest_plugin_state_fits_in_a_line() {
        let state = soundcraft_engine::model::b64::MAX_DECODED_BYTES.div_ceil(3) * 4;
        let request = json!({"id": 1, "method": "engine.execute", "params": {"command": "mix.insert_state", "params": {"track": "Audio 1", "slot": 0, "state": ""}}});
        assert!(state + request.to_string().len() <= MAX_LINE);
    }

    /// The control server on a loopback port, with a stand-in UI that echoes each method.
    fn serve_echo() -> std::net::SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = sync_channel::<ControlRequest>(MAX_QUEUED_REQUESTS);
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default()));
        std::thread::spawn(move || {
            for req in rx {
                let _ = req.reply.send(json!({"ok": true, "method": req.method}));
            }
        });
        addr
    }

    #[test]
    fn requests_are_answered_with_their_ids() {
        let addr = serve_echo();
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":3,\"method\":\"ping\"}\r\n\n{\"id\":4,\"method\":\"pong\"}\n").unwrap();
        let mut r = BufReader::new(c);
        for (id, method) in [(3, "ping"), (4, "pong")] {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["id"].as_i64(), v["method"].as_str()), (Some(id), Some(method)), "{line}");
        }
    }

    #[test]
    fn extra_connections_are_refused() {
        let addr = serve_echo();
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }
}
