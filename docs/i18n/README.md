# i18n tools and notes

Working files for SoundCraft's own catalog (`zh-Hans`). The runtime mechanism and the language list
are described in [`../localization.md`](../localization.md); the translator workflow in
[`../localization-zh-hans.md`](../localization-zh-hans.md); wording decisions in
[`glossary.md`](glossary.md).

| Path | What |
|---|---|
| `tools/gen_zh_hans_locale.py` | turns `crates/ui-egui/src/i18n/zh-hans.tsv` into `crates/ui-egui/locales/zh-Hans/mod.rs`; `--check` is the CI/`xtask`-style staleness gate |
| `tools/subset_web_font.py` | rebuilds `assets/fonts/NotoSansSC-SoundCraftSubset.ttf` from the catalog's glyph set and the pinned Noto Sans SC source |

## Why the catalog lives here

The org keeps shared translations in `storytold/craft-i18n`, read through an optional
`CRAFT_I18N_DIR` build input. SoundCraft has not cut over (roadmap stage 3d), so — like the pt-BR
catalog — this one ships in the repository, and the TSV is the pre-cutover file that
`craft-i18n`'s `tools/sync_app.py` consumes when the app does migrate. Nothing here depends on the
other repository being present.

## Clean room, restated

Write every row from the plain meaning of the English source. Do not read, copy or "check against"
another product's translated interface or translation files, and do not download copyleft or
proprietary string tables for reference. If a term needs a decision, write the reasoning in
[`glossary.md`](glossary.md) instead of citing a source we must not use.

The translations are original work by SoundCraft contributors (AI-assisted), licensed like the rest
of the repository (MIT OR Apache-2.0); see `crates/ui-egui/src/i18n/LICENSE-translations.txt`.