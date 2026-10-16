# File-format parity

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-14 · **Change:** minor (MP3 bounce) · **Target:** Avid Pro Tools Ultimate 2026.4.1

Every format Pro Tools reads or writes, and ours. The Pro Tools list comes from the document types
its bundle declares (`CFBundleDocumentTypes` in Info.plist: names only, nothing else read) and its
documented import/bounce options. Ours comes from `crates/audio-io` (`detect_format`, `decode`,
`ENCODE_EXTENSIONS`), `crates/engine/src/cmd/file.rs`, `crates/engine/src/score.rs`,
`crates/engine/src/clip_group_file.rs` and `crates/video`.

**File formats: ~40 % ready** (estimated: weighted by how often a Pro Tools user hits each row;
session interchange carries a third of the weight and is at 0 %). Remaining ≈ 70–120 h.

Legend: ✅ supported and tested · 🟡 partial · ❌ missing · — not applicable.

## Sessions and interchange

| Format | Pro Tools | Read | Write | Tests / notes |
|---|---|---|---|---|
| Pro Tools session `.ptx` (and `.ptf` from 7–9) | native | ❌ | ❌ | Proprietary, undocumented. Owner decision needed on black-box format study (G-03) |
| Session templates `.ptxt` | read/write | ❌ | ❌ | We have no templates at all (G-16) |
| AAF (Advanced Authoring Format) | import/export | ❌ | ❌ | #40. The post-production handoff from picture editors (G-03) |
| OMF | import/export | ❌ | ❌ | Legacy, still delivered by some editors |
| Media Composer–compatible session | export | ❌ | ❌ | AAF variant |
| ADM BWF (Dolby Atmos master) | import/export | ❌ | ❌ | G-12 |
| SoundCraft session `.scraft` + `Audio Files/` | — | ✅ | ✅ | JSON (serde); mutated-file property test loads or fails cleanly; plugin state embedded. Fades save optional `curvature` (-1..1); missing values default to 0 and preserve legacy envelopes |
| Import Session Data (from another session) | Pro Tools sessions | 🟡 | — | From `.scraft` only |
| Session info as text | export | — | ✅ | `file.export_session_text` |
| Clip groups `.rgrp` / ours `.scgrp` | read/write | 🟡 | 🟡 | Our own `.scgrp` format; Pro Tools' clip-group files not read |
| Track presets, fade presets, I/O settings, workspace, sync settings | read/write | ❌ | ❌ | Their equivalents don't exist yet (G-16) |

## Audio

| Format | Pro Tools | Read | Write | Tests / notes |
|---|---|---|---|---|
| WAV / BWF (PCM 16/24/32, float 32), `bext` | native | ✅ | ✅ | Native reader/writer, TPDF dither option; also 8-bit and float 64 read |
| RF64 / BW64 (> 4 GiB) | read/write | ✅ | ✅ | `encode_wav_rf64` |
| WAV `WAVE_FORMAT_EXTENSIBLE` multichannel with channel mask | read/write | ✅ | ✅ | `encode_with_channel_mask`; default 5.1 import creates a stereo track (#89, PR #128) |
| AIFF / AIFF-C | native | ✅ | ✅ (`fl32`) | Native |
| FLAC | import | ✅ | ✅ | Own encoder |
| MP3 | import, bounce | ✅ (symphonia) | ✅ | Own encoder: MPEG-1 Layer III CBR 32–320 kbps, joint stereo, bit reservoir; other session rates resampled to 44.1/48 kHz; surround folds to stereo. No gapless (LAME) tag yet |
| AAC / M4A / MP4 audio | import, bounce | ✅ (symphonia) | ❌ | G-21 |
| ALAC | import | ✅ | ❌ | |
| Ogg Vorbis | — | ✅ | ❌ | Beyond Pro Tools |
| CAF | — | ✅ | ❌ | |
| Sound Designer / SD II | import (legacy) | ❌ | — | Rare today |
| ReCycle / REX | import | ❌ | — | Loops |
| MXF audio (OP-Atom, OP1a) | import | ❌ | ❌ | Avid media; FilmCraft has an MXF reader to learn from |
| iXML / polyphonic field-recorder metadata | read | ❌ | ❌ | G-17 |
| Peak overviews | `.wfm` cache | ✅ (own, in memory) | — | `audio-io::peaks`; not cached on disk |

## MIDI and notation

| Format | Pro Tools | Read | Write | Tests / notes |
|---|---|---|---|---|
| Standard MIDI File (type 0/1) | import/export | ✅ | 🟡 | Exported MIDI omits imported controllers and ignores the tracks filter (#87, #98; PR #125) |
| MusicXML (to Sibelius) | export | — | ✅ | `file.export_sibelius`, `score.rs` |
| Printable score | print | — | ✅ (SVG) | |

## Video

| Format | Pro Tools | Read | Write | Tests / notes |
|---|---|---|---|---|
| QuickTime / MP4, H.264 | import, bounce | ✅ | ❌ | Own pure-Rust decoder (Baseline/Main/High); mutated-movie fuzzing |
| ProRes (all flavours) | import | ✅ | ❌ | Own decoder |
| Motion JPEG | import | ✅ | ❌ | |
| HEVC, AV1 | import | ❌ | ❌ | G-17 |
| DNxHD / DNxHR, MXF video | import | ❌ | ❌ | G-17 |
| Bounce with video | export | — | ❌ | G-17 |

## Plugin and preset files

| Format | Pro Tools | Ours |
|---|---|---|
| AAX plug-ins | the only format it hosts | ❌ (cannot: Avid SDK licence, PACE signing) |
| VST3 / Audio Units / CLAP | — (AAX only) | ✅ hosted (ahead of Pro Tools) |
| Plug-in settings `.tfx` | read/write | ❌ (our own JSON presets per plugin) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | minor | MP3 written by Bounce Mix and Bounce Stems (own encoder). Readiness estimate unchanged |
| 2026-10-13 | minor | Fade curvature round-trips in sessions with a zero default for older files. Format version and readiness estimate unchanged |
| 2026-10-10 | major | Created: format inventory against Pro Tools 2026.4.1's declared document types and import/bounce options |
