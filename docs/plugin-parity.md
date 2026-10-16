# Plugin parity: built-in processors and plugin hosting

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-16 · **Change:** minor (Editors: AU Cocoa views on macOS (#501)) · **Target:** Avid Pro Tools Ultimate 2026.4.1

Pro Tools users live in plugins: the bundled set covers everyday mixing, and third-party plugins
cover the rest. This doc tracks both. Our plugins have our own names and are written from
scratch (clean-room rule): Avid plugin names are not used here, only categories.

**Built-in plugins and AudioSuite: breadth ~40 %, ready ~35 %** (80–120 h).
**Third-party hosting: breadth ~70 %, ready ~45 %** (40–60 h). Both estimated.

## Built-in processors (measured from `crates/dsp/src/plugins`)

| Category | Pro Tools bundles (approx., by category) | SoundCraft (ids) | Gap |
|---|---|---|---|
| EQ | 1-band and 7-band EQ, channel strip, vintage EQ models | `eq_1band`, `eq_7band`, `channel_strip` | Vintage models, linear phase, dynamic EQ; parametric UI (#139) |
| Dynamics | compressor/limiter, expander/gate, de-esser, maximizer, vintage compressors, multiband | `compressor`, `expander_gate`, `de_esser`, `maximizer` | Vintage/opto models, multiband, sidechain input (no key input anywhere) |
| Reverb | algorithmic room/plate/hall; convolution (paid tier) | `room_reverb`, `plate_reverb` | Hall/chamber, convolution, early-reflection editor |
| Delay | modulated delay, tape/analog delays | `mod_delay` | Tape, ping-pong, multi-tap, tempo-sync depth |
| Modulation | chorus, flanger, phaser, tremolo, ring/frequency shift | `chorus`, `flanger`, `phaser` | Tremolo, auto-pan, ring modulation |
| Harmonic | lo-fi, rectifier, amp simulation, saturation, tape | `lofi`, `rectifier`, `saturator` | Amp and cabinet models, tape |
| Pitch and time | pitch shift, varispeed effects, time shift | `pitch_shifter`, `time_shift` | Formant control, varispeed effect, pitch correction |
| Utility | trim, gain, invert, DC offset, signal generator, dither, time adjuster | `trim`, `gain`, `invert`, `dc_offset_removal`, `signal_generator`, `dither` | Time adjuster, meter/analyzer plugins, noise shaping options |
| Instruments | several virtual instruments (multi-sampler, piano, drum machine, organ, synths, sample player; Massive X Player added in 2026.4) | `subtractive_synth`, `drum_synth`; `mix.instrument` takes the instrument's parameters in `params` and refuses unknown fields (#265) | Sampler (PR #108 open), piano, organ, wavetable, content library |
| **Total** | **≈ 80 processors and instruments** (estimate from Avid's bundle documentation) | **26 processors + 2 instruments** | |

AudioSuite: every catalog AudioSuite leaf maps to one of our processes (`AUDIOSUITE` in
`crates/ui-egui/src/menus.rs`), so the AudioSuite menu is 100 % present. Depth: no preview,
handles, "clip by clip / entire selection" and multi-input modes beyond the basics; hosted
plugins can't be used offline as AudioSuite yet.

Fidelity: **unmeasured.** Nothing compares a SoundCraft processor's frequency response, gain
reduction curve or reverb tail with the Pro Tools equivalent (G-07 in [`gaps.md`](gaps.md)).
User report #152: a saved instrument level does not reach the bounce.

## Third-party plugin hosting

| Feature | Pro Tools | SoundCraft | Notes |
|---|---|---|---|
| Plugin format | AAX only (Native and DSP) | CLAP, VST3, Audio Units (macOS) | Users' libraries mostly ship VST3/AU too. AAX (#109) needs Avid's SDK licence and PACE signing: owner decision, likely never |
| VST2, LV2, LADSPA | — | ❌ (#30) | Beyond Pro Tools; VST2 SDK is no longer licensable |
| Scan, vendor menus, instrument picker | ✅ | ✅ | #23 |
| Audio, parameters, automation, latency (PDC) | ✅ | ✅ | |
| Notes / MIDI to instruments | ✅ | ✅ | But no live MIDI input (G-02) |
| State saved in sessions, presets | ✅ | ✅ | |
| Create/destroy off the audio thread | ✅ | ✅ | |
| Editors: CLAP | — | 🟡 floating windows; embedded editors on Windows | Embedded on macOS/Linux missing |
| Editors: VST3 | — | 🟡 macOS (verified with a commercial pitch editor) and Windows (verified with a free JUCE compressor) | Linux views missing |
| Editors: AU | — | 🟡 Cocoa views (AUv2) in a host window, verified with the Apple units that have one | AUv3 view controllers not hosted (AUv3 extensions are not loaded yet) |
| Multiple editor pages | ✅ | 🟡 | #6 |
| macOS hardened-runtime entitlement for third-party code | ✅ | ❌ | #153, #149 — signed builds refuse third-party VST3 |
| Sidechain / key input | ✅ | ❌ | All three hosts |
| ARA 2 (pitch/time editors in the timeline) | ✅ | ❌ | Menu leaves Track/Clip/Window ▸ ARA still missing |
| Crash isolation | in-process (DSP on HDX) | in-process | Out-of-process hosting would beat Pro Tools |
| Plugin delay reporting changes on the fly | ✅ | 🟡 | Aux insert latency bug #103 (PR #133) |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-16 | minor | Editors: AU Cocoa views open in a host window on macOS (#501) |
| 2026-10-13 | trivial | mix.instrument sets built-in instrument parameters in params (#265) |
| 2026-10-12 | minor | Editors: VST3 and embedded CLAP editors on Windows |
| 2026-10-10 | major | Created: built-in processor inventory by category and third-party hosting checklist |
