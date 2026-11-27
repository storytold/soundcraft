# Where SoundCraft falls short of Pro Tools

> **Last reviewed:** 2026-11-18 · **Last updated:** 2026-11-27 · **Change:** minor (G-01: sends go to output paths as well as busses; I/O Setup's Default restores the default input and output paths; stereo input selectors offer the default pairs; previously minor: G-01: Setup › I/O maps named input paths for recording and monitoring, verified on Core Audio with an RME UFX; previously minor: G-01: Setup › I/O routes named output paths to hardware channels, verified on Core Audio with an RME UFX; Core Audio opens every output of an interface; output selectors list every interface output pair; input selectors follow the device's recording width; previously trivial: G-04: Number of Loops accepts up to 1,000; crossfade remains open; previously minor: G-06: recording to disk is done (#84, long takes #888); streaming sources remain; previously minor: G-02 shrunk: MIDI input devices work; G-03: strict external-WAV AAF import; previously minor: G-05: plugin editors on Linux/X11; previously trivial: G-09: the pt-BR catalog now covers helper-passed tooltips, the status bar, the remove-session prompt and engine error text; G-08: Frequency Shifter, 28 processors; G-04: volume automation read at every sample, #1354; G-01: `setup.playback_engine` applies the device buffer in the desktop app; G-04: Clip Looping crossfade and loop count) · **Target:** Avid Pro Tools Ultimate 2026.4.1

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
| G-01 | **Alpha blocker.** Named input and output paths (outputs past the main mix's width on Core Audio only); no input sub-paths or duplex low-latency path (device, rate and buffer choice done; ASIO opt-in) | Hardware | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-02 | **Alpha blocker.** MIDI devices: input works (checked on Linux only); no output, clock or loop/merge recording | Hardware / MIDI | [hardware-parity.md](hardware-parity.md) | 12–22 |
| G-03 | **Alpha blocker.** Session exchange remains incomplete (strict external-WAV AAF import only; no AAF/OMF export, OMF import or `.ptx`) | File formats | [file-format-parity.md](file-format-parity.md) | 50–90 |
| G-04 | Open correctness bugs in features counted as done (~25 issues) | Stability | this file | 25–40 |
| G-05 | (Entitlement part: alpha blocker for the mix row) Third-party VST3s blocked in the signed macOS build; AU editors missing | Ecosystem | [plugin-parity.md](plugin-parity.md) | 10–15 |
| G-06 | Sources decoded whole into RAM (recording to disk is done: #84, #888) | Performance | [hardware-parity.md](hardware-parity.md) | 15–25 |
| G-07 | Nothing is compared against Pro Tools' output (no fidelity harness) | Features | this file | 20–30 |
| G-08 | Built-in plugin catalogue thin and unverified | Features | [plugin-parity.md](plugin-parity.md) | 80–120 |
| G-09 | English-first UI with a pt-BR catalog (not native-reviewed); all fixed UI strings covered, no CJK/RTL rendering | Localization | [localization-parity.md](localization-parity.md) | 20–40 |
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
| G-20 | MIDI/instrument depth: MIDI learn, notation editing | Features | [target-app-parity.md](target-app-parity.md) | 40–60 |
| G-21 | Compressed export (MP3, AAC/M4A, Ogg/Opus) | File formats | [file-format-parity.md](file-format-parity.md) | 10–20 |
| G-22 | Avid cloud services (collaboration, Sketch, Splice, sign-in) | Ecosystem | — | not estimated (owner decision) |

## Details

### G-01. Audio interface support

- **Done (2026-10-12):** Setup › Playback Engine chooses the audio system, output and input
  device, sample rate and device buffer size (kept with the preferences, per machine; `ui.audio_device`
  for agents); f32 and 16/32-bit integer devices; ASIO behind the opt-in `asio` build feature
  (verified on Windows with an SSL 2+ at 128 samples).
- **Done (2026-10-25):** `setup.playback_engine` and `setup.hardware` apply `device` (the output
  name) and `buffer_size` in the desktop app on the same path as the Playback Engine window
  (`ui.audio_device`) and return the open device. `buffer_size` is any integer 16..=8192 and is
  not rounded onto the window's power-of-two list. Headless (CLI, no app) still stores the request
  and returns `applied: false`, with `buffer_size` null when unset; playback does not read those
  session values.
- **Done (2026-11-25):** Setup › I/O maps named output paths to physical channels, and track
  output selectors assign them; I/O Setup edits reroute the open stream. Core Audio opens all of
  an interface's outputs, so paths reach outputs past the main mix; an interface that offers only
  its full width opens at the rate it runs at, and the mix resamples. On an RME Fireface UFX with a
  cable from outputs 3–4 to inputs 5–6, a track on a stereo path at channel 3 is heard on outputs
  3–4; moving the path to channel 7 while it plays silences them, and moving it back restores
  them. Paths on ADAT outputs 17–18 and 21–22 (past the 16-channel main) play at the track's level.
  The assignment survives save and reopen. With a 2-channel output device, I/O Setup marks the
  path Unavailable and playback goes on. Other hosts open only the main mix's width, so a path
  there must fit within it.
- **Done (2026-11-25):** Track input selectors use the selected device's usable recording width.
  Track output selectors list every further pair of the device's outputs by its default path name
  (Out 3-4 to Out 29-30 on a 30-output RME UFX); choosing one, or naming it in `track.output`,
  makes that output path, as Pro Tools' default I/O settings hold one path per pair. Track input
  and output lists scroll in the existing Edit and Mix selectors.
- **Done (2026-11-25):** Setup › I/O stores named inputs with a contiguous hardware range and
  format. Audio tracks select them by name for recording and input monitoring. On the same UFX
  and cable, a mono named input at channel 5 records the same samples as `In 5`, and the path and
  the track's assignment survive save and reopen.
- **Done (2026-11-27):** Sends go to output paths as well as busses, as in Pro Tools: the send
  selector lists the output paths as the track output selector does (an interface pair makes its
  path), then the busses, and `mix.send` takes `output`. A send to a path plays on that path's
  hardware channels in the path's width (a mono path's send has no panner). I/O Setup's Default
  deletes unused output paths, restores default names and the main path on channels 1-2, and makes
  a path for every pair of the device's outputs.
- **Done (2026-11-27):** A stereo track's input selector offers Pro Tools' default input pairs
  (In 1-2, In 3-4 …) instead of every adjacent pair; a pair off those that a track already uses
  (In 2-3 in an older session) stays listed. I/O Setup's Inputs page lists the device's default
  input paths, and its Default deletes unused named inputs and moves tracks on a named input that
  sits on a default path back to that numbered input.
- **Missing:** hardware input sub-paths and noncontiguous mappings, outputs past the main mix's
  width outside Core Audio, one duplex stream on one clock (input and output are still separate
  cpal streams), latency reporting and record-offset compensation, ASIO in official builds (owner decision:
  Steinberg's SDK is proprietary or GPLv3), JACK, and verification on Linux. The mix
  still runs in fixed 512-sample blocks.
- **Evidence:** code above; Playback Engine window says "Mix block size: 512 samples"; issue #42
  (no input device selection, forced monitoring feedback with virtual routing).
- **Impact:** blocks tracking with any multichannel interface; latency makes overdubs hard.
- **Estimate:** ≈ 25–40 h (device enumeration and selection, buffer size, duplex stream with one
  clock, ASIO feature build, per-channel input mapping into I/O Setup, latency reporting and
  record-offset compensation). Needs hands-on hardware testing.

### G-02. MIDI devices

- **Done (2026-10-12):** MIDI input devices (`crates/playback/src/midi.rs`, `midir`): keyboards
  play record-enabled and input-monitored MIDI and instrument tracks live and are recorded into
  MIDI clips, with a device and channel choice per track. Controllers, pitch bend, aftertouch
  and program changes reach the instrument live. Notes are kept per device and channel, and a
  full input queue releases the notes of its device and marks the take incomplete. The device
  switches (Setup › Playback Engine, also opened by Setup › MIDI › MIDI Input Devices) are app
  preferences. User guide: [midi-input.md](midi-input.md). On Linux, live play was checked with
  a hardware keyboard (Akai MPK mini 3) and recording with a virtual one;
  **not yet on macOS or Windows** (type-checked only).
- **Missing:** MIDI output (external synths, MIDI tracks without an instrument), MIDI clock/MTC
  out, loop and merge recording (a MIDI take replaces what it covers; a loop pass piles every
  pass into one clip), audition from the MIDI editor (`midi_editor.rs:288`), MIDI
  input on FreeBSD and the web. `setup.midi_input_devices` and `setup.midi_studio` still only
  store names in the session; the real device switches are app preferences.
- **Impact:** you can play and record a keyboard, not drive hardware synths or sync other gear.
- **Estimate:** ≈ 12–22 h (output tracks and thru, clock out, merge/loop takes, controller path).

### G-03. Session interchange with Pro Tools

- **Done (2026-11-14):** strict AAF import for CompositionMob SourceClip/Filler edits with external
  mono/stereo PCM WAV media. It creates editable clips with source handles, relink and absolute or
  relative timecode mapping. The reader bounds cached property data to 128 MiB and rejects duplicate
  local references before expanding objects.
- **Missing:** gain, fades, transitions, embedded essence, split channels and mixed sample rates in
  AAF; AAF export; OMF import and export; opening or saving `.ptx`/`.ptf`; Media
  Composer–compatible sessions (#40). Our session format is `.scraft` JSON.
- **Impact:** a main-format requirement for beta. Only the strict AAF subset can come in; other AAFs
  and handing a session back remain blocked for post-production interchange.
- **Estimate:** AAF/OMF import + export ≈ 50–90 h (FilmCraft already writes AAF/OMF; we can learn
  from it but not share code). `.ptx` is proprietary and undocumented: owner decision on whether
  black-box format study is in the clean-room rules; not included in the estimate.

### G-04. Correctness bugs in "done" features

**Fixed (2026-10-19, #791):** Float32 WAV and AIFF-C encoding retain finite samples above full scale. Whole-file and streaming tests cover both formats, integer clipping, and the existing NaN/infinity behavior. A bounce regression checks preserved headroom and file/report agreement with normalization off, then checks normalized output.

**Fixed (2026-10-18):** Mono device playback averages the stereo channels before output clipping,
**Fixed (2026-10-16):** Master fader volume automation uses the first and last samples of each
**Fixed (2026-10-16):** Aux, folder and master latency calculations now skip inserts already
render block, matching track faders. A breakpoint at the next block no longer ramps the preceding
block, and ramps reach their last-sample value. Regression tests cover nonzero render starts,
one-sample callbacks and several block sizes. Since #1354, track and master volume automation is read at
every sample, so its interior breakpoints no longer depend on the block; send levels and plugin parameters
are still read once per block.

disabled for producing NaN or infinity. Subsequent blocks compensate only for healthy inserts;
regression tests verify rendered impulse timing, retained healthy latency and reset/retry at
64- and 512-sample blocks, with audio tracks as the control.

instead of discarding the right side. Surround mixes average both sides of the ITU stereo
fold-down. Regression tests cover right-only and left-only audio, equal and opposite-polarity
channels, clipping order, centered mono tracks, each 5.1 channel and 1-, 37- and 512-frame callbacks.
**Fixed (2026-10-16):** Split into Mono now gives each output track its own source channel,
including surround channels, instead of repeating channel zero. Clip edits, source timestamps,
Undo/Redo and save/reopen are covered by regression tests; missing or empty media fails atomically.

**Open (2026-10-25):** the Clip Looping dialog (#1030) has no option to crossfade the loops into
each other.

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
  GUIs on macOS (#485). VST3 and embedded CLAP editors are hosted on Windows and Linux/X11.
- **Estimate:** ≈ 10–15 h (entitlement ≈ 1 h; AU Cocoa views and embedded CLAP on macOS most of it).

### G-06. Memory-bound media

- **Done:** recording to disk: takes are written to disk as they record (#84), and a take that
  reaches the readable file size goes on in a new file and is placed whole, crash recovery included
  (#888).
- **Missing:** streaming sources from disk. Every source is decoded whole into the `SourcePool`.
- **Impact:** hour-long multitrack sessions exhaust memory.
- **Estimate:** ≈ 15–25 h.

### G-07. Fidelity harness against Pro Tools

- **Missing:** any measured comparison: null tests of bounces of the same material, fade curve
  and pan-law checks, plugin frequency/dynamics responses, elastic-audio quality, automation
  timing. Pro Tools is installed; it can be driven black-box with synthetic sessions (outputs kept
  under `plan/`, never committed).
- **Estimate:** ≈ 20–30 h for the harness and the first 20 scored cases; turns many "≈" in
  target-app-parity.md into measurements.

### G-08. Built-in plugins

See [`plugin-parity.md`](plugin-parity.md): 28 processors + 3 instruments against Pro Tools'
roughly 80 bundled processors and instruments; no fidelity checks; generic parameter UIs; EQ
parametric request #139. ≈ 80–120 h.

### G-09. Localization

See [`localization-parity.md`](localization-parity.md): a pt-BR catalog covers the menu labels and
every fixed UI string passed to `text` / `localized_format!` or localized inside a button, toggle or
close button (test-checked), including the Edit-window status bar, the remove-session question and
the send help text; engine errors are matched against catalog templates at display time by
`error_text`. What remains is independent native-speaker review plus egui's lack of complex-script
shaping and RTL, and the missing other Pro Tools languages; values built at run time (file, track
and device names) and engine command parameter documentation stay untranslated. egui has no
complex-script shaping or RTL; no CJK font fallback (PR #141 open). ≈ 70–110 h plus
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

Session templates, track-preset recall UI and browser, workspace/sound browser with previews,
batch rename (new in 2026.4), disk-usage and relink depth remain. Track-preset saves capture the
current hosted-plugin state before writing (#661); recall UI, partial recall and preset browsing remain
tracked in #639. ≈ 25–40 h.

### G-17. Video and post

No HEVC, DNxHD/HR or AV1 decode; no bounce with video; no external timecode/9-pin; no iXML
field-recorder conform. ≈ 30–50 h.

### G-18. Speech-to-text

Pro Tools transcribes dialogue to a searchable transcript (upgraded in 2026.4). We have a
settings command only. Needs an openly licensed local speech model. ≈ 30–50 h.

### G-19. UI feel

See [`ui-parity.md`](ui-parity.md). ≈ 60–100 h.

### G-20. MIDI and instrument depth

MIDI learn, drum machine depth, notation editing (tuplets, lyrics,
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
| 2026-11-27 | minor | G-01: stereo input selectors offer Pro Tools' default pairs; the Inputs page lists the default input paths and has Default |
| 2026-11-27 | minor | G-01: sends go to output paths (Out N-M and named paths) as well as busses; I/O Setup's Default restores the default output paths |
| 2026-11-25 | minor | G-01: named hardware input paths for recording and monitoring (verified on Core Audio with an RME UFX) |
| 2026-11-25 | minor | G-01: Setup › I/O routes named output paths to hardware channels (verified on Core Audio with an RME UFX); Core Audio opens every output of an interface; output selectors list every interface output pair; input selectors follow the device's recording width |
| 2026-11-23 | trivial | G-04: Clip Looping Number of Loops accepts up to 1,000; crossfade remains open; estimates unchanged |
| 2026-11-14 | minor | G-03: strict external-WAV AAF import with bounded property caching; AAF/OMF export, OMF import and `.ptx` remain; estimates unchanged |
| 2026-11-13 | trivial | G-09: the pt-BR catalog now covers tooltips localized inside buttons and toggles, the Edit-window status bar, the remove-session question, the send help text and engine errors shown in the status line; estimates unchanged |
| 2026-11-09 | trivial | G-08: 28 processors + 3 instruments with the Frequency Shifter |
| 2026-11-04 | trivial | G-04: track and master volume automation read at every sample, so breakpoints act on their own sample in any block size (#1354); estimates unchanged |
| 2026-10-30 | minor | G-06: recording to disk is done (takes written as they record, #84; long takes continue in a new file, #888); only streaming sources from disk remain |
| 2026-10-30 | minor | G-05: VST3 and embedded CLAP editors on Linux/X11 (#527) |
| 2026-10-25 | trivial | G-04: Clip Looping (#1030) lacks the loop crossfade option, and Number of Loops stops at 512 while length and fill make up to 1000 loops |
| 2026-10-25 | trivial | G-01: `setup.playback_engine` and `setup.hardware` apply the device and buffer in the desktop app; headless returns `applied: false` |
| 2026-10-24 | trivial | G-08: 27 processors + 3 instruments, as the plugin registry holds; G-20: the sampler shipped (#108), so it is no longer listed as missing |
| 2026-10-19 | trivial | G-04: Float32 encoding retains finite headroom (#791); integer clipping and non-finite handling retained |
| 2026-10-18 | trivial | G-04: mono device stereo/surround fold-down fixed and regression-tested; estimates unchanged |
| 2026-10-16 | trivial | G-04: master volume automation block boundaries fixed and regression-tested; estimates unchanged |
| 2026-10-16 | trivial | G-04: failed aux/folder/master inserts excluded from latency compensation after containment; estimates unchanged |
| 2026-10-16 | trivial | G-04: Split into Mono channel preservation fixed and regression-tested; estimates unchanged |
| 2026-10-16 | minor | G-16: capture current hosted-plugin state before saving a track preset (#661); broader preset work remains (#639) |
| 2026-10-12 | minor | G-01 shrunk: audio device, rate and buffer choice; opt-in ASIO |
| 2026-10-12 | minor | G-02 shrunk: MIDI input devices, live play-through and MIDI recording landed; output, clock and loop/merge recording remain |
| 2026-10-10 | minor | Marked the alpha blockers (G-01, G-02, G-03; parts of G-05 and G-06) from the core-workflow gate |
| 2026-10-10 | major | Created from the full re-measure against Pro Tools Ultimate 2026.4.1: 22 ranked gaps with evidence and estimates |
