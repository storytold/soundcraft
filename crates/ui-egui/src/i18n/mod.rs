//! UI localisation. Strings in code stay English and are the default lookup keys; a per-language
//! catalog (`*.tsv`, see `zh-hans.tsv` for the format) maps them to display text at render time.
//! Command ids, menu paths used for logic, the control channel, the CLI and MCP never see
//! translated text, so agents and scripts are unaffected. A string without a translation is shown
//! in English.
//!
//! # Adding a language
//! 1. Add `xx.tsv` next to `zh-hans.tsv` (copy its header; translate from the *meaning* of the
//!    English text, clean-room).
//! 2. Add one row to [`LANGUAGES`] (code, native name, catalog, plural rule).
//!
//! That is all: the Appearance dropdown, the system-locale match and the catalog tests (parse,
//! placeholders, plural forms, menu coverage) pick it up from the registry.
//!
//! # Looking strings up
//! - [`t`]: a plain string. [`tr_ctx`]: when one English word needs different translations.
//! - [`tr_id`]: a command-id keyed string with the English label as fallback (menu items), so a
//!   translation survives rewording of the English text and can differ per command.
//! - [`trn`]: plural-aware (`{n}` is filled in). [`fmt`]: fill `{name}` placeholders after [`t`];
//!   translators may reorder placeholders freely.
//!
//! Unlike the reference implementation the catalog is compiled in as plain UTF-8 rather than
//! deflated by `build.rs`: `include_str!` costs no build script, and a desktop binary's size
//! budget is not the web build's.

mod catalog;
mod system;

pub use system::{LOCALE_OVERRIDE, init_system_locale, system_lang};

use std::cell::Cell;
use std::sync::OnceLock;

use catalog::Catalog;

/// One supported UI language.
pub struct LangInfo {
    /// BCP 47 code in canonical case: language lowercase, script titlecase (`en`, `zh-Hans`). This
    /// is also the `ui.language` value, so it is matched case-insensitively.
    pub code: &'static str,
    /// The language's name in itself, shown in the Appearance dropdown.
    pub name: &'static str,
    /// Catalog file contents (empty for the built-in English).
    pub source: &'static str,
    /// Plural form index for a count (English: 0 = one, 1 = other; Chinese: always 0, the noun
    /// never changes). A catalog's `@plural` entries list one form per index.
    pub plural: fn(u64) -> usize,
    /// Must the catalog cover every menu string? (checked by the tests)
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
    LangInfo { code: "en", name: "English", source: "", plural: plural_one_other, complete_menus: false, catalog: OnceLock::new() },
    LangInfo {
        code: "zh-Hans",
        name: "简体中文",
        source: include_str!("zh-hans.tsv"),
        // Chinese nouns do not inflect for number: "1 个片段" and "3 个片段".
        plural: plural_none,
        complete_menus: true,
        catalog: OnceLock::new(),
    },
];

impl LangInfo {
    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| Catalog::parse(self.source))
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

    /// A language by its exact code.
    pub fn from_code(code: &str) -> Option<Lang> {
        LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code)).map(Lang)
    }

    /// Resolve the saved `language` preference: a language code, or `auto` (and anything unknown,
    /// e.g. a code written by a newer version) to follow the desktop locale.
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
/// Called once per frame by the shell, so a change in Appearance takes effect immediately.
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
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
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
    // Keyed on the language rather than the whole code, so the canonical spelling (`zh-Hans`) and
    // any region variant (`zh_CN`, `zh_TW`) get the same full-width punctuation.
    match current().code().split(['-', '_']).next() {
        Some(language) if language.eq_ignore_ascii_case("zh") => "",
        _ => " ",
    }
}

#[cfg(test)]
mod tests {
    use super::catalog::parse_entries;
    use super::*;

    const ZH: fn() -> Lang = || Lang::from_code("zh-Hans").expect("zh-Hans registered");

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
    fn the_code_keeps_the_canonical_bcp_47_spelling() {
        assert_eq!(Lang::from_code("zh-Hans").map(Lang::code), Some("zh-Hans"));
        // A preference saved with any other casing still resolves to the same language.
        for code in ["zh-hans", "ZH-HANS", "zh-Hans"] {
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
        assert_eq!(Lang::from_pref("ZH-Hans"), ZH());
        assert_eq!(Lang::from_pref("zh-CN"), ZH());
        assert_eq!(Lang::from_pref("en"), Lang::EN);
        // `auto`, an empty value and unknown codes follow the desktop (English under test).
        assert_eq!(Lang::from_pref("auto"), Lang::EN);
        assert_eq!(Lang::from_pref(""), Lang::EN);
        assert_eq!(Lang::from_pref("xx-unknown"), Lang::EN);
    }

    #[test]
    fn bundled_catalogs_parse_without_errors() {
        for lang in Lang::all() {
            let (entries, errors) = parse_entries(lang.0.source);
            assert!(errors.is_empty(), "{}: {errors:?}", lang.code());
            assert_eq!(entries.len(), lang.0.source.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).count());
        }
        assert!(Lang::EN.0.source.is_empty());
    }

    /// Duplicate keys silently shadow each other; placeholders and trailing ellipses must survive
    /// translation, or a message loses its argument or its "opens a dialog" hint.
    #[test]
    fn catalogs_have_unique_keys_and_matching_placeholders() {
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            let (entries, _) = parse_entries(lang.0.source);
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

    /// The parity gate: a translated interface must not leave English menu items behind. Every
    /// node the menu bar draws, and every label of a menu-only UI command, needs an entry.
    #[test]
    fn simplified_chinese_covers_every_menu_label() {
        let zh = ZH();
        assert!(zh.complete_menus(), "Simplified Chinese participates in the coverage gate");
        let mut missing: Vec<&str> = Vec::new();
        for root in crate::menus::tree() {
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if !has(zh, &node.label) {
                    missing.push(Box::leak(node.label.clone().into_boxed_str()));
                }
                stack.extend(node.children);
            }
        }
        for (_, label, _, _) in crate::menus::UI_COMMANDS {
            if !has(zh, label) {
                missing.push(label);
            }
        }
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "Simplified Chinese is missing {} menu label(s): {missing:?}", missing.len());
    }

    /// Command labels reach the shortcuts window and the command dialogs, so they are held to the
    /// same coverage as menu paths — a translated menu over English shortcuts would be half a
    /// translation. (The parity report and MCP keep the English labels: they are machine surfaces.)
    #[test]
    fn simplified_chinese_covers_every_command_label() {
        let zh = ZH();
        let mut missing: Vec<&str> = soundcraft_engine::command_specs().iter().filter(|c| !has(zh, c.label)).map(|c| c.label).collect();
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "Simplified Chinese is missing {} command label(s): {missing:?}", missing.len());
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
        // A command-id entry wins over the label's translation.
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
