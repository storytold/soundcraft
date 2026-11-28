# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-26 · **Change:** minor (MIDI Thru to external gear: a keyboard plays through to the tracks' MIDI outputs; previously minor: MIDI output to external gear: MIDI and instrument tracks play to a port and channel; previously minor: record to disk: done, #84 and #888, previously trivial: bounces of a bus fed by an aux compensate its latency, #246; previously minor: MIDI input devices, live play-through with controllers, and MIDI recording) · **Target:** Avid Pro Tools Ultimate 2026.4.1

A DAW is judged by how it drives audio interfaces, MIDI gear, control surfaces and sync. This is
SoundCraft's weakest dimension. Ours is measured from `crates/playback` (cpal) and the absence of
any MIDI-device or control-surface code; Pro Tools' from its documentation and Setup menus.

**Hardware: ~12 % ready** (estimated). **Performance: ~35 %** (estimated; nothing benchmarked
against Pro Tools). Remaining ≈ 110–180 h for hardware, 40–70 h for performance.

Legend: ✅ works · 🟡 partial · ❌ missing.

## Audio interfaces

| Feature | Pro Tools (macOS / Windows) | SoundCraft macOS | Windows | Linux | FreeBSD | Web |
|---|---|---|---|---|---|---|
| Driver API | Core Audio / ASIO | 🟡 Core Audio via cpal | 🟡 WASAPI shared via cpal; ASIO with the opt-in `asio` build feature (verified with an SSL 2+) | 🟡 ALSA/Pulse via cpal; no JACK | 🟡 cpal (ALSA layer) | 🟡 Web Audio |
| Choose playback device | ✅ Playback Engine | 🟡 Setup › Playback Engine (not yet verified on macOS) | ✅ Setup › Playback Engine (WASAPI, ASIO) | 🟡 (not yet verified) | 🟡 (not yet verified) | ❌ |
| Choose input device | ✅ | 🟡 Setup › Playback Engine (not yet verified on macOS) | ✅ (WASAPI, ASIO) | 🟡 (not yet verified) | ❌ no recording | ❌ |
| Buffer size (32–2048) | ✅ | 🟡 device buffer when the driver allows it (not yet verified); mix in 512-sample blocks | 🟡 device buffer 16–8192 (verified 128 on ASIO, 256 on WASAPI); mix in 512-sample blocks | 🟡 (not yet verified) | 🟡 | ❌ |
| Duplex stream on one clock | ✅ | ❌ separate input/output streams | ❌ | ❌ | ❌ | ❌ |
| Hardware I/O mapping (I/O Setup to physical channels) | ✅ | 🟡 multichannel output in order; no per-channel map | 🟡 | 🟡 | 🟡 | ❌ |
| Low-latency monitoring | ✅ | ❌ (`options.low_latency_monitoring` is a stored flag) | ❌ | ❌ | ❌ | ❌ |
| Latency reporting and record offset compensation | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Sample rate follows the session; resample when it can't | ✅ (switches device) | 🟡 resamples (input pitch bug fixed, #28) | 🟡 | 🟡 | 🟡 | 🟡 |
| Avid HDX / HD Native DSP, Hybrid Engine DSP mode | ✅ | ❌ (out of reach: Avid hardware) | ❌ | ❌ | ❌ | ❌ |
| Disk allocation / record to disk | ✅ | ✅ takes written to disk as they record (#84); a long take goes on in a new file (#888) | 🟡 same recorder (not yet verified on Windows) | 🟡 same recorder (input not yet verified) | ❌ no recording | ❌ |

## MIDI

| Feature | Pro Tools | SoundCraft (all platforms) |
|---|---|---|
| MIDI input devices (keyboards, pads) | ✅ | 🟡 Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM) through `midir`: every input port is opened, hot-plugged devices are picked up, ports can be switched off in Setup › Playback Engine; each MIDI/instrument track chooses a device and channel at its input selector. Notes are recorded into MIDI clips with controllers, pitch bend and aftertouch. On Linux, live play was checked with a hardware keyboard (Akai MPK mini 3) and recording with a virtual one; not on macOS or Windows, and there is none on FreeBSD or the web |
| MIDI output to external gear | ✅ | 🟡 MIDI and instrument tracks play their clips to one port and channel each ([midi-output.md](midi-output.md)): track offsets, delay compensation, controller and program chase on start and locate, Note-Offs then All Notes Off on Stop, mute and solo on MIDI tracks, All MIDI Notes Off. Played on macOS (USB MIDI interface to a hardware synth, IAC bus); not tried on Linux or Windows. No note chasing, default program per track, multiple destinations, or MIDI mute on instrument tracks |
| MIDI Thru, input filter | ✅ | 🟡 record-enabled and input-monitored MIDI/instrument tracks play the keyboard through their instrument and to their MIDI output port and channel, at once and without offsets ([midi-output.md](midi-output.md) section 5; notes, the sustain pedal, controllers, pitch bend, aftertouch and program changes); Options › MIDI Thru switches it, on by default; held notes end on disarm, routing changes, Stop and All MIDI Notes Off; echoes from a MIDI loop are dropped. Measured on macOS (IAC bus to a virtual port), not with hardware, Linux or Windows. No system exclusive or Real-Time Properties through thru; the input filter keeps the filtered kinds out of recordings |
| MIDI Beat Clock out, MTC generate/chase | ✅ | ❌ |
| MIDI learn / controller mapping | ✅ (via surfaces and plug-in maps) | ❌ |
| Virtual MIDI keyboard | ✅ (MIDI Keyboard window) | 🟡 Window › MIDI Keyboard (Shift+K): clicked keys play and thru on record-enabled and input-monitored tracks whose input is All devices, and record on the record-enabled ones, or step-enter into the selected clip when no MIDI track is record-enabled; no computer-keyboard keys, octave or velocity keys |

## Control surfaces

| Feature | Pro Tools | SoundCraft |
|---|---|---|
| EUCON (S1/S3/S4/S6, Dock, Control app) | ✅ | ❌ Avid-proprietary SDK; owner decision |
| HUI | ✅ | ❌ |
| Mackie Control Universal | ✅ (via HUI emulation and personalities) | ❌ |
| Generic MIDI controller personalities | ✅ | ❌ |

## Sync and video hardware

| Feature | Pro Tools | SoundCraft |
|---|---|---|
| LTC / MTC chase, 9-pin machine control | ✅ (Ultimate) | ❌ |
| Word clock / video reference awareness | ✅ via interface | ❌ |
| Video output to a second display or video card | ✅ | ❌ (video window only) |
| Satellite linking of systems | ✅ | ❌ |

## Performance

| Feature | Pro Tools | SoundCraft | Evidence |
|---|---|---|---|
| Parallel mixing across cores | ✅ | ✅ | `soundcraft-mix` renders independent strips in parallel (rayon) |
| Plugin delay compensation | ✅ | ✅ | Bounces of a bus that an aux returns onto are compensated like the main mix (#246). |
| Disk streaming of long sources | ✅ | ❌ | Whole files decoded into the `SourcePool` |
| Track count (Ultimate: 2,048 voices) | ✅ | unmeasured | No benchmark exists; add `cargo xtask bench` |
| Real-time audio thread discipline | ✅ | ✅ | No blocking/allocation in steady state, audio-thread-aware logger |
| Freeze / commit to save CPU | ✅ | 🟡 | `track.commit` renders; `track.freeze` only sets a flag (no CPU saved) |
| GPU-accelerated UI | — | ✅ (wgpu/glow via eframe) | llvmpipe X11 issue #107 |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-26 | minor | MIDI output to external gear: MIDI and instrument tracks play their clips to one port and channel each, with offsets, delay compensation, chase, Stop releases, MIDI-track mute and solo, and All MIDI Notes Off; played on macOS; guide in `midi-output.md` |
| 2026-11-18 | minor | MIDI input devices (`midir`): live play-through to record-enabled MIDI and instrument tracks (notes, pedal, controllers, pitch bend, aftertouch), MIDI recording into clips, per-track device and channel, device switches in Playback Engine; guide in `midi-input.md` |
| 2026-10-30 | minor | Record to disk: takes are written to disk as they record (#84) and long takes continue in a new file (#888) |
| 2026-10-13 | trivial | bounces of a bus fed by an aux compensate its latency (#246) |
| 2026-10-12 | minor | Audio device settings: system, output, input, sample rate and buffer size; opt-in ASIO |
| 2026-10-10 | major | Created: audit of audio interface, MIDI, control-surface, sync and performance support against Pro Tools 2026.4.1 |
