//! Parser and lookup tables for translation catalogs (`*.tsv`, format documented in `zh-hans.tsv`).

use std::collections::HashMap;

/// A parsed catalog. Values are owned for the life of the process (catalogs are built once).
#[derive(Debug, Default)]
pub struct Catalog {
    /// context-free strings: English source → translation (the hot path, looked up every frame)
    plain: HashMap<String, String>,
    /// strings with a disambiguating context, keyed `context \u{1} source`
    contextual: HashMap<String, String>,
    /// command-id keyed strings
    ids: HashMap<String, String>,
    /// plural messages: `one|other` → forms
    plurals: HashMap<String, Vec<String>>,
}

/// A catalog entry as read from the file: (context, source, translation).
pub type Entry = (String, String, String);

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Read the entries of a catalog file. Malformed lines are returned as errors (and skipped), so a
/// bad translation never breaks the UI; the tests insist the bundled catalogs have none.
pub fn parse_entries(text: &str) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.split('\t');
        match (cols.next(), cols.next(), cols.next(), cols.next()) {
            (Some(ctx), Some(src), Some(tr), None) if !src.is_empty() && !tr.is_empty() => {
                entries.push((unescape(ctx), unescape(src), unescape(tr)));
            }
            _ => errors.push(format!("line {}: expected `context<TAB>source<TAB>translation`", n + 1)),
        }
    }
    (entries, errors)
}

impl Catalog {
    pub fn parse(text: &str) -> Catalog {
        let mut c = Catalog::default();
        for (ctx, src, tr) in parse_entries(text).0 {
            match ctx.as_str() {
                "" => {
                    c.plain.insert(src, tr);
                }
                "@id" => {
                    c.ids.insert(src, tr);
                }
                "@plural" => {
                    c.plurals.insert(src, tr.split('|').map(str::to_string).collect());
                }
                _ => {
                    c.contextual.insert(format!("{ctx}\u{1}{src}"), tr);
                }
            }
        }
        c
    }

    pub fn plain(&self, s: &str) -> Option<&str> {
        self.plain.get(s).map(String::as_str)
    }

    pub fn contextual(&self, ctx: &str, s: &str) -> Option<&str> {
        self.contextual.get(&format!("{ctx}\u{1}{s}")).map(String::as_str)
    }

    pub fn id(&self, id: &str) -> Option<&str> {
        self.ids.get(id).map(String::as_str)
    }

    /// The plural form `index` of the message whose English forms are `one|other`.
    pub fn plural(&self, one: &str, other: &str, index: usize) -> Option<&str> {
        let forms = self.plurals.get(&format!("{one}|{other}"))?;
        forms.get(index.min(forms.len().saturating_sub(1))).map(String::as_str)
    }
}

/// `{name}` placeholders of a template, in order of appearance.
#[cfg(test)]
pub fn placeholders(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(a) = rest.find('{') {
        let after = &rest[a + 1..];
        match after.find('}') {
            Some(b) => {
                out.push(&after[..b]);
                rest = &after[b + 1..];
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_split_on_tabs_and_skip_comments() {
        let (entries, errors) = parse_entries("# comment\n\n\tPlay\t播放\nmenu\tMix\t混音\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries, [("".into(), "Play".into(), "播放".into()), ("menu".into(), "Mix".into(), "混音".into())]);
    }

    #[test]
    fn malformed_lines_are_errors_rather_than_panics() {
        let (entries, errors) = parse_entries("no tabs here\n\tOnly source\t\n\t\ttranslation\n");
        assert!(entries.is_empty());
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn escapes_round_trip() {
        let (entries, _) = parse_entries("\tA\\tB\\\\C\\nD\tX\\tY\n");
        assert_eq!(entries[0].1, "A\tB\\C\nD");
        assert_eq!(entries[0].2, "X\tY");
    }

    #[test]
    fn placeholders_are_listed_in_order() {
        assert_eq!(placeholders("{a} and {b} then {a}"), ["a", "b", "a"]);
        assert_eq!(placeholders("{unclosed"), Vec::<&str>::new());
        assert_eq!(placeholders("plain"), Vec::<&str>::new());
    }

    #[test]
    fn lookups_follow_the_entry_kind() {
        let c = Catalog::parse("\tPlay\t播放\nzoom\tZoom\t缩放\n@id\tplay\t走带播放\n@plural\t{n} clip|{n} clips\t{n} 个片段\n");
        assert_eq!(c.plain("Play"), Some("播放"));
        assert_eq!(c.contextual("zoom", "Zoom"), Some("缩放"));
        assert_eq!(c.id("play"), Some("走带播放"));
        assert_eq!(c.plural("{n} clip", "{n} clips", 0), Some("{n} 个片段"));
        // A single Chinese form serves every index.
        assert_eq!(c.plural("{n} clip", "{n} clips", 5), Some("{n} 个片段"));
        assert_eq!(c.plain("Unknown"), None);
    }
}
