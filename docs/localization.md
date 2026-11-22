# Localization

SoundCraft's interface ships in English and 简体中文 (Simplified Chinese), and English source text
is the key: the strings in the code are what the catalogs translate, so a missing translation falls
back to readable English instead of an empty label. Catalogs are compiled into the binary; a
release has no catalog files to install or find, and the web build carries them too.

This page describes the mechanism. For the Chinese catalog itself see
[`localization-zh-hans.md`](localization-zh-hans.md); for terminology see
[`i18n/glossary.md`](i18n/glossary.md); for coverage against the incumbent see
[`localization-parity.md`](localization-parity.md).

## Supported languages

| Language | Code | Catalog | Notes |
|---|---|---|---|
| English (source) | `en` | — (the strings live in the code) | fallback for every catalog |
| 简体中文 — Simplified Chinese | `zh-Hans` | [`crates/ui-egui/locales/zh-Hans/mod.rs`](../crates/ui-egui/locales/zh-Hans/mod.rs) | generated from [`zh-hans.tsv`](../crates/ui-egui/src/i18n/zh-hans.tsv) |
| Português (Brasil) — Portuguese (Brazil) | `pt-BR` | [`crates/ui-egui/locales/pt-BR/mod.rs`](../crates/ui-egui/locales/pt-BR/mod.rs) | hand-written |

The registry is the `Language` enum in
[`crates/ui-egui/src/localization.rs`](../crates/ui-egui/src/localization.rs): `ALL` is the order
the Appearance window offers, `id()` is the tag stored in preferences, and `native_name()` is the
English source name the menu draws through the catalog.

## How a string is localized

- `localization::text("English source")` returns the translation for the language entered for the
  current frame, or the source when the catalog has no entry.
- `localized_format!("{name} send level", name = …)` translates the template and then fills it; the
  translation keeps the same placeholders (a test enforces that).
- `localization::error_text(message)` matches an engine error against the catalog's error templates
  and carries the embedded values over, so a low-level library error stays in English rather than
  turning into a half-translated sentence.
- A catalog module exposes `pub(super) fn translate(source: &str) -> Option<&'static str>` and, if
  it translates engine errors, `pub(super) const ERROR_TEMPLATES`.

Values built at run time (file, track and device names), command ids, menu paths used for lookup,
preference keys and values, file formats, the control protocol and third-party plugin names are
never translated. Engine diagnostics are English in the zh-Hans catalog by design; pt-BR carries
error templates.

The zh-Hans module is generated: edit
[`crates/ui-egui/src/i18n/zh-hans.tsv`](../crates/ui-egui/src/i18n/zh-hans.tsv) (one row per
string, `context<TAB>English<TAB>translation`) and rerun

```sh
python3 docs/i18n/tools/gen_zh_hans_locale.py          # rewrite the locale module
python3 docs/i18n/tools/gen_zh_hans_locale.py --check  # fail when it is out of date
```

The TSV is also the file `storytold/craft-i18n`'s `tools/sync_app.py` reads, so the catalog can move
to the shared repository when SoundCraft takes that step.

## Choosing the language

`Setup ▸ Appearance ▸ Language` (or the `ui.language` command) selects it while the app runs, and
the choice is a global preference saved with the UI state:

```sh
soundcraft-cli run --demo --cmd 'ui.language={"language":"zh-Hans"}'
```

With no parameter, `ui.language` reports the current language; an unknown tag is an error. In the
browser the language starts from `navigator.language` and follows the saved preference once one
exists.

## Fonts

egui's bundled faces carry no Han glyphs, so the interface needs a CJK fallback
([`crates/ui-egui/src/fonts.rs`](../crates/ui-egui/src/fonts.rs)):

- Native builds append the operating system's own Han face after every family, resolved through
  fontconfig (`lang=zh-cn`) with a list of per-platform candidates; nothing is redistributed.
- The web build cannot reach system fonts, so it embeds `assets/fonts/NotoSansSC-SoundCraftSubset.ttf`,
  a Noto Sans SC (OFL-1.1) subset at weight 400 covering the UI glyph set. Regenerate it after a
  catalog change with `python3 docs/i18n/tools/subset_web_font.py`, and keep
  [`ATTRIBUTION.md`](../ATTRIBUTION.md) in step; `cargo xtask assets` enforces that.
- Han comes before the Hangul fallback for every interface language except Korean, so a Chinese
  interface and Chinese names draw Chinese shapes even where fontconfig would resolve the Korean
  face of Noto Sans CJK first.

## Tests and gates

`cargo test -p soundcraft-ui-egui` covers the catalog and the fonts:

- every catalog entry parses, keys are unique and translations keep the English placeholders;
- every string passed to `localization::text` / `localized_format!` in `crates/ui-egui/src` has an
  entry in every shipped catalog, and every reference menu label is translated;
- the bundled web face draws the whole zh-Hans catalog in every family and stays inside its 2 MiB
  budget;
- with a system Han font installed, every family draws Chinese text.

`cargo xtask ci` runs those with the rest of the gates (fmt, clippy, assets, layers, wasm).

## Adding another language

1. Add a `locales/<tag>/mod.rs` with `translate` (and `ERROR_TEMPLATES` when engine errors should be
   translated), plus the `#[path]` module declaration in `localization.rs`.
2. Add the `Language` variant: `ALL`, `id()`, `native_name()`, `text()`, and the `error_text`
   template arm.
3. Translate `every_localized_source_string_has_a_catalog_entry`'s set (the test lists what is
   missing by file and string) and the menu labels.
4. Add a font fallback for the script, and for the web build either bundle a subset or document the
   gap; extend `ATTRIBUTION.md` when a file is added.
5. Update [`localization-parity.md`](localization-parity.md), `ROADMAP.md` and `docs/gaps.md`.

## Relation to `storytold/craft-i18n`

The org keeps shared translations in `craft-i18n` and reads them through an optional `CRAFT_I18N_DIR`
build input. SoundCraft has not cut over (roadmap stage 3d): like pt-BR, this catalog lives in the
repository, and its TSV is the pre-cutover file the sync script consumes, so a later migration only
moves data.