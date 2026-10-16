# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-13 · **Change:** trivial (bounces of a bus fed by an aux compensate its latency (#246); previously minor: audio device, rate and buffer choice; ASIO feature; JACK and PipeWire hosts; input monitoring) · **Target:** Avid Pro Tools Ultimate 2026.4.1

A DAW is judged by how it drives audio interfaces, MIDI gear, control surfaces and sync. This is
SoundCraft's weakest dimension. Ours is measured from `crates/playback` (cpal) and the absence of
any MIDI-device or control-surface code; Pro Tools' from its documentation and Setup menus.

**Hardware: ~12 % ready** (estimated). **Performance: ~35 %** (estimated; nothing benchmarked
against Pro Tools). Remaining ≈ 110–180 h for hardware, 40–70 h for performance.

Legend: ✅ works · 🟡 partial · ❌ missing.

## Audio interfaces

| Feature | Pro Tools (macOS / Windows) | SoundCraft macOS | Windows | Linux | FreeBSD | Web |
|---|---|---|---|---|---|---|
| Driver API | Core Audio / ASIO | 🟡 Core Audio via cpal; JACK (untested) | 🟡 WASAPI shared via cpal; ASIO with the opt-in `asio` build feature (verified with an SSL 2+ before the move to cpal 0.18, not since); JACK by default (untested) | 🟡 ALSA via cpal (Pulse/PipeWire through their ALSA plugins); native PipeWire (the automatic choice) and JACK, chosen in Playback Engine (default features; `--no-default-features` builds are ALSA only) | 🟡 cpal (ALSA layer) | 🟡 Web Audio |
| Choose playback device | ✅ Playback Engine | 🟡 Setup › Playback Engine lists the host's output devices (not yet verified on macOS) | 🟡 Setup › Playback Engine (WASAPI, ASIO; verified before the move to cpal 0.18, not since) | 🟡 checked on PipeWire | 🟡 (not yet verified) | ❌ |
| Choose input device | ✅ | 🟡 Setup › Playback Engine lists the host's input devices (#42; not yet verified on macOS) | 🟡 (WASAPI, ASIO; verified before the move to cpal 0.18, not since) | 🟡 checked on PipeWire | ❌ no recording | ❌ |
| Buffer size (32–2048) | ✅ | 🟡 device buffer when the driver allows it (not yet verified); mix in 512-sample blocks | 🟡 device buffer 16–8192 (128 on ASIO and 256 on WASAPI verified before the move to cpal 0.18); mix in 512-sample blocks | 🟡 Playback Engine asks the host for 16–8192 frames and shows what the devices run at; checked on PipeWire (which keeps to its own quantum limits) | 🟡 | ❌ |
| Duplex stream on one clock | ✅ | ❌ separate input/output streams | ❌ | ❌ | ❌ | ❌ |
| Hardware I/O mapping (I/O Setup to physical channels) | ✅ | 🟡 multichannel output in order; no per-channel map | 🟡 | 🟡 | 🟡 | ❌ |
| Low-latency monitoring | ✅ | ❌ (`options.low_latency_monitoring` is a stored flag) | ❌ | ❌ | ❌ | ❌ |
| Latency reporting and record offset compensation | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Sample rate follows the session; resample when it can't | ✅ (switches device) | 🟡 resamples (input pitch bug fixed, #28) | 🟡 | 🟡 | 🟡 | 🟡 |
| Avid HDX / HD Native DSP, Hybrid Engine DSP mode | ✅ | ❌ (out of reach: Avid hardware) | ❌ | ❌ | ❌ | ❌ |
| Disk allocation / record to disk | ✅ | ❌ recording into RAM (PR #84) | ❌ | ❌ | ❌ | ❌ |

## MIDI

| Feature | Pro Tools | SoundCraft (all platforms) |
|---|---|---|
| MIDI input devices (keyboards, pads) | ✅ | 🟡 Linux (ALSA sequencer), macOS (CoreMIDI) and Windows (WinMM) through `midir`: every input port is opened, hot-plugged devices are picked up, ports can be switched off in Setup › Playback Engine; each MIDI/instrument track chooses a device and channel at its input selector. Notes are recorded into MIDI clips with controllers, pitch bend and aftertouch. Checked with a virtual keyboard on Linux only; not on FreeBSD or the web |
| MIDI output to external gear | ✅ | ❌ |
| MIDI Thru, input filter | ✅ | 🟡 record-enabled and input-monitored MIDI/instrument tracks play the keyboard through their instrument (notes and the sustain pedal; other controllers and pitch bend are recorded but not played); the input filter keeps the filtered kinds out of recordings. No thru to external gear |
| MIDI Beat Clock out, MTC generate/chase | ✅ | ❌ |
| MIDI learn / controller mapping | ✅ (via surfaces and plug-in maps) | ❌ |
| Virtual MIDI keyboard | ✅ (MIDI Keyboard window) | 🟡 window exists, no live audition path |

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
| 2026-10-13 | trivial | bounces of a bus fed by an aux compensate its latency (#246) |
| 2026-10-12 | minor | Audio device settings: system, output, input, sample rate and buffer size; opt-in ASIO |
| 2026-10-10 | major | Created: audit of audio interface, MIDI, control-surface, sync and performance support against Pro Tools 2026.4.1 |
| 2026-10-12 | minor | Linux driver API: opt-in native JACK and PipeWire hosts (cpal 0.18), with a host picker in Playback Engine, `--audio-host` and `SOUNDCRAFT_AUDIO_HOST`; output and input device pickers in Playback Engine |
| 2026-10-12 | minor | Opt-in ASIO (Windows) and JACK (Windows, macOS) hosts; streams open in the device's own sample format (i16/i24/i32…), not f32 only |
| 2026-10-12 | minor | JACK and PipeWire are default features of the desktop app; PipeWire is the automatic host on Linux |
| 2026-10-12 | minor | JACK host: Playback Engine lists the interfaces behind the physical JACK ports and wires the streams to the chosen one (cpal alone connects to `system:*`, which under PipeWire's JACK is the desktop's default microphone and speakers) |
| 2026-10-12 | minor | Buffer size choice in Playback Engine (asked of the host, shown as it really runs); input monitoring reads the input through a lock-free buffer that holds about two callbacks and follows clock drift, instead of a locked queue that dropped blocks and could run half a second late; the track input menu lists the device's real channels |
| 2026-10-12 | minor | MIDI input devices (`midir`): live play-through to record-enabled MIDI and instrument tracks, MIDI recording into clips, per-track device and channel, device switches in Playback Engine |
