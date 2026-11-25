# Localization parity

> **Last reviewed:** 2026-11-22 · **Last updated:** 2026-11-22 · **Change:** minor (zh-Hans catalog covers the fixed UI strings, with a Han system-font fallback and an OFL web subset; native-speaker review, RTL and complex-script shaping still pending) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft defaults to English and supports selecting Brazilian Portuguese and Simplified Chinese.
Both catalogs cover all fixed UI strings found in the current source: command labels, menus,
Appearance, About/Credits, Audio Health, Playback Engine, System Usage, Workspace, plugin parameter
and display names, dialogs and buttons, tooltips (including those localized inside buttons and
toggles), accessibility names, the Edit-window status bar, the remove-session question and formatted
messages. pt-BR also translates engine errors, which are matched against catalog templates at
display time by `localization::error_text`; zh-Hans keeps engine diagnostics in English. Values built
at run time (file, track and device names), engine command parameter documentation and third-party
plugin names stay as they are, and neither catalog has been independently or native-speaker
reviewed. Pro Tools ships its UI in **8 languages** (bundle `.lproj` folders: English, German,
Spanish, French, Japanese, Korean, Simplified Chinese, Traditional Chinese).

**Localization: ~19 %** (measured: 2 of the target's 8 UI languages, and neither translation has had
a native-speaker pass). Remaining ≈ 55–90 h: RTL layout and complex-script shaping (egui has
neither: ≈ 25–40 h, or ship Arabic/Hindi menus-only), the remaining catalogs (≈ 3–5 h of agent
translation each), the Japanese/Korean font faces, and native-speaker review (human).

Open work: PR #143 (French and Spanish, a Language setting), PR #141 (bundled multilingual font
fallbacks), issue #67 (Ukrainian).

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Est. to full (h) | In Pro Tools |
|---|---|---|---|---|---|---|---:|---|
| English | en | all (source) | ✅ | Latin ✅ | — | **full** | 0 | ✅ |
| Simplified Chinese | zh-Hans | ✅ 100 % of fixed UI strings: menus, dialogs, buttons, tooltips, accessibility names, status bar, formatted messages | ✅ | Han ✅ (system font on the desktop, OFL subset on the web) | no | **full** | 0–4 (native review) | ✅ |
| Spanish | es | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Hindi | hi | 0 (0 %) | ❌ | ❌ no Devanagari font or shaping | no | none | 8–15 | ❌ |
| Arabic | ar | 0 (0 %) | ❌ | ❌ no RTL layout, no shaping | no | none | 15–25 | ❌ |
| French | fr | 0 (0 %) — PR #143 pending | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Portuguese (Brazil) | pt-BR | ✅ 100 % of fixed UI strings: menus, dialogs, buttons, tooltips, accessibility names, status bar, formatted messages and engine errors; catalog tests pass | ✅ | Latin ✅ | no | **full** | 0–4 (native review) | ❌ |
| Indonesian | id | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ❌ |
| Japanese | ja | 0 (0 %) | ❌ | 🟡 Han fallback draws Japanese kanji, no kana face and no IME testing | no | none | 6–10 | ✅ |
| German | de | 0 (0 %) | ❌ | Latin ✅ | no | none | 3–5 | ✅ |
| Korean | ko | 0 (0 %) | ❌ | 🟡 Hangul system-font fallback, no IME testing | no | none | 6–10 | ✅ |
| Vietnamese | vi | 0 (0 %) | ❌ | 🟡 Latin with stacked diacritics, untested | no | none | 3–5 | ❌ |

Other languages shipped: **2**. (Pro Tools also ships Traditional Chinese, zh-Hant.)

Per-language hours exclude the shared catalog and script work above. A Han face now ships for the
desktop (system font, never redistributed) and the web (an OFL subset); Traditional Chinese and
Japanese would need their own Han faces, since the fallback list is Simplified-first.

## Usage

- Pick the language in Appearance ▸ Language, or run the `ui.language` command with
  `{"language": "pt-BR"}` or `{"language": "zh-Hans"}` (or `"en"`); with no parameter it reports the
  current language. Example: `soundcraft-cli run --demo --cmd 'ui.language={"language":"zh-Hans"}'`.
- The choice is a global preference saved with the UI state (like the theme); recalling a saved
  window configuration does not change it.
- Strings missing from the catalog fall back to English, and the tests check placeholder parity,
  duplicate keys and that every string passed to `text` / `localized_format!` has an entry in every
  shipped catalog.
- How a catalog is edited depends on how it is kept. pt-BR is hand-written: add a
  `"English text" => "translated text"` arm to `crates/ui-egui/locales/pt-BR/mod.rs`. zh-Hans is
  generated: add the row to `crates/ui-egui/src/i18n/zh-hans.tsv` and run
  `python3 docs/i18n/tools/gen_zh_hans_locale.py` — never edit the generated module by hand. See
  [localization.md](localization.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-22 | minor | Added the Simplified Chinese catalog on the shared `localization` module (1,383 strings, all menu labels and every `text`/`localized_format!` source covered), a Han system-font fallback ordered ahead of Hangul except for Korean, and a 1.1 MiB OFL Noto Sans SC subset for the web build; catalog and font tests added; localization estimate ~12 % → ~19 % |
| 2026-11-13 | minor | Completed the pt-BR catalog: tooltips passed to buttons and toggles, `Not available yet.`, the send hover help, `no instrument`, the search placeholders, the Edit-window status bar (bit depth, track count, undo, ms/frame), the remove-session question and the transport/sample-rate readouts; engine errors now reach the status line through `error_text` |
| 2026-11-05 | minor | Added persisted language selection, the `ui.language` command, English fallback and a pt-BR catalog (menus, dialogs, tooltips, accessibility text, formatted messages); added a Usage section and catalog tests |
| 2026-10-10 | major | Created: English only, no catalog; the twelve key languages at 0 %; Pro Tools' 8 languages listed from its bundle |