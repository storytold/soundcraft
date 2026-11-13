//! The keys a command takes, read from its params doc.
//!
//! A command's `params` doc (what `engine.commands` shows) also lists the keys it accepts, and
//! `Engine::execute` refuses any other key. Before, a misspelt or misplaced key was dropped while
//! the call still returned ok: `mix.send {db: -6}` made a silent send, because the key is `level_db`.
//!
//! The keys are the names in the doc's leading `{…}` (and in a second `{…}` after ` or `):
//! - every top-level item, between commas, names its key before its first `:`. `?` marks it
//!   optional, and `|` or `/` separate alternatives (`{number | id}`, `{at?|start/end}`);
//! - after the `:` comes the value. Brackets nested in it (`{…}`, `[…]`, `(…)`) describe values,
//!   so their commas and colons do not start keys.
//!
//! The shared helpers read some keys in pairs, so a doc that names one also accepts the other:
//! `track` and `tracks` ([`super::tracks_param`]), `clip` and `clips` ([`super::clip_ids_param`]),
//! and `start` with `end` also accepts `length` ([`super::range_param`]). [`UNPAIRED`] lists the
//! commands that read such a key without its helper, and so do not take the pair.

use super::{CommandSpec, bad};
use crate::Result;
use serde_json::Value;

/// Keys that a shared helper reads together with a documented key.
const COMPANIONS: [(&str, &str); 4] = [("track", "tracks"), ("tracks", "track"), ("clip", "clips"), ("clips", "clip")];

/// Paired keys that these commands do not take: they read the documented key alone (`track`
/// through `track_param`, `clip` through `clip_id`, `clips` as a list, `start` and `end` one by one).
const UNPAIRED: [(&str, &[&str]); 5] = [
    (
        "tracks",
        &[
            "clip.place_source",
            "edit.move_clips",
            "event.midi_track_offsets",
            "event.retrospective_record",
            "event.step_input",
            "file.import_audio",
            "setup.disk_allocation",
            "track.comments",
            "track.folder_toggle",
            "track.inspect",
            "track.move",
            "track.playlist_promote",
            "track.rename",
            "track.scroll_to",
            "video.offset",
        ],
    ),
    // Its `tracks` are names in the other session.
    ("track", &["file.import_session_data"]),
    ("clip", &["clip.auto_rename", "clip.clear", "edit.select"]),
    ("clips", &["clip.inspect", "edit.fades_create", "edit.trim_clip", "midi.note_add", "midi.note_delete", "midi.note_edit", "midi.notes", "midi.notes_move"]),
    ("length", &["edit.select", "edit.tce_to_timeline", "edit.trim_clip", "markers.add"]),
];

/// At most this many unknown keys are named in one error.
const MAX_NAMED: usize = 8;

/// An unknown key longer than this is shortened in the error and gets no suggestion.
const MAX_KEY_SHOWN: usize = 64;

/// The keys a params doc names, in order and without repeats, before the [`COMPANIONS`] and
/// `length` are added. `None` when the doc does not start with a balanced `{…}`.
pub fn doc_keys(doc: &str) -> Option<Vec<&str>> {
    let mut keys = Vec::new();
    let mut rest = doc.trim_start();
    loop {
        let inner = rest.strip_prefix('{')?;
        let end = closing_brace(inner)?;
        for item in split_top(inner.get(..end)?, ',') {
            let key_part = split_top(item, ':').next().unwrap_or_default();
            for name in key_part.split(['|', '/']) {
                let name = name.trim().trim_end_matches('?').trim_end();
                if is_key(name) && !keys.contains(&name) {
                    keys.push(name);
                }
            }
        }
        match inner.get(end + 1..).unwrap_or_default().trim_start().strip_prefix("or ") {
            Some(next) if next.trim_start().starts_with('{') => rest = next.trim_start(),
            _ => return Some(keys),
        }
    }
}

/// Whether command `id`, whose doc names `keys`, takes `key`: the doc names it, or a helper the
/// command uses reads it with a key the doc names.
fn accepted(id: &str, keys: &[&str], key: &str) -> bool {
    keys.contains(&key) || (paired(keys, key) && !UNPAIRED.iter().any(|(k, ids)| *k == key && ids.contains(&id)))
}

/// Whether a shared helper reads `key` together with one of `keys`.
fn paired(keys: &[&str], key: &str) -> bool {
    COMPANIONS.iter().any(|(doc, also)| *also == key && keys.contains(doc)) || (key == "length" && keys.contains(&"start") && keys.contains(&"end"))
}

/// Refuse params that are an object with a key `spec` does not take. The error names the unknown
/// keys (the first [`MAX_NAMED`]), suggests a documented key that looks like the first one, and
/// lists the keys the command takes. Params that are not an object (`null`) are left alone.
pub fn check(spec: &CommandSpec, params: &Value) -> Result<()> {
    let Some(obj) = params.as_object() else { return Ok(()) };
    let keys = doc_keys(spec.params).unwrap_or_default();
    let unknown: Vec<&str> = obj.keys().map(String::as_str).filter(|k| !accepted(spec.id, &keys, k)).collect();
    let Some(first) = unknown.first() else { return Ok(()) };
    let named: Vec<String> = unknown.iter().take(MAX_NAMED).map(|k| format!("`{}`", shown(k))).collect();
    let mut msg = if unknown.len() == 1 { format!("unknown parameter {}", named.join(", ")) } else { format!("unknown parameters {}", named.join(", ")) };
    if unknown.len() > MAX_NAMED {
        msg.push_str(&format!(" and {} more", unknown.len() - MAX_NAMED));
    }
    if let Some(s) = suggestion(&keys, first) {
        msg.push_str(&format!(" (did you mean `{s}`?)"));
    }
    if keys.is_empty() {
        msg.push_str("; it takes no parameters");
    } else {
        msg.push_str(&format!("; it takes {}", keys.join(", ")));
    }
    Err(bad(spec.id, msg))
}

/// `key`, cut to [`MAX_KEY_SHOWN`] characters.
fn shown(key: &str) -> String {
    if key.chars().count() <= MAX_KEY_SHOWN { key.to_string() } else { format!("{}…", key.chars().take(MAX_KEY_SHOWN).collect::<String>()) }
}

/// The documented key closest to `key`: one containing it or contained in it (`db` → `level_db`),
/// else one within two edits. Ties go to the key listed first.
fn suggestion<'a>(keys: &[&'a str], key: &str) -> Option<&'a str> {
    if key.len() < 2 || key.chars().count() > MAX_KEY_SHOWN {
        return None;
    }
    if let Some(k) = keys.iter().find(|k| k.contains(key) || (k.len() >= 2 && key.contains(**k))) {
        return Some(k);
    }
    keys.iter().map(|k| (edit_distance(k, key), *k)).filter(|(d, _)| *d <= 2).min_by_key(|(d, _)| *d).map(|(_, k)| k)
}

/// Levenshtein distance by characters. Callers bound `b` to [`MAX_KEY_SHOWN`] characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut row = Vec::with_capacity(prev.len());
        row.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let delete = prev.get(j + 1).map_or(usize::MAX, |d| d + 1);
            let insert = row.last().map_or(usize::MAX, |d| d + 1);
            let change = prev.get(j).map_or(usize::MAX, |d| d + usize::from(ca != *cb));
            row.push(delete.min(insert).min(change));
        }
        prev = row;
    }
    prev.last().copied().unwrap_or_default()
}

/// A key name: lower-case ASCII letters, digits and `_`, not starting with a digit.
fn is_key(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The byte index in `s` of the `}` that closes a `{` just before `s`, skipping nested brackets.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' | ']' | ')' => depth = depth.checked_sub(1)?,
            _ => {}
        }
    }
    None
}

/// `s` split at each `sep` outside brackets.
fn split_top(s: &str, sep: char) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut parts = Vec::new();
    for (i, c) in s.char_indices() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                parts.push(s.get(start..i).unwrap_or_default());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(s.get(start..).unwrap_or_default());
    parts.into_iter()
}

#[cfg(test)]
mod tests;
