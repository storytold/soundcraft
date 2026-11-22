# i18n glossary

The decisions behind SoundCraft's Simplified Chinese wording. The strings themselves live in
`crates/ui-egui/src/i18n/zh-hans.tsv`; the mechanism is in
[`../localization.md`](../localization.md) and the translator workflow in
[`../localization-zh-hans.md`](../localization-zh-hans.md).

**Clean room.** Every entry is written from the plain meaning of the English source text. Do not
consult another product's translation files, string tables or localized interface — not proprietary
ones and not copyleft ones. Where a term is hard, argue it here with the reasoning, not with a
source we are not allowed to read.

## 1. Terms we keep

| English | Ours | Why |
|---|---|---|
| Session | 会话 | the ordinary word for a DAW project in Chinese; 工程 reads as engineering |
| Track | 轨道 | covers audio and MIDI tracks; 音轨 reads as audio-only |
| Clip | 片段 | a piece of audio or MIDI on the timeline |
| Region | 区域 | kept distinct from clip |
| Bus | 总线 | signal-routing bus |
| Solo / Mute | 独奏 / 静音 | |
| Automation | 自动化 | |
| Export / Import | 导出 / 导入 | |
| Snap to Grid | 吸附到网格 | |
| Normalize | 标准化 | not 归一化, which is the loudness-community word |
| Sample rate | 采样率 | |
| Mono / Stereo | 单声道 / 立体声 | |
| Quantize | 量化 | |
| Undo / Redo | 撤销 / 重做 | 撤销 (not 撤消) is the standard spelling |
| Preferences | 首选项 | the majority DAW wording in Chinese |
| Meter (level) | 电平表 | the level meter |
| Meter (time signature) | 拍号 | same English word, different sense: the catalog carries both |
| Tempo | 速度 | |
| Velocity | 力度 | MIDI note velocity |
| Pan | 声像 | the 规范 acoustics term (sound image) |

## 2. Deliberate choices that differ from the obvious translation

| English | Ours | Why |
|---|---|---|
| Transport | 走带 | the standard Chinese audio-engineering word for the transport section |
| Bypass | 旁通 | the community word for bypassing a plug-in |
| Record / Record Enable | 录音 / 启用录音 | the action, not the medium |
| Latch / Touch (automation) | 锁存 / 触碰 | |
| Start / End (clip boundary) | 开头 / 结尾 | positions keep 起点 / 终点 |
| Key (signature) | 调号 | musical sense |
| Trim (edit) / Trim (automation) | 修剪 / 微调 | two senses, kept apart |
| Slip / Spot | 滑动 / 定位 | |
| Bounce | 导出 | Bounce Mix = 导出混音 |
| Big Counter | 大计数器 | the transport readout |

## 3. Shape of a translated string

- Chinese punctuation: use full-width `：`、`，`、`（`、`）`、`。`、`？` inside sentences; keep ASCII
  `·`、`·`、`—`、`/` and the `>` `›` path separators as they are.
- Keep a leading space when the English has one: such fragments are appended to a label at run time
  (`" It is used by {users}"`).
- Preserve every `{placeholder}`, including format specs (`{ms:.1}`, `{seconds:.2}`) and positional
  `{}`; a test compares placeholder sets.
- Keep trailing `…` in menu items (`首选项…`), and keep UI symbols (`+`, `−`, `−∞`, `⬆`) as they are.
- No spaces around Chinese text; one space between Chinese and Latin words is fine where it helps
  (`24 位`, `MIDI 轨道`).

## 4. Review status

The catalog is AI-authored from the English source and has not been reviewed by a native-speaking
audio engineer. Treat wording as provisional until it is; changes go in the TSV, and the module is
regenerated.