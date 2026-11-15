//! A ReaPack repository index (`index.xml`, format version 1), reduced to what installing JSFX needs:
//! the `effect` packages, their versions and their sources.

use crate::ReapackError;
use crate::xml::{Ev, walk};
use std::cmp::Ordering;

/// A repository index is big but bounded; these caps keep a hostile one from exhausting memory.
const MAX_PACKAGES: usize = 50_000;
const MAX_VERSIONS: usize = 200;
const MAX_SOURCES: usize = 64;
const MAX_URL: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub url: String,
    /// File name or path relative to the category; the package name when absent.
    pub file: Option<String>,
    pub platform: String,
    /// Overrides the package type for this one file (`data` files do not belong in Effects).
    pub kind: Option<String>,
    /// Multihash in hex (`1220` + SHA-256).
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub name: String,
    pub author: String,
    pub time: String,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub category: String,
    pub name: String,
    pub desc: String,
    pub versions: Vec<Version>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    pub name: String,
    pub packages: Vec<Package>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Text(String),
    Num(u64),
}

fn parts(v: &str) -> Vec<Part> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut numeric = false;
    let flush = |cur: &mut String, numeric: bool, out: &mut Vec<Part>| {
        if !cur.is_empty() {
            out.push(if numeric { cur.parse().map_or(Part::Text(cur.clone()), Part::Num) } else { Part::Text(cur.to_lowercase()) });
            cur.clear();
        }
    };
    for c in v.chars() {
        if c.is_ascii_digit() {
            if !numeric {
                flush(&mut cur, numeric, &mut out);
            }
            numeric = true;
            cur.push(c);
        } else if c.is_alphabetic() {
            if numeric {
                flush(&mut cur, numeric, &mut out);
            }
            numeric = false;
            cur.push(c);
        } else {
            flush(&mut cur, numeric, &mut out);
        }
    }
    flush(&mut cur, numeric, &mut out);
    out
}

/// Orders version names: numbers numerically, `1.0pre1` before `1.0`, `1.0.1` after `1.0`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(&pb) {
        match (x, y) {
            (Part::Num(m), Part::Num(n)) => match m.cmp(n) {
                Ordering::Equal => {}
                o => return o,
            },
            (Part::Text(m), Part::Text(n)) => match m.cmp(n) {
                Ordering::Equal => {}
                o => return o,
            },
            // A number beats a pre-release tag at the same position.
            (Part::Num(_), Part::Text(_)) => return Ordering::Greater,
            (Part::Text(_), Part::Num(_)) => return Ordering::Less,
        }
    }
    // Equal so far: extra text (a pre-release tag) makes the longer one older, extra numbers newer.
    match pa.len().cmp(&pb.len()) {
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => {
            if matches!(pa.get(pb.len()), Some(Part::Text(_))) {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        Ordering::Less => {
            if matches!(pb.get(pa.len()), Some(Part::Text(_))) {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
    }
}

fn is_prerelease(v: &str) -> bool {
    parts(v).iter().any(|p| matches!(p, Part::Text(_)))
}

impl Package {
    /// The newest stable version, else the newest of any kind.
    pub fn latest(&self) -> Option<&Version> {
        let by = |a: &&Version, b: &&Version| compare_versions(&a.name, &b.name);
        self.versions.iter().filter(|v| !is_prerelease(&v.name)).max_by(by).or_else(|| self.versions.iter().max_by(by))
    }
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Parses an index. Only `effect` packages are kept; `index version` must be 1.
pub fn parse(xml: &str) -> Result<Index, ReapackError> {
    let mut index = Index { name: String::new(), packages: Vec::new() };
    let mut seen_root = false;
    let mut category = String::new();
    let mut pkg: Option<Package> = None;
    let mut ver: Option<Version> = None;
    let mut src: Option<Source> = None;
    walk(xml, &mut |ev| {
        match ev {
            Ev::Start { name, attrs } => match name {
                "index" => {
                    if attr(&attrs, "version") != Some("1") {
                        return Err(ReapackError::Unsupported("index format version (only version 1 is supported)".into()));
                    }
                    seen_root = true;
                    index.name = attr(&attrs, "name").unwrap_or("").to_string();
                }
                "category" => category = attr(&attrs, "name").unwrap_or("").to_string(),
                "reapack" => {
                    let is_effect = attr(&attrs, "type").is_some_and(|t| t.eq_ignore_ascii_case("effect"));
                    let pname = attr(&attrs, "name").unwrap_or("");
                    pkg = (is_effect && !pname.is_empty() && !category.is_empty() && index.packages.len() < MAX_PACKAGES).then(|| Package {
                        category: category.clone(),
                        name: pname.to_string(),
                        desc: attr(&attrs, "desc").filter(|d| !d.is_empty()).unwrap_or(pname).to_string(),
                        versions: Vec::new(),
                    });
                }
                "version" if pkg.as_ref().is_some_and(|p| p.versions.len() < MAX_VERSIONS) => {
                    ver = Some(Version {
                        name: attr(&attrs, "name").unwrap_or("").to_string(),
                        author: attr(&attrs, "author").unwrap_or("").to_string(),
                        time: attr(&attrs, "time").unwrap_or("").to_string(),
                        sources: Vec::new(),
                    });
                }
                "source" if ver.as_ref().is_some_and(|v| v.sources.len() < MAX_SOURCES) => {
                    src = Some(Source {
                        url: String::new(),
                        file: attr(&attrs, "file").filter(|f| !f.is_empty()).map(str::to_string),
                        platform: attr(&attrs, "platform").unwrap_or("all").to_ascii_lowercase(),
                        kind: attr(&attrs, "type").map(str::to_ascii_lowercase),
                        hash: attr(&attrs, "hash").map(str::to_ascii_lowercase),
                    });
                }
                _ => {}
            },
            Ev::Text(t) => {
                if let Some(s) = src.as_mut()
                    && s.url.len() + t.len() <= MAX_URL
                {
                    s.url.push_str(&t);
                }
            }
            Ev::End(name) => match name {
                "source" => {
                    if let (Some(mut s), Some(v)) = (src.take(), ver.as_mut()) {
                        s.url = s.url.trim().to_string();
                        if !s.url.is_empty() {
                            v.sources.push(s);
                        }
                    }
                }
                "version" => {
                    if let (Some(v), Some(p)) = (ver.take(), pkg.as_mut())
                        && !v.name.is_empty()
                        && !v.sources.is_empty()
                    {
                        p.versions.push(v);
                    }
                }
                "reapack" => {
                    if let Some(p) = pkg.take()
                        && !p.versions.is_empty()
                    {
                        index.packages.push(p);
                    }
                }
                "category" => category.clear(),
                _ => {}
            },
        }
        Ok(())
    })?;
    if !seen_root {
        return Err(ReapackError::Parse("not a ReaPack index (no <index> element)".into()));
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<index version="1" name="Test Repo" commit="abc">
  <category name="Dynamics">
    <reapack name="comp.jsfx" type="effect" desc="Simple Compressor">
      <metadata><description><![CDATA[{\rtf1 hi}]]></description><link rel="website">https://example.com</link></metadata>
      <version name="1.0" author="A" time="2020-01-01T00:00:00Z">
        <source>https://example.com/comp-1.0.jsfx</source>
      </version>
      <version name="1.1" author="A" time="2021-01-01T00:00:00Z">
        <changelog><![CDATA[better]]></changelog>
        <source>https://example.com/comp-1.1.jsfx</source>
        <source file="comp_lib.jsfx-inc" hash="1220AA">https://example.com/lib?a=1&amp;b=2</source>
        <source file="../Data/preset.rpl" type="data">https://example.com/p.rpl</source>
      </version>
      <version name="1.2pre1"><source>https://example.com/comp-1.2pre1.jsfx</source></version>
    </reapack>
    <reapack name="tool.lua" type="script"><version name="1"><source>https://example.com/t.lua</source></version></reapack>
  </category>
  <category name="Filter/EQ">
    <reapack name="eq.jsfx" type="effect"><version name="2.0"><source platform="darwin64">https://example.com/eq-mac</source></version></reapack>
    <reapack name="empty.jsfx" type="effect"></reapack>
  </category>
</index>"#;

    #[test]
    fn parses_effect_packages_only() {
        let idx = parse(SAMPLE).unwrap();
        assert_eq!(idx.name, "Test Repo");
        let names: Vec<_> = idx.packages.iter().map(|p| (p.category.as_str(), p.name.as_str())).collect();
        assert_eq!(names, [("Dynamics", "comp.jsfx"), ("Filter/EQ", "eq.jsfx")]);
        assert_eq!(idx.packages[0].desc, "Simple Compressor");
        assert_eq!(idx.packages[1].desc, "eq.jsfx");
        let v = &idx.packages[0].versions[1];
        assert_eq!((v.name.as_str(), v.author.as_str(), v.sources.len()), ("1.1", "A", 3));
        assert_eq!(v.sources[1].url, "https://example.com/lib?a=1&b=2");
        assert_eq!((v.sources[1].file.as_deref(), v.sources[1].hash.as_deref()), (Some("comp_lib.jsfx-inc"), Some("1220aa")));
        assert_eq!(v.sources[2].kind.as_deref(), Some("data"));
        assert_eq!(idx.packages[1].versions[0].sources[0].platform, "darwin64");
    }

    #[test]
    fn latest_prefers_the_newest_stable_version() {
        let idx = parse(SAMPLE).unwrap();
        assert_eq!(idx.packages[0].latest().unwrap().name, "1.1");
    }

    #[test]
    fn version_ordering() {
        use Ordering::*;
        for (a, b, o) in [
            ("1.0", "1.1", Less),
            ("1.10", "1.9", Greater),
            ("1.0pre1", "1.0", Less),
            ("1.0", "1.0.1", Less),
            ("2", "10", Less),
            ("v1.2", "1.2", Equal),
            ("1.0beta", "1.0alpha", Greater),
            ("1.2-rc1", "1.2", Less),
        ] {
            assert_eq!(compare_versions(a, b), o, "{a} vs {b}");
            assert_eq!(compare_versions(b, a), o.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn rejects_other_formats() {
        assert!(matches!(parse("<index version=\"2\"></index>"), Err(ReapackError::Unsupported(_))));
        assert!(matches!(parse("<html></html>"), Err(ReapackError::Parse(_))));
        assert!(parse("not xml at all").is_err());
    }
}
