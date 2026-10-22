# zh-Hans terminology glossary (curated)

Conclusions drawn from the generated [`cross-check-report.md`](cross-check-report.md)
(six open-source DAW corpora) plus Chinese audio-community and standard-terminology
references — see [`sources/SOURCES.md`](sources/SOURCES.md). This file records
*decisions and their evidence*; the shipped strings stay in
`crates/ui-egui/src/i18n/zh-hans.tsv`, and the short-form terminology table in
[`docs/localization-zh-hans.md`](../localization-zh-hans.md) stays in sync with the
**Decision** column. How the catalog is built and switched is in
[`docs/localization.md`](../localization.md).

Verdicts: **keep** — our wording is right, evidence noted; **consider** — a real
case for changing it (the 2026-10-20 pass applied the ones it had — see §3);
**false positive** — the cross-check matched two different senses of one English
word.

Last reviewed: 2026-10-20 (corpus 2026-10-20).

## 1. Where the ecosystem agrees with us (spot checks)

| Term | Ours | Evidence |
|---|---|---|
| Session | 会话 | Ardour 会话; community usage identical |
| Track | 轨道 | Audacity/LMMS 轨道, Ardour/MuseScore 音轨. 轨道 kept deliberately: it is generic (audio *and* MIDI tracks) and matches the incumbent's community usage; 音轨 reads as "audio track only" |
| Clip | 片段 | Audacity 片段 |
| Region | 区域 | Ardour 区域 (Ardour says region where we say clip; both words kept distinct) |
| Bus | 总线 | Ardour 总线 |
| Solo / Mute | 独奏 / 静音 | 6/6 and 6/7 across the corpus |
| Automation | 自动化 | Ardour 自动化 |
| Export / Import | 导出 / 导入 | 5/6 and 5/6 |
| Composer | 作曲家 | 作曲家 3/4 (Ardour, Audacity, Mixxx; MuseScore 曲作者) — revised from 作曲者 in the 2026-10-20 pass (§3) |
| Snap to Grid | 吸附到网格 | Snap to 2/2 (Ardour/Audacity 吸附到); MuseScore 吸附于网格 — aligned in the 2026-10-20 pass |
| All (command-label quantifier) | 全部 | 全部 3/5 (Hydrogen/Mixxx/MuseScore), 所有 2/5 (Ardour/Audacity); running sentences keep 所有 — see §3 |
| Normalize | 标准化 | Audacity/LMMS 标准化, Ardour 规范化 — this is the one family (§3); 归一化 is the loudness-community word and is deliberately not used |
| Sample rate | 采样率 | 6/6 — the strongest consensus in the corpus |
| Mono / Stereo | 单声道 / 立体声 | 5/5 and 3/5 |
| Quantize | 量化 | Ardour/LMMS/Mixxx 量化 |
| Undo / Redo | 撤销 / 重做 | 3/4 and 3/5. 撤销 (not 撤消) matches 现代汉语/termonline 规范写法 |
| Preferences | 首选项 | Ardour/Audacity/Hydrogen/Mixxx 首选项; MuseScore 偏好设置 — we follow the 4-DAW majority |

## 2. Deliberate divergences from the reference corpora

These differ from one or more references on purpose; do not "fix" them to match
another DAW without a real reason.

| Term | Ours | References | Why ours |
|---|---|---|---|
| Transport | 走带 | Ardour 播送, Audacity 播录, Mixxx 传输 | 走带 (走带控制) is the standard Chinese audio-engineering word for transport; the refs are all one-off coinages |
| Pan | 声像 | Ardour/LMMS/Hydrogen 声相, MuseScore 声像 | 声像 is the 规范 acoustics term (termonline: 声像 "sound image"); 声相 is the older folk spelling |
| Latch / Touch (automation) | 锁存 / 触碰 | Ardour 暂锁 / 改动 | The incumbent's community tutorials standardise on 触碰/锁存; Ardour's coinages never spread |
| Bypass | 旁通 | Ardour 略过, MuseScore 旁路 | 旁通 is the community-standard word for bypassing a plug-in; 旁路 (Cubase circles) is acceptable, 略过 is not |
| Start / End (of clip) | 开头 / 结尾 | 开始 / 结束 / 起点 / 终点 | For clip boundaries 开头/结尾 reads naturally in Chinese editing contexts; 起点/终点 suits positions, not object edges |
| Key (signature context) | 调号 | Audacity 键, Mixxx 调性 | Music meaning is ours; the refs translated the keyboard/data meaning |
| Meter (time signature context) | 拍号 | Ardour 节拍 | We also have Meters = 电平表 elsewhere — the two senses are both covered, deliberately |
| Record | 录音 | Ardour/Audacity/Hydrogen 录制 | Community usage for the transport action is 录音; Record Enable 启用录音 follows. 录制 would also be acceptable |
| Trim | 修剪 | Ardour 修剪, Audacity 修剪 | Consensus; unrelated to automation Trim = 微调 (context-separated in the catalog) |

## 3. Resolved decisions (2026-10-20 wording pass)

Applied in `crates/ui-egui/src/i18n/zh-hans.tsv` and mirrored in the terminology
table of `docs/localization-zh-hans.md`. Kept here so the reasoning survives the
next pass; do not re-open a row without new evidence.

| Term | Before | Decision | Evidence / why |
|---|---|---|---|
| Composer (metadata) | 作曲者 | 作曲家 | 作曲家 3/4 (Ardour, Audacity, Mixxx; MuseScore 曲作者/作曲); 作曲者 is uncommon in CN metadata forms |
| Normalize to -0.1 dBFS | 归一化至 | 标准化至 | One word family: Normalize = 标准化 (2/3 consensus). 归一化 is the loudness-community word, but two families in one catalog was worse — switching Normalize too was considered and rejected |
| Power (waveform style) | 电源 | 功率 | Context is View ▸ Waveforms (peak vs power display), not the plug-in enable the old row assumed; mains power is meaningless here. Audacity's 开关 is the plug-in sense — noise |
| Slip / SLIP (edit mode) | 滑移 | 滑动 | Community usage is 滑动模式; 滑移 has no corpus support. Applied to the edit-mode row and the SLIP toolbar tag |
| Shuffle / SHUFFLE (edit mode) | 随机 | 推移 | 随机 reads as "random order" (playlist shuffle, Mixxx 随机排序). The edit mode ripples later clips when one moves, so 推移 is the movement metaphor; applied to the edit-mode tag, the toolbar tag, `推移模式`/`推移锁定` tooltips and Shuffle Lock = 推移锁定 (PR review, 2026-10-20) |
| Strip Silence | 去除无声 | 去除无声 (kept) | No consensus (only Ardour 片段无声, a poor phrase). 去除静音 would clash with Mute = 静音; the command deletes silent ranges, so 去除无声 stays. Revisit only if the incumbent's Chinese UI can be observed |
| Deselect All | 取消全部选择 | 取消全选 | Pairs with Select All = 全选; 取消全部选择 is merely longer |
| All (command-label quantifier) | 所有… | 全部… | 全部 3/5. All labels now use it: All Automation 全部自动化, All Marker Rulers 全部标记标尺, All MIDI Notes Off 关闭全部 MIDI 音符, Clear All Clip Indicators 清除全部片段指示, Close All Workspaces 关闭全部工作区, Glide to All Enabled 滑变到全部已启用项, Write to All Enabled 写入全部已启用项, Hide All Floating Windows 隐藏全部浮动窗口. Running sentences keep 所有 (Display on All Channels 在所有通道上显示) |
| Clear All Mutes / Clear All Solos | 清除全部静音 / 独奏 | 取消全部静音 / 取消全部独奏 | Clear cancels a state here, it deletes nothing; 清除 stays for removal (Clear Automation 清除自动化) |
| Automation Trim family | 清除 Trim 自动化 etc. | 清除微调自动化 / 合并微调自动化 / 微调播放列表 / 微调到全部已启用项 / 微调到当前项 | Keeps the §2 automation-Trim = 微调 divergence and removes the bare English "Trim" from labels; clip trim stays 修剪 |
| Tempo Change | 速度变更 | 速度更改 | Family consistency: Change Duration 更改时长, Change Meter 更改拍号, Change Velocity 更改力度; Tempo Change (BPM) was already 速度更改 |
| Recall (memory location) | 调用 | 调取 | Aligns with Recall Memory Location = 调取记忆位置 |
| Object / Bed | 对象/床 | 对象/声床 | Aligns with the command help 底层声床 (immersive object vs the bed) |
| Snap to Grid / On Grid | 对齐网格 (both) | 吸附到网格 / 在网格 | Snap to = 吸附到 (2/2 in the corpus; tooltips already said 吸附到网格). On Grid splits clips at grid lines and parallels 在选区 / 在瞬态 |
| Online (console state) | 在线 | 联机 | Transport Online 走带联机, Video Track Online 视频轨联机; 在线 reads as "connected to the internet". No Offline labels exist yet — if they appear, use 脱机 |
| System Usage | 系统用量 | 系统使用情况 | No corpus data; a window title reads better as a full noun phrase |
| Scene And Take (clip metadata) | 场景与版本 | 场次与镜次 | Film metadata senses: scene 场次, take 镜次. Playlist Take = 版本 is a different sense and stays |
| Minimal (view preset) | 精简 | 最简 | Thin (automation thinning) = 精简; Minimal hides every optional Edit-window column, so 最简 avoids the word collision |
| Ticks (timebase unit) | Tick (unchanged) | Tick | Unit token kept like dB/kHz/Hz; Delay (ticks) 延迟（Tick） capitalised consistently. Corpus 颗粒刻/拍子记号 are other senses (§4) |
| Nudge Value | 微调值 | 微移值 | Nudge = 微移 (Grid / Nudge row) |
| 512-sample block | 一帧 | 一块 | 帧 means video frame; audio blocks are 块 |
| mono track-minute | 每单声道轨道分钟 | 每条单声道轨道每分钟 | Chinese rate phrase needs 每分钟 and a classifier |
| Track Lane / Track Lane Color | 轨道道 / 轨道道颜色 | 轨道标记道 / 轨道标记道颜色 | Marker-display lanes; 轨道道 is a stutter, lane = 道 (标记道/标尺道) |
| Track Transcription Lane | 轨道转写层 | 轨道转写道 | lane = 道, consistent with 标记道 |
| Note templates | {note} 附点 / {note} 三连音 | 附点 {note} / {note}三连音 / 附点 {note}三连音 | Chinese attributive order; works with the now-translated note labels (1/4 音符 etc.) |
| Tab | 切换 | Tab | Key name, kept untranslated like Ctrl/Shift |
| Mic Preamps | 麦克风前置放大 | 麦克风前置放大器 | Noun; 放大 alone reads as the verb |
| audio on (status) | 音频输出中 | 音频已开启 | The status means enabled, not currently playing |

## 4. False positives in the generated report (do not act)

Same English word, different sense — the cross-check cannot know the context:

| Report row | Why it is noise |
|---|---|
| Cut Time… → 二二拍 (MuseScore) | MuseScore's "cut time" is the 2/2 time signature; ours is the Edit ▸ time operation |
| Dynamics → 力度记号 (MuseScore) | Musical dynamics marks vs the dynamics *processor section*; ours 动态 is right |
| Page → 页面 (MuseScore) | Score page vs MIDI-editor paging |
| Paste Special → 粘贴特殊符号 (MuseScore) | Note-entry special characters vs paste variants |
| Meter → 指示表 (Audacity) / 节拍 (Ardour) | Level meter vs time signature — both senses covered (see §2) |
| Threshold dB → 阙值 (Audacity) | 阙值 is a typo for 阈值; ours is correct |
| Velocity + → 速度 (Audacity) | Velocity as playback speed vs MIDI velocity 力度 — corpus majority 力度 4/4 |
| Splice → 拼接 (Ardour) | Splice here is the brand/platform name, kept untranslated |
| Object → 对象物体 (Ardour) | One-off coinage |
| Rectified → 已矫正 (Ardour) | Different rectifier context (waveform rectify = 整流, ours) |
| Swing % → 摆动 (Ardour) | 摇摆 is the established swing-feel word (Hydrogen agrees) |
| SHUFFLE → 随机排序 (Mixxx) | Mixxx's shuffle randomises playlist order; our Shuffle ripples clips (edit mode, §3). Same English word, different sense |
| Power → 开关 (Audacity) | Audacity's switch enables a plug-in; our Power is the waveform display style (功率, §3) — same English word, different sense |
| Ticks → 颗粒刻 (Hydrogen) / 拍子记号 (MuseScore) | Sequencer tick marks and score beat marks; our Tick is the timebase unit (§3) |
| Minimal → 最小 (Audacity) | A different Audacity feature; ours is the Edit-window view preset 最简 (§3) |
| Take (playlist) ↔ scene/take (metadata) | Two senses of take: playlist Take = 版本, film take = 镜次 — do not unify |

## 5. Corpus coverage

1,032 deduplicated catalog keys compared: 171 consensus, 56 majority-differs,
43 split, 762 no-data (SoundCraft-specific command labels — clip gain plumbing,
memory locations, per-slot sends/inserts — have no counterpart in the
corpora). The 2026-10-20 wording pass rewrote 48 catalog rows and removed 5 dead
ones (superseded save/close strings, Middle, and a Shuffle row no code looked
up). The review that followed landed the Shuffle decision (§3), folded the
`@id` command-id rows into plain rows, and added the preference commands; the
catalog now has 1,139 entries (the last dead row, `Interface Language`, was
dropped when its `ui.language` command went away; a 2026-10-22 pass then added the
remaining toolbar, send, bounce and Memory Locations strings).

For no-data terms the glossary above and the terminology table are
the only authority; when coining new wording, prefer §2's rationale over
inventing new families of words.

## 6. General vocabulary (web sources, appendix)

From the community glossaries and standard-terminology platforms in
[`sources/SOURCES.md`](sources/SOURCES.md) — useful when translating *new*
strings that no corpus covers:

| English | 简体中文 | Note |
|---|---|---|
| gain staging | 增益规划 / 电平匹配 | community split; avoid literal 增益阶段 |
| headroom | 余量 / 动态余量 | termonline has 动态余量 |
| clipping | 削波 | (Mixxx 削波) |
| de-esser | 齿音消除器 | matches our Dyn3 De-Esser rendering |
| comping | 拼接录制 / 合成最佳版 | takes comping; 社区常直接说 comping |
| dither | 抖动 | 母带处理语境 |
| stems | 分轨 | matches our Bounce Stems = 导出分轨 |
| downmix | 缩混 / 下混 | our Trim and Downmix = 修剪并缩混 |
| playhead | 播放头 | (Ardour 指针 — divergent) |
| transient | 瞬态 | Tab to Transient = Tab 至瞬态 |
| latency | 延迟 | low-latency monitoring = 低延迟监听 |
| block (audio buffer/mix) | 块 | not 帧 (video frame) |
| scene / take (film metadata) | 场次 / 镜次 | playlist take = 版本 |
| lane | 道 | marker lane 标记道, transcription lane 转写道 |
| enable / enabled | 启用 / 已启用 | Record-enable = 启用录音 |
| online / offline (console) | 联机 / 脱机 | 在线/离线 is the internet sense |
| composer | 作曲家 | metadata; 作曲者 is only a fallback |
