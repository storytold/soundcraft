# Localization parity

> **Last reviewed:** 2026-11-01 · **Last updated:** 2026-11-01 · **Change:** major (language setting, zh-Hans catalog, CJK font fallback) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft's strings live in code (`crates/engine/src/cmd/*`, `crates/ui-egui/src/*`). A catalog
(`soundcraft-l10n`, L0) now translates them at display time: keys are the English source strings,
a language without an entry falls back to English, and command ids, menu paths and agent-facing
docs stay English because they are match keys. `ui.language` switches the interface live and is
saved with the preferences; the SoundCraft menu has a Language submenu (and the macOS system menu
bar rebuilds in the new language). Pro Tools ships its UI in **8 languages** (bundle `.lproj`
folders: English, German, Spanish, French, Japanese, Korean, Simplified Chinese, Traditional
Chinese).

**Localization: ~25 %** (measured: 2 of the target's 8 UI languages; English complete, and
zh-Hans covers the full menu tree, toolbar/transport and the common dialogs — roughly 600
catalog entries — while generated sentences (clear-clip confirmation, some status messages) and
the secondary panels still show English). Remaining ≈ 45–85 h: per-language translations
(≈ 3–5 h each) plus native-speaker review, bundled CJK fonts for the web build (PR #141 open),
RTL layout and complex-script shaping (egui has neither: ≈ 25–40 h, or ship Arabic/Hindi
menus-only), and the secondary panels' long tail.

Open work: PR #143 (French and Spanish translations), PR #141 (bundled multilingual font
fallbacks), issue #67 (Ukrainian).

CJK text renders on the desktop through a system-font fallback loaded at runtime (never
redistributed: Microsoft YaHei, PingFang/Hiragino Sans GB, Noto Sans CJK SC, same pattern as the
existing Hangul fallback). The web build has no system fonts and needs the bundled-font work
from PR #141.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---:|---|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | menus, toolbar, transport, common dialogs (~600); panels and generated sentences fall back | 🟡 common dialogs and tooltips; generated sentences pending | ✅ desktop (system CJK font fallback); ❌ web (no bundled font) | no | **catalog** | 1–2 | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese | pt | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | ✅ Hangul fallback (system font) | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **0**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-24 | major | Language setting (`ui.language`, Language menu, persisted), `soundcraft-l10n` catalog keyed by the English source, zh-Hans covering the full menu tree + toolbar/transport + common dialogs, Simplified-Chinese system-font fallback |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
