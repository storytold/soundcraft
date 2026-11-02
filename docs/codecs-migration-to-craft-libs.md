# SoundCraft Audio I/O Migration to craft-libs

> **Tracking Issue:** [storytold/soundcraft#1274](https://github.com/storytold/soundcraft/issues/1274)  
> **Upstream PR:** [storytold/craft-libs#504](https://github.com/storytold/craft-libs/pull/504)  
> **Author:** `el-j`  

## Overview
Pure-Rust audio reader/writer infrastructure originally hosted in SoundCraft (`soundcraft-audio-io`) is now centralized inside `craft-libs` as the canonical One True Source for all ArtCraft applications.

### Canonical Locations in `craft-libs`
- `crates/audio-io` (`craft-audio-io`): Multi-format audio decoding/encoding (WAV, AIFF, FLAC, MP3, AAC, OGG Vorbis) and ITU-R BS.1770 integrated loudness measurement
- `crates/aac` (`craft-aac`): AAC-LC CBR/VBR audio encoder & decoder
- `crates/codecs` (`craft-codecs`): Umbrella crate re-exporting all video and audio encoders

This centralization enables `soundcraft`, `filmcraft`, and `encodercraft` to share identical, SIMD-accelerated, zero-panic audio decoding, encoding, and loudness measurement without cross-application dependencies.
