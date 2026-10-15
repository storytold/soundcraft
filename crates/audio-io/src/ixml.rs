//! iXML field-recorder metadata (the `iXML` chunk of a WAV, per the iXML specification v2).
//!
//! A small XML reader for the few elements we show: no DTDs, so no entities beyond the five
//! predefined ones and character references. A chunk it can't read is dropped whole, with a
//! warning; the audio around it is never affected.

use crate::{IxmlInfo, IxmlTrack};

/// Larger chunks are skipped unread. Recorders write a few KiB.
const MAX_LEN: usize = 1 << 20;
const MAX_DEPTH: usize = 64;

/// The iXML in chunk body `c`, or `None` (logged) when it is too large or not a readable iXML
/// document.
pub(crate) fn parse(c: &[u8]) -> Option<IxmlInfo> {
    if c.len() > MAX_LEN {
        log::warn!("iXML chunk of {} bytes ignored: larger than {MAX_LEN}", c.len());
        return None;
    }
    match read(c) {
        Ok(info) => Some(info),
        Err(why) => {
            log::warn!("iXML chunk ignored: {why}");
            None
        }
    }
}

fn read(c: &[u8]) -> Result<IxmlInfo, &'static str> {
    // Recorders pad the chunk with NULs to leave room for edits.
    let end = c.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
    let doc = String::from_utf8_lossy(c.get(..end).unwrap_or_default());
    let mut s = doc.trim_start_matches('\u{feff}');
    let mut stack: Vec<&str> = Vec::new();
    let mut text = String::new();
    let mut info = IxmlInfo::default();
    let mut track: Option<IxmlTrack> = None;
    let mut done = false;
    loop {
        let Some((before, rest)) = s.split_once('<') else {
            if !s.trim().is_empty() {
                return Err("text outside the root element");
            }
            break;
        };
        if stack.is_empty() {
            if !before.trim().is_empty() {
                return Err("text outside the root element");
            }
        } else {
            unescape(before, &mut text)?;
        }
        if let Some(r) = rest.strip_prefix("!--") {
            s = r.split_once("-->").ok_or("unterminated comment")?.1;
        } else if let Some(r) = rest.strip_prefix("![CDATA[") {
            let (cdata, r) = r.split_once("]]>").ok_or("unterminated CDATA section")?;
            if stack.is_empty() {
                return Err("CDATA outside the root element");
            }
            text.push_str(cdata);
            s = r;
        } else if let Some(r) = rest.strip_prefix('?') {
            s = r.split_once("?>").ok_or("unterminated processing instruction")?.1;
        } else if rest.starts_with('!') {
            return Err("document type declarations are not read");
        } else {
            let (tag, r) = split_tag(rest)?;
            s = r;
            if let Some(name) = tag.strip_prefix('/') {
                let open = stack.pop().ok_or("closing tag without an opening one")?;
                if open != name.trim_end() {
                    return Err("mismatched closing tag");
                }
                close(&stack, open, text.trim(), &mut info, &mut track);
                done = stack.is_empty();
            } else {
                let empty = tag.ends_with('/');
                let name = tag.trim_end_matches('/').split(|ch: char| ch.is_ascii_whitespace()).next().unwrap_or_default();
                if name.is_empty() {
                    return Err("element without a name");
                }
                if done {
                    return Err("more than one root element");
                }
                if stack.is_empty() && name != "BWFXML" {
                    return Err("the root element is not BWFXML");
                }
                if stack.len() >= MAX_DEPTH {
                    return Err("elements nested too deep");
                }
                stack.push(name);
                if stack.as_slice() == ["BWFXML", "TRACK_LIST", "TRACK"] {
                    track = Some(IxmlTrack::default());
                }
                if empty {
                    stack.pop();
                    close(&stack, name, "", &mut info, &mut track);
                    done = stack.is_empty();
                }
            }
            text.clear();
        }
    }
    if !done {
        return Err("the document ends early");
    }
    Ok(info)
}

/// Splits `rest` (just after a `<`) at the `>` that ends the tag, skipping quoted attribute values.
fn split_tag(rest: &str) -> Result<(&str, &str), &'static str> {
    let mut quote = None;
    for (i, ch) in rest.char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(q), _) if q == ch => quote = None,
            (None, '>') => return Ok((rest.get(..i).unwrap_or_default(), rest.get(i + 1..).unwrap_or_default())),
            _ => {}
        }
    }
    Err("unterminated tag")
}

/// Records the text of element `name`, closed inside `parents`.
fn close(parents: &[&str], name: &str, text: &str, info: &mut IxmlInfo, track: &mut Option<IxmlTrack>) {
    let set = |field: &mut String| {
        if field.is_empty() {
            *field = text.to_string();
        }
    };
    match (parents, name) {
        (["BWFXML"], "PROJECT") => set(&mut info.project),
        (["BWFXML"], "SCENE") => set(&mut info.scene),
        (["BWFXML"], "TAKE") => set(&mut info.take),
        (["BWFXML"], "TAPE") => set(&mut info.tape),
        (["BWFXML"], "NOTE") => set(&mut info.note),
        (["BWFXML"], "CIRCLED") => info.circled = text.eq_ignore_ascii_case("TRUE"),
        (["BWFXML", "TRACK_LIST"], "TRACK") => info.tracks.extend(track.take()),
        (["BWFXML", "TRACK_LIST", "TRACK"], field) => {
            if let Some(t) = track.as_mut() {
                match field {
                    "CHANNEL_INDEX" => t.channel_index = text.parse().ok(),
                    "INTERLEAVE_INDEX" => t.interleave_index = text.parse().ok(),
                    "NAME" => t.name = text.to_string(),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Appends `s` to `out`, resolving the predefined entities and character references.
fn unescape(s: &str, out: &mut String) -> Result<(), &'static str> {
    let mut rest = s;
    while let Some((plain, r)) = rest.split_once('&') {
        out.push_str(plain);
        let (entity, r) = r.split_once(';').ok_or("unterminated entity reference")?;
        let ch = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity.strip_prefix('#').and_then(|dec| dec.parse().ok())
                };
                code.and_then(char::from_u32).filter(|&ch| ch != '\0').ok_or("unknown entity")?
            }
        };
        out.push(ch);
        rest = r;
    }
    out.push_str(rest);
    Ok(())
}
