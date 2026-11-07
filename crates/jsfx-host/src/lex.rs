//! EEL2 lexer. Identifiers are case-insensitive (lower-cased here).

use crate::JsfxError;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    P(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub(crate) struct Lexed {
    pub tok: Tok,
    pub line: u32,
}

const P3: &[&str] = &["===", "!=="];
const P2: &[&str] = &["==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=", "-=", "*=", "/=", "%=", "^=", "|=", "&=", "~=", "\\="];
const P1: &[&str] = &["+", "-", "*", "/", "%", "^", "|", "&", "~", "!", "<", ">", "=", "?", ":", ";", ",", "(", ")", "[", "]", "\\"];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '#'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

fn lex_number(c: &[char], i: &mut usize) -> Option<f64> {
    let at = |k: usize| c.get(k).copied();
    if at(*i) == Some('0') && matches!(at(*i + 1), Some('x' | 'X')) && at(*i + 2).is_some_and(|d| d.is_ascii_hexdigit()) {
        let mut j = *i + 2;
        let mut v: u64 = 0;
        while let Some(d) = at(j).and_then(|d| d.to_digit(16)) {
            v = v.wrapping_mul(16).wrapping_add(u64::from(d));
            j += 1;
        }
        *i = j;
        return Some(v as f64);
    }
    let mut s = String::new();
    let mut j = *i;
    while let Some(d) = at(j).filter(char::is_ascii_digit) {
        s.push(d);
        j += 1;
    }
    if at(j) == Some('.') {
        s.push('.');
        j += 1;
        while let Some(d) = at(j).filter(char::is_ascii_digit) {
            s.push(d);
            j += 1;
        }
    }
    if matches!(at(j), Some('e' | 'E')) {
        let mut k = j + 1;
        let sign = at(k).filter(|&x| x == '+' || x == '-');
        if sign.is_some() {
            k += 1;
        }
        if at(k).is_some_and(|d| d.is_ascii_digit()) {
            s.push('e');
            if let Some(sg) = sign {
                s.push(sg);
            }
            while let Some(d) = at(k).filter(char::is_ascii_digit) {
                s.push(d);
                k += 1;
            }
            j = k;
        }
    }
    *i = j;
    s.parse::<f64>().ok()
}

pub(crate) fn lex(src: &str, first_line: u32) -> Result<Vec<Lexed>, JsfxError> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = first_line;
    let mut out = Vec::new();
    let err = |line: u32, msg: &str| JsfxError::Syntax(format!("line {line}: {msg}"));
    while let Some(&ch) = c.get(i) {
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'/') {
            while c.get(i).is_some_and(|&x| x != '\n') {
                i += 1;
            }
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < c.len() && !(c.get(i) == Some(&'*') && c.get(i + 1) == Some(&'/')) {
                if c.get(i) == Some(&'\n') {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(char::is_ascii_digit)) {
            // A leading '.' number: lex_number needs the integer part to be optional.
            let n = lex_number(&c, &mut i).ok_or_else(|| err(line, "bad number"))?;
            out.push(Lexed { tok: Tok::Num(n), line });
            continue;
        }
        if is_ident_start(ch) {
            let mut s = String::new();
            while let Some(&x) = c.get(i).filter(|&&x| is_ident_char(x) || (s.is_empty() && is_ident_start(x))) {
                s.push(x.to_ascii_lowercase());
                i += 1;
            }
            out.push(Lexed { tok: Tok::Ident(s), line });
            continue;
        }
        if ch == '"' {
            i += 1;
            let mut text = String::new();
            while let Some(&x) = c.get(i).filter(|&&x| x != '"') {
                if x == '\n' {
                    line += 1;
                }
                text.push(x);
                i += 1;
            }
            i += 1;
            out.push(Lexed { tok: Tok::Str(text), line });
            continue;
        }
        if ch == '\'' {
            // Character constant: up to four characters packed big-endian, e.g. 'b' or 'abcd'.
            let mut j = i + 1;
            let mut v: u64 = 0;
            while let Some(&x) = c.get(j).filter(|&&x| x != '\'' && x != '\n') {
                v = (v << 8) | (u64::from(u32::from(x)) & 0xff);
                j += 1;
                if j - i > 5 {
                    return Err(err(line, "character constant too long"));
                }
            }
            if c.get(j) != Some(&'\'') {
                return Err(err(line, "unterminated character constant"));
            }
            out.push(Lexed { tok: Tok::Num(v as f64), line });
            i = j + 1;
            continue;
        }
        if ch == '$' {
            if c.get(i + 1) == Some(&'\'') && c.get(i + 3) == Some(&'\'') {
                let v = c.get(i + 2).map_or(0, |&x| x as u32);
                out.push(Lexed { tok: Tok::Num(f64::from(v)), line });
                i += 4;
                continue;
            }
            let mut j = i + 1;
            let mut name = String::new();
            while let Some(&x) = c.get(j).filter(|x| x.is_ascii_alphanumeric()) {
                name.push(x.to_ascii_lowercase());
                j += 1;
            }
            let v = match name.as_str() {
                "pi" => std::f64::consts::PI,
                "e" => std::f64::consts::E,
                "phi" => 1.618_033_988_749_895,
                n if n.starts_with('x') && n.len() > 1 => {
                    u64::from_str_radix(n.get(1..).unwrap_or(""), 16).map_err(|_| err(line, "bad $x constant"))? as f64
                }
                _ => return Err(err(line, "unknown $ constant")),
            };
            out.push(Lexed { tok: Tok::Num(v), line });
            i = j;
            continue;
        }
        let rest3: String = c.get(i..(i + 3).min(c.len())).unwrap_or(&[]).iter().collect();
        let rest2: String = rest3.chars().take(2).collect();
        let rest1: String = rest3.chars().take(1).collect();
        if let Some(p) = P3.iter().find(|p| **p == rest3) {
            out.push(Lexed { tok: Tok::P(p), line });
            i += 3;
        } else if let Some(p) = P2.iter().find(|p| **p == rest2) {
            out.push(Lexed { tok: Tok::P(p), line });
            i += 2;
        } else if let Some(p) = P1.iter().find(|p| **p == rest1) {
            out.push(Lexed { tok: Tok::P(p), line });
            i += 1;
        } else {
            return Err(err(line, &format!("unexpected character '{ch}'")));
        }
    }
    out.push(Lexed { tok: Tok::Eof, line });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s, 1).map(|v| v.into_iter().map(|l| l.tok).collect()).unwrap_or_default()
    }

    #[test]
    fn numbers_and_operators() {
        assert_eq!(toks("x+=1.5e2;"), vec![Tok::Ident("x".into()), Tok::P("+="), Tok::Num(150.0), Tok::P(";"), Tok::Eof]);
        assert_eq!(toks("0x10 .5 $pi")[..3], [Tok::Num(16.0), Tok::Num(0.5), Tok::Num(std::f64::consts::PI)]);
        assert_eq!(toks("a===b !== c")[1], Tok::P("==="));
    }

    #[test]
    fn comments_and_case() {
        assert_eq!(toks("// hi\nFoo /* x\ny */ Bar"), vec![Tok::Ident("foo".into()), Tok::Ident("bar".into()), Tok::Eof]);
    }

    #[test]
    fn bad_character_is_an_error() {
        assert!(lex("a @ b", 1).is_err());
    }
}
