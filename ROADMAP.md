# SoundCraft roadmap

**Stage: pre-alpha** · next: alpha, ~5% and ~110–180 h away (the alpha gate's three failing workflows)

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** minor (readiness by audience with hours to ~95 %) · **Target:** Avid Pro Tools Ultimate 2026.4.1

SoundCraft is a clean-room, pure-Rust digital audio workstation that aims at full parity with
Avid Pro Tools, and then beyond it: faster, open, scriptable and agent-controllable. This page is
the one-page summary; the detail is in [`docs/target-app-parity.md`](docs/target-app-parity.md)
(assessment), [`docs/gaps.md`](docs/gaps.md) (ranked work list), [`docs/roadmap.md`](docs/roadmap.md)
(milestones and beta plan) and [`docs/parity-checklist.md`](docs/parity-checklist.md) (generated
menu checklist).

## Headline numbers

| Number | Value | Kind |
|---|---|---|
| Menu breadth | **482 / 512 Pro Tools menu leaves (94.1 %)**; engine alone 397 / 512 | measured (`cargo xtask parity` + UI-layer aliases) |
| Feature breadth | **~70 %** | estimated (menus plus plugins, hardware, formats, immersive, cloud) |
| **Ready for real work** | **~40 %** | estimated, weighted by dimension |
| Mainstream practitioner (tracking/mixing engineer) | **~36 %** | estimated: 50.6 % weekly-area depth × 0.90 interaction × 0.92 stability × 0.85 file exchange ([how](docs/target-app-parity.md#mainstream-practitioner-and-essentials-user)) |
| Essentials user (song or podcast, defaults) | **~57 %** | estimated: 72.4 % core-feature depth × 0.90 launch/stability × 0.90 discoverability × 0.97 files ([how](docs/target-app-parity.md#mainstream-practitioner-and-essentials-user)) |
| Remaining to alpha | **~110–180 Opus 5.5 agent-hours** (the failing gate rows) | estimated, calibrated on PR history |
| Remaining to beta | **~350–570 Opus 5.5 agent-hours** (includes the alpha work; ~60–70 % parallelizable) | estimated |
| Remaining to full parity | **~950–1,450 Opus 5.5 agent-hours** | estimated |

| Audience | Ready % | Agent-hours to ~95 % | Work that dominates |
|---|---:|---:|---|
| Full target (ready for real work) | ~40 % | ~950–1,450 h | Hardware, the stock plugin catalogue, AAF/OMF and immersive, localization, fidelity |
| Mainstream practitioner | ~36 % | ~380–600 h | Interface and MIDI recording, AAF/OMF, stock-plugin depth, core-path bugs |
| Essentials user | ~57 % | ~110–200 h | Launch and platform fixes, onboarding, effect panels and presets, MP3/AAC export |

Hours are Opus 5.5 agent wall-clock hours, calibrated on this repo's PRs; 60–70 % parallelizes.
Detail: [target-app-parity.md](docs/target-app-parity.md#readiness-by-audience).

**Why pre-alpha:** the core-workflow gate in [docs/roadmap.md](docs/roadmap.md#alpha-gate) fails.
Editing, mixing, bouncing, and saving and reopening work end to end. But a Pro Tools user can't
record from a chosen audio interface at a chosen buffer size (only the default devices, a fixed
512-sample block, and takes held in RAM). They can't record MIDI from a keyboard (there is no
MIDI device layer). And they can't bring a session in or hand one back (no AAF/OMF/`.ptx`).
Readiness (~40 %) sits right at the alpha bar, but a gate failure keeps the app pre-alpha. The
2026-10-07 estimate ("~65 % overall, 110–150 h to 100 %") counted feature presence. This one
measures readiness and adds hardware, localization and field bug reports.

## By dimension

| Dimension | Ready | Remaining (h) | Doc |
|---|---:|---:|---|
| Features | ~45 % | 520–810 | [target-app-parity.md](docs/target-app-parity.md#feature-areas) |
| UI/UX fidelity | ~55 % | 60–100 | [ui-parity.md](docs/ui-parity.md) |
| File formats | ~40 % | 70–120 | [file-format-parity.md](docs/file-format-parity.md) |
| Hardware (interfaces, MIDI, surfaces, sync) | ~12 % | 110–180 | [hardware-parity.md](docs/hardware-parity.md) |
| Localization | ~12 % (1 of Pro Tools' 8 languages) | 70–110 | [localization-parity.md](docs/localization-parity.md) |
| Performance | ~35 % (unmeasured) | 40–70 | [hardware-parity.md](docs/hardware-parity.md#performance) |
| Stability | ~45 % | 50–80 | [gaps.md](docs/gaps.md) |
| Platforms | ~65 % (5 platforms built, macOS exercised) | 20–40 | [gaps.md](docs/gaps.md) |
| Ecosystem / plugins | ~30 % | in Features + Hardware | [plugin-parity.md](docs/plugin-parity.md) |
| AI features | ~5 % | 30–50 | [gaps.md](docs/gaps.md) |
| Agent control | ahead of Pro Tools | 3 | [control-protocol.md](docs/control-protocol.md), [mcp.md](docs/mcp.md) |

## Features

| Area | Breadth | Ready | Remaining (h) |
|---|---:|---:|---:|
| Recording and monitoring | 75 % | 40 % | 40–60 |
| Editing | 92 % | 65 % | 40–60 |
| Mixing and routing | 85 % | 60 % | 35–50 |
| Automation | 80 % | 55 % | 20–30 |
| Built-in plugins and AudioSuite | 40 % | 35 % | 80–120 |
| Third-party plugin hosting | 70 % | 45 % | 40–60 |
| MIDI and instruments | 70 % | 30 % | 50–80 |
| Score and notation | 60 % | 35 % | 20–30 |
| Elastic Audio, TCE, Beat Detective | 70 % | 35 % | 40–60 |
| Surround and immersive | 55 % | 35 % | 50–80 |
| Video and post | 60 % | 40 % | 40–60 |
| Session, file management, interchange | 55 % | 35 % | 60–100 |
| Collaboration and cloud (Avid services) | 0 % | 0 % | owner decision |
| Agent control | 100 % | 100 % | 3 |

Weights and evidence per row: [target-app-parity.md](docs/target-app-parity.md#feature-areas).

## Languages

| Language | Code | Status | Translated |
|---|---|---|---:|
| English | en | full | 100 % |
| Simplified Chinese | zh-Hans | none | 0 % |
| Spanish | es | none (PR #143 open) | 0 % |
| Hindi | hi | none | 0 % |
| Arabic | ar | none | 0 % |
| French | fr | none (PR #143 open) | 0 % |
| Portuguese | pt | none | 0 % |
| Indonesian | id | none | 0 % |
| Japanese | ja | none | 0 % |
| German | de | none | 0 % |
| Korean | ko | none | 0 % |
| Vietnamese | vi | none | 0 % |

Other languages shipped: 0. Detail: [localization-parity.md](docs/localization-parity.md).

## Upcoming

Ranked; the alpha gate, the alpha checklist and the beta plan are in [docs/roadmap.md](docs/roadmap.md).

1. **Alpha blocker:** audio interfaces: device and buffer-size choice, duplex stream, ASIO; record to disk (G-01, G-06). 35–55 h.
2. **Alpha blocker:** MIDI devices: record from keyboards, play instruments live, output (G-02). 20–35 h.
3. **Alpha blocker:** AAF/OMF import and export (G-03). 50–90 h.
4. macOS plugin entitlement (#153) (G-05). 1 h (needed for the mix row); AU editors are hosted (#501).
5. Land the open bug-fix PRs and clear the correctness backlog (G-04). 15–25 h.
6. PR CI running `cargo xtask ci` on three OSes (G-14). 5–10 h.
7. Fidelity harness against Pro Tools (G-07). 20–30 h.

## Robustness

Property tests drive random edit sequences (with full undo), mutated session files and hostile
command parameters through the engine and mixer; they have caught fade, overflow and
unbounded-allocation bugs. Every command also runs with empty parameters on empty and demo
sessions. Hosted plugins live in isolated unsafe crates (`clap-host`, `vst3-host`, `au-host`) and
are created, loaded and destroyed off the audio thread. The video decoders survive hundreds of
mutated and truncated movies. Field reports from the first public week are tracked in
[gaps.md](docs/gaps.md) (G-04).

## Progress log

- **2026-10-16:** Audio Units open their own editor on macOS: the unit's Cocoa view in a host window that follows its size, with edits made in it going into the session like the generic sliders' (#501). Units without a view keep the generic sliders. No readiness percentage change.
- **2026-10-13:** `mix.instrument` takes the built-in instrument's parameters in `params` (validated, clamped) and refuses any other field, so a synth setting written beside `plugin` is an error instead of being accepted and ignored (#265)
- **2026-10-13:** A bounce of a bus that an aux returns onto lines up with the timeline: the aux insert's lookahead is compensated like the main mix's (it left the bus late and cut the end off), and tracks routed to that bus wait for it too (#246)
- **2026-10-13:** Delay compensation covers the inserts of the master faders: a bounce of the main mix with a Maximizer on the master lines up with the same mix without it (it came out 71 samples late) (#351)
- **2026-10-12 (audio device settings):** Setup › Playback Engine chooses the audio system, output and input device, sample rate and buffer size, kept per machine with the preferences; 16/32-bit integer devices are converted; ASIO is available behind the opt-in `asio` build feature (verified with an SSL 2+ at 128 samples). Channel routing, one duplex clock and latency compensation remain (G-01). No readiness percentage change.
- **2026-10-12 (Mix in a separate window):** Window › Mix in Separate Window shows the Mix window in its own native window, so Edit and Mix can be on screen at once (e.g. one per monitor), as in Pro Tools. No readiness percentage change.
- **2026-10-12 (plugin editors on Windows):** VST3 editors and embedded CLAP editors open in a SoundCraft host window on Windows (owned by the app window, resized on request, closed from its title bar); verified with a JUCE VST3 compressor and a CLAP effect. No readiness percentage change.
- **2026-10-12:** `soundcraft-cli convert` rejects an unparsable or zero `--rate` and an unknown `--bit-depth` (exit 1, nothing written) instead of converting with a default (#256)
- **2026-10-12:** Timecode labels with huge fields no longer overflow: frame counts are computed in 128 bits and clamp, so `setup.current_timecode` and `session.format_time` saturate instead of failing (#248)
- **2026-10-12:** `session.format_time` with the smallest bar number no longer fails with an internal error: the bar offset saturates (#249)
- **2026-10-12 (plugin editors on Windows):** VST3 editors and embedded CLAP editors open in a SoundCraft host window on Windows (owned by the app window, resized on request, closed from its title bar); verified with a JUCE VST3 compressor and a CLAP effect. No readiness percentage change.
- **2026-10-12 (Mix in a separate window):** Window › Mix in Separate Window shows the Mix window in its own native window, so Edit and Mix can be on screen at once (e.g. one per monitor), as in Pro Tools. No readiness percentage change.
- **2026-10-12 (audio device settings):** Setup › Playback Engine chooses the audio system, output and input device, sample rate and buffer size, kept per machine with the preferences; 16/32-bit integer devices are converted; ASIO is available behind the opt-in `asio` build feature (verified with an SSL 2+ at 128 samples). Channel routing, one duplex clock and latency compensation remain (G-01). No readiness percentage change.
- **2026-10-10 (UI docking):** Tracks and Groups, Clips, MIDI Editor, Memory Locations, and Undo History use movable panels. Edit and Mix retain separate arrangements; Clips and MIDI Editor belong to Edit. Transport, plug-in, and specialist utility windows retain their existing hosts. Panel moves and reset are available as UI commands, and saved UI preferences retain arrangements. Common toolbar and track record-enable buttons share interaction, accessible labels and keyboard focus through `craft-ui`, retaining blue tool selection and red record-arm states. No readiness percentage change.
- **2026-10-10** — Stage re-normalized to **pre-alpha** by the core-workflow gate (interface recording, MIDI recording and session interchange fail). Full re-measure against Pro Tools Ultimate 2026.4.1 and the standard
  progress-docs set (this page, target-app parity, gaps, roadmap, UI/format/hardware/plugin/
  localization parity). v0.4.0 released. Community fixes: Cut All Automation, snap to neighbours,
  tempo stretch, I/O Setup, clip export names, menu-only dialogs, playback stop on new session.
- **2026-10-09** — Light theme and live system appearance, clip-gain fader and nudge, missing-media
  warnings, session audio health, malformed-JSON and encoder-extension checks in the CLI, macOS
  system menu bar, Windows DX12 default, Linux fontconfig, metronome click in playback.
- **2026-10-08** — v0.2.x–v0.3.0 released (Flatpak, AppImage zsync); VST3 quit crash fixed;
  Ctrl-as-Cmd shortcuts on Windows/Linux; plugin menus by vendor, AU inserts, instrument picker;
  rotating log file; branded DMG.
- **2026-10-07** — Repository published; About window credits; release CI on macOS runners.
- **2026-10-06** — Video track, Audio Unit hosting, plugin state and editors, Renderer; Mix
  window views; score export and print; surround mixing; VST3 hosting; property tests.
- **2026-10-05** — Foundation crates, Edit and Mix windows, playback, CLI/MCP, parity report,
  recording and punch, CLAP hosting, Elastic Audio, Beat Detective, release pipeline.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-13 | trivial | Progress log: mix.instrument params, unknown fields refused (#265) |
| 2026-10-13 | trivial | Progress log: bus bounce compensates aux returns (#246) |
| 2026-10-13 | trivial | Progress log: master insert latency compensated (#351) |
| 2026-10-12 | trivial | Progress log: convert option validation (#256) |
| 2026-10-12 | trivial | Progress log: timecode overflow (#248) |
| 2026-10-12 | trivial | Progress log: bar offset saturates (#249) |
| 2026-10-10 | minor | Readiness-by-audience table with hours: full 950–1,450 h, mainstream 380–600 h, essentials 110–200 h |
| 2026-10-10 | minor | Added mainstream practitioner ~36 % and essentials user ~57 % to the headline numbers |
| 2026-10-10 | minor | Stage alpha → pre-alpha: the core-workflow gate fails on interface recording, MIDI recording and session interchange; alpha distance added |
| 2026-10-10 | major | Rewritten to the progress-docs standard: stage alpha (re-normalized the same day), two numbers (breadth ~70 %, ready ~40 %), dimension/feature/language tables, hours re-calibrated; area table moved to docs/target-app-parity.md, milestones to docs/roadmap.md |
| 2026-10-07 | major | Menu parity 94 %, ~65 % estimated feature parity, alpha list |
| 2026-10-06 | minor | Alpha and parity estimates |
