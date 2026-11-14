# Localization parity

> **Last reviewed:** 2026-11-05 · **Last updated:** 2026-11-13 · **Change:** minor (pt-BR catalog: helper-passed tooltips, Edit-window status bar, remove-session prompt, send help text and engine error text at display time) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft defaults to English and supports selecting Brazilian Portuguese. The pt-BR catalog
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

Open work: PR #143 (French and Spanish, a
Language setting), PR #141 (bundled multilingual font fallbacks), issue #67 (Ukrainian).

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese (Brazil) | pt-BR | Menu labels and the `localization::text` / `localized_format!` strings in `crates/ui-egui/src` (checked by a test; not native-reviewed; engine errors are translated at display time by `localization::error_text` from templates in the catalog, while low-level library errors and third-party plugin names stay English) | 🟡 partial | Latin ✅ | no | partial | 2–4 | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | ❌ no CJK font fallback, no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | ❌ no Hangul font fallback | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **0**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above.

## Usage

- Pick the language in Appearance ▸ Language, or run the `ui.language` command with
  `{"language": "pt-BR"}` (or `"en"`); with no parameter it reports the current language.
  Example: `soundcraft-cli run --demo --cmd 'ui.language={"language":"pt-BR"}'`.
- The choice is a global preference saved with the UI state (like the theme); recalling a saved
  window configuration does not change it.
- Strings missing from the catalog fall back to English. Add one with a `"English text" => "translated text"`
  arm in `crates/ui-egui/locales/pt-BR/mod.rs`; tests check placeholder parity, duplicates and that every
  string passed to `text` / `localized_format!` has an entry.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-13 | minor | Completed the pt-BR catalog: tooltips passed to buttons and toggles, `Not available yet.`, the send hover help, `no instrument`, the search placeholders, the Edit-window status bar (bit depth, track count, undo, ms/frame), the remove-session question and the transport/sample-rate readouts; engine errors now reach the status line through `error_text` |
| 2026-11-05 | minor | Added persisted language selection, the `ui.language` command, English fallback and a pt-BR catalog (menus, dialogs, tooltips, accessibility text, formatted messages); added a Usage section and catalog tests |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |
