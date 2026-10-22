# Localization parity

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-22 · **Change:** trivial (remaining UI literals translated; coverage gate extended to widget-helper tooltips) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft ships **English** (the source language) and **简体中文**. Every menu path segment,
every command label, every panel title, dialog, tooltip and status message the UI draws goes
through a catalog compiled into the binary (`crates/ui-egui/src/i18n/`), and the Chinese catalog
covers all of them; engine-side diagnostics (audio trouble text, health-report issue text, plugin
names) stay English because the control channel, the CLI and MCP report the same strings. Pro Tools
ships its UI in **8 languages** (bundle `.lproj` folders: English, German, Spanish, French,
Japanese, Korean, Simplified Chinese, Traditional Chinese).

**Localization: ~25 %** (measured: 2 of the target's 8 UI languages complete; menu, command and
literal coverage enforced by tests). Remaining ≈ 30–45 h: ≈ 3–5 h of agent translation per
language for the six missing Pro Tools languages (German, Spanish, French, Japanese, Korean,
Traditional Chinese — Japanese and Traditional Chinese ≈ 6–10 h each for the kana/JIS faces and
the Taiwan/HK terminology), then native-speaker review (human). Arabic and Hindi would add
≈ 25–40 h for RTL layout and complex-script shaping, which egui does not have.

What is done:

- **The catalog**: `zh-hans.tsv`, 1,141 entries — plain strings, plural forms (`@plural`) and a
  disambiguating `automation` context; parser support for command-id keys (`@id`) stays but no row
  needs it. Chinese takes one plural form.
- **The build**: `build.rs` packs the TSV into a shared key table plus per-language marker streams
  and deflates them into the binary, so releases and the web build carry no catalog files and no
  run-time file I/O; corrupt blobs fall back to English instead of crashing.
- **The plumbing**: English literals stay in the code as keys;
  `t`/`tr`/`tr_ctx`/`tr_id`/`trn`/`fmt` translate at draw time; `has` reports coverage. Command
  ids, menu paths used for logic, the control channel, the CLI, MCP and the parity report never see
  translated text.
- **Deliberate wording deviations**: the Chinese catalog maps meaning, not words — the Shuffle edit
  mode is 推移 (随机 would read as random order), automation Trim is 微调 while clip trim is 修剪,
  and Bypass is 旁通. Every such choice, with its corpus evidence, is recorded in
  [`i18n/glossary.md`](i18n/glossary.md) and the terminology table of
  [`localization-zh-hans.md`](localization-zh-hans.md).
- **Language choice**: one engine preference, `interface.language` (`auto` or a code), set from
  Setup ▸ Appearance (dropdown with live preview, Apply/Cancel/Reset) or with the
  `prefs.get`/`prefs.set`/`prefs.reset` commands; the old `ui.language` UI command is gone. Persisted
  as `preferences.json` on the desktop and `localStorage` on the web, migrated once from the old
  `ui.json` value. Auto reads `SOUNDCRAFT_LOCALE`, the desktop's preferred-language list
  (`sys-locale`, or `LC_ALL`/`LC_MESSAGES`/`LANG` on Linux/BSD), then `LANGUAGE`, `LC_ALL`,
  `LC_MESSAGES`, `LANG`; the web passes `navigator.languages`. `zh`, `zh_CN`, `zh-SG` and `zh-Hans`
  resolve here; `zh-TW`/`zh-HK`/`zh-Hant` do not. Switching is live — no restart.
- **Fonts**: a Han fallback (`system-han`) is appended to every family and, for Chinese and English
  interfaces, ordered before the Hangul fallback so Han glyphs draw Chinese-shaped; the web build
  embeds a Noto Sans SC subset (OFL, attributed) because browsers have no system fonts.
- **Gates**: tests cover packed round-trips, parsing, duplicate keys, placeholders, trailing
  `…`/colons, every menu path segment, every UI command and engine command label, every plain-lookup
  literal in the crate's top-level UI sources (read from disk, so new files are covered), the plural
  and locale-resolution rules, and the font fallback order and web-font glyph coverage.
- **Docs**: [`localization.md`](localization.md) is the mechanism reference;
  [`localization-zh-hans.md`](localization-zh-hans.md) covers the Chinese UI, its terminology and
  how to add another language; [`i18n/`](i18n/README.md) is the corpus-based wording knowledge
  base.

Open work: Traditional Chinese (needs a `zh-hant` catalog — the tag negotiation already routes
`zh-TW`/`zh-HK`/`zh-Hant` to it — plus a Taiwan/HK terminology pass), Japanese/Korean catalogs, RTL
layout, IME testing, and native-speaker review of the Chinese wording.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---|---|---:|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-hans | all (1,141 entries; menus, commands, dialogs, tooltips, status) | ✅ | ✅ Han fallback; bundled web subset; IME untested | no | **full** (engine diagnostics stay English) | 3–5 (review) | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese | pt | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | 🟡 Han fallback covers most glyphs; kana need the JIS face set, untested | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | ✅ Hangul fallback | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |
| Traditional Chinese | zh-hant | 0 (0 %) | ❌ | ✅ Han fallback | no | none | 6–10 | ✅ |

Other languages shipped: **1** (Simplified Chinese, plus the English source).

Per-language hours exclude the shared catalog and script work, which are done.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | trivial | Catalog 1,106 → 1,141 entries: the remaining UI literals that bypassed `t()` (toolbar icon tooltips, send headers and tooltips, bounce status, Memory Locations headers, Freeze/Unfreeze, MIDI editor buttons, record-hint tooltips) now translate; the coverage test also scans `icon_button`/`text_toggle` tooltip literals; malformed catalog lines are reported at run time |
| 2026-10-20 | minor | Wording pass and review: catalog 1,106 entries (Shuffle = 推移, new preference command labels); `interface.language` preference with `prefs.*` commands, Appearance preview and persistence (native `preferences.json`, web `localStorage`), legacy `ui.json` migration; catalogs packed and deflated at build time; Han-before-Hangul fallback order; bundled Noto Sans SC subset for the web; docs refreshed and wording knowledge base re-cross-checked (1,032 keys) |
| 2026-10-19 | major | Simplified Chinese shipped: catalog (1,114 entries), language selector, locale detection, Han font fallback, coverage gates; localization re-measured 12 % → 25 % |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
