//! Downloading. The default fetcher runs the system `curl`, restricted to HTTPS: this keeps a TLS
//! stack (and its C dependencies) out of the workspace, which has to build on FreeBSD, riscv64 and
//! the web. Swapping in a native HTTP client later only means another [`Fetch`] implementation.

use crate::ReapackError;
use std::io::Read;
use std::process::{Command, Stdio};

/// Gets the bytes behind an `https://` URL, failing when they exceed `max_bytes`.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, ReapackError>;
}

impl<T: Fetch + ?Sized> Fetch for std::sync::Arc<T> {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, ReapackError> {
        (**self).get(url, max_bytes)
    }
}

/// Downloads with the system `curl`.
pub struct CurlFetch;

const CONNECT_TIMEOUT_S: &str = "15";
const TOTAL_TIMEOUT_S: &str = "120";

impl Fetch for CurlFetch {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, ReapackError> {
        if !crate::valid_url(url) {
            return Err(ReapackError::Invalid(format!("not an https URL: {url}")));
        }
        let ua = concat!("SoundCraft/", env!("CARGO_PKG_VERSION"), " (ReaPack client)");
        let mut cmd = Command::new("curl");
        // `-q` must come first: it stops curl reading the user's ~/.curlrc, which could change
        // the protocol, proxy or output behaviour this code relies on.
        cmd.args(["-q", "--silent", "--show-error", "--fail", "--location", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "5"])
            .args(["--connect-timeout", CONNECT_TIMEOUT_S, "--max-time", TOTAL_TIMEOUT_S, "--user-agent", ua])
            .arg("--max-filesize")
            .arg(max_bytes.to_string())
            .arg("--")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| ReapackError::Network(format!("cannot run curl (is it installed?): {e}")))?;
        let mut body = Vec::new();
        if let Some(out) = child.stdout.take() {
            // `--max-filesize` only works when the server announces a size, so cap what we read too.
            if let Err(e) = out.take(max_bytes as u64 + 1).read_to_end(&mut body) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ReapackError::Network(format!("reading {url}: {e}")));
            }
        }
        if body.len() > max_bytes {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ReapackError::Invalid(format!("{url} is larger than {max_bytes} bytes")));
        }
        let mut msg = String::new();
        if let Some(err) = child.stderr.take() {
            let _ = err.take(2000).read_to_string(&mut msg);
        }
        let status = child.wait().map_err(|e| ReapackError::Network(e.to_string()))?;
        if status.success() { Ok(body) } else { Err(ReapackError::Network(format!("{url}: {}", msg.trim()))) }
    }
}
