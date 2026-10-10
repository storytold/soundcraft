# Text and font support

SoundCraft embeds unmodified, openly licensed Noto fonts. Track names, clip names,
markers, playlists, dialogs and other egui text can fall back to these fonts on
macOS, Windows, Linux, FreeBSD and Web/WASM, including offline and on machines
without language fonts installed. Regular and bold text have matching fallbacks;
monospace fields use the regular fallback for scripts absent from the base font.

| Script / language group | Bundled font |
|---|---|
| Thai | Noto Sans Thai |
| Simplified / Traditional Chinese, Japanese kana, Korean Hangul | Noto Sans CJK |
| Arabic, including Arabic-script text used by Urdu, Sindhi and Kashmiri | Noto Sans Arabic |
| Hebrew | Noto Sans Hebrew |
| Devanagari (Hindi, Marathi, Nepali, Sanskrit and others) | Noto Sans Devanagari |
| Bengali / Assamese | Noto Sans Bengali |
| Gujarati | Noto Sans Gujarati |
| Gurmukhi (Punjabi) | Noto Sans Gurmukhi |
| Odia | Noto Sans Oriya |
| Tamil | Noto Sans Tamil |
| Telugu | Noto Sans Telugu |
| Kannada | Noto Sans Kannada |
| Malayalam | Noto Sans Malayalam |
| Ol Chiki (Santali) | Noto Sans Ol Chiki |
| Meetei Mayek (Manipuri) | Noto Sans Meetei Mayek |
| Sinhala | Noto Sans Sinhala |
| Tibetan | Noto Serif Tibetan |

These are font and text-data capabilities, not translations of the application
menus. Latin UI fonts and available system UI fonts retain their existing priority.
The bundled fonts cover their upstream character repertoires; this is not a promise
to cover every historical script, rare ideograph or Unicode character.

The pan-CJK collections contain all five regional faces. The fallback uses face 2
(Simplified Chinese), so shared Han characters use that region's glyph forms.
Japanese, Korean, Traditional Chinese and Hong Kong variants of shared Han are
not automatically selected by the content or operating system locale.

The egui 0.36 renderer shapes text, including contextual glyphs and combining
marks. Full Unicode bidirectional paragraph layout and visual cursor/selection
mapping remain limited upstream, including multiword Arabic/Hebrew names and
text mixed with Latin, numbers or punctuation.
Adding font coverage does not resolve those layout limitations. Text is always
stored in its original Unicode order; SoundCraft does not reverse or replace
session strings to approximate right-to-left rendering.

Tests check script sample glyphs in all three UI font families without system
fonts, retain the Thai combining-mark regression, and exercise multilingual input
through real track/clip rename dialogs followed by session serialization.

All font files, upstream revisions and licenses are recorded in
[`ATTRIBUTION.md`](../ATTRIBUTION.md) and the font directories' README files.
License texts are compiled into **About SoundCraft → Bundled font licenses**.
