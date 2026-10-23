# Korean (ko) interface

SoundCraft's Korean catalog is [`crates/ui-egui/src/i18n/ko.tsv`](../crates/ui-egui/src/i18n/ko.tsv).
It carries the same 1,141 keys as the Simplified Chinese reference catalog (a test holds the two
key-for-key), so the menu, command, menu-path and looked-up-literal coverage gates apply to Korean
too. The mechanism, the preference and the catalog format are in [`localization.md`](localization.md).

Translations are written clean-room from the meaning of SoundCraft's English text; no proprietary
translation resources were consulted or copied. Some entries start from the strings contributed in
#835. An AI pass produced and checked the first draft; a native speaker reviews every string.

## Terminology

Common Korean audio-engineering usage: established loanwords stay loanwords, and Korean words are
used where they are the everyday UI term.

| English | Korean | Note |
|---|---|---|
| Track / Clip / Session | 트랙 / 클립 / 세션 | |
| Send / Insert / Bus / Aux | 센드 / 인서트 / 버스 / Aux | |
| Fader / Pan / Gain / Trim | 페이더 / 팬 / 게인 / 트림 | |
| Mute / Solo | 뮤트 / 솔로 | DAW usage, not the general 음소거 |
| Automation (off/read/touch/latch/write) | 오토메이션 (끄기/읽기/터치/래치/쓰기) | |
| Grid / Snap to Grid | 그리드 / 그리드에 스냅 | |
| Shuffle / Slip / Spot | 셔플 / 슬립 / 스팟 | edit modes |
| Preset | 프리셋 | |
| Ruler | 룰러 | |
| Memory Location | 메모리 위치 | feature name, translated by meaning |
| Regions (MIDI track view) | 클립 | SoundCraft calls them clips everywhere else |
| Meter (time signature) | 박자표 | level meters are 미터 |
| Delay (insert category) | 딜레이 | timing offsets are 지연 (지연 보정, 지연(틱)) |
| Sample rate / Bit depth | 샘플 레이트 / 비트 심도 | |
| Elastic Audio / Elastic Properties | Elastic Audio / Elastic Audio 속성 | feature name, kept in English |
| Beat Detective | 비트 탐지 | feature name, translated by meaning |
| Universe / Video Universe | 세션 개요 / 비디오 개요 | feature name, translated by meaning |
| Edit window / Mix window | 편집 창 / 믹스 창 | |
| Undo / Redo | 실행 취소 / 다시 실행 | |

Product, plug-in and technology names (AudioSuite, CLAP, VST3, Dolby Atmos, D-Verb, …), units and
user-assigned names are not translated. Narrow-strip and small-button fallbacks (`I A-E`, `S A-E`,
`t/l`, `Vel +10`) keep their Latin abbreviations so they stay shorter than the full labels. Counts use one form with a counter word: `{n} clip|{n}
clips` → `클립 {n}개`.

## Not covered yet

- **Web build**: the bundled web font is a Noto Sans SC subset with no Hangul, so Korean shows
  missing-glyph boxes on the web. Desktop draws Hangul from the system font (Apple SD Gothic Neo,
  Malgun Gothic, Noto Sans CJK KR, NanumGothic). A Hangul subset for the web is a follow-up.
- **IME**: Hangul composition in text fields (track and clip renaming) is untested.
- Engine diagnostics, the control protocol, MCP and the parity report stay English by design.
