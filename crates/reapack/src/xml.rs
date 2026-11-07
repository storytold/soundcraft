//! A small, strict XML event reader for ReaPack indexes: elements, attributes, text, CDATA,
//! entities. Comments, processing instructions and a plain DOCTYPE are skipped; a DOCTYPE with an
//! internal subset (where entity bombs live) is refused. Nesting depth is capped.

use crate::ReapackError;

const MAX_DEPTH: usize = 64;
const MAX_ATTRS: usize = 32;

pub(crate) enum Ev<'a> {
    Start {
        name: &'a str,
        attrs: Vec<(String, String)>,
    },
    End(&'a str),
    /// Character data, entities decoded; whitespace-only runs are not reported.
    Text(String),
}

fn err(msg: &str) -> ReapackError {
    ReapackError::Parse(msg.to_string())
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')
}

fn after(s: &str, n: usize) -> &str {
    s.get(n..).unwrap_or("")
}

/// Replaces the five predefined entities and numeric character references; anything else is kept.
pub(crate) fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or(""));
        rest = after(rest, i);
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = after(rest, 1);
            continue;
        };
        let ent = rest.get(1..end).unwrap_or("");
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = after(rest, end + 1);
            }
            None => {
                out.push('&');
                rest = after(rest, 1);
            }
        }
    }
    out.push_str(rest);
    out
}

/// Feeds every event of `s` to `f`, stopping at the first error.
pub(crate) fn walk(s: &str, f: &mut dyn FnMut(Ev<'_>) -> Result<(), ReapackError>) -> Result<(), ReapackError> {
    let mut rest = s;
    let mut depth = 0usize;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if !rest.trim().is_empty() {
                f(Ev::Text(decode(rest)))?;
            }
            break;
        };
        let text = rest.get(..lt).unwrap_or("");
        if !text.trim().is_empty() {
            f(Ev::Text(decode(text)))?;
        }
        rest = after(rest, lt);
        if let Some(t) = rest.strip_prefix("<!--") {
            let end = t.find("-->").ok_or_else(|| err("unterminated comment"))?;
            rest = after(t, end + 3);
        } else if let Some(t) = rest.strip_prefix("<![CDATA[") {
            let end = t.find("]]>").ok_or_else(|| err("unterminated CDATA"))?;
            f(Ev::Text(t.get(..end).unwrap_or("").to_string()))?;
            rest = after(t, end + 3);
        } else if let Some(t) = rest.strip_prefix("<?") {
            let end = t.find("?>").ok_or_else(|| err("unterminated processing instruction"))?;
            rest = after(t, end + 2);
        } else if let Some(t) = rest.strip_prefix("<!") {
            let end = t.find('>').ok_or_else(|| err("unterminated declaration"))?;
            if t.get(..end).unwrap_or("").contains('[') {
                return Err(err("DOCTYPE internal subsets are not supported"));
            }
            rest = after(t, end + 1);
        } else if let Some(t) = rest.strip_prefix("</") {
            let end = t.find('>').ok_or_else(|| err("unterminated end tag"))?;
            f(Ev::End(t.get(..end).unwrap_or("").trim()))?;
            depth = depth.saturating_sub(1);
            rest = after(t, end + 1);
        } else {
            let t = after(rest, 1);
            let name_len = t.find(|c: char| !is_name_char(c)).unwrap_or(t.len());
            if name_len == 0 {
                return Err(err("expected an element name"));
            }
            let name = t.get(..name_len).unwrap_or("");
            let mut p = after(t, name_len);
            let mut attrs = Vec::new();
            let empty;
            loop {
                p = p.trim_start();
                if let Some(r) = p.strip_prefix("/>") {
                    p = r;
                    empty = true;
                    break;
                }
                if let Some(r) = p.strip_prefix('>') {
                    p = r;
                    empty = false;
                    break;
                }
                let n = p.find(|c: char| !is_name_char(c)).unwrap_or(p.len());
                if n == 0 || attrs.len() >= MAX_ATTRS {
                    return Err(err("malformed attribute"));
                }
                let an = p.get(..n).unwrap_or("");
                p = after(p, n).trim_start().strip_prefix('=').ok_or_else(|| err("expected '=' after an attribute name"))?.trim_start();
                let q = p.chars().next().filter(|c| matches!(c, '"' | '\'')).ok_or_else(|| err("attribute value is not quoted"))?;
                p = after(p, 1);
                let end = p.find(q).ok_or_else(|| err("unterminated attribute value"))?;
                attrs.push((an.to_string(), decode(p.get(..end).unwrap_or(""))));
                p = after(p, end + 1);
            }
            f(Ev::Start { name, attrs })?;
            if empty {
                f(Ev::End(name))?;
            } else {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(err("elements nested too deeply"));
                }
            }
            rest = p;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(s: &str) -> Result<Vec<String>, ReapackError> {
        let mut out = Vec::new();
        walk(s, &mut |e| {
            out.push(match e {
                Ev::Start { name, attrs } => format!("<{name} {}>", attrs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")),
                Ev::End(n) => format!("</{n}>"),
                Ev::Text(t) => format!("'{t}'"),
            });
            Ok(())
        })?;
        Ok(out)
    }

    #[test]
    fn elements_attributes_text_and_entities() {
        let ev = events("<?xml version=\"1.0\"?><!-- c --><a x='1' y=\"a&amp;b&#65;&#x42;\"><b/>t &lt; u<![CDATA[<raw>]]></a>").unwrap();
        assert_eq!(ev, ["<a x=1,y=a&bAB>", "<b >", "</b>", "'t < u'", "'<raw>'", "</a>"]);
    }

    #[test]
    fn unknown_entities_survive() {
        assert_eq!(decode("a &nbsp; b & c &#xZZ; &#1114112;"), "a &nbsp; b & c &#xZZ; &#1114112;");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        for bad in ["<a", "<a x=1>", "<a x='1>", "<a><!-- x", "<a><![CDATA[", "<>", "<a =1>", "<!DOCTYPE a [<!ENTITY x 'y'>]><a/>"] {
            assert!(events(bad).is_err(), "{bad}");
        }
        assert!(events(&format!("{}{}", "<a>".repeat(200), "</a>".repeat(200))).is_err());
        // Non-ASCII text and a stray '<' at the very end do not trip the slicing.
        assert!(events("<a>héllo ✓</a><").is_err());
        assert_eq!(events("<a>héllo ✓</a>").unwrap().len(), 3);
    }
}
