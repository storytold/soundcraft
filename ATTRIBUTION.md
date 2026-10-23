# Attribution

Every asset bundled with or committed to SoundCraft is listed here with its author, source and
license. `cargo xtask assets` fails CI when a tracked asset file has no row.

**Rule:** no Avid, Pro Tools, Adobe or other proprietary icons, images, sounds or presets — ever.
Assets must be original, public domain (CC0) or permissively licensed. See `AGENTS.md`.

## Icons

All UI icons (tools, transport, track buttons) are drawn procedurally in
`crates/ui-egui/src/icons.rs` by the SoundCraft contributors (MIT OR Apache-2.0). No UI icon image
files are used. The app icon (`assets/app-icon/`, a singing nightingale drawn as original SVG by
the SoundCraft contributors) is listed under Files below.

## Audio

The demo session's audio (drums, bass, pad) and its MIDI parts are synthesised in code in
`crates/engine/src/demo.rs` by the SoundCraft contributors (MIT OR Apache-2.0). No sample files
are bundled.

## Fonts

SoundCraft uses the default fonts shipped inside the `egui` crate (Ubuntu-Light: Ubuntu Font
Licence 1.0; Hack: MIT; Noto Emoji: OFL-1.1; emoji-icon-font: OFL-1.1/MIT) and, at runtime, the
operating system's own UI fonts when available (never redistributed; native builds only).

The web build has no operating-system fonts, and egui's bundled fonts carry no Han glyphs. It alone
bundles `assets/fonts/NotoSansSC-SoundCraftSubset.ttf`: a subset of the Noto Sans SC variable font,
pinned at weight 400 and reduced with fontTools to the UI glyph set (the zh-Hans catalog, the
GB 2312 level-1 common characters, ASCII, Latin-1 letters and the UI's punctuation, arrows and
triangles; about 1.1 MiB). It is SIL Open Font License 1.1 — see `assets/fonts/OFL.txt`. The full
font is never bundled, and native builds do not embed the subset (they keep using the system's
fonts, which also cover user-typed text outside the subset).

## Files

| Path | Author | Source | License |
|---|---|---|---|
| `docs/brand/artcraft-logo.svg` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms (`docs/brand/LICENSE-brand.txt`) |
| `docs/brand/artcraft-logo.png` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-logo-white.svg` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-logo-white.png` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-mark.svg` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-mark.png` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-mark-black.svg` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `docs/brand/artcraft-mark-black.png` | ArtCraft Team | craftrules `assets/brand/` | ArtCraft brand terms |
| `assets/app-icon/icon.svg` | SoundCraft contributors | original (hand-placed SVG paths; placeholder app icon) | MIT OR Apache-2.0 |
| `packaging/macos/dmg/` (all files) | @XusBadia | original (macOS DMG window background and layout, from the app icon; see `packaging/macos/dmg/README.md`) | MIT OR Apache-2.0 |
| `assets/app-icon/soundcraft-1024.png` | SoundCraft contributors | original, rendered from `icon.svg` | MIT OR Apache-2.0 |
| `assets/app-icon/soundcraft-macos-512.png` | SoundCraft contributors | original, rendered from `icon.svg` | MIT OR Apache-2.0 |
| `assets/app-icon/soundcraft.icns` | SoundCraft contributors | original, rendered from `icon.svg` | MIT OR Apache-2.0 |
| `assets/app-icon/soundcraft.ico` | SoundCraft contributors | original, rendered from `icon.svg` | MIT OR Apache-2.0 |
| `assets/app-icon/hicolor/` | SoundCraft contributors | original, rendered from `icon.svg` (16–512 px PNGs and the scalable SVG) | MIT OR Apache-2.0 |
| `docs/images/` | SoundCraft contributors | screenshots of SoundCraft rendered from the synthesised demo session | MIT OR Apache-2.0 |
| `assets/fonts/NotoSansSC-SoundCraftSubset.ttf` | Google LLC / Noto Project Authors | Noto Sans SC variable font, subset to the UI glyph set and pinned at weight 400 (generated with fontTools) | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`) |
| `assets/fonts/OFL.txt` | Noto Project Authors / SIL | SIL Open Font License 1.1 text | OFL-1.1 |
| `crates/ui-egui/src/i18n/zh-hans.tsv` | SoundCraft contributors (AI-assisted) | original translation of SoundCraft's English source keys (clean-room; no proprietary translation resources) | MIT OR Apache-2.0 |
| `crates/ui-egui/src/i18n/LICENSE-translations.txt` | SoundCraft contributors (AI-assisted) | original clean-room statement and translation license | MIT OR Apache-2.0 |
