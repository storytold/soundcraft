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

SoundCraft bundles regular and bold Noto fallback fonts for Thai, Chinese, Japanese, Korean,
Arabic, Hebrew and Indic scripts on all platforms, including Web/WASM, under the SIL Open Font
License 1.1. The unmodified upstream fonts and licenses are in `assets/fonts/`; licenses are also
available in About SoundCraft. See [text support](docs/text-support.md) for coverage and RTL limits.
It uses the default fonts shipped inside the `egui` crate
(Ubuntu-Light: Ubuntu Font Licence 1.0; Hack: MIT; Noto Emoji: OFL-1.1; emoji-icon-font: OFL-1.1/MIT)
and, at runtime, the operating system's own UI fonts when available (never redistributed).

## Files

| Path | Author | Source | License |
|---|---|---|---|
| `assets/fonts/noto-sans-thai/NotoSansThai-Regular.ttf` | The Noto Project Authors | [Noto Sans Thai](https://github.com/notofonts/noto-fonts/tree/main/hinted/ttf/NotoSansThai), revision recorded in the adjacent README | SIL Open Font License 1.1 (`assets/fonts/noto-sans-thai/OFL.txt`) |
| `assets/fonts/noto-sans-thai/NotoSansThai-Bold.ttf` | The Noto Project Authors | [Noto Sans Thai](https://github.com/notofonts/noto-fonts/tree/main/hinted/ttf/NotoSansThai), revision recorded in the adjacent README | SIL Open Font License 1.1 (`assets/fonts/noto-sans-thai/OFL.txt`) |
| `assets/fonts/noto-sans-thai/OFL.txt` | The Noto Project Authors | [upstream license](https://github.com/notofonts/noto-fonts/blob/main/LICENSE) | SIL Open Font License 1.1 |
| `assets/fonts/noto/NotoSansArabic-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansArabic) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansArabic-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansArabic) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansHebrew-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansHebrew) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansHebrew-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansHebrew) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansDevanagari-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansDevanagari) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansDevanagari-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansDevanagari) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansBengali-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansBengali) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansBengali-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansBengali) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansGujarati-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansGujarati) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansGujarati-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansGujarati) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansGurmukhi-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansGurmukhi) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansGurmukhi-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansGurmukhi) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansOriya-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansOriya) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansOriya-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansOriya) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansTamil-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansTamil) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansTamil-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansTamil) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansTelugu-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansTelugu) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansTelugu-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansTelugu) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansKannada-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansKannada) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansKannada-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansKannada) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansMalayalam-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansMalayalam) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansMalayalam-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansMalayalam) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansOlChiki-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansOlChiki) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansOlChiki-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansOlChiki) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansMeeteiMayek-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansMeeteiMayek) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansMeeteiMayek-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansMeeteiMayek) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansSinhala-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansSinhala) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSansSinhala-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSansSinhala) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSerifTibetan-Regular.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSerifTibetan) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/NotoSerifTibetan-Bold.ttf` | The Noto Project Authors | [upstream](https://github.com/notofonts/noto-fonts/tree/ffebf8c1ee449e544955a7e813c54f9b73848eac/hinted/ttf/NotoSerifTibetan) | SIL Open Font License 1.1 (`assets/fonts/noto/OFL.txt`) |
| `assets/fonts/noto/OFL.txt` | The Noto Project Authors | [upstream license](https://github.com/notofonts/noto-fonts/blob/ffebf8c1ee449e544955a7e813c54f9b73848eac/LICENSE) | SIL Open Font License 1.1 |
| `assets/fonts/noto-cjk/NotoSansCJK-Regular.ttc` | Adobe (copyright 2014–2021), The Noto Project Authors | [upstream](https://github.com/notofonts/noto-cjk/tree/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/OTC) | SIL Open Font License 1.1 (`assets/fonts/noto-cjk/OFL.txt`) |
| `assets/fonts/noto-cjk/NotoSansCJK-Bold.ttc` | Adobe (copyright 2014–2021), The Noto Project Authors | [upstream](https://github.com/notofonts/noto-cjk/tree/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/OTC) | SIL Open Font License 1.1 (`assets/fonts/noto-cjk/OFL.txt`) |
| `assets/fonts/noto-cjk/OFL.txt` | The Noto Project Authors | [upstream license](https://github.com/notofonts/noto-cjk/blob/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/LICENSE) | SIL Open Font License 1.1 |
| `assets/fonts/noto-cjk/COPYRIGHT.txt` | Adobe, Google | copyright and trademark notices from the bundled fonts' name tables | notices supplied with the SIL Open Font License 1.1 fonts |
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
