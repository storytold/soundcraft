# SoundCraft Audio I/O Migration to craft-libs

> **Tracking Issue:** [storytold/soundcraft#1274](https://github.com/storytold/soundcraft/issues/1274)  
> **Upstream PR Series:** [storytold/craft-libs#439-#448](https://github.com/storytold/craft-libs/pull/439) (craft-audio & codecs foundation)  
> **Author:** `el-j`  

## Overview
Pure-Rust audio reader/writer infrastructure and codecs originally hosted in SoundCraft are undergoing centralization inside `craft-libs` through the dedicated `craft-audio` and `craft-media` PR series ([#439-#448](https://github.com/storytold/craft-libs/pull/439)) as the canonical One True Source for all ArtCraft applications.

### Canonical Locations in `craft-libs`
- `crates/audio` (`craft-audio`): Core audio types, ring buffers, sample conversion ([#441](https://github.com/storytold/craft-libs/pull/441))
- `crates/audio/src/flac` (`craft-audio::flac`): Lossless FLAC encoder and decoder ([#442](https://github.com/storytold/craft-libs/pull/442))
- `crates/audio/src/aac` (`craft-audio::aac`): AAC encoder & decoder ([#443](https://github.com/storytold/craft-libs/pull/443), [#444](https://github.com/storytold/craft-libs/pull/444))
- `crates/audio/src/opus` (`craft-audio::opus`): Opus encoder & decoder ([#445](https://github.com/storytold/craft-libs/pull/445), [#446](https://github.com/storytold/craft-libs/pull/446))
- `crates/audio/src/ac3` (`craft-audio::ac3`): AC-3 / E-AC-3 Dolby Digital decoders ([#447](https://github.com/storytold/craft-libs/pull/447))
- `crates/audio/src/mp12` (`craft-audio::mp12`): MP1 / MP2 decoders ([#448](https://github.com/storytold/craft-libs/pull/448))
- `crates/media` (`craft-media`): Ogg container demuxer ([#440](https://github.com/storytold/craft-libs/pull/440))

Once SoundCraft adopts these upstream `craft-libs` crates, `soundcraft`, `filmcraft`, and `encodercraft` will share identical, SIMD-accelerated, zero-panic audio decoding and encoding without cross-application dependencies. Local crates within SoundCraft will be deprecated or re-export the `craft-*` implementations.
