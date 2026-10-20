# Simplified Chinese UI

SoundCraft's interface can be shown in **简体中文**. The catalog is compiled into the binary
([`crates/ui-egui/src/i18n/zh-hans.tsv`](../crates/ui-egui/src/i18n/zh-hans.tsv), packed and
deflated at build time), so there is nothing to install and no locale files to find at run time.
It has **1,106 entries**: every menu item and path segment, every command label, and every string
the UI looks up literally — dialogs, window titles, tooltips, status messages and UI error
templates. Engine diagnostics and machine surfaces stay English (see
[What is translated](#what-is-translated)).

The mechanism — preference commands, packing, catalog format, adding a language — is described in
[`localization.md`](localization.md). Wording research and decisions are in
[`i18n/README.md`](i18n/README.md) and [`i18n/glossary.md`](i18n/glossary.md).

## Selecting the language

**Setup ▸ Appearance** opens the Appearance window; its **Language** row lists **Auto (…)**, English
and 简体中文. Choosing a language previews the window itself; **Apply** stores it, **Cancel** or
closing discards it, and **Reset** returns the language (with scale, motion and theme) to defaults.
Without the window, use the preference commands:

```json
{"method":"engine.execute","params":{"command":"prefs.set","params":{"path":"interface.language","value":"zh-hans"}}}
{"method":"engine.execute","params":{"command":"prefs.get","params":{"path":"interface.language"}}}   // "zh-hans"
{"method":"engine.execute","params":{"command":"prefs.reset","params":{"path":"interface.language"}}} // "auto"
```

The preference is saved with the rest of the settings — `preferences.json` in the settings folder on
the desktop, `localStorage` (`soundcraft.preferences`) on the web — so it survives a restart. The old
`ui.language` UI command has been removed; the language is an ordinary preference now. It can be
changed while the app runs: menus, panels, tooltips and window titles switch on the next frame, no
restart.

**Auto** follows the desktop. The system locale is read **once, at startup**
(`i18n::init_system_locale()`, called by the desktop entry point before the first frame, and by the
offscreen renderer); anything that never calls it — library tests, embeddings — draws English under
Auto, so no test depends on the machine's locale. The web shell passes `navigator.languages` to
`i18n::init_system_locale_with`, so the browser's language list is Auto's input there. With Auto, the
first of these that names a supported language wins:

1. `SOUNDCRAFT_LOCALE` (an explicit override for scripts, tests and screenshots)
2. the desktop's preferred-language list (`sys-locale`: macOS/Windows preferred languages;
   `LC_ALL`, `LC_MESSAGES`, `LANG` on Linux/BSD)
3. `LANGUAGE` (a colon-separated priority list, e.g. `zh_CN:en_US`), then `LC_ALL`, `LC_MESSAGES`,
   then `LANG`

`zh`, `zh_CN.UTF-8`, `zh-CN`, `zh-SG`, `zh-Hans` and `zh-Hans-CN` all resolve to this catalog;
`zh_TW`, `zh-HK` and `zh-Hant` negotiate to `zh-hant`, which is not shipped, so they fall back to
English (Traditional Chinese is a parity gap). `C` and `POSIX` mean English. Anything unsupported
means English, never a crash or an empty window. Auto never writes the detected language back into
the preference, so changing the desktop language changes the app on its next start.

Example: `SOUNDCRAFT_LOCALE=zh_CN.UTF-8 soundcraft --demo --control 0`.

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
| Engine messages (`Trouble`, audio-health issue text) and third-party plugin names | ❌ English (machine text / user data) |
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
  Undo/Redo item names switch on the very next frame — no restart. A language change also
  re-installs the font fallback (one deferred frame while egui rebuilds its font atlas).

## Files and integration

| Path | Role |
|---|---|
| `crates/ui-egui/src/i18n/mod.rs` | registry `LANGUAGES`, `Lang`, lookup API, plural rules, tests |
| `crates/ui-egui/src/i18n/catalog.rs` | TSV parser and lookup tables |
| `crates/ui-egui/src/i18n/system.rs` | locale detection from the system and environment |
| `crates/ui-egui/src/i18n/zh-hans.tsv` | the translations (1,106 entries) — the source of truth |
| `crates/ui-egui/build.rs` | packs the catalogs and a shared key table into `OUT_DIR/i18n/*.deflate` |
| `crates/ui-egui/src/i18n/LICENSE-translations.txt` | the catalog's clean-room statement and licence |
| `crates/engine/src/prefs.rs`, `crates/engine/src/cmd/prefs.rs` | `interface.language` preference and `prefs.get/set/reset` |
| `apps/soundcraft/src/{config,main}.rs` | desktop persistence (`preferences.json`, one-time legacy migration) |
| `apps/soundcraft-web/src/web.rs` | web persistence (`localStorage`, `navigator.languages`) |
| `crates/ui-egui/src/extra_windows.rs` | the Appearance window (language dropdown and preview) |
| `crates/ui-egui/src/fonts.rs` | CJK font fallback (`system-han`, `system-hangul`; bundled Han subset on the web) |

Lookups, in order of preference (the full API is in [`localization.md`](localization.md#runtime)):

- `i18n::t("Play")` — a plain string.
- `i18n::tr_ctx(lang, "automation", mode.label())` — one English word that needs different
  translations in different places (`automation` context for Off/Read/Touch/Latch/…).
- `i18n::tr_id(lang, id, label)` — keyed by command id, so a translation survives rewording of
  the English label (menu items, shortcuts window).
- `i18n::trn(lang, n, "{n} clip", "{n} clips")` — plural-aware; Chinese always takes form 0.
- `i18n::fmt(t("Recording stopped: {cause}. {then}"), &[("cause", cause), ("then", then)])` —
  fills placeholders after translation, so a translator may reorder them.
- `i18n::has(lang, "…")` and `i18n::tr(lang, "…")` — check or translate for an explicit language.

Entries are `context<TAB>English source<TAB>translation`; the context is empty for plain strings,
`@id` for command ids (supported by the parser, but the current catalog uses plain rows for every
command), `@plural` for `one|other` forms, and a word (`automation`) to disambiguate.
Keep the English key, placeholders, contexts, escapes (`\n`, `\t`, `\\`) and any trailing `…`
intact. The file is packed, not read at run time — edit the TSV and rebuild; `build.rs` re-runs on
any change under `src/i18n/`.

## Terminology

| English | Simplified Chinese | Note |
|---|---|---|
| Session / Track / Clip | 会话 / 轨道 / 片段 | |
| Playlist / Take | 播放列表 / 版本 | playlist version; film scene/take metadata is 场次/镜次 |
| Region / Clip group | 区域 / 片段组 | Regions 区域; Clip Groups 片段组 |
| Insert / Send | 插入 / 发送 | `Inserts A-E` → `插入 A-E` |
| Bus / Main output | 总线 / 主输出 | |
| Fader / Pan | 推子 / 声像 | |
| Automation: Off/Read/Touch/Latch/Write/Trim | 关闭/读取/触碰/锁存/写入/微调 | `context = automation`; clip trim = 修剪 |
| Grid / Nudge | 网格 / 微移 | Nudge Value = 微移值 |
| Snap to Grid / On Grid | 吸附到网格 / 在网格 | `Snap to` = 吸附到 everywhere |
| Edit modes: Shuffle/Spot/Slip/Grid/Rel Grid | 推移/定位/滑动/网格/相对网格 | Shuffle Lock = 推移锁定; tooltips 推移模式 |
| Bars\|Beats / Min:Secs / Timecode / Feet+Frames / Samples | 小节\|拍 / 分:秒 / 时间码 / 英尺+帧 / 采样 | |
| Bounce / Commit | 导出 / 提交 | |
| Memory Location / Marker | 记忆位置 / 标记 | |
| Pre-roll / Post-roll | 预卷 / 后卷 | |
| Solo / Mute / Record-enable | 独奏 / 静音 / 启用录音 | |
| Composer | 作曲家 | metadata |
| Normalize / Normalize to -0.1 dBFS | 标准化 / 标准化至 -0.1 dBFS | one word family; 归一化 is not used |
| All (command-label prefix) | 全部 | running sentences keep 所有 |
| Deselect All / Clear All Mutes / Clear All Solos | 取消全选 / 取消全部静音 / 取消全部独奏 | Clear cancels a state; 清除 stays for deletion |
| Online / offline (console) | 联机 / 脱机 | 在线/离线 is the internet sense |
| Object / Bed | 对象 / 声床 | immersive object vs the bed |
| Scene / Take (clip metadata) | 场次 / 镜次 | playlist Take = 版本 |
| System Usage | 系统使用情况 | window title |
| Preferences / Get / Set / Reset Preferences | 首选项 / 获取首选项 / 设置首选项 / 重置首选项 | |
| Ticks | Tick | unit token, like dB/Hz; Delay (ticks) = 延迟（Tick） |
| Lane | 道 | marker lane 标记道, transcription lane 转写道 |

SoundCraft, ArtCraft, AudioSuite, CLAP, VST3, MIDI, AAX, Pro Tools, kHz, dB, Hz and file
extensions keep their spelling. Numeric formats (sample rates, frame rates, timecode) are not
translated; unit words around them are (`{n} 声道`, `{value} 秒`). The evidence for each deliberate
choice, including divergences from other DAWs, is recorded in
[`i18n/glossary.md`](i18n/glossary.md) — consult it before "fixing" a term.

## Fonts

The shell appends two CJK fallbacks to every font family (`system-han` and `system-hangul` in
`fonts.rs`): PingFang SC / Songti SC / STHeiti and Apple SD Gothic Neo on macOS, Microsoft YaHei /
SimHei and Malgun Gothic on Windows, Source Han Sans SC, Noto Sans CJK SC, WenQuanYi and AR PL UMing
(and Noto/Nanum Hangul) on Linux. Chinese and English interfaces order Han before Hangul, so Han
glyphs draw Chinese-shaped instead of from the Korean Noto face; a Korean interface reverses the
two. The catalog itself carries no font assets.

On the web there are no system fonts: the wasm build embeds a **Noto Sans SC subset**
([`assets/fonts/NotoSansSC-SoundCraftSubset.ttf`](../assets/fonts/NotoSansSC-SoundCraftSubset.ttf),
OFL, see [`ATTRIBUTION.md`](../ATTRIBUTION.md)) as the lowest-priority fallback of every family. A
test fails when the catalog starts using a character the subset lacks, so Chinese labels never draw
as boxes in the browser.

Font resolution happens **once per process**
(`REGULAR_CACHE`, `BOLD_CACHE`, `HANGUL_CACHE`, `HAN_CACHE`): on Linux each family is matched
through `fc-match`, a subprocess, and a child of the process keeps copies of its open files until
it execs — in tests that can make a just-released autosave lock look taken. With the caches,
resolution runs at most four times per process (fontconfig answer first, then the built-in candidate
list). A language change re-installs the fonts, so the order follows the new language.

The UI has no complex-script shaping and no right-to-left layout, so scripts that need joining or
bidi order do not render correctly yet. IME input is provided by the platform and passes through
to text fields; it has not been tested against every desktop, and the automated UI tests do not
exercise it.

## Validation and maintenance

```
cargo test -p soundcraft-ui-egui --lib i18n     # catalog, locale and coverage tests
cargo test -p soundcraft-ui-egui --lib fonts    # fallback order and bundled web-font coverage
cargo clippy -p soundcraft-ui-egui --all-targets -- -D warnings
cargo xtask layers
```

The tests enforce, for the bundled catalog:

- it packs, unpacks and parses with no malformed lines and no duplicate keys;
- placeholders, trailing `…` and trailing colons match the English source;
- every menu path segment (`menus::tree()`) and every `UI_COMMANDS` label has an entry;
- every command label from `command_specs()` has an entry (the shortcuts window and command
  dialogs read those, not the menu paths);
- every literal passed to a plain lookup — `i18n::t("…")` or a `t("…")` imported into scope —
  in the crate's top-level `src/*.rs` files has an entry: the test reads the source tree from disk
  and scans the call sites, so a new top-level UI file is covered automatically;
- Chinese counts take a single plural form;
- zh/zh-CN/zh-Hans tags resolve here and zh-TW/zh-Hant do not;
- the font fallback order follows the interface language and the bundled web font covers every
  catalog character.

To look at the result, render offscreen and read the PNGs:
```
cargo run -p soundcraft-ui-egui --example ui_shot -- out.png script.jsonl
# script.jsonl:
#   {"method":"engine.execute","params":{"command":"prefs.set","params":{"path":"interface.language","value":"zh-hans"}}}
#   {"steps":3}
#   {"shot":"zh-main.png"}
```

Compare with the English render (`prefs.set` `"en"`, or `SOUNDCRAFT_LOCALE=en_US.UTF-8` for Auto)
and with `plan/protools/screenshots/`. Check the edit window, the mix window, the menus, the
shortcuts window and one dialog; watch for clipped labels (Chinese is narrower than English but not
uniformly so).

Before coining or revising wording, consult the translation knowledge base in
[`docs/i18n/`](i18n/README.md): it cross-checks this catalog against the Chinese
UIs of six open-source DAWs (`python3 docs/i18n/tools/cross_check.py` regenerates
`docs/i18n/cross-check-report.md`) and records the curated conclusions in
[`docs/i18n/glossary.md`](i18n/glossary.md). Decisions made there feed back into
the terminology table above.

The integration tests (`crates/ui-egui/tests/`) assert English labels and never call
`init_system_locale`, so they stay deterministic on a Chinese desktop; `SOUNDCRAFT_LOCALE` only
matters for runs that read it at startup.

When an English string changes, update its key in the catalog in the same commit and keep the
context and placeholders. A missed key is not a crash: it draws in English, and the coverage
tests fail first.

## Adding another language

1. Copy the header of `crates/ui-egui/src/i18n/zh-hans.tsv` to
   `crates/ui-egui/src/i18n/xx.tsv` and translate from the meaning of the English
   text — no proprietary translation memory, no strings lifted from another DAW.
2. Add one row to `LANGUAGES` in `i18n/mod.rs` (code, native name, `catalog!("xx")`, plural rule,
   `complete_menus`).
3. The tag negotiation already maps region/script tags (for example `zh-TW` → `zh-hant`); add a
   registry row or extend `candidates` only if the new language needs tags it does not cover.
   `auto` picks a registered language up automatically.
4. Run the tests; the coverage failures list exactly which strings are missing. Set
   `complete_menus` only when the coverage test passes.
5. Add a licence note for the translation and an `ATTRIBUTION.md` row for any bundled font.
6. Update [`localization-parity.md`](localization-parity.md) and the language table in
   [`ROADMAP.md`](../ROADMAP.md).

The full checklist, including the shared key table and packed format, is in
[`localization.md`](localization.md#adding-a-language).
