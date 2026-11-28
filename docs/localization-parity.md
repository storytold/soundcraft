# Localization parity

> **Last reviewed:** 2026-11-13 · **Last updated:** 2026-11-27 · **Change:** minor (Spanish catalog completed, with native-speaker terminology review; previously minor: French and Spanish catalogs, partial; previously minor: pt-BR catalog completed: all fixed UI strings translated and covered by tests; native-speaker review, CJK/RTL font fallbacks and RTL script shaping still pending) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft defaults to English and supports selecting Brazilian Portuguese, French and Spanish. The
Spanish catalog covers the same fixed UI strings and engine errors as pt-BR; the French catalog is
partial (missing strings show in English). The pt-BR catalog
covers all fixed UI strings found in the current source: command labels, menus, Appearance,
About/Credits, Audio Health, Playback Engine, System Usage, Workspace, plugin parameter and display
names, dialogs and buttons, tooltips (including those localized inside buttons and toggles),
accessibility names, the Edit-window status bar, the remove-session question and formatted
messages. Engine errors are matched against catalog templates at display time by
`localization::error_text`. Values built at run time (file, track and device names), engine command
parameter documentation and third-party plugin names stay as they are, and the catalog has not been
independently or native-speaker reviewed. Pro Tools ships its UI in **8 languages** (bundle
`.lproj` folders: English, German, Spanish, French, Japanese, Korean, Simplified Chinese,
Traditional Chinese).

**Localization: ~12 %** (measured: 1 of the target's 8 UI languages, and English is the only one
complete). Remaining ≈ 70–110 h: a catalog and extraction pass (≈ 15–25 h), a font fallback for
CJK/Devanagari/Arabic (PR #141 open), RTL layout and complex-script shaping (egui has neither:
≈ 25–40 h, or ship Arabic/Hindi menus-only), then ≈ 3–5 h of agent translation per language, plus
native-speaker review (human).

Open work: completing the French catalog (rows also kept in storytold/craft-i18n for
when SoundCraft reads it), PR #141 (bundled multilingual font fallbacks), issue #67 (Ukrainian).

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| Spanish | es | ✅ 100 % of fixed UI strings (1,960) and engine errors (254), as pt-BR; catalog and menu-coverage tests pass | 🟡 partial | Latin ✅ | yes (es-ES, sound engineer; terminology reviewed) | full | 0–2 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 🟡 1,368 of ~2,210 catalog strings (≈ 62 %); engine errors not yet translated | 🟡 partial | Latin ✅ | no | partial | 1–3 | ✅ |
|| Portuguese (Brazil) | pt-BR | ✅ 100 % of fixed UI strings: menus, dialogs, buttons, tooltips, accessibility names, status bar, formatted messages and engine errors; catalog tests pass | 🟡 partial | Latin ✅ | no (native review pending) | full | 0–4 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | ❌ no Hangul font fallback | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **0**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above.

## Usage

- Pick the language in Appearance ▸ Language, or run the `ui.language` command with
  `{"language": "pt-BR"}` (or `"en"`, `"fr"`, `"es"`); with no parameter it reports the current language.
  Example: `soundcraft-cli run --demo --cmd 'ui.language={"language":"pt-BR"}'`.
- The choice is a global preference saved with the UI state (like the theme); recalling a saved
  window configuration does not change it.
- Strings missing from the catalog fall back to English. Add one with a `"English text" => "translated text"`
  arm in `crates/ui-egui/locales/<tag>/mod.rs`; tests check placeholder parity, duplicates and that every
  string passed to `text` / `localized_format!` has a pt-BR entry.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-27 | minor | Completed the Spanish catalog (all fixed UI strings and engine errors) with a native-speaker terminology review; Spanish menu-coverage test; the path prompt's `File path` and `Folder path` fields are translated apart from the Auto Pan `Path` parameter |
| 2026-11-24 | minor | French and Spanish catalogs (`locales/fr`, `locales/es`, 1,368 strings each) from PR #143, on the `localization` layer; Appearance ▸ Language lists them; estimates unchanged |
| 2026-11-13 | minor | Completed the pt-BR catalog: tooltips passed to buttons and toggles, `Not available yet.`, the send hover help, `no instrument`, the search placeholders, the Edit-window status bar (bit depth, track count, undo, ms/frame), the remove-session question and the transport/sample-rate readouts; engine errors now reach the status line through `error_text` |
| 2026-11-05 | minor | Added persisted language selection, the `ui.language` command, English fallback and a pt-BR catalog (menus, dialogs, tooltips, accessibility text, formatted messages); added a Usage section and catalog tests |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
