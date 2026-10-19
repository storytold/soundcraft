# Localization parity

> **Last reviewed:** 2026-10-19 · **Last updated:** 2026-10-19 · **Change:** major (Simplified Chinese shipped) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft ships **English** (the source language) and **简体中文**. Every menu path segment,
every command label, every panel title, dialog, tooltip and status message the UI draws goes
through a compiled-in catalog (`crates/ui-egui/src/i18n/`), and the Chinese catalog covers all of
them; engine-side diagnostics (audio trouble text, health-report issue text, plugin names) stay
English because the control channel, the CLI and MCP report the same strings. Pro Tools ships its
UI in **8 languages** (bundle `.lproj` folders: English, German, Spanish, French, Japanese, Korean,
Simplified Chinese, Traditional Chinese).

**Localization: ~25 %** (measured: 2 of the target's 8 UI languages complete; menu, command and
literal coverage enforced by tests). Remaining ≈ 30–45 h: ≈ 3–5 h of agent translation per
language for the six missing Pro Tools languages (German, Spanish, French, Japanese, Korean,
Traditional Chinese — Japanese and Traditional Chinese ≈ 6–10 h each for the kana/JIS faces and
the Taiwan/HK terminology), then native-speaker review (human). Arabic and Hindi would add
≈ 25–40 h for RTL layout and complex-script shaping, which egui does not have.

What is done:

- **The catalog**: `zh-hans.tsv`, 1,114 entries — plain strings, command-id keys (`@id`), plural
  forms (`@plural`) and a disambiguating `automation` context. Chinese takes one plural form.
- **The plumbing**: English literals stay in the code as keys; `t`/`tr`/`tr_ctx`/`tr_id`/`trn`/`fmt`
  translate at draw time; `tl!` for expression position. Command ids, menu paths used for logic,
  the control channel, the CLI, MCP and the parity report never see translated text.
- **Language choice**: Setup ▸ Appearance ▸ Language, or the `ui.language` command (`auto` or a
  code/locale tag). Auto reads `SOUNDCRAFT_LOCALE`, `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG` in
  that order. `zh`, `zh_CN`, `zh-SG` and `zh-Hans` resolve here; `zh-TW`/`zh-HK`/`zh-Hant` do not.
- **Fonts**: a simplified-Chinese fallback (`system-han`) is appended to every family; the Hangul
  fallback predates this work. No font assets are bundled.
- **Gates**: tests cover parsing, duplicate keys, placeholders, trailing `…`/colons, every menu
  path segment, every command label from `command_specs()`, every plain-lookup literal in the
  crate (`i18n::t("…")` or a bare `t("…")`, read from disk so new files are covered), and the
  plural and locale-resolution rules.
- **Docs**: [`localization-zh-hans.md`](localization-zh-hans.md) covers selecting the language,
  the file layout, the terminology table and how to add another language.

Open work: Traditional Chinese (needs a `zh-hant` catalog and a script-aware `lang_from_tag`
fallback, plus a Taiwan/HK terminology pass), Japanese/Korean catalogs, RTL layout, IME testing,
and native-speaker review of the Chinese wording.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | all (1,114 entries; menus, commands, dialogs, tooltips, status) | ✅ | ✅ Han fallback (`system-han`); IME untested | no | **full** (engine diagnostics stay English) | 3–5 (review) | ✅ |
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
| 2026-10-19 | major | Simplified Chinese shipped: catalog (1,114 entries), language selector, locale detection, Han font fallback, coverage gates; localization re-measured 12 % → 25 % |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
