# Where SoundCraft falls short of Pro Tools

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-16 · **Change:** minor (G-17: iXML metadata read) · **Target:** Avid Pro Tools Ultimate 2026.4.1

Every known shortfall, one entry each, ranked by how much it stops a Pro Tools user from doing
real work in SoundCraft. This is the work list: pick from the top. Rows marked **alpha blocker**
fail the core-workflow gate in [`roadmap.md`](roadmap.md#alpha-gate) and keep SoundCraft pre-alpha. Numbers and weights live in
[`target-app-parity.md`](target-app-parity.md); the plan that orders these into milestones is
[`roadmap.md`](roadmap.md). Hours are Opus 5.5 agent-hours (calibration in target-app-parity.md).
Estimates marked "≈" are judgements; issue numbers are `storytold/soundcraft` GitHub issues.

When you close a gap, delete its entry (or shrink it) in the same PR, and bump the timestamp.

## Ranked gaps

| # | Gap | Dimension | Doc | Est. (h) |
|---|---|---|---|---:|
| G-01 | **Alpha blocker.** No channel routing or duplex low-latency path (device, rate and buffer choice done; ASIO opt-in) | Hardware | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-02 | **Alpha blocker.** No MIDI device input or output | Hardware / MIDI | [hardware-parity.md](hardware-parity.md) | 20–35 |
| G-03 | **Alpha blocker.** Cannot exchange sessions with Pro Tools (no AAF/OMF, no `.ptx`) | File formats | [file-format-parity.md](file-format-parity.md) | 50–90 |
| G-04 | Open correctness bugs in features counted as done (~25 issues) | Stability | this file | 25–40 |
| G-05 | (Entitlement part: alpha blocker for the mix row) Third-party VST3s blocked in the signed macOS build; AU editors missing | Ecosystem | [plugin-parity.md](plugin-parity.md) | 15–25 |
| G-06 | (Record-to-disk part: alpha blocker for the tracking row) Sources decoded whole into RAM; recording into RAM | Performance | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-07 | Nothing is compared against Pro Tools' output (no fidelity harness) | Features | this file | 20–30 |
| G-08 | Built-in plugin catalogue thin and unverified | Features | [plugin-parity.md](plugin-parity.md) | 80–120 |
| G-09 | English only, no string catalog, no CJK/RTL rendering | Localization | [localization-parity.md](localization-parity.md) | 70–110 |
| G-10 | No control surfaces (HUI/MCU, EUCON) | Hardware | [hardware-parity.md](hardware-parity.md) | 40–70 |
| G-11 | No external sync (MTC, LTC, MIDI clock, video reference) | Hardware | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-12 | Immersive delivery: no Dolby Atmos/ADM BWF, MPEG-H, binaural | Features | [file-format-parity.md](file-format-parity.md) | 50–80 |
| G-13 | Sidechain inputs, ARA, out-of-process plugin sandboxing | Ecosystem | [plugin-parity.md](plugin-parity.md) | 30–50 |
| G-14 | Windows/Linux/Web exercised only lightly at runtime; no PR CI running the tests | Platforms / Stability | this file | 20–40 |
| G-15 | Elastic Audio depth: one algorithm, no Elastic Pitch | Features | [target-app-parity.md](target-app-parity.md) | 30–45 |
| G-16 | Session templates, track presets, workspace browser, batch rename | Features | [target-app-parity.md](target-app-parity.md) | 25–40 |
| G-17 | Video: HEVC/DNx decode, video export, external timecode | Features / Formats | [file-format-parity.md](file-format-parity.md) | 30–50 |
| G-18 | Speech-to-text transcription | AI | this file | 30–50 |
| G-19 | UI feel: generic plugin windows, shortcut collisions, pro-grade meters and scaling | UI/UX | [ui-parity.md](ui-parity.md) | 60–100 |
| G-20 | MIDI/instrument depth: MIDI learn, sampler, notation editing | Features | [target-app-parity.md](target-app-parity.md) | 40–60 |
| G-21 | Compressed export (MP3, AAC/M4A, Ogg/Opus) | File formats | [file-format-parity.md](file-format-parity.md) | 10–20 |
| G-22 | Avid cloud services (collaboration, Sketch, Splice, sign-in) | Ecosystem | — | not estimated (owner decision) |

## Details

### G-01. Audio interface support

- **Done (2026-10-12):** Setup › Playback Engine chooses the audio system, output and input
  device, sample rate and device buffer size (kept with the preferences, per machine; `ui.audio_device`
  for agents); f32 and 16/32-bit integer devices; ASIO behind the opt-in `asio` build feature
  (verified on Windows with an SSL 2+ at 128 samples).
- **Missing:** routing to physical channels (I/O Setup), one duplex stream on one clock (input and
  output are still separate cpal streams), latency reporting and record-offset compensation, ASIO
  in official builds (owner decision: Steinberg's SDK is proprietary or GPLv3), JACK, and
  verification on macOS and Linux. The mix still runs in fixed 512-sample blocks.
  `setup.playback_engine` still stores `device`/`buffer_size` in the session, unused.
- **Evidence:** code above; Playback Engine window says "Mix block size: 512 samples"; issue #42
  (no input device selection, forced monitoring feedback with virtual routing).
- **Impact:** blocks tracking with any multichannel interface; latency makes overdubs hard.
- **Estimate:** ≈ 25–40 h (device enumeration and selection, buffer size, duplex stream with one
  clock, ASIO feature build, per-channel I/O mapping into I/O Setup, latency reporting and
  record-offset compensation). Needs hands-on hardware testing.

### G-02. MIDI devices

- **Missing:** all MIDI hardware I/O. No `midir`/CoreMIDI/WinMM/ALSA sequencer code exists;
  `setup.midi_input_devices` and `setup.midi_studio` only store names. You cannot record from a
  keyboard, play a hosted instrument live, drive external synths, or send MIDI clock/MTC.
  `midi_editor.rs:288` notes audition "needs a live MIDI input path".
- **Impact:** MIDI production is limited to drawing notes and importing SMF.
- **Estimate:** ≈ 20–35 h (device layer, realtime input to armed tracks and instruments with
  timestamps, MIDI record/merge/loop, thru, output tracks, clock out).

### G-03. Session interchange with Pro Tools

- **Missing:** opening or saving `.ptx`/`.ptf`, AAF and OMF import and export (#40), Media
  Composer–compatible sessions. Our session format is `.scraft` JSON.
- **Impact:** the standard's main-format bar for beta. A Pro Tools user cannot bring a project in
  or hand one back; post-production (picture editors deliver AAF) is blocked.
- **Estimate:** AAF/OMF import + export ≈ 50–90 h (FilmCraft already writes AAF/OMF; we can learn
  from it but not share code). `.ptx` is proprietary and undocumented: owner decision on whether
  black-box format study is in the clean-room rules; not included in the estimate.

### G-04. Correctness bugs in "done" features

Open on 2026-10-10 (several have PRs open):
editing #3, #5, #140, #144; clip/session #86 (PR #123), #88 (PR #130), #91 (PR #124), #92 (PR #132);
automation #104 (PR #134); mix/bounce #59 (PR #131), #103 (PR #133), #152; playback #101 (PR #129);
MIDI #87, #98 (PR #125); import #89 (PR #128); CLI #95, #96 (PR #126, #127); MCP #145 (PR #146);
web #63, #64, #136; Clip List #4 (PR #75); plugin view #6; shortcuts #7, #41;
launch #142; Linux #45, #107.
- **Impact:** each one is small, but together they say "presence is not parity": an outside
  contributor found seven in one sitting (#116–#122).
- **Estimate:** ≈ 25–40 h at 0.5–1.5 h each, plus review of the open PRs.

### G-05. Plugin hosting on macOS

- **Missing:** the `com.apple.security.cs.disable-library-validation` entitlement in the signed
  build, so hardened runtime refuses third-party VST3 bundles (#153, #149). AU editor views
  (`crates/au-host/src/lib.rs` header: "Not hosted yet: Audio Unit editor views"). Embedded CLAP
  GUIs; VST3 editors on Windows/Linux.
- **Estimate:** ≈ 15–25 h (entitlement ≈ 1 h; AU Cocoa views and VST3 HWND/X11 views most of it).

### G-06. Memory-bound media

- **Missing:** streaming sources from disk; recording to disk. Every source is decoded whole into
  the `SourcePool`; takes accumulate in RAM until stop (PR #84 open).
- **Impact:** hour-long multitrack sessions and long takes exhaust memory; a crash during a long
  take loses it.
- **Estimate:** ≈ 15–25 h.

### G-07. Fidelity harness against Pro Tools

- **Missing:** any measured comparison: null tests of bounces of the same material, fade curve
  and pan-law checks, plugin frequency/dynamics responses, elastic-audio quality, automation
  timing. Pro Tools is installed; it can be driven black-box with synthetic sessions (outputs kept
  under `plan/`, never committed).
- **Estimate:** ≈ 20–30 h for the harness and the first 20 scored cases; turns many "≈" in
  target-app-parity.md into measurements.

### G-08. Built-in plugins

See [`plugin-parity.md`](plugin-parity.md): 26 processors + 2 instruments against Pro Tools'
roughly 80 bundled processors and instruments; no fidelity checks; generic parameter UIs; EQ
parametric request #139. ≈ 80–120 h.

### G-09. Localization

See [`localization-parity.md`](localization-parity.md): no catalog, every string literal in code;
egui has no complex-script shaping or RTL; no CJK font fallback (PR #141 open). ≈ 70–110 h plus
native-speaker review.

### G-10. Control surfaces

None: no HUI or Mackie Control, no EUCON (Avid-proprietary, needs Avid's SDK: owner decision),
no generic MIDI controller mapping. Pro Tools users on S1/S3/S4/S6/Dock or HUI devices can't
switch. ≈ 40–70 h for HUI/MCU plus a mapping layer; hardware needed to verify.

### G-11. Sync

No MTC/LTC chase or generation, no MIDI Beat Clock (`setup.midi_beat_clock` stores a flag only),
no video reference or word clock awareness, no Satellite-style linking. ≈ 15–25 h after G-01/G-02.

### G-12. Immersive delivery

No Dolby Atmos renderer integration, ADM BWF import/export, re-renders, binaural monitoring,
MPEG-H (new in Pro Tools 2026.4), surround/object automation. ≈ 50–80 h; Dolby's renderer and
SDKs are licensed, so we'd write our own ADM and object renderer.

### G-13. Plugin ecosystem depth

No sidechain (key input) buses for any host; no ARA (Melodyne-style editing is a menu in Pro
Tools); plugins run in-process, so a crashing plugin can take the session down. ≈ 30–50 h. AAX
cannot be hosted (#109): Avid's AAX SDK licence and PACE signing; owner decision, likely never.

### G-14. Platforms and CI

Only `freebsd.yml` and `windows-arm64.yml` run on pull requests, and only for manifest paths;
nothing runs `cargo xtask ci` on PRs. Windows and Linux have startup and rendering reports
(#41, #45, #107, #142). ≈ 20–40 h (a PR CI matrix plus runtime smoke tests on three OSes).

### G-15. Elastic Audio depth

One warp algorithm; Pro Tools offers several per track (polyphonic, rhythmic, monophonic,
varispeed, high-quality offline) plus Elastic Pitch and warp-marker editing depth. ≈ 30–45 h.

### G-16. Session management

No session templates, track presets, workspace/sound browser with previews, batch rename (new in
2026.4), disk-usage and relink depth. ≈ 25–40 h.

### G-17. Video and post

No HEVC, DNxHD/HR or AV1 decode; no bounce with video; no external timecode/9-pin; no iXML
field-recorder conform (iXML metadata is read and shown by `info`, but import doesn't use it yet). ≈ 30–50 h.

### G-18. Speech-to-text

Pro Tools transcribes dialogue to a searchable transcript (upgraded in 2026.4). We have a
settings command only. Needs an openly licensed local speech model. ≈ 30–50 h.

### G-19. UI feel

See [`ui-parity.md`](ui-parity.md). ≈ 60–100 h.

### G-20. MIDI and instrument depth

MIDI learn, a sampler (PR #108), drum machine depth, notation editing (tuplets, lyrics,
articulations, parts). ≈ 40–60 h.

### G-21. Compressed export

Bounce writes WAV/BWF/RF64, AIFF and FLAC; Pro Tools also writes MP3 and AAC/M4A in Bounce Mix.
≈ 10–20 h (pure-Rust encoders).

### G-22. Avid cloud services

Collaboration, cloud projects, Sketch, Splice, Learn, sign-in: 11 of the 30 menu leaves still
missing. Out of scope unless the owner decides otherwise.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-16 | minor | G-17: iXML metadata read from WAV/BWF/RF64; conform and track naming on import still missing |
| 2026-10-12 | minor | G-01 shrunk: audio device, rate and buffer choice; opt-in ASIO |
| 2026-10-10 | minor | Marked the alpha blockers (G-01, G-02, G-03; parts of G-05 and G-06) from the core-workflow gate |
| 2026-10-10 | major | Created from the full re-measure against Pro Tools Ultimate 2026.4.1: 22 ranked gaps with evidence and estimates |
