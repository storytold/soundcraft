# Reference corpora index

Chinese UI translations of open-source DAWs, used by
[`tools/cross_check.py`](../tools/cross_check.py) to ground zh-Hans wording
decisions. The files themselves live in `raw/` (gitignored — they are
GPL-licensed project data, not ours to redistribute inside an MIT/Apache tree).

Downloaded: 2026-10-20. Re-download with the commands below to refresh.

| File in `raw/` | Project | Version / branch | Upstream license | Notes |
|---|---|---|---|---|
| `ardour_zh_CN.po` | Ardour | 5.12 (Debian repack `1:5.12.0-3`) | GPL-2.0-or-later | upstream moved `.po` files out of the git tree (they are generated from Ardour's translation platform); Debian 9.8 repack strips them, so 5.12 is the newest obtainable |
| `audacity_zh_CN.po` | Audacity | master `203cd22` | GPL-2.0 | the au3/ gettext catalog |
| `audacity_zh_CN.ts` | Audacity | master `203cd22` | GPL-2.0 | the share/ Qt catalog (shared menus/dialogs) |
| `hydrogen_zh_CN.ts` | Hydrogen | main `00509be` | GPL-2.0-or-later | |
| `lmms_zh_CN.ts` | LMMS | master `4fde529` | GPL-2.0-or-later | `data/locale/zh_CN.ts` |
| `mixxx_zh_CN.ts` | Mixxx | main `44dc9ed` | GPL-2.0-or-later | `res/translations/mixxx_zh_CN.ts` |
| `musescore_zh_CN.ts` | MuseScore | main `7b2d005` | GPL-3.0 | main UI catalog; notation terminology is the closest open reference for score-editor strings |
| `instruments_zh_CN.ts` | MuseScore | main `7b2d005` | GPL-3.0 | instrument-name catalog; treated as MuseScore by the cross-check |

All projects are mirrored on GitHub; `master`/`main` refer to the branches the
archives were cut from.

## How each file was obtained

GitHub raw / API were reachable, but `codeload` zips are the most robust route
for whole trees, and two projects needed a detour:

```sh
# Audacity, LMMS, Mixxx, Hydrogen, MuseScore — codeload zip, extract the locale file
curl -fsSL -o /tmp/x.zip https://codeload.github.com/audacity/audacity/zip/refs/heads/master
unzip -j /tmp/x.zip 'audacity-master/au3/locale/zh_CN.po' -d docs/i18n/sources/raw/
#   → rename to audacity_zh_CN.po; same pattern for the others:
#     LMMS     lmms-master/data/locale/zh_CN.ts                 → lmms_zh_CN.ts
#     Mixxx    mixxx-main/res/translations/mixxx_zh_CN.ts       → mixxx_zh_CN.ts
#     Hydrogen hydrogen-main/data/i18n/hydrogen_zh_CN.ts        → hydrogen_zh_CN.ts
#     MuseScore MuseScore-main/share/locale/musescore_zh_CN.ts  → musescore_zh_CN.ts
#               MuseScore-main/share/locale/instruments_zh_CN.ts→ instruments_zh_CN.ts

# Ardour — git.ardour.org (Gitea) requires sign-in; Ardour's GitHub tree contains
# only a README (zip export disabled); current Debian repacks strip .po. The last
# Debian version that still ships the translations is 1:5.12.0-3, at gtk2_ardour/po/zh.po:
curl -fsSL -o docs/i18n/sources/raw/ardour_zh_CN.po \
  https://sources.debian.org/data/main/a/ardour/1%3A5.12.0-3/gtk2_ardour/po/zh.po
```

When Ardour re-exposes current translations (their translation platform export
or a public API), replace the 5.12 file with that and note the new source here.

## Other reference material consulted (web, not downloaded)

- **术语在线** (termonline.cn) — 全国科学技术名词审定委员会的规范术语库；声学/电声学
  学科可查「声像」「混响」「均衡」等规范中文名。用于裁决纯中文写法（如 撤消/撤销）。
- **Ubuntu 中文 Wiki 的 Ardour 教程翻译**（wiki.ubuntu.com.cn）— 中文音频社区的
  Ardour 术语惯用法的另一来源（区域、总线、播送等词的社区用法）。
- **midifan 的 Cubase 中文教程文章**（midifan.com）与 **cubase.net.cn** — 观察
  Steinberg 官方中文手册术语的社区转述（推子、通道条、插入、发送、声像）。仅作
  术语事实参考，不复制任何原文表述。
- **rockjins 的音乐中英速查表**（rockjins.js.org）— 通用音乐制作词汇的社区对照表。
- **wordln 等词汇站的音乐制作词条页** — 通用入门词汇，权重最低，仅作旁证。

Pro Tools 官方中文界面字符串是 Avid 的专有翻译资产，**不下载、不抽取、不作为
语料**；中文术语与 Avid 的取舍差异记录在 [`../glossary.md`](../glossary.md)。
