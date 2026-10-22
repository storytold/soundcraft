# Localization

SoundCraft's interface ships in English and 简体中文 (Simplified Chinese). English is the source
language: the strings in the code are the keys, and translations are plain TSV catalogs compiled
into the binary. A release therefore has no catalog files to install or find at run time, and the
web build carries its catalogs too. Everything that is *not* display text stays English: command
ids, menu paths used for logic, preference paths and values, file formats and the control
protocol. The interface language can be changed while the app is running.

This page describes the localization mechanism. For the Chinese catalog itself see
[`localization-zh-hans.md`](localization-zh-hans.md); for coverage versus the incumbent see
[`localization-parity.md`](localization-parity.md); for wording research and decisions see
[`i18n/README.md`](i18n/README.md).

## Supported languages

The registry is `LANGUAGES` in [`crates/ui-egui/src/i18n/mod.rs`](../crates/ui-egui/src/i18n/mod.rs).
Language 0 is always English, the fallback.

| Language | Code | Catalog | Plural forms | Menus complete |
|---|---|---|---|---|
| English (source) | `en` | — (strings live in the code) | one / other | — (source) |
| 简体中文 — Simplified Chinese | `zh-hans` | [`crates/ui-egui/src/i18n/zh-hans.tsv`](../crates/ui-egui/src/i18n/zh-hans.tsv) · 1,139 entries | one (Chinese has no plural inflection) | yes |

`code` is the lowercase BCP 47 tag stored in the preference; `name` is the native name shown in the
Appearance window; `plural` picks the form to use for a count; `complete_menus` promises that the
catalog covers every composed-interface string, which the tests enforce (see
[Tests and gates](#tests-and-gates)). The web build embeds the same catalogs.

## Choosing the language

### The preference

`interface.language` is an ordinary engine preference ([`crates/engine/src/prefs.rs`](../crates/engine/src/prefs.rs)):
`"auto"` (the default) or a language code such as `en` or `zh-hans`. The engine stores and
validates it but does not know which languages ship; an unregistered code is accepted and resolves
like `auto` in the UI. The value must be non-empty, at most 16 characters long, and ASCII letters,
digits or `-`.

Preference commands, usable from the CLI, the control channel, MCP and scripts (all always
enabled, all UI-free):

| Command | Label | Parameters |
|---|---|---|
| `prefs.get` | Get Preferences | `{"path": "section\|section.key"}`, omitted = everything |
| `prefs.set` | Set Preferences | `{"path": "section.key", "value": …}` or `{"values": {"section.key": …}}`; validated, all or nothing |
| `prefs.reset` | Reset Preferences | `{"path": "section\|section.key"}`, omitted = everything |

```json
{"method":"engine.execute","params":{"command":"prefs.set","params":{"path":"interface.language","value":"zh-hans"}}}
{"method":"engine.execute","params":{"command":"prefs.get","params":{"path":"interface.language"}}}
```

The old `ui.language` UI command has been removed, menu row included: language is a preference,
not a window action, and `ui.inspect` no longer reports it.

### Appearance window

**Setup ▸ Appearance** (command `window.ui_customization`, listed as **UI Customization** under
**Window**) shows the language row: **Auto (…)** with the detected language's name, English and
简体中文. Choosing a language previews *the window itself* in it — the preview is scoped to that
window through `i18n::language_scope`, so the rest of the app keeps drawing in the committed
language. **Apply** stores the choice with `prefs.set`; **Cancel** or closing the window discards it.
**Reset** restores the default interface scale, reduced motion, theme and language (`auto`) in one
go. If an agent changes the preference while a draft is open, the stale draft is dropped.

### What `auto` means

`auto` resolves the system language once, at startup, before the first frame (native
`SoundApp` startup calls `i18n::init_system_locale`; the web shell passes `navigator.languages` to
`i18n::init_system_locale_with`). Sources are tried in this order, and the first tag that matches a
registered language wins:

1. `SOUNDCRAFT_LOCALE` — a development/testing override; the value is trimmed, and empty counts as
   unset.
2. The desktop's preferred-language list (`sys-locale`: macOS and Windows preferred languages;
   `LC_ALL`, `LC_MESSAGES`, `LANG` on Linux and BSD).
3. The POSIX environment directly: `LANGUAGE`, then `LC_ALL`, `LC_MESSAGES`, `LANG`.

Hostile input is bounded: at most 64 tags, each at most 128 bytes and filtered to ordinary tag
characters, and the platform call is wrapped so a broken system API cannot crash the app. Tags are
normalised (strip a codeset or `@modifier`, `_` → `-`, lowercase) and tried from most to least
specific: `zh_CN.UTF-8` tries `zh-cn`, then `zh-hans`, then `zh`. Region tags that imply Traditional
Chinese (`zh-TW`, `zh-HK`, `zh-MO`, `zh-Hant`) negotiate to `zh-hant`, which does not ship yet, so
they fall back to English. `C` and `POSIX` mean English; an unsupported language falls back to
English.

The detected language is never written back: with `auto`, the preference stays `"auto"` and the
choice follows the system on the next start. The web build has no platform locale API; the browser's
language list is the whole input.

### Switching live

The shell resolves the preference every frame, before anything draws
(`SoundApp::logic` → `i18n::apply_preference`). Labels, menus, tooltips and dialogs are drawn
through `t`/`tr`/`fmt`, so open windows switch language with no restart. Because the font fallback
order follows the language, a language change re-installs the fonts (`fonts::install`) and the
first frame in the new language is deferred by one frame while egui rebuilds the font atlas.

## Persistence

The preference is stored by each shell; the engine never touches the filesystem.

**Desktop:** `preferences.json`, beside `ui.json`, in the settings directory
([`apps/soundcraft/src/config.rs`](../apps/soundcraft/src/config.rs)):

| Platform | Default settings directory |
|---|---|
| macOS | `~/Library/Application Support/SoundCraft` |
| Windows | `%APPDATA%\SoundCraft` |
| Linux / BSD | `$XDG_CONFIG_HOME/soundcraft`, else `~/.config/soundcraft` |

`SOUNDCRAFT_CONFIG_DIR` replaces the whole directory (no app-name suffix; an explicitly empty value
disables the settings directory), and `portable.txt` beside the executable uses
`SoundCraftData` next to it when that folder is writable. The file is loaded leniently — bounded at
64 KiB, unknown keys ignored, a malformed or oversized document is reported and replaced by
defaults — and written atomically (temporary file, flush, rename) whenever the preference revision
changes and on exit. `SOUNDCRAFT_NO_PREFS` disables loading and saving; use it for throwaway runs.
On first start, a language stored by the old shell in `ui.json` is migrated once into
`preferences.json` (an old `auto` or a missing value is just the default). Hand-editing
`preferences.json` while the app runs has no effect: it is read at startup and rewritten on the
next change — use `prefs.set` instead.

**Web:** the same JSON under the `soundcraft.preferences` key in `localStorage`
([`apps/soundcraft-web/src/web.rs`](../apps/soundcraft-web/src/web.rs)), loaded before the first
frame and rewritten when the revision changes. A browser that refuses storage (private mode) simply
gets no persistence — loading and saving are skipped — and a failed write (for example a full
quota) logs a warning; the session keeps working with defaults.

## Catalog format

A catalog is one UTF-8 TSV file per language,
[`crates/ui-egui/src/i18n/<code>.tsv`](../crates/ui-egui/src/i18n/). Each line has three
TAB-separated columns: **context**, **source** (English) and **translation**. Blank lines and lines
starting with `#` are comments and pass through the build unchanged; the shipped catalogs use the
header to document the rules.

```
# format examples; the shipped catalog has no @id rows today
Play	播放
Open {path}	打开 {path}
@id	edit.undo	撤销
@plural	{n} clip|{n} clips	{n} 个片段
automation	Off	关闭
```

| Context | Meaning |
|---|---|
| empty | A plain key: menus, command labels, window titles, dialogs, tooltips. |
| `@id` | A key by stable command id instead of the English label, so a label can change without breaking translations; lookup uses `tr_id`. The shipped catalogs currently use none — plain rows cover every command. |
| `@plural` | Column 2 is the English `one|other` pair (a language may need more forms); column 3 is that language's forms, `|`-separated. `trn` picks the form with the registry's plural rule, then fills `{n}`. |
| any other text | A namespace (`automation`) for strings that read the same in English but differ by meaning. Layout uses `tr_ctx`. |

Rules the tests enforce:

- Translations are keyed by *(context, source)*. A key is looked up exactly as written in the
  code — `t("Play")` is the source string, not a symbol.
- `{name}` placeholders must survive translation; the catalog must use the same set of placeholders
  as its English source.
- A trailing ellipsis (`…`) marks a menu item that opens a dialog and must stay; a trailing colon
  must stay a colon (`:` may become `：` in Chinese, but must not appear or vanish).
- The same *(context, source)* may appear only once; duplicate keys fail the catalog test.
- A malformed line (not exactly three TAB-separated, non-empty source and translation) is a
  warning in the build, which keeps the line verbatim so the run-time parser reports it and the
  tests fail; the app itself never panics on a bad catalog.
- Escapes inside source and translation: `\n`, `\t` and `\\`.

The shipped Simplified Chinese catalog has **1,139 entries**: 1,123 plain, 9 `@plural` and 7 in the
`automation` namespace. For example `SHUFFLE` is 推移 (edit-mode toolbar), `Shuffle Lock` is
推移锁定, and the Shuffle tooltips are 推移模式.

## Build pipeline

[`crates/ui-egui/build.rs`](../crates/ui-egui/build.rs) packs the catalogs at compile time and
re-runs when `src/i18n/` changes:

1. It reads `src/i18n/*.tsv`, largest catalog first: the largest defines the key order, so its
   entries are almost all the short `\x01` marker and smaller catalogs mostly reference
   neighbouring keys.
2. A shared key table is built once from all catalogs: every `context<TAB>source` pair in
   first-appearance order, numbered.
3. Each catalog becomes a `OUT_DIR/i18n/<code>.tsv.deflate` blob, and the key table a
   `OUT_DIR/i18n/keys.deflate` blob, both compressed with the best Deflate level.
4. Each entry line is packed with one marker byte; non-entry lines (comments, headers) pass
   through verbatim unless they themselves start with a marker byte, in which case `\x03`
   escapes them:

| Prefix | Meaning |
|---|---|
| `\x01` | the next expected key (the common case); the translation follows the marker directly |
| `\x02<delta>` | the key is `next expected index + delta` (signed decimal), followed by `TAB` + translation |
| `\x03` | the rest of the line is literal text (a comment or header line that started with a marker byte) |

The runtime embeds both blobs with `include_bytes!` (the `catalog!` macro) — there is no file I/O
and no build-time JSON to parse, which keeps the web download small. Corrupt or truncated blobs are
contained: the runtime refuses to inflate past `MAX_CATALOG_BYTES` (16 MiB) and, if a catalog cannot
be inflated or parsed, warns and falls back to English rather than crashing.

## Runtime

| Piece | Where |
|---|---|
| Language registry (`LANGUAGES`) | `crates/ui-egui/src/i18n/mod.rs` |
| Catalog parser and lookups | `crates/ui-egui/src/i18n/catalog.rs` |
| System-locale detection | `crates/ui-egui/src/i18n/system.rs` |
| Catalogs | `crates/ui-egui/src/i18n/*.tsv` |
| Preference model and commands | `crates/engine/src/prefs.rs`, `crates/engine/src/cmd/prefs.rs` |
| Shell wiring (persistence, fonts) | `apps/soundcraft/src/{main,config}.rs`, `apps/soundcraft-web/src/web.rs` |
| Appearance window | `crates/ui-egui/src/extra_windows.rs` |
| Lookups in panels | `crates/ui-egui/src/*.rs` |

The current language is a thread-local in `i18n/mod.rs` (`CURRENT`, private), default English. UI
code calls:

| Call | Use |
|---|---|
| `t("…")` | translate in the current language (the common case) |
| `tr(lang, "…")` | translate for an explicit language |
| `tr_ctx(lang, "context", "…")` | translate a namespaced string |
| `tr_id(lang, "edit.undo", "Undo")` | translate by command id (falls back to the label) |
| `trn(lang, n, "one", "other")` | plural form for `n`, then fill `{n}` |
| `fmt("Open {path}", args)` | fill `{name}` placeholders from a lookup once |
| `has(lang, "…")` | is this key translated in that language |
| `sentence_space()` | `""` in Chinese, `" "` elsewhere, for sentences joined from parts |

`fmt` substitutes in a single pass: substituted values are never rescanned for placeholders. A
missing translation returns the English source, so no label is ever blank. `apply_preference`
resolves the preference and installs it as the current language; `with_language` and
`language_scope` draw in another language for the duration of a closure/scope (the Appearance
preview uses the scope form). Scopes are per thread and restore the previous language on drop;
work on a background thread starts from English. A command-line or headless process that never
calls `init_system_locale` draws English under `auto`.

## Fonts

Native builds use the operating system's own fonts
([`crates/ui-egui/src/fonts.rs`](../crates/ui-egui/src/fonts.rs)) and redistribute no font files
(egui's default fonts cover Latin). At startup, and again whenever the interface language changes,
SoundCraft appends two CJK fallbacks to every font family: a Han face (PingFang, Songti, Microsoft
YaHei, Noto Sans CJK's Simplified Chinese face, Source Han Sans SC, WenQuanYi, …) and a Hangul face
(Apple SD Gothic Neo, Malgun Gothic, Noto Sans CJK's Korean face, NanumGothic). Every interface
language gets both, so Korean session, track and clip names draw whatever the UI language. The
order follows the language: Chinese and English put Han before Hangul — otherwise a Linux `fc-match`
that resolves the Korean Noto face first would steal the Han glyphs and draw Korean-styled shapes —
and a Korean interface reverses the two. Font files are found through well-known paths and
`fc-match` on Linux/BSD, must be TrueType/OpenType under 64 MiB, and are resolved and cached once
per process; `SOUNDCRAFT_NO_SYSTEM_FONTS` disables loading them.

The web build has no system fonts, so it ships a **Noto Sans SC subset** under the SIL Open Font
License ([`assets/fonts/NotoSansSC-SoundCraftSubset.ttf`](../assets/fonts/NotoSansSC-SoundCraftSubset.ttf),
attributed in [`ATTRIBUTION.md`](../ATTRIBUTION.md)) and adds it as the lowest-priority fallback of
every family. The subset carries weight 400 and a superset of the UI glyph set, not the whole font:
printable ASCII and Latin-1, the GB 2312 level-1 common Han characters (3,755), the UI's
punctuation, arrows and symbols, and every BMP character any shipped catalog uses — about 4,000
codepoints and 1.1 MiB. A test fails when the Chinese catalog uses a character the subset lacks, so
the asset is regenerated with the catalog:

```
python3 docs/i18n/tools/subset_web_font.py --check        # does the shipped subset still match?
python3 docs/i18n/tools/subset_web_font.py NotoSansSC-VariableFont_wght.ttf   # regenerate
```

The full font is a [noto-cjk release](https://github.com/notofonts/noto-cjk); the script pins
weight 400 with fontTools' instancer, collects the codepoints above and runs the subsetter, keeping
the OFL name table. Chinese labels therefore render on the web as well as on the desktop under
2 MiB.

Localization is text-only: the UI has no complex-script shaping and no right-to-left layout, so
scripts that need joining or bidi order are not supported yet. Input methods (IME) are provided by
the platform and pass through to normal text fields; they are not exercised by the automated UI
tests.

## Tests and gates

The localization tests live in `crates/ui-egui/src/i18n/mod.rs`, `catalog.rs` and `system.rs`, and
run in `cargo xtask ci`:

| Test / gate | Enforces |
|---|---|
| `packed_catalogs_round_trip_to_their_sources` | pack → embed → inflate → unpack → parse reproduces each TSV |
| `bundled_catalogs_unpack_and_parse_without_errors` | every shipped catalog inflates, parses and has no malformed lines |
| `catalogs_have_unique_keys_and_matching_placeholders` | no duplicate keys; placeholders, trailing ellipsis and colon match the English source |
| `simplified_chinese_covers_the_composed_interface` | Simplified Chinese (the catalog with `complete_menus`) covers every menu-tree label, UI command label, engine command label and menu path segment |
| `every_looked_up_literal_has_a_translation` | every bare `t("…")` literal in the UI crate has an entry |
| `cjk_fallback_order_follows_the_interface_language`, `every_family_puts_han_before_hangul_for_chinese` | the CJK fallback order follows the interface language (Han first except Korean) |
| `bundled_web_font_covers_the_zh_hans_catalog`, `bundled_web_font_draws_the_zh_hans_catalog_in_every_family`, `bundled_web_font_stays_within_its_size_budget` | the bundled web font covers every catalog glyph in every family and stays inside its 2 MiB budget |
| `every_family_draws_hangul_and_han_with_system_fonts` | both CJK fallbacks reach every family on a system that has the fonts (skipped otherwise) |
| `translations_look_translated`, `placeholders_are_filled_after_translation`, `chinese_counts_take_one_form` | spot checks that lookups return real translations and `trn`/`fmt` fill correctly |
| `chinese_tags_resolve_to_simplified`, `candidates_walk_from_specific_to_general`, `preferences_resolve_with_fallback` | tag negotiation, including `zh-CN`, `zh_TW` and `C`/`POSIX` |
| `fmt_does_not_rescan_substituted_values`, `drawing_language_is_per_thread_and_restored` | placeholder safety and thread-local scoping |
| `prefs` tests in `crates/engine/src/prefs.rs` / `cmd/prefs.rs` | defaults, validation, all-or-nothing sets, reset, lenient JSON |
| `language_is_an_engine_preference_not_a_ui_command` | no `ui.language` command remains |
| migration and storage tests in `apps/soundcraft/src/config.rs` | atomic round trip, size cap, one-time legacy migration |

`complete_menus` is a promise, not a preference: a catalog may ship without it (the coverage gate
does not check that language), but the Chinese catalog sets it, so the gate runs for every shipped
Chinese string. The gate is written for Simplified Chinese, so a new language that sets
`complete_menus` extends it in the same change. `cargo xtask layers` keeps the interface crate the
only egui user; the catalogs and the i18n module live in it.

## Adding a language

1. Copy the header of `crates/ui-egui/src/i18n/zh-hans.tsv` to
   `crates/ui-egui/src/i18n/<code>.tsv` and translate — clean-room: no proprietary translation
   memory, glossary or copied strings (see the [knowledge base](i18n/README.md)); keep
   placeholders, ellipses and colons.
2. Add a `LangInfo` row to `LANGUAGES` in `mod.rs`: code, native name, `source: catalog!("<code>")`,
   plural rule, `complete_menus`. English must stay index 0.
3. Add the plural rule only if the language needs it; languages with one form use `plural_none`.
4. Region fallbacks for `auto` (for example `zh-TW` → `zh-hant`) are a code change in `candidates`
   or the registry — do it in the same change that ships the language.
5. Run the catalog tests: `cargo test -p soundcraft-ui-egui --lib i18n`. Coverage failures list the
   missing strings. Set `complete_menus` only when the coverage test passes.
6. Translation files are covered by [`LICENSE-translations.txt`](../crates/ui-egui/src/i18n/LICENSE-translations.txt)
   (MIT OR Apache-2.0, same as the code). A bundled font needs a row in `ATTRIBUTION.md`.
7. Update [`localization-parity.md`](localization-parity.md) and the language table in
   [`ROADMAP.md`](../ROADMAP.md).

Render a language offscreen to check fonts and layout — the script can set the preference, and
`SOUNDCRAFT_LOCALE` covers the `auto` path:

```sh
printf '%s\n' \
  '{"method":"engine.execute","params":{"command":"prefs.set","params":{"path":"interface.language","value":"zh-hans"}}}' \
  '{"shot":"zh-hans.png"}' > /tmp/zh-hans.jsonl
cargo run -p soundcraft-ui-egui --example ui_shot -- /tmp/zh-hans.png /tmp/zh-hans.jsonl
```

## Wording

The Chinese wording is researched, not guessed: `docs/i18n/` collects the translations of six
open-source DAWs, `tools/cross_check.py` compares them key by key, and every deliberate choice is
recorded in [`i18n/glossary.md`](i18n/glossary.md). Regenerate the report after a corpus or
catalog change:

```sh
python3 docs/i18n/tools/cross_check.py
```

The corpora are third-party (GPL) data used only for comparison and shipped nowhere; the catalog
itself is original work under SoundCraft's licence.

## Related docs

- [`localization-zh-hans.md`](localization-zh-hans.md) — the Simplified Chinese UI: what is translated, terminology, font and acceptance notes.
- [`localization-parity.md`](localization-parity.md) — coverage versus the incumbent, language roadmap, estimates.
- [`i18n/README.md`](i18n/README.md) — the corpus knowledge base and how to use it.
- [`i18n/glossary.md`](i18n/glossary.md) — curated terminology decisions.
- [`i18n/cross-check-report.md`](i18n/cross-check-report.md) — generated cross-corpus comparison.
- [`contributors.md`](contributors.md) — the other thing compiled into the binary by `build.rs`.
