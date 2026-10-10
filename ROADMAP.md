# SoundCraft roadmap

SoundCraft is a clean-room, pure-Rust digital audio workstation that aims at full parity with
Avid Pro Tools, and then beyond it: faster, open, scriptable and agent-controllable. This file is
the honest status: what works today, what is missing, and how far we are.

## Where we are (2026-10-07)

| Measure | Value | How |
|---|---|---|
| Menu-catalog parity (engine + UI) | **482 / 512 menu items (94 %)**; engine alone 397 / 512, see [`docs/parity.md`](docs/parity.md) | `cargo xtask parity` compares the incumbent's 512 menu leaves (names observed black-box) with our command registry. Most of what is left is Avid's online services (sign-in, cloud projects, collaboration, Splice, Learn), Dolby Atmos-specific items, HEAT and ARA |
| Estimated overall feature parity | **~65 %** | judgement across the areas below, weighted by how much working engineers rely on them. Menu items are only the surface; depth (editing feel, plugin catalogue, MIDI and score editing, video) is where the gap is |
| Distance to an **alpha** | **~85 % there, ~12–18 wall-clock hours** of Claude Opus 5.5 work | see [Alpha](#alpha) below |
| Distance to **100 % parity** | **~110–150 wall-clock hours** of Claude Opus 5.5 work with a single agent; roughly 40–55 hours with 3–4 agents in parallel (the last three features in this session were built in parallel this way) | sum of the per-area estimates below |

### Area by area

| Area | Status | Parity | Remaining (h) |
|---|---|---:|---:|
| Session model, save/load, undo | Tracks, playlists, clips, fades, clip gain, automation, markers, groups, busses, video; JSON `.scraft` + Audio Files folder; plugin state; autosave; copy-on-write undo with gesture coalescing | 85 % | 8 |
| Audio file I/O | WAV/BWF/RF64, AIFF/AIFC, FLAC (read/write); MP3, OGG, AAC, ALAC, CAF (read); peaks. Whole files are decoded into memory: disk streaming for very long sessions is still to do | 70 % | 12 |
| Editing | Slip/Shuffle/Spot/Grid, cut/copy/paste/clear/duplicate/repeat/shift, Paste Special (merge MIDI, markers, clip gain/effects), separate/heal/trim/consolidate/strip silence, fades & crossfades, nudge, playlists with comping, every tool gesture, edit groups, clip groups (and `.scgrp` import/export) | 75 % | 20 |
| Mixer & routing | Faders, pan, mute/solo (SIP, implicit solo), sends pre/post, busses, aux inputs, master faders, VCAs, routing folders, 10 inserts, plugin delay compensation, clip effects, preamp/instrument/object views | 75 % | 15 |
| Automation | Breakpoint lanes for volume/pan/mute/sends/plugin params, trim automation; live Write/Touch/Latch passes; write/thin/glide/convert/coalesce. Surround-pan automation is still to do | 60 % | 15 |
| Plugins (built-in) | 26 original processors + 3 instruments (subtractive synth, drum synth, sampler), AudioSuite offline processing for the whole AudioSuite menu | 45 % | 30 |
| Third-party plugins | CLAP, VST3 and Audio Units (macOS): audio, parameters, latency, notes, state saved in sessions, instances created/destroyed off the audio thread, editors (floating CLAP GUIs, VST3 views on macOS, verified with Melodyne). Missing: AU editors, embedded CLAP GUIs, VST3 editors on Windows/Linux, ARA | 80 % | 10 |
| MIDI & notation | MIDI/instrument tracks, SMF import/export, MIDI editor (piano roll, velocity, rubber-band select, keyboard transpose/move), event list, step input, quantize/transpose/velocity/duration, real-time properties, Score Editor, MusicXML export (Sibelius) and printable SVG score | 55 % | 20 |
| Recording | Input capture, punch in/out (selection, on the fly, pre/post-roll), loop record into playlists, input monitoring through the channel strip, autosave and recovery | 60 % | 10 |
| Elastic Audio / TCE / Beat Detective | Pitch-preserving warp on Elastic tracks, TCE to timeline, conform to tempo, Beat Detective and Identify Beat windows; audio-to-MIDI pitch detection | 45 % | 15 |
| Surround | Main and bus formats from stereo to 9.1.6 and Ambisonics, surround panner with divergence/centre/LFE/height, ITU fold-down, multichannel bounce, Renderer window, object/bed routing | 60 % | 12 |
| Video | Picture track with thumbnails, Video and Video Universe windows, H.264 (Baseline/Main/High), ProRes (all flavours) and Motion JPEG decoders in pure Rust, sync offset, relinking. Missing: HEVC/AV1/DNx, timecode-track placement, video export | 55 % | 15 |
| UI fidelity | Edit + Mix windows, toolbar, rulers, track headers, menus for the whole catalog (macOS system menu bar), floating windows, dialogs (every file command has a path prompt), command palette | 70 % | 20 |
| Agent control | CLI, JSON control channel, MCP server (headless + bridged), offscreen UI renders | 90 % (ahead of the incumbent) | 3 |
| Release engineering | Signed macOS universal, Windows x64/x86, Linux AppImage/deb/rpm/tar/Flatpak, FreeBSD, Web/WASM on every push to `release` | 80 % | 3 |

## Alpha

An alpha is a build people can download, open the demo or their own audio in, record, edit, mix
and bounce without crashes or lost work. Feature-wise SoundCraft is past that bar; what is left is
shipping and hardening:

1. **Publish the repository and cut the first release** (needs a decision from the maintainers:
   the GitHub repo, the release secrets, and a push to the `release` branch). The pipelines exist
   but have never run end to end. ~2 h once unblocked, mostly fixing what the first run finds.
2. **Platform smoke tests** of the installers on Windows, Linux and FreeBSD (only macOS has been
   exercised by hand). ~3–4 h.
3. **Disk streaming** of long sources, so an hour-long multitrack session does not need all of its
   audio in RAM. ~5–6 h.
4. **A hands-on QA pass** in the live app: recording on real interfaces, long sessions, plugin-heavy
   mixes, save/reopen loops. ~3–4 h.
5. **Docs**: a short user guide next to the existing agent/CLI docs. ~1 h.

## Robustness

Property tests drive random edit sequences (with full undo), mutated session files and hostile
command parameters through the engine and mixer; they have caught fade, overflow and
unbounded-allocation bugs. Every command also runs with empty parameters on empty and demo
sessions. Hosted plugins live in isolated unsafe crates (`clap-host`, `vst3-host`, `au-host`) and
are created, loaded and destroyed off the audio thread. The video decoders survive hundreds of
mutated and truncated movies.

## Current focus

1. The alpha list above.
2. Surround-pan automation; AU editor views.
3. Built-in plugin depth (more processors, better UIs).
4. MIDI and score editing depth.

## Milestones

- **M0–M4 (done):** workspace, gates, model, IO, DSP, mix engine, command engine, Edit/Mix UI, realtime playback, metering, plugins, bounce.
- **M5 (done):** editing depth (tools, modes, playlists, fades, comping, clip groups).
- **M6 (done for the first cut):** recording, punch, loop record, monitoring.
- **M7 (in progress):** MIDI and notation editing.
- **M8 (done):** CLI, control channel, MCP, agent acceptance test.
- **M9 (in progress):** AudioSuite breadth (done), Elastic Audio, Beat Detective.
- **M10 (next):** alpha release on all platforms.
- **M11 (mostly done):** third-party plugin hosting (CLAP, VST3, AU), video, surround.
