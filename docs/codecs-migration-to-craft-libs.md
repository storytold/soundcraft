# SoundCraft Audio I/O Migration to craft-libs

> **Tracking Issue:** [storytold/soundcraft#1274](https://github.com/storytold/soundcraft/issues/1274)  
> **Upstream PR:** [storytold/craft-libs#504](https://github.com/storytold/craft-libs/pull/504)  
> **Author:** `el-j`  

## Overview
Pure-Rust audio reader/writer infrastructure originally hosted in SoundCraft (`soundcraft-audio-io`) is undergoing centralization inside `craft-libs` ([storytold/craft-libs#504](https://github.com/storytold/craft-libs/pull/504)) as the canonical One True Source for all ArtCraft applications.

### Canonical Locations in `craft-libs`
- `crates/audio-io` (`craft-audio-io`): Universal multi-format audio decoding (WAV, AIFF, FLAC, MP3, AAC, OGG Vorbis, CAF) and lossless audio encoding (WAV, AIFF, FLAC)
- `crates/aac` (`craft-aac`): AAC-LC CBR/VBR audio encoder & decoder
- `crates/codecs` (`craft-codecs`): Umbrella crate re-exporting all video and audio encoders

Once SoundCraft adopts these upstream `craft-libs` crates, `soundcraft`, `filmcraft`, and `encodercraft` will share identical, SIMD-accelerated, zero-panic audio decoding and encoding without cross-application dependencies. Local crates within SoundCraft will be deprecated or re-export the `craft-*` implementations.
