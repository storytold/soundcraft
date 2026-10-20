//! UI localisation. English strings in code are the source keys; each language ships one UTF-8
//! catalog (`src/i18n/<code>.tsv`, `context<TAB>source<TAB>translation`) that `build.rs` packs and
//! deflates into the binary. Command ids, menu paths used for logic, the control channel, the CLI
//! and MCP never see translated text, so agents and scripts are unaffected; a string without a
//! translation is shown in English.
//!
//! # Adding a language
//! 1. Add `xx.tsv` next to `zh-hans.tsv` (copy its header; translate from the *meaning* of the
//!    English text, clean-room: no proprietary translation resources).
//! 2. Add one row to [`LANGUAGES`]: the lowercase BCP 47 code, the language's native name,
//!    `catalog!("xx")`, the plural rule, and `complete_menus` when the catalog translates every
//!    menu string (the tests hold complete catalogs to that promise).
//!
//! That is all: the Appearance language choice, the system-locale match and the catalog tests pick
//! it up from the registry, and `build.rs` packs it on the next build.
//!
//! # Looking strings up
//! - [`t`]: a plain string. [`tr_ctx`]: when one English word needs different translations.
//! - [`tr_id`]: a command-id keyed string with the English label as fallback (menu items), so a
//!   translation survives rewording of the English text and can differ per command.
//! - [`trn`]: plural-aware (`{n}` is filled in). [`fmt`]: fill `{name}` placeholders after [`t`];
//!   translators may reorder placeholders freely.
//!
//! Catalogs are deflated and packed by `build.rs` (see its header): the desktop binary and the web
//! build share one compact representation, and a catalog is inflated once on first use. The
//! preference value (`interface.language`) is `auto` or a lowercase code such as `zh-hans`;
//! [`Lang::from_code`] still accepts the canonical BCP 47 spelling (`zh-Hans`) for older saves.
//! English is the fallback when a string or the whole system locale is untranslated.

mod catalog;
mod system;

pub use system::{LOCALE_OVERRIDE, init_system_locale, init_system_locale_with, system_lang};

use std::cell::Cell;
use std::io::Read as _;
use std::sync::OnceLock;

use catalog::Catalog;

/// Upper bound on an inflated catalog (the packed blob is a handful of bytes per entry; a corrupted
/// or hostile blob must not allocate without limit).
const MAX_CATALOG_BYTES: usize = 16 << 20;

/// The deflated catalog that `build.rs` packed for `$code`.
macro_rules! catalog {
    ($code:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/i18n/", $code, ".tsv.deflate"))
    };
}

/// The keys table shared by every packed catalog (deflated lines of `context<TAB>source`).
const KEYS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/i18n/keys.deflate"));

/// One supported UI language.
pub struct LangInfo {
    /// Lowercase BCP 47 code: language lowercase, script lowercase (`en`, `zh-hans`). This is also
    /// the `interface.language` value; lookups accept any casing, so a preference written before
    /// the casing changed (`zh-Hans`) still resolves.
    pub code: &'static str,
    /// The language's name in itself, shown in the Appearance dropdown.
    pub name: &'static str,
    /// The deflated, packed catalog (empty for the built-in English source language).
    pub source: &'static [u8],
    /// Plural form index for a count (English: 0 = one, 1 = other; Chinese: always 0, the noun
    /// never changes). A catalog's `@plural` entries list one form per index.
    pub plural: fn(u64) -> usize,
    /// Must the catalog cover every menu, command and menu-path string? (checked by the tests)
    pub complete_menus: bool,
    catalog: OnceLock<Catalog>,
}

fn plural_one_other(n: u64) -> usize {
    usize::from(n != 1)
}

fn plural_none(_: u64) -> usize {
    0
}

/// The registry. English first: it is the fallback and the source language.
pub static LANGUAGES: [LangInfo; 2] = [
    LangInfo { code: "en", name: "English", source: b"", plural: plural_one_other, complete_menus: false, catalog: OnceLock::new() },
    LangInfo {
        code: "zh-hans",
        name: "简体中文",
        source: catalog!("zh-hans"),
        // Chinese nouns do not inflect for number: "1 个片段" and "3 个片段".
        plural: plural_none,
        complete_menus: true,
        catalog: OnceLock::new(),
    },
];

/// Inflate one deflated blob, refusing to grow past [`MAX_CATALOG_BYTES`].
fn inflate(bytes: &[u8], what: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut decoder = flate2::read::DeflateDecoder::new(bytes);
    let mut capped = std::io::Read::take(&mut decoder, MAX_CATALOG_BYTES as u64 + 1);
    if let Err(e) = capped.read_to_end(&mut out) {
        eprintln!("i18n: inflating {what}: {e}");
        return None;
    }
    if out.len() > MAX_CATALOG_BYTES {
        eprintln!("i18n: {what} inflates past the {MAX_CATALOG_BYTES}-byte cap");
        return None;
    }
    Some(out)
}

/// Expand one packed catalog back into `context<TAB>source<TAB>translation` text, using the shared
/// keys table; the result is exactly the source file (minus its trailing newline).
fn unpack(source: &[u8]) -> Option<String> {
    let keys = inflate(KEYS, "the keys table")?;
    let keys = String::from_utf8(keys).ok()?;
    let keys: Vec<&str> = keys.lines().collect();
    let packed = inflate(source, "the catalog")?;
    let packed = String::from_utf8(packed).ok()?;
    let mut text = String::with_capacity(packed.len() * 2);
    let mut next = 0usize;
    for (n, line) in packed.lines().enumerate() {
        if n > 0 {
            text.push('\n');
        }
        match line.chars().next() {
            Some('\u{1}') => {
                // The next expected key, in the keys table's order.
                text.push_str(keys.get(next)?);
                text.push('\t');
                text.push_str(&line[1..]);
                next = next.saturating_add(1);
            }
            Some('\u{2}') => {
                let (offset, translation) = line[1..].split_once('\t')?;
                let offset: i64 = offset.parse().ok()?;
                let index = usize::try_from((next as i64).checked_add(offset)?).ok()?;
                text.push_str(keys.get(index)?);
                text.push('\t');
                text.push_str(translation);
                next = index.saturating_add(1);
            }
            Some('\u{3}') => text.push_str(&line[1..]),
            _ => text.push_str(line),
        }
    }
    Some(text)
}

impl LangInfo {
    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| {
            if self.source.is_empty() {
                return Catalog::default();
            }
            match unpack(self.source) {
                Some(text) => Catalog::parse(&text),
                None => {
                    eprintln!("i18n: the {} catalog is unreadable; showing English", self.code);
                    Catalog::default()
                }
            }
        })
    }

    /// The expanded source text of the bundled catalog (tests only; the UI uses [`Self::catalog`]).
    #[cfg(test)]
    fn expanded_source(&self) -> Option<String> {
        if self.source.is_empty() { Some(String::new()) } else { unpack(self.source) }
    }
}

/// A language the UI can be shown in (a handle into [`LANGUAGES`]).
#[derive(Clone, Copy)]
pub struct Lang(&'static LangInfo);

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.0.code)
    }
}

impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        self.0.code == other.0.code
    }
}

impl Eq for Lang {}

impl Lang {
    pub const EN: Lang = Lang(&LANGUAGES[0]);

    pub fn code(self) -> &'static str {
        self.0.code
    }

    /// A language by its code; casing is ignored (`zh-Hans` finds `zh-hans`).
    pub fn from_code(code: &str) -> Option<Lang> {
        LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code)).map(Lang)
    }

    /// Resolve the saved `interface.language` preference: a language code, or `auto` (and anything
    /// unknown, e.g. a code written by a newer version) to follow the desktop locale.
    pub fn from_pref(pref: &str) -> Lang {
        if pref.trim().eq_ignore_ascii_case("auto") || pref.trim().is_empty() {
            return system_lang();
        }
        Lang::from_code(pref).or_else(|| lang_from_tag(pref)).unwrap_or_else(system_lang)
    }

    /// Every registered language.
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().map(Lang)
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Does this language's catalog claim to cover every menu string?
    pub fn complete_menus(self) -> bool {
        self.0.complete_menus
    }

    fn catalog(self) -> &'static Catalog {
        self.0.catalog()
    }
}

/// Candidate language codes for a locale tag, most specific first: `zh_CN.UTF-8` →
/// `zh-cn`, `zh-hans`, `zh`.
fn candidates(tag: &str) -> Vec<String> {
    let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
    let parts: Vec<&str> = base.split('-').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for n in (1..=parts.len()).rev() {
        out.push(parts[..n].join("-"));
    }
    if parts.first().is_some_and(|p| *p == "zh") && !parts.iter().any(|p| matches!(*p, "hans" | "hant")) {
        // Chinese by region when no script is given; Taiwan, Hong Kong and Macao read Traditional.
        let script = if parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) { "zh-hant" } else { "zh-hans" };
        out.insert(out.len() - 1, script.to_string());
    }
    out
}

/// The registered language for a locale tag such as `zh_CN.UTF-8`, `zh-CN`, `en-US`; `None` if the
/// language isn't supported. `C`/`POSIX` mean English.
pub fn lang_from_tag(tag: &str) -> Option<Lang> {
    let cands = candidates(tag);
    if matches!(cands.first().map(String::as_str), Some("c" | "posix")) {
        return Some(Lang::EN);
    }
    cands.iter().find_map(|c| Lang::from_code(c))
}

thread_local! {
    // Independent app/test threads must not change each other's drawing language.
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::EN) };
}

/// Set the UI language for drawing (the shell calls this once per frame from the preference), so
/// widgets can translate without every call site carrying a language around.
pub fn set_current(lang: Lang) {
    CURRENT.set(lang);
}

/// The language the UI is drawn in.
pub fn current() -> Lang {
    CURRENT.get()
}

/// Temporarily draw in another language, restoring the previous one even on unwinding.
pub fn with_language<R>(lang: Lang, draw: impl FnOnce() -> R) -> R {
    let _restore = language_scope(lang);
    draw()
}

/// Keep the language active until this guard is dropped on the drawing thread.
#[must_use]
pub fn language_scope(lang: Lang) -> impl Drop {
    struct Restore {
        previous: Lang,
        _thread: std::marker::PhantomData<std::rc::Rc<()>>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            set_current(self.previous);
        }
    }
    let restore = Restore { previous: current(), _thread: std::marker::PhantomData };
    set_current(lang);
    restore
}

/// Apply the saved preference to this thread's drawing language; returns the resolved language.
/// Called once per frame by the shell, so a change in the language preference takes effect
/// immediately.
pub fn apply_preference(pref: &str) -> Lang {
    let lang = Lang::from_pref(pref);
    set_current(lang);
    lang
}

/// Does `lang` have a catalog entry for this plain string? (English never does: it is the source.)
pub fn has(lang: Lang, s: &str) -> bool {
    lang.catalog().plain(s).is_some()
}

/// Translate an English UI string into the current language ([`tr`] with [`current`]).
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// Translate an English UI string; unknown strings come back unchanged.
pub fn tr(lang: Lang, s: &str) -> &str {
    lang.catalog().plain(s).unwrap_or(s)
}

/// Like [`tr`], for an English string that needs a disambiguating `context`.
pub fn tr_ctx<'a>(lang: Lang, context: &str, s: &'a str) -> &'a str {
    lang.catalog().contextual(context, s).unwrap_or_else(|| tr(lang, s))
}

/// A string keyed by its command id, falling back to the translation of the English `label`.
pub fn tr_id<'a>(lang: Lang, id: &str, label: &'a str) -> &'a str {
    lang.catalog().id(id).unwrap_or_else(|| tr(lang, label))
}

/// Fill `{name}` placeholders. Unknown placeholders are left as written.
///
/// The template is scanned once: a substituted value is never scanned again, so user data that
/// happens to look like a placeholder (a session named `Mix {n}`, say) stays literal, and only
/// placeholders that the template itself contains are replaced.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // An unclosed `{` is literal text.
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        match args.iter().find(|(k, _)| *k == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A plural-aware message: `one`/`other` are the English forms (with `{n}` where the count goes).
pub fn trn(lang: Lang, n: u64, one: &str, other: &str) -> String {
    let idx = (lang.0.plural)(n);
    let text = lang.catalog().plural(one, other, idx).unwrap_or(if n == 1 { one } else { other });
    fmt(text, &[("n", &n.to_string())])
}

/// Separator between two sentences of a composed message: scripts writing full-width punctuation
/// (Chinese) need none, others take a space. Messages that are one `fmt` template never need this.
pub fn sentence_space() -> &'static str {
    // Keyed on the language rather than the whole code, so every region variant (`zh-CN`, `zh-Hans`)
    // gets the same full-width punctuation.
    match current().code().split(['-', '_']).next() {
        Some(language) if language.eq_ignore_ascii_case("zh") => "",
        _ => " ",
    }
}

#[cfg(test)]
mod tests {
    use super::catalog::parse_entries;
    use super::*;

    const ZH: fn() -> Lang = || Lang::from_code("zh-hans").expect("zh-hans registered");

    #[test]
    fn chinese_tags_resolve_to_simplified() {
        for tag in ["zh", "zh-CN", "zh_CN.UTF-8", "zh-Hans", "zh-Hans-CN", "zh-SG", "zh-SG.UTF-8"] {
            assert_eq!(lang_from_tag(tag), Some(ZH()), "{tag}");
        }
        assert_eq!(lang_from_tag("en_US.UTF-8"), Some(Lang::EN));
        assert_eq!(lang_from_tag("C"), Some(Lang::EN));
        assert_eq!(lang_from_tag("POSIX"), Some(Lang::EN));
        assert_eq!(lang_from_tag(""), None);
        assert_eq!(lang_from_tag("_"), None);
        // Traditional Chinese locales are not claimed by the Simplified catalog.
        for tag in ["zh-TW", "zh_HK", "zh-Hant", "zh-Hant-TW"] {
            assert_ne!(lang_from_tag(tag), Some(ZH()), "{tag}");
        }
    }

    #[test]
    fn codes_are_lowercase_bcp_47() {
        for lang in Lang::all() {
            assert_eq!(lang.code(), lang.code().to_ascii_lowercase(), "{} is not lowercase", lang.code());
        }
        assert_eq!(Lang::from_code("zh-hans").map(Lang::code), Some("zh-hans"));
        // The canonical BCP 47 spelling, and any other casing, still resolves (older preferences).
        for code in ["zh-hans", "zh-Hans", "ZH-HANS"] {
            assert_eq!(Lang::from_code(code), Some(ZH()), "{code}");
        }
    }

    #[test]
    fn candidates_walk_from_specific_to_general() {
        assert_eq!(candidates("zh_CN.UTF-8"), ["zh-cn", "zh-hans", "zh"]);
        assert_eq!(candidates("zh-Hant-HK"), ["zh-hant-hk", "zh-hant", "zh"]);
        assert_eq!(candidates("en_US"), ["en-us", "en"]);
    }

    #[test]
    fn preferences_resolve_with_fallback() {
        assert_eq!(Lang::from_pref("zh-hans"), ZH());
        assert_eq!(Lang::from_pref("zh-Hans"), ZH());
        assert_eq!(Lang::from_pref("zh-CN"), ZH());
        assert_eq!(Lang::from_pref("en"), Lang::EN);
        // `auto`, an empty value and unknown codes follow the desktop (English under test).
        assert_eq!(Lang::from_pref("auto"), Lang::EN);
        assert_eq!(Lang::from_pref(""), Lang::EN);
        assert_eq!(Lang::from_pref("xx-unknown"), Lang::EN);
    }

    #[test]
    fn bundled_catalogs_unpack_and_parse_without_errors() {
        for lang in Lang::all() {
            let text = lang.0.expanded_source().unwrap_or_else(|| panic!("{} catalog unpacks", lang.code()));
            let (entries, errors) = parse_entries(&text);
            assert!(errors.is_empty(), "{}: {errors:?}", lang.code());
            assert_eq!(entries.len(), text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).count());
        }
        assert!(Lang::EN.0.source.is_empty(), "English is the source language and has no catalog");
    }

    /// The packed catalogs must expand back to exactly the files in the tree, so a reviewer can
    /// read and edit plain TSV while the binary carries the small deflated form.
    #[test]
    fn packed_catalogs_round_trip_to_their_sources() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/i18n");
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            let expanded = lang.0.expanded_source().unwrap_or_else(|| panic!("{} catalog unpacks", lang.code()));
            let path = dir.join(format!("{}.tsv", lang.code()));
            let Ok(original) = std::fs::read_to_string(&path) else { continue };
            assert_eq!(expanded, original.trim_end_matches('\n'), "{} does not round-trip", lang.code());
        }
    }

    /// Duplicate keys silently shadow each other; placeholders and trailing ellipses must survive
    /// translation, or a message loses its argument or its "opens a dialog" hint.
    #[test]
    fn catalogs_have_unique_keys_and_matching_placeholders() {
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            let text = lang.0.expanded_source().unwrap_or_default();
            let (entries, _) = parse_entries(&text);
            let mut seen = std::collections::HashSet::new();
            for (ctx, src, tr) in &entries {
                let key = format!("{ctx}\u{1}{src}");
                assert!(seen.insert(key), "{}: duplicate key {src:?} in context {ctx:?}", lang.code());
                let (mut a, mut b) = (catalog::placeholders(src), catalog::placeholders(tr));
                if ctx == "@plural" {
                    // The source lists every form on one line, so `{n}` may legitimately appear
                    // once per form while a language with a single form names it once.
                    a.dedup();
                    b.dedup();
                }
                a.sort_unstable();
                b.sort_unstable();
                assert_eq!(a, b, "{}: placeholders differ for {src:?} → {tr:?}", lang.code());
                assert_eq!(src.ends_with('…'), tr.ends_with('…'), "{}: ellipsis differs for {src:?} → {tr:?}", lang.code());
                // Chinese writes a full-width colon; both forms are acceptable.
                let colon = |s: &str| s.ends_with(':') || s.ends_with('：');
                assert_eq!(colon(src), colon(tr), "{}: colon differs for {src:?} → {tr:?}", lang.code());
            }
        }
    }

    /// The parity gate: a translated interface must not leave English text behind. It covers every
    /// node the menu bar draws, every label of a menu-only UI command, every engine command label
    /// and every segment of every engine menu path — the whole composed interface, not just leaves.
    #[test]
    fn simplified_chinese_covers_the_composed_interface() {
        let zh = ZH();
        assert!(zh.complete_menus(), "Simplified Chinese participates in the coverage gate");
        let mut missing: Vec<String> = Vec::new();
        for root in crate::menus::tree() {
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if !has(zh, &node.label) {
                    missing.push(node.label.clone());
                }
                stack.extend(node.children);
            }
        }
        for (_, label, _, _) in crate::menus::UI_COMMANDS {
            if !has(zh, label) {
                missing.push((*label).to_owned());
            }
        }
        for spec in soundcraft_engine::command_specs() {
            if !has(zh, spec.label) {
                missing.push(spec.label.to_owned());
            }
            for segment in spec.menu {
                if !has(zh, segment) {
                    missing.push((*segment).to_owned());
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "Simplified Chinese is missing {} label(s): {missing:?}", missing.len());
    }

    #[test]
    fn translations_look_translated() {
        let zh = ZH();
        assert_eq!(tr(zh, "Play"), "播放");
        assert_eq!(tr(zh, "Track"), "轨道");
        assert_eq!(tr(zh, "no such label"), "no such label");
        assert_eq!(tr(Lang::EN, "Play"), "Play");
        // Context disambiguates a word the menu and a panel use differently.
        assert_eq!(tr_ctx(zh, "transport", "Play"), "播放");
        // A command-id entry wins over the label's translation (and falls back to the label).
        assert_eq!(tr_id(zh, "edit.undo", "Undo"), "撤销");
        assert_eq!(tr_id(zh, "no.such.id", "Play"), "播放");
    }

    #[test]
    fn chinese_counts_take_one_form() {
        let zh = ZH();
        for n in [0, 1, 2, 11, 100] {
            assert_eq!(trn(zh, n, "{n} clip", "{n} clips"), format!("{n} 个片段"));
            assert_eq!(trn(zh, n, "{n} take", "{n} takes"), format!("{n} 个版本"));
        }
        // A count that has no catalog entry still reads correctly in English.
        assert_eq!(trn(zh, 1, "{n} marker", "{n} markers"), "1 marker");
        assert_eq!(trn(Lang::EN, 2, "{n} take", "{n} takes"), "2 takes");
    }

    #[test]
    fn placeholders_are_filled_after_translation() {
        let zh = ZH();
        let line = fmt(tr(zh, "Open {path}"), &[("path", "/tmp/Song.scraft")]);
        assert_eq!(line, "打开 /tmp/Song.scraft");
        // A translator may move the placeholder.
        assert_eq!(fmt(tr(zh, "Search {n} commands"), &[("n", "3")]), "搜索 3 条命令");
    }

    /// A substituted value is user data: placeholder-looking text inside it must stay literal, and
    /// a template placeholder with no argument stays a placeholder.
    #[test]
    fn fmt_does_not_rescan_substituted_values() {
        assert_eq!(fmt("{a} and {b}", &[("a", "{b}"), ("b", "X")]), "{b} and X");
        assert_eq!(fmt("Mix {n}", &[("n", "{n}")]), "Mix {n}");
        assert_eq!(fmt("Open {path}", &[("other", "x")]), "Open {path}");
        assert_eq!(fmt("no placeholders", &[("a", "b")]), "no placeholders");
        assert_eq!(fmt("{unclosed", &[]), "{unclosed");
        assert_eq!(fmt("{a}{a}", &[("a", "x")]), "xx");
    }

    #[test]
    fn drawing_language_is_per_thread_and_restored() {
        assert_eq!(current(), Lang::EN);
        with_language(ZH(), || {
            assert_eq!(current(), ZH());
            assert_eq!(t("Play"), "播放");
        });
        assert_eq!(current(), Lang::EN);
        let _scope = language_scope(ZH());
        assert_eq!(current(), ZH());
    }

    /// Every plainly looked-up literal in the UI crate must have an entry: a missing key is a
    /// silent English fallback, which is exactly the drift this guards against. The sources are
    /// read from disk (tests only), so a new UI file is covered without touching this test.
    #[test]
    fn every_looked_up_literal_has_a_translation() {
        let Ok(entries) = std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")) else {
            return; // packaged source without the tree: nothing to scan
        };
        let zh = ZH();
        let mut files: Vec<std::path::PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        let mut missing = Vec::new();
        for path in files.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs")) {
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            for key in looked_up_literals(&text) {
                if !has(zh, &key) {
                    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    missing.push(format!("{name}: {key}"));
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "{} literal(s) are looked up but untranslated: {missing:#?}", missing.len());
    }

    /// The literal first arguments of plain lookups — `i18n::t("…")` and a `t("…")` imported into
    /// scope. A variable argument is skipped: it can only be checked where it is defined.
    fn looked_up_literals(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find("t(") {
            let before = rest[..at].chars().next_back();
            let qualified = rest[..at].ends_with("i18n::");
            // `split(`, `test(` and friends end in `t(`; only a bare call or `i18n::t` counts.
            if !qualified && before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                rest = &rest[at + 2..];
                continue;
            }
            let after = &rest[at + 2..];
            let Some(body) = after.trim_start().strip_prefix('"') else {
                rest = after;
                continue;
            };
            // Read the literal, honouring escapes, then insist on the closing `)` or `,`.
            let mut key = String::new();
            let mut chars = body.char_indices();
            let mut closed = None;
            while let Some((i, c)) = chars.next() {
                match c {
                    '"' => {
                        closed = Some(i);
                        break;
                    }
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            key.push(match escaped {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                    }
                    _ => key.push(c),
                }
            }
            let Some(end) = closed else {
                rest = body;
                continue;
            };
            let tail = body[end + 1..].trim_start();
            rest = &body[end + 1..];
            if tail.starts_with(')') || tail.starts_with(',') {
                out.push(key);
            }
        }
        out
    }
}
