//! `sprintf`-style formatting for EEL2 strings. Output goes into caller-owned buffers, so formatting
//! allocates nothing once they have their capacity.

use std::io::Write;

/// Longest string an effect can build; longer results are truncated.
pub(crate) const STR_MAX: usize = 4096;

#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    alt: bool,
    width: usize,
    prec: Option<usize>,
}

/// Rewrites Rust's `1.5e3` exponent as C does (`1.5e+03`).
fn c_exponent(body: &mut Vec<u8>, start: usize, upper: bool) {
    let Some(pos) = body.get(start..).and_then(|s| s.iter().position(|&b| b == b'e')).map(|p| p + start) else { return };
    let exp: i32 = body.get(pos + 1..).and_then(|e| std::str::from_utf8(e).ok()).and_then(|e| e.parse().ok()).unwrap_or(0);
    body.truncate(pos);
    let _ = write!(body, "{}{}{:02}", if upper { 'E' } else { 'e' }, if exp < 0 { '-' } else { '+' }, exp.abs());
}

/// Drops trailing zeros (and a bare point) from the fraction in `body[start..]`, before any exponent.
fn strip_zeros(body: &mut Vec<u8>, start: usize) {
    let tail_at = body.get(start..).and_then(|s| s.iter().position(|&b| b == b'e' || b == b'E')).map(|p| p + start);
    let tail: Vec<u8> = tail_at.map(|p| body.split_off(p)).unwrap_or_default();
    if body.get(start..).is_some_and(|s| s.contains(&b'.')) {
        while body.last() == Some(&b'0') {
            body.pop();
        }
        if body.last() == Some(&b'.') {
            body.pop();
        }
    }
    body.extend_from_slice(&tail);
}

fn float_body(body: &mut Vec<u8>, v: f64, conv: u8, spec: &Spec) {
    let prec = spec.prec;
    let upper = conv.is_ascii_uppercase();
    let start = body.len();
    if !v.is_finite() {
        body.extend_from_slice(if v.is_nan() { b"nan" } else { b"inf" });
        return;
    }
    match conv.to_ascii_lowercase() {
        b'f' => {
            let _ = write!(body, "{:.*}", prec.unwrap_or(6), v);
        }
        b'e' => {
            let _ = write!(body, "{:.*e}", prec.unwrap_or(6), v);
            c_exponent(body, start, upper);
        }
        _ => {
            // %g: the shorter of %e and %f at `p` significant digits, without trailing zeros.
            let p = prec.unwrap_or(6).max(1);
            let _ = write!(body, "{:.*e}", p - 1, v);
            let exp: i32 = body
                .get(start..)
                .and_then(|s| s.iter().position(|&b| b == b'e').map(|i| i + start))
                .and_then(|i| body.get(i + 1..))
                .and_then(|e| std::str::from_utf8(e).ok())
                .and_then(|e| e.parse().ok())
                .unwrap_or(0);
            body.truncate(start);
            if exp < -4 || exp >= p as i32 {
                let _ = write!(body, "{:.*e}", p - 1, v);
                c_exponent(body, start, upper);
            } else {
                let _ = write!(body, "{:.*}", (p as i32 - 1 - exp).max(0) as usize, v);
            }
            if !spec.alt {
                strip_zeros(body, start);
            }
        }
    }
}

/// Formats `fmt` with `args` (numbers; `%s` takes a string handle) into `out`. `body` is scratch.
pub(crate) fn format_into(out: &mut Vec<u8>, body: &mut Vec<u8>, fmt: &[u8], args: &[f64], strings: &[Vec<u8>]) {
    out.clear();
    let mut next = args.iter().copied();
    let mut i = 0;
    while let Some(&b) = fmt.get(i) {
        i += 1;
        if b != b'%' {
            out.push(b);
            continue;
        }
        let mut spec = Spec::default();
        while let Some(&f) = fmt.get(i) {
            match f {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'0' => spec.zero = true,
                b'#' => spec.alt = true,
                _ => break,
            }
            i += 1;
        }
        while let Some(d) = fmt.get(i).filter(|d| d.is_ascii_digit()) {
            spec.width = (spec.width * 10 + usize::from(d - b'0')).min(STR_MAX);
            i += 1;
        }
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            let mut p = 0usize;
            while let Some(d) = fmt.get(i).filter(|d| d.is_ascii_digit()) {
                p = (p * 10 + usize::from(d - b'0')).min(400);
                i += 1;
            }
            spec.prec = Some(p);
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        if conv == b'%' {
            out.push(b'%');
            continue;
        }
        body.clear();
        let mut sign = "";
        let mut numeric = true;
        match conv {
            b'd' | b'i' => {
                let v = next.next().unwrap_or(0.0) as i64;
                let _ = write!(body, "{}", v.unsigned_abs());
                if let Some(p) = spec.prec {
                    while body.len() < p {
                        body.insert(0, b'0');
                    }
                }
                sign = if v < 0 {
                    "-"
                } else if spec.plus {
                    "+"
                } else if spec.space {
                    " "
                } else {
                    ""
                };
            }
            b'u' => {
                let _ = write!(body, "{}", next.next().unwrap_or(0.0) as i64 as u64);
            }
            b'x' => {
                let _ = write!(body, "{:x}", next.next().unwrap_or(0.0) as i64 as u64);
            }
            b'X' => {
                let _ = write!(body, "{:X}", next.next().unwrap_or(0.0) as i64 as u64);
            }
            b'c' => {
                numeric = false;
                let c = next.next().unwrap_or(0.0) as i64 as u8;
                if c != 0 {
                    body.push(c);
                }
            }
            b's' => {
                numeric = false;
                let h = next.next().unwrap_or(0.0);
                if let Some(s) = (h >= 0.0).then(|| strings.get(h as usize)).flatten() {
                    body.extend_from_slice(s.get(..spec.prec.unwrap_or(usize::MAX).min(s.len())).unwrap_or(&[]));
                }
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let v = next.next().unwrap_or(0.0);
                float_body(body, v.abs(), conv, &spec);
                sign = if v.is_sign_negative() && !v.is_nan() {
                    "-"
                } else if spec.plus {
                    "+"
                } else if spec.space {
                    " "
                } else {
                    ""
                };
            }
            other => {
                // Unknown conversion: keep it literally.
                out.push(b'%');
                out.push(other);
                continue;
            }
        }
        let len = sign.len() + body.len();
        let pad = spec.width.saturating_sub(len);
        if spec.left {
            out.extend_from_slice(sign.as_bytes());
            out.extend_from_slice(body);
            out.resize(out.len() + pad, b' ');
        } else if spec.zero && numeric {
            out.extend_from_slice(sign.as_bytes());
            out.resize(out.len() + pad, b'0');
            out.extend_from_slice(body);
        } else {
            out.resize(out.len() + pad, b' ');
            out.extend_from_slice(sign.as_bytes());
            out.extend_from_slice(body);
        }
        if out.len() > STR_MAX {
            break;
        }
    }
    out.truncate(STR_MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(fmt: &str, args: &[f64]) -> String {
        let (mut out, mut body) = (Vec::new(), Vec::new());
        let strings = vec![b"zero".to_vec(), b"hello".to_vec()];
        format_into(&mut out, &mut body, fmt.as_bytes(), args, &strings);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn integers_and_flags() {
        assert_eq!(f("%d|%i|%u", &[42.0, -7.9, 3.0]), "42|-7|3");
        assert_eq!(f("[%5d][%-5d][%05d][%+d][% d]", &[42.0, 42.0, -42.0, 5.0, 5.0]), "[   42][42   ][-0042][+5][ 5]");
        assert_eq!(f("%x %X %.3d", &[255.0, 255.0, 7.0]), "ff FF 007");
        assert_eq!(f("%c%c", &[72.0, 105.0]), "Hi");
    }

    #[test]
    fn floats() {
        assert_eq!(f("%f", &[3.5]), "3.500000");
        assert_eq!(f("%.2f|%8.3f|%-8.1f|", &[1.23456, -2.0, 1.25]), "1.23|  -2.000|1.2     |");
        assert_eq!(f("%.0f", &[2.5]), "2");
        assert_eq!(f("%e", &[12345.678]), "1.234568e+04");
        assert_eq!(f("%.1E", &[0.00012]), "1.2E-04");
        assert_eq!(f("%f %f %f", &[f64::NAN, f64::INFINITY, f64::NEG_INFINITY]), "nan inf -inf");
    }

    #[test]
    fn general_format_matches_c() {
        for (v, want) in [
            (0.0, "0"),
            (100000.0, "100000"),
            (1_000_000.0, "1e+06"),
            (123456789.0, "1.23457e+08"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.23456789, "1.23457"),
            (0.5, "0.5"),
            (-2.50, "-2.5"),
        ] {
            assert_eq!(f("%g", &[v]), want, "{v}");
        }
        assert_eq!(f("%.3g", &[1234.5678]), "1.23e+03");
    }

    #[test]
    fn strings_and_literals() {
        assert_eq!(f("%s and %s", &[1.0, 0.0]), "hello and zero");
        assert_eq!(f("%.2s|%6s|%-6s|", &[1.0, 0.0, 0.0]), "he|  zero|zero  |");
        assert_eq!(f("100%% %z %s", &[99.0]), "100% %z ");
        assert_eq!(f("trailing %", &[]), "trailing ");
        assert_eq!(f("%d %d", &[1.0]), "1 0", "missing arguments read as 0");
    }

    #[test]
    fn output_is_capped() {
        assert_eq!(f("%9999d", &[1.0]).len(), STR_MAX);
        assert!(f(&"x".repeat(10_000), &[]).len() <= STR_MAX);
    }
}
