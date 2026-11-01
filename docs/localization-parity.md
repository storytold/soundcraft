# Localization parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-22 · **Change:** minor (CJK font fallback: the UI families append system Japanese, Chinese and Korean faces, so Japanese and Chinese session, track and clip names draw instead of missing-glyph boxes, #141; previous: major, first per-language status) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft is English only. There is no string catalog: every menu label, dialog and tooltip is a
literal in Rust (`crates/engine/src/cmd/*`, `crates/ui-egui/src/*`). Pro Tools ships its UI in
**8 languages** (bundle `.lproj` folders: English, German, Spanish, French, Japanese, Korean,
Simplified Chinese, Traditional Chinese).

**Localization: ~12 %** (measured: 1 of the target's 8 UI languages, and English is the only one
complete). Remaining ≈ 70–110 h: a catalog and extraction pass (≈ 15–25 h), font fallbacks for
Devanagari/Arabic (system CJK faces landed, #141), RTL layout and complex-script shaping (egui has neither:
≈ 25–40 h, or ship Arabic/Hindi menus-only), then ≈ 3–5 h of agent translation per language, plus
native-speaker review (human).

Open work: PR #143 (French and Spanish, a Language setting), #141 (system CJK fallback landed;
Devanagari/Arabic fallback and RTL shaping still open), issue #67 (Ukrainian).

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | 0 (0 %) | ❌ | 🟡 system Chinese font fallback, no IME testing | no | none | 6–10 | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese | pt | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | 🟡 system Japanese font fallback, no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | 🟡 system Hangul font fallback, no IME testing | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **0**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | minor | CJK font fallback: system Japanese, Chinese and Korean faces are appended to every UI family, so Japanese and Chinese session, track and clip names draw instead of boxes (#141) |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
