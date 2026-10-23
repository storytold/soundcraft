# Simplified Chinese UI

SoundCraft's interface can be shown in **简体中文**. The catalog is compiled into the binary
(`crates/ui-egui/src/i18n/zh-hans.tsv`), so there is nothing to install and no locale files to
find at runtime.

## Selecting the language

**Setup ▸ Appearance ▸ Language**, or the `ui.language` command:

```
{"method":"engine.execute","params":{"command":"ui.language","params":{"language":"zh-Hans"}}}
{"method":"engine.execute","params":{"command":"ui.language","params":{"language":"auto"}}}
{"method":"engine.execute","params":{"command":"ui.language","params":{}}}
```

The last request returns the saved preference and the resolved language, for example
`{"language":"auto","resolved":"zh-Hans"}`.

Without a `language` parameter the command reports the saved preference and the language it
resolves to. The preference is saved with the rest of the UI state: `ui.set` writes it as
`{"language": "zh-Hans"}` and `ui.inspect` reports it back as `ui.language`, so it survives a
restart; **Auto** follows the desktop locale. The environment is read **once, at
startup** (`i18n::init_system_locale()`, called by the desktop and web entry points, and by the
offscreen renderer); anything that never calls it — library tests, embeddings — draws English under
Auto, so no test depends on the machine's locale. With Auto, the first of these that names a
supported language wins:

1. `SOUNDCRAFT_LOCALE` (an explicit override for scripts; read by `init_system_locale()`)
2. `LANGUAGE` (a colon-separated priority list, e.g. `zh_CN:en_US`)
3. `LC_ALL`, then `LC_MESSAGES`, then `LANG`

Library tests do not read these variables: under `cfg(test)` the desktop's answer comes from the
thread-local `TEST_SYSTEM_TAGS` hook (set with `with_system_tags`), so `SOUNDCRAFT_LOCALE` does
not control them.

`zh`, `zh_CN.UTF-8`, `zh-CN`, `zh-SG`, `zh-Hans` and `zh-Hans-CN` all resolve to this catalog;
`zh_TW`, `zh-HK` and `zh-Hant` resolve to no catalog and fall back to English (Traditional
Chinese is not shipped). `C` and `POSIX` mean English. Anything unsupported means English, never a
crash or an empty window.

Example: `SOUNDCRAFT_LOCALE=zh_CN.UTF-8 soundcraft --demo --control 0`.

The web build (`soundcraft-web`) reads the same variables; it does not interrogate the browser,
so pick the language in Appearance there.

## What is translated

English source strings stay in the code and are the lookup keys; the catalog maps them at draw
time. Command ids, menu paths used for logic, the control channel, the CLI and MCP never see
translated text, so scripts and agents are unaffected.

| Surface | Covered |
|---|---|
| Menu bar, all menu items and their paths | ✅ every path segment |
| Command labels (shortcuts window, command dialogs) | ✅ every command |
| Panel titles, dock tabs, column headers, ruler labels | ✅ |
| Dialogs (New Tracks, Bounce, Import, Save changes, Clear, Path…) | ✅ |
| Tooltips, status bar, counters, edit-mode and automation labels | ✅ |
| Error and recovery messages raised by the UI | ✅ templates with `{placeholders}` |
| Engine messages (`Trouble`, audio-health issue text, plugin names) | ❌ English |
| Parity report, CLI and MCP output | ❌ English (machine surfaces) |
| User data: session/track/clip names, file paths, device names | ❌ never translated |

Engine-side diagnostics stay English deliberately: they are also what the control channel,
`soundcraft-cli` and MCP report, and a translated message there would break scripts that match on
it. The parity report and MCP output are machine surfaces for the same reason.

Two details worth knowing:

- The status bar keeps the text it was given, translated when the event happened. Switching
  language mid-session does not rewrite the message already shown; the next message uses the new
  language.
- Widget text is translated at draw time, so menus, panels, tooltips, window titles and the
  Undo/Redo item names switch on the very next frame — no restart.

## Files and integration

| Path | Role |
|---|---|
| `crates/ui-egui/src/i18n/mod.rs` | registry `LANGUAGES`, `Lang`, lookup API, plural rules, tests |
| `crates/ui-egui/src/i18n/catalog.rs` | TSV parser and lookup tables |
| `crates/ui-egui/src/i18n/system.rs` | locale detection from the environment, read once at startup |
| `crates/ui-egui/src/i18n/zh-hans.tsv` | the translations (1,114 entries) |
| `crates/ui-egui/src/fonts.rs` | simplified-Chinese font fallback (`system-han`) |

Lookups, in order of preference:

- `i18n::t("Play")` — a plain string.
- `i18n::tr_ctx(lang, "automation", mode.label())` — one English word that needs different
  translations in different places (`automation` context for Off/Read/Touch/Latch/…).
- `i18n::tr_id(lang, id, label)` — keyed by command id, so a translation survives rewording of
  the English label (menu items, shortcuts window).
- `i18n::trn(lang, n, "{n} clip", "{n} clips")` — plural-aware; Chinese always takes form 0.
- `i18n::fmt(t("Recording stopped: {cause}. {then}"), &[("cause", cause), ("then", then)])` —
  fills placeholders after translation, so a translator may reorder them.
- `tl!("…")` — shorthand for `i18n::t` in expression position.

Entries are `context<TAB>English source<TAB>translation`; the context is empty for plain strings,
`@id` for command ids, `@plural` for `one|other` forms, and a word (`automation`) to disambiguate.
Keep the English key, placeholders, contexts and any trailing `…` intact.

## Terminology

| English | Simplified Chinese | Note |
|---|---|---|
| Session / Track / Clip | 会话 / 轨道 / 片段 | |
| Playlist / Take | 播放列表 / 版本 | |
| Region group | 片段组 | |
| Insert / Send | 插入 / 发送 | `Inserts A-E` → `插入 A-E` |
| Bus / Main output | 总线 / 主输出 | |
| Fader / Pan | 推子 / 声像 | |
| Automation: Off/Read/Touch/Latch/Write/Trim | 关闭/读取/触碰/锁存/写入/微调 | `context = automation` |
| Grid / Nudge | 网格 / 微移 | |
| Edit modes: Shuffle/Spot/Slip/Grid/Rel Grid | 随机/定位/滑移/网格/相对网格 | |
| Bars\|Beats / Min:Secs / Timecode / Feet+Frames / Samples | 小节\|拍 / 分:秒 / 时间码 / 英尺+帧 / 采样 | |
| Bounce / Commit | 导出 / 提交 | |
| Memory Location / Marker | 记忆位置 / 标记 | |
| Pre-roll / Post-roll | 预卷 / 后卷 | |
| Solo / Mute / Record-enable | 独奏 / 静音 / 启用录音 | |

SoundCraft, ArtCraft, AudioSuite, CLAP, VST3, MIDI, AAX, Pro Tools, kHz, dB, Hz and file
extensions keep their spelling. Numeric formats (sample rates, frame rates, timecode) are not
translated; unit words around them are (`{n} 声道`, `{value} 秒`).

## Fonts

The shell appends a Han fallback to every font family (`system-han` in `fonts.rs`): PingFang SC /
Songti SC / STHeiti on macOS, Microsoft YaHei / SimHei on Windows, Source Han Sans SC, Noto Sans
CJK SC, WenQuanYi and AR PL UMing on Linux. The catalog itself carries no font assets. If a glyph
is missing on a platform, look at `fonts::definitions()` first — not at the catalog.

Font resolution happens **once per process** (`REGULAR/BOLD/HANGUL/HAN_CACHE`): on Linux each
family is matched through `fc-match`, a subprocess, and a child of the process keeps copies of its
open files until it execs — running that per `definitions()` call made the autosave tests in
`tests/recovery.rs` flaky and cost ~3.5 minutes of the test suite. With the caches, resolution runs
at most four times per process (fontconfig answer first, then the built-in candidate list) and the
library tests finish in seconds.

IME input, complex-script shaping and right-to-left layout are not implemented (egui has no
shaping engine). Chinese *display* works; Chinese *text entry* into fields relies on the platform
IME and has not been tested against every desktop.

## Validation and maintenance

```
cargo test -p soundcraft-ui-egui --lib i18n     # catalog, locale and coverage tests
cargo clippy -p soundcraft-ui-egui --all-targets -- -D warnings
cargo xtask layers
```

The tests enforce, for the bundled catalog:

- it parses with no malformed lines and no duplicate keys;
- placeholders, trailing `…` and trailing colons match the English source;
- every menu path segment (`menus::tree()`) and every `UI_COMMANDS` label has an entry;
- every command label from `command_specs()` has an entry (the shortcuts window and command
  dialogs read those, not the menu paths);
- every literal passed to a plain lookup — `i18n::t("…")` or a `t("…")` imported into scope —
  anywhere in the crate has an entry: the test reads `crates/ui-egui/src/*.rs` from disk and
  scans the call sites, so a new UI file is covered automatically;
- Chinese counts take a single plural form;
- zh/zh-CN/zh-Hans tags resolve here and zh-TW/zh-Hant do not.

To look at the result, render offscreen and read the PNGs:
```
cargo run -p soundcraft-ui-egui --example ui_shot -- out.png script.jsonl
# script.jsonl:
#   {"method":"engine.execute","params":{"command":"ui.language","params":{"language":"zh-Hans"}}}
#   {"steps":3}
#   {"shot":"zh-main.png"}
```

Compare with the English render (`"language":"en"`, or `LANG=en_US.UTF-8`) and with
`plan/protools/screenshots/`. Check the edit window, the mix window, the menus, the shortcuts
window and one dialog; watch for clipped labels (Chinese is narrower than English but not
uniformly so).

The integration tests (`crates/ui-egui/tests/`) assert English labels and never call
`init_system_locale`, so they stay deterministic on a Chinese desktop; `SOUNDCRAFT_LOCALE` only
matters for runs that read it at startup.

When an English string changes, update its key in the catalog in the same commit and keep the
context and placeholders. A missed key is not a crash: it draws in English, and the coverage
tests fail first.

## Adding another language

1. Copy the header of `zh-hans.tsv` to `xx.tsv` and translate from the meaning of the English
   text — no proprietary translation memory, no strings lifted from another DAW.
2. Add one row to `LANGUAGES` in `i18n/mod.rs` (code, native name, `include_str!`, plural rule,
   `complete_menus`).
3. Register the locale tags in `lang_from_tag` if the language needs region or script fallbacks.
4. Run the tests; the coverage failures list exactly which strings are missing.

The Appearance dropdown, the Auto detection and every catalog test pick the new language up from
the registry.
