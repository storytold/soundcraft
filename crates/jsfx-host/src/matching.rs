//! EEL2's `match(needle, haystack, ...)` / `matchi`: the whole haystack must fit the needle,
//! where `*` matches any run of characters, `?` one, `+` one or more, and `%d %u %i %x %X %f %e %g
//! %c %s` (optionally with a width, `%4d`) capture what they cover into the extra arguments.
//! Backtracking is bounded, so a hostile pattern cannot hang the host, and matching works in
//! buffers allocated once ([`MatchScratch`]), so it can run on the audio thread.

/// What one `%` conversion covered: its kind (`b'd'`, `b's'`...) and where it is in the haystack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Capture {
    pub kind: u8,
    pub start: usize,
    pub len: usize,
}

/// Reusable buffers for [`matches`].
pub(crate) struct MatchScratch {
    elems: Vec<Elem>,
    pub caps: Vec<Capture>,
    /// Copies of the needle and haystack (the strings they come from may be written to).
    pub needle: Vec<u8>,
    pub hay: Vec<u8>,
}

impl MatchScratch {
    pub fn new(max_text: usize) -> MatchScratch {
        MatchScratch {
            elems: Vec::with_capacity(MAX_ELEMENTS + 1),
            caps: Vec::with_capacity(MAX_ELEMENTS + 1),
            needle: Vec::with_capacity(max_text),
            hay: Vec::with_capacity(max_text),
        }
    }
}

/// Most matching steps before giving up (no match).
const MAX_STEPS: usize = 100_000;
/// Longest needle, in pattern elements.
const MAX_ELEMENTS: usize = 256;

#[derive(Debug, Clone, Copy)]
enum Elem {
    Lit(u8),
    /// `*` (0..), `?` (exactly 1), `+` (1..).
    Any {
        min: usize,
        max: Option<usize>,
    },
    Conv {
        kind: u8,
        width: Option<usize>,
    },
}

fn parse(needle: &[u8], out: &mut Vec<Elem>) -> Option<()> {
    out.clear();
    let mut i = 0;
    while let Some(&c) = needle.get(i) {
        i += 1;
        let e = match c {
            b'*' => Elem::Any { min: 0, max: None },
            b'?' => Elem::Any { min: 1, max: Some(1) },
            b'+' => Elem::Any { min: 1, max: None },
            b'%' => {
                let start = i;
                while needle.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let width = std::str::from_utf8(needle.get(start..i)?).ok().and_then(|w| w.parse().ok()).filter(|w| *w > 0);
                let kind = *needle.get(i)?;
                i += 1;
                match kind {
                    b'%' => Elem::Lit(b'%'),
                    b'd' | b'u' | b'i' | b'x' | b'X' | b'f' | b'e' | b'g' | b'c' | b's' => Elem::Conv { kind, width },
                    _ => return None,
                }
            }
            _ => Elem::Lit(c),
        };
        out.push(e);
        if out.len() > MAX_ELEMENTS {
            return None;
        }
    }
    Some(())
}

/// The value of `text` as conversion `kind` (0 for `%s`), `None` when it is not one.
pub(crate) fn convert(kind: u8, text: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(text).ok()?;
    let num = Some;
    match kind {
        b's' => (!text.is_empty()).then_some(0.0),
        b'c' => text.first().filter(|_| text.len() == 1).map(|c| f64::from(*c)),
        b'd' | b'i' => {
            let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            num(s.parse::<f64>().ok()?)
        }
        b'u' => {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            num(s.parse::<f64>().ok()?)
        }
        b'x' | b'X' => {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) || s.len() > 13 {
                return None;
            }
            num(u64::from_str_radix(s, 16).ok()? as f64)
        }
        _ => {
            // Floats: digits are required; Rust's parser would also take "inf" and "nan".
            if !s.bytes().any(|b| b.is_ascii_digit()) || !s.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) {
                return None;
            }
            num(s.parse::<f64>().ok()?)
        }
    }
}

struct Matcher<'a> {
    elems: &'a [Elem],
    hay: &'a [u8],
    fold: bool,
    steps: usize,
    caps: &'a mut Vec<Capture>,
}

impl Matcher<'_> {
    fn go(&mut self, e: usize, h: usize) -> bool {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return false;
        }
        let rest = self.hay.len() - h;
        let Some(&elem) = self.elems.get(e) else { return rest == 0 };
        match elem {
            Elem::Lit(c) => {
                let same = |a: u8, b: u8| if self.fold { a.eq_ignore_ascii_case(&b) } else { a == b };
                self.hay.get(h).is_some_and(|&x| same(x, c)) && self.go(e + 1, h + 1)
            }
            Elem::Any { min, max } => {
                let most = max.unwrap_or(rest).min(rest);
                (min..=most).rev().any(|n| self.go(e + 1, h + n))
            }
            Elem::Conv { kind, width } => {
                let (lo, hi) = match (width, kind) {
                    (Some(w), _) => (w, w),
                    (None, b'c') => (1, 1),
                    (None, _) => (1, rest),
                };
                for n in (lo..=hi.min(rest)).rev() {
                    // Converting a candidate reads it: that work counts too.
                    self.steps += n;
                    if self.steps > MAX_STEPS {
                        return false;
                    }
                    if self.hay.get(h..h + n).and_then(|t| convert(kind, t)).is_none() {
                        continue;
                    }
                    let mark = self.caps.len();
                    self.caps.push(Capture { kind, start: h, len: n });
                    if self.go(e + 1, h + n) {
                        return true;
                    }
                    self.caps.truncate(mark);
                    if self.steps > MAX_STEPS {
                        return false;
                    }
                }
                false
            }
        }
    }
}

/// Whether `scratch.hay` fits `scratch.needle` (ignoring ASCII case when `fold`), and the work
/// that took in steps (at most a little over `MAX_STEPS`); the conversions' captures are then
/// in `scratch.caps`.
pub(crate) fn matches(scratch: &mut MatchScratch, fold: bool) -> (bool, usize) {
    scratch.caps.clear();
    let MatchScratch { elems, caps, needle, hay } = scratch;
    if parse(needle, elems).is_none() {
        return (false, 0);
    }
    let mut m = Matcher { elems, hay, fold, steps: 0, caps };
    let hit = m.go(0, 0);
    (hit, m.steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The captures as numbers, or the text for `%s`.
    fn run(needle: &[u8], hay: &[u8], fold: bool) -> Option<Vec<String>> {
        let mut sc = MatchScratch::new(64);
        sc.needle.extend_from_slice(needle);
        sc.hay.extend_from_slice(hay);
        if !matches(&mut sc, fold).0 {
            return None;
        }
        Some(
            sc.caps
                .iter()
                .map(|c| {
                    let t = &sc.hay[c.start..c.start + c.len];
                    if c.kind == b's' { String::from_utf8_lossy(t).into_owned() } else { convert(c.kind, t).unwrap_or(f64::NAN).to_string() }
                })
                .collect(),
        )
    }

    #[test]
    fn wildcards_and_literals() {
        assert!(run(b"*.wav", b"kick.wav", false).is_some());
        assert!(run(b"*.wav", b"kick.WAV", false).is_none());
        assert!(run(b"*.wav", b"kick.WAV", true).is_some());
        assert!(run(b"a?c", b"abc", false).is_some());
        assert!(run(b"a+c", b"ac", false).is_none());
        assert!(run(b"abc", b"abcd", false).is_none());
    }

    #[test]
    fn conversions_capture() {
        assert_eq!(run(b"%ddB", b"12dB", false), Some(vec!["12".to_string()]));
        assert_eq!(run(b"%ddB", b"-6dB", false), Some(vec!["-6".to_string()]));
        assert_eq!(run(b"%ddB", b"xdB", false), None);
        assert_eq!(run(b"%s=%f", b"gain=0.5", false), Some(vec!["gain".to_string(), "0.5".to_string()]));
        assert_eq!(run(b"%2d%2d", b"1234", false), Some(vec!["12".to_string(), "34".to_string()]));
        assert_eq!(run(b"#%x", b"#ff", false), Some(vec!["255".to_string()]));
    }

    #[test]
    fn hostile_patterns_give_up() {
        assert!(run(b"*a*a*a*a*a*a*a*a*a*a*a*a*b", &[b'a'; 60], false).is_none());
    }
}
