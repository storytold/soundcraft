//! Bake the contributor credits into the binary as static Rust tables. Nothing is read at run time.
//!
//! - contributors/contributors.json: commit stats per GitHub username, with a mark for whose people.toml
//!   entry is verified.
//! - contributors/people.toml: the names people chose for themselves. A name is used only when the
//!   JSON says the entry was verified (`people_entry`).
//!
//! A missing or malformed file gives empty tables (or no names) and a warning.
//!
//! It also embeds Droid Sans Mono, the monospace face with a plain zero, from craft-fonts' opt-in
//! `fonts/monospace-manifest.txt` (see craft-fonts' `docs/integration.md`) into `OUT_DIR/craft_fonts.rs`,
//! which `fonts.rs` includes. `CRAFT_FONTS_DIR` unset (the default) embeds nothing and the monospace
//! family stays egui's Hack. Set to the absolute path of a craft-fonts checkout, it embeds the face; a
//! checkout without it (an older one) is a warning and embeds nothing. With `CRAFT_FONTS_REQUIRED=1`
//! (exactly `1`, as in craft-fonts-build) a missing checkout or face is a build error instead, for a
//! build that must carry the face.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

const FACE_FAMILY: &str = "Droid Sans Mono";
const FACE_MANIFEST: &str = "fonts/monospace-manifest.txt";

fn main() {
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    if let Err(e) = std::fs::write(out_dir.join("craft_fonts.rs"), craft_fonts_table()) {
        println!("cargo::error=writing craft_fonts.rs: {e}");
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contributors");
    let json = read(&dir.join("contributors.json"), "the About window shows no contributors", |t| {
        serde_json::from_str::<Value>(t).map_err(|e| e.to_string())
    })
    .unwrap_or(Value::Null);
    let people = read(&dir.join("people.toml"), "contributors are shown by username only", |t| t.parse::<toml::Table>().map_err(|e| e.to_string()))
        .unwrap_or_default();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("credits.rs");
    if let Err(e) = std::fs::write(&out, generate(&json, &people)) {
        println!("cargo::warning=writing {}: {e}", out.display());
    }
}

/// The `&[EmbeddedFace]` expression `fonts.rs` includes: the plain-zero face, or `&[]`.
/// ponytail: craft-fonts-build's `Embed` reads only craft-fonts' default manifest, so this reads the
/// opt-in monospace one itself with the shared `parse_manifest`. Upgrade: an `Embed` option for
/// another manifest in craft-libs, then call that here. It also trusts the checkout `CRAFT_FONTS_DIR`
/// names (the builder's own): `Embed`'s checks of hostile manifest paths are not repeated. And the
/// whole 108 KB face is embedded when it is found, on the web build too; subsetting it to the glyphs
/// the UI draws would shrink that if size ever matters.
fn craft_fonts_table() -> String {
    let required = std::env::var_os("CRAFT_FONTS_REQUIRED").is_some_and(|v| v == "1");
    let dir = std::env::var_os("CRAFT_FONTS_DIR").filter(|d| !d.is_empty()).map(PathBuf::from);
    let face = match &dir {
        Some(dir) => read_face(dir),
        None => Err("CRAFT_FONTS_DIR is not set".to_owned()),
    };
    match face {
        Ok(table) => table,
        Err(e) if required => {
            println!("cargo::error=CRAFT_FONTS_REQUIRED=1, but {FACE_FAMILY} from craft-fonts is not available: {e}");
            "&[]".to_owned()
        }
        Err(e) => {
            if dir.is_some() {
                println!("cargo::warning=building without {FACE_FAMILY} from craft-fonts (the monospace zero stays slashed): {e}");
            }
            "&[]".to_owned()
        }
    }
}

/// The one-face table for the craft-fonts checkout at `dir`.
fn read_face(dir: &Path) -> Result<String, String> {
    let manifest = dir.join(FACE_MANIFEST);
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let entries = craft_fonts_build::parse_manifest(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let face = entries
        .iter()
        .find(|f| f.family == FACE_FAMILY && f.style == "Regular")
        .ok_or_else(|| format!("{} lists no {FACE_FAMILY} Regular", manifest.display()))?;
    // Before canonicalize, so a font file that is missing now and added later rebuilds.
    println!("cargo::rerun-if-changed={}", dir.join(&face.file).display());
    let file = dir.join(&face.file).canonicalize().map_err(|e| format!("{}: {e}", face.file))?;
    let file = file.to_str().ok_or_else(|| format!("{}: the path is not UTF-8", face.file))?;
    let scripts: Vec<String> = face.scripts.iter().map(|s| format!("{s:?}")).collect();
    Ok(format!(
        "&[EmbeddedFace {{ family: {:?}, style: {:?}, scripts: &[{}], index: {}, bytes: include_bytes!({:?}) }}]",
        face.family,
        face.style,
        scripts.join(", "),
        face.index,
        file,
    ))
}

fn read<T>(path: &std::path::Path, fallback: &str, parse: impl Fn(&str) -> Result<T, String>) -> Option<T> {
    println!("cargo::rerun-if-changed={}", path.display());
    let r = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| parse(&t));
    if let Err(e) = &r {
        println!("cargo::warning={}: {e}; {fallback}", path.display());
    }
    r.ok()
}

/// A non-empty string field of someone's people.toml entry, as a Rust `Option<&str>` literal.
fn name(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        Some(x) => format!("Some({x:?})"),
        None => "None".into(),
    }
}

fn s(v: &Value, k: &str) -> String {
    format!("{:?}", v.get(k).and_then(Value::as_str).unwrap_or(""))
}

fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

fn rows(json: &Value, k: &str) -> Vec<Value> {
    json.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn generate(json: &Value, people: &toml::Table) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "pub static TOTAL_COMMITS: u64 = {};", n(json, "commits"));
    let _ = writeln!(o, "pub static CONTRIBUTORS: &[Contributor] = &[");
    for c in rows(json, "contributors") {
        let login = c.get("login").and_then(Value::as_str).unwrap_or("");
        if login.is_empty() {
            continue;
        }
        // Names only from a verified, self-chosen people.toml entry.
        let entry = people.get("people").and_then(|p| p.get(login)).filter(|_| c.get("people_entry").and_then(Value::as_bool) == Some(true));
        let field = |k: &str| entry.and_then(|e| e.get(k)).and_then(toml::Value::as_str);
        let synced = entry.and_then(|e| e.get("sync_github_name")).and_then(toml::Value::as_bool) == Some(true);
        let display = field("display_name")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| if synced { c.get("github_name").and_then(Value::as_str) } else { None });
        let _ = writeln!(
            o,
            "    Contributor {{ login: {login:?}, display_name: {}, real_name: {}, prs: {}, commits: {}, lines_added: {}, lines_deleted: {}, binary_added: {}, binary_deleted: {}, first_commit: {}, last_commit: {} }},",
            name(display),
            name(field("real_name")),
            n(&c, "prs"),
            n(&c, "commits"),
            n(&c, "lines_added"),
            n(&c, "lines_deleted"),
            n(&c, "binary_added"),
            n(&c, "binary_deleted"),
            s(&c, "first_commit"),
            s(&c, "last_commit"),
        );
    }
    let _ = writeln!(o, "];");
    let _ = writeln!(o, "pub static MODELS: &[Model] = &[");
    for m in rows(json, "models") {
        let _ = writeln!(
            o,
            "    Model {{ company: {}, model: {}, version: {}, commits: {}, lines_added: {}, lines_deleted: {} }},",
            s(&m, "company"),
            s(&m, "model"),
            s(&m, "version"),
            n(&m, "commits"),
            n(&m, "lines_added"),
            n(&m, "lines_deleted"),
        );
    }
    let _ = writeln!(o, "];");
    o
}
