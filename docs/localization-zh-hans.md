# Simplified Chinese UI

SoundCraft's interface can be shown in **简体中文**. The catalog is compiled into the binary
(`crates/ui-egui/locales/zh-Hans/mod.rs`), so there is nothing to install and no locale files to
find at run time. It covers **every string the UI looks up literally** — menu items and path
segments, command labels, dialogs, window titles, tooltips, status-bar and accessibility text and
the formatted messages — and every reference menu label in `crates/engine/catalog/menus.txt`.
Engine diagnostics and machine-facing surfaces stay English (see
[What is translated](#what-is-translated)).

The mechanism is described in [`localization.md`](localization.md); wording decisions are in
[`i18n/glossary.md`](i18n/glossary.md).

## Selecting the language

**Setup ▸ Appearance ▸ Language** lists **English** and **简体中文**. Choosing a language applies
immediately — menus, panels, dialogs and window titles switch on the next frame, no restart.

Without the window, use the command:

```sh
soundcraft-cli run --demo --cmd 'ui.language={"language":"zh-Hans"}'
```

`ui.language` with no parameter reports the current language (`{"language":"en"}`), and an unknown
tag is an error. The choice is a global preference saved with the UI state (like the theme), so it
survives a restart; recalling a saved window configuration never changes it. In the browser the
interface starts from `navigator.language`, and the saved preference wins once one exists.

## What is translated

English source strings stay in the code and are the lookup keys; the catalog maps them at draw
time. Command ids, menu paths used for lookup, preference keys and values, file formats, the
control protocol, the CLI and MCP never see translated text, and neither do values built at run
time (file, track and device names) or third-party plugin names.

Engine error messages stay English: a half-translated diagnostic is worse than a whole one, and the
`error_text` templates pt-BR carries are deliberately omitted here.

## Files

| Path | What |
|---|---|
| `crates/ui-egui/src/i18n/zh-hans.tsv` | the translator-facing catalog: `context<TAB>English<TAB>translation`, escapes `\n`, `\t`, `\\` |
| `crates/ui-egui/locales/zh-Hans/mod.rs` | generated from the TSV; the UI reads this |
| `docs/i18n/tools/gen_zh_hans_locale.py` | regenerates the module (`--check` fails when stale) |
| `assets/fonts/NotoSansSC-SoundCraftSubset.ttf` | web-only Han font subset, OFL-1.1 |
| `docs/i18n/tools/subset_web_font.py` | regenerates the subset from the catalog's glyphs |

## Editing translations

```sh
# 1. edit crates/ui-egui/src/i18n/zh-hans.tsv (one row per string)
python3 docs/i18n/tools/gen_zh_hans_locale.py   # 2. regenerate the module
cargo test -p soundcraft-ui-egui --lib          # 3. catalog and font tests
```

Keep the English placeholders (`{name}`, `{}`, `{ms:.1}`) exactly as they are: a test compares them.
Rows are keyed by the English source, so rewording English text requires updating the row with it.
`@plural` rows pair `singular|plural` on one row because Chinese has no plural inflection.

Look the result up, don't guess: `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png` with a
script that sets the language, then read the PNG (and the accessibility tree) before claiming a
screen is right.

## Terminology

The short table; the reasoning is in [`i18n/glossary.md`](i18n/glossary.md).

| English | 中文 | Note |
|---|---|---|
| Session | 会话 | |
| Track | 轨道 | audio and MIDI alike; 音轨 reads as audio only |
| Clip | 片段 | |
| Bus | 总线 | |
| Transport | 走带 | standard audio-engineering word |
| Pan | 声像 | 规范 acoustics term |
| Bypass | 旁通 | |
| Record | 录音 | Record Enable = 启用录音 |
| Preferences | 首选项 | |
| Bounce | 导出 | |
| Slip / Spot | 滑动 / 定位 | |
| Meter (level) / Meter (time signature) | 电平表 / 拍号 | two senses, two words |
| Undo / Redo | 撤销 / 重做 | |

## Fonts

egui's bundled faces carry no Han glyphs, so `crates/ui-egui/src/fonts.rs` appends a Han fallback in
every family: the operating system's own face on the desktop (fontconfig `lang=zh-cn`, plus
per-platform candidates), and the bundled Noto Sans SC subset in the browser, which has no system
fonts. Han is ordered before the Hangul fallback for every language except Korean, so Chinese text
never draws Korean-styled shapes on Linux where fontconfig resolves Noto Sans CJK's Korean face
first.

The web subset is 1.1 MiB and must stay under 2 MiB (test-enforced). After a catalog change run
`python3 docs/i18n/tools/subset_web_font.py`; the test that draws the whole catalog in every family
fails until you do.

## Validation

- every localized source string has a zh-Hans entry (test lists what is missing);
- placeholders match the English source, keys are unique, every menu label is covered;
- the bundled web face draws the whole catalog in every family and fits its budget;
- with a system Han font installed, every family draws 轨道片段混音.

## Known gaps

- Engine diagnostics stay English.
- No native-speaker review yet; the catalog is AI-authored and should be read by a Chinese-speaking
  audio engineer before it is called finished.
- Traditional Chinese (`zh-Hant`) is not shipped; the Han face list is Simplified-first.
- No IME testing beyond what the platform provides, and no complex-script shaping (egui has none).