# Localization parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-05 · **Change:** minor (expanded pt-BR UI catalog: menus, dialogs, tooltips and formatted messages) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft defaults to English and supports selecting Brazilian Portuguese. The pt-BR catalog
covers the main menu labels, Appearance controls, About/Credits, the Audio Health, Playback Engine,
System Usage and Workspace panels, dialogs and their buttons, tooltips, accessibility names and the
formatted messages they use. Coverage is not yet complete or independently verified: parts of the
engine command descriptions, plugin/editor surfaces and long help text still fall back to their
English source strings. Pro Tools ships its UI in **8 languages** (bundle `.lproj` folders: English,
German, Spanish, French, Japanese, Korean, Simplified Chinese, Traditional Chinese).

**Localization: ~12 %** (measured: 1 of the target's 8 UI languages, and English is the only one
complete). Remaining ≈ 70–110 h: a catalog and extraction pass (≈ 15–25 h), a font fallback for
CJK/Devanagari/Arabic (PR #141 open), RTL layout and complex-script shaping (egui has neither:
≈ 25–40 h, or ship Arabic/Hindi menus-only), then ≈ 3–5 h of agent translation per language, plus
native-speaker review (human).

Open work: PR #1346 (Brazilian Portuguese, partial catalog), PR #143 (French and Spanish, a
Language setting), PR #141 (bundled multilingual font fallbacks), issue #67 (Ukrainian).

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese (Brazil) | pt-BR | Menus, Appearance, main panels, dialogs, tooltips and formatted messages (partial, not independently verified) | 🟡 partial | Latin ✅ | no | partial | 3–5 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | ❌ no Hangul font fallback | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **0**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-05 | minor | Expanded the pt-BR catalog and formatted messages (PR #1346): dialogs/tooltips/accessibility text localized, user-defined route names kept untranslated |
| 2026-11-03 | minor | Added persisted language selection, the `ui.language` command, English fallback, and initial pt-BR translations for main menus and Appearance controls |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
