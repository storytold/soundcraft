//! The host bridge's decisions, kept free of `web_sys` so they are tested natively: which parent
//! origin the bridge may talk to, and whether an opened document fits.

/// The origin the bridge posts to and hears from, or `None` for no bridge (saves download).
///
/// - `own`: this page's origin.
/// - `parent`: the parent window's origin, `None` when reading it threw (a cross-origin parent).
/// - `allowed`: the `?host_origin=` query value, an origin the embedding page vouches for.
///
/// A same-origin parent is accepted. A cross-origin parent is accepted only when `allowed` is a
/// valid URL origin (`scheme://host[:port]`, nothing else); messages then go to that exact origin,
/// so the browser drops them if the parent is anything else. Opaque origins (`"null"`) never match.
pub fn target_origin(own: &str, parent: Option<&str>, allowed: Option<&str>) -> Option<String> {
    if let Some(parent) = parent
        && parent == own
        && is_origin(own)
    {
        return Some(own.to_string());
    }
    canonical_origin(allowed?.trim())
}

/// `s` as a browser serialises an origin (lowercase scheme and host, no default port), or `None`
/// when it isn't one [`is_origin`] accepts. `MessageEvent.origin` is compared with this string
/// exactly, so `https://Files.Example:443` must become `https://files.example` to ever match.
pub fn canonical_origin(s: &str) -> Option<String> {
    if !is_origin(s) {
        return None;
    }
    let (scheme, rest) = s.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    match (scheme.as_str(), port) {
        ("https", Some(443)) | ("http", Some(80)) | (_, None) => Some(format!("{scheme}://{host}")),
        (_, Some(port)) => Some(format!("{scheme}://{host}:{port}")),
    }
}

/// True for a URL origin: `http(s)://host[:port]` with no path, query, fragment or credentials,
/// and only in forms whose browser serialisation [`canonical_origin`] can produce: an ASCII host
/// (an IDN must be given in punycode, `xn--…`), and a numeric host only as a dotted-decimal IPv4.
pub fn is_origin(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else { return false };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) || rest.is_empty() {
        return false;
    }
    // IPv6 literals aren't accepted; name the host instead.
    if rest.contains(['/', '?', '#', '@', ' ', '\\', '[', ']']) {
        return false;
    }
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')) {
        return false;
    }
    // Browsers rewrite numeric hosts (`0x7f.1`, `127.1`, `010.0.0.1`) to dotted decimal; accept
    // only the form they would produce.
    let last = host.trim_end_matches('.').rsplit('.').next().unwrap_or("");
    if last.bytes().all(|b| b.is_ascii_digit()) || last.to_ascii_lowercase().starts_with("0x") {
        let parts: Vec<&str> = host.split('.').collect();
        let decimal = |p: &&str| !p.is_empty() && (*p == "0" || !p.starts_with('0')) && p.parse::<u8>().is_ok();
        if parts.len() != 4 || !parts.iter().all(decimal) {
            return false;
        }
    }
    match port {
        Some(p) => p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok(),
        None => true,
    }
}

/// Refuse `soundcraft:open` bytes (`len`) when they, plus the `queued` bytes still waiting for the
/// next frame, exceed `cap`. Checked before the bytes are copied; draining the queue frees room.
pub fn check_open(name: &str, queued: f64, len: f64, cap: u64) -> Result<(), String> {
    if queued + len > cap as f64 {
        return Err(format!("{name}: file is too large to open ({len} bytes, {queued} already waiting; the limit is {cap} bytes)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &str = "https://app.example";

    #[test]
    fn same_origin_parent_enables_the_bridge() {
        assert_eq!(target_origin(OWN, Some(OWN), None).as_deref(), Some(OWN));
    }

    #[test]
    fn cross_origin_parent_disables_the_bridge() {
        // Reading a cross-origin parent's origin throws (None) or names another origin.
        assert_eq!(target_origin(OWN, None, None), None);
        assert_eq!(target_origin(OWN, Some("https://evil.example"), None), None);
        assert_eq!(target_origin(OWN, Some("null"), None), None);
        assert_eq!(target_origin("null", Some("null"), None), None);
    }

    #[test]
    fn host_origin_parameter_must_be_an_exact_origin() {
        let files = "https://files.example:8443";
        assert_eq!(target_origin(OWN, None, Some(files)).as_deref(), Some(files));
        for bad in
            ["*", "null", "files.example", "https://files.example/", "https://a@files.example", "javascript://x", "https://", "https://x:99999"]
        {
            assert_eq!(target_origin(OWN, None, Some(bad)), None, "{bad}");
        }
    }

    #[test]
    fn host_origin_is_stored_as_the_browser_serialises_it() {
        // `MessageEvent.origin` is lowercase with no default port; the stored origin must match it.
        for (given, stored) in [
            ("https://Files.Example", "https://files.example"),
            ("HTTPS://FILES.EXAMPLE", "https://files.example"),
            ("https://files.example:443", "https://files.example"),
            ("http://files.example:80", "http://files.example"),
            ("http://files.example:443", "http://files.example:443"),
            ("https://files.example:80", "https://files.example:80"),
            ("https://files.example:08443", "https://files.example:8443"),
            ("https://xn--bcher-kva.example", "https://xn--bcher-kva.example"),
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("  https://Files.Example:443  ", "https://files.example"),
        ] {
            assert_eq!(target_origin(OWN, None, Some(given)).as_deref(), Some(stored), "{given}");
        }
    }

    #[test]
    fn host_origin_a_browser_would_serialise_differently_is_refused() {
        for bad in [
            "https://b\u{fc}cher.example",
            "https://files.example:",
            "https://0x7f.0.0.1",
            "https://127.1",
            "https://010.0.0.1",
            "https://256.0.0.1",
            "https://files%2eexample",
            "https://[::1]",
        ] {
            assert_eq!(target_origin(OWN, None, Some(bad)), None, "{bad}");
        }
    }

    #[test]
    fn open_cap_counts_documents_still_waiting() {
        assert!(check_open("a.scraftz", 0.0, 100.0, 100).is_ok());
        let e = check_open("a.scraftz", 0.0, 101.0, 100).unwrap_err();
        assert!(e.contains("too large") && e.contains("a.scraftz"));
        // Two documents that each fit are refused together until the first is drained.
        assert!(check_open("b.mid", 60.0, 60.0, 100).is_err());
        assert!(check_open("b.mid", 0.0, 60.0, 100).is_ok());
    }
}
