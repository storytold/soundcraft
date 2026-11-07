# Interchange core

Pure Rust Plan → Session constructor, originally developed in vhhr7/soundcraft_helper.
Rational endpoints are converted to samples independently; sources retain complete media
bounds and clips retain source offsets. No processes, UI or filesystem access.

`aaf` reads a strict subset of AAF (structured storage, via the `cfb` crate) into a Plan:
timeline slots of SourceClip/Filler, resolved through MasterMobs to SourceMobs with a
WAVEDescriptor and one external file locator. Gain, fades, transitions, selectors, nested
sequences, embedded essence and split channels fail explicitly. The caller supplies the
opened AAF and a `MediaResolver` that confines, relinks and probes each WAV; the engine's
resolver accepts only files named exactly in the media folder the user chose.

The reader's plans were checked byte-for-byte against the earlier pyaaf2 prototype on the
synthetic fixture, twelve edited variants of it and a 22-track/124-edit real-world AAF.

## Untrusted input and provenance

AAF files are treated as hostile input; the bounds (sector chains, directory size, stream
size, collection counts, source-chain depth) and the locator rules (local paths only, no
`..`, relative locators stay under the AAF's folder, missing media reported per file rather
than aborting) are listed in the `aaf` module documentation and covered by tests in
`crates/engine/tests/aaf_import.rs`.

The reader was written from the published AAF Object Specification and Low-Level Container
Specification. No code from the AAF SDK or from any GPL/LGPL/MPL project was used; the
fixture was generated with pyaaf2 (MIT) as a file writer only.
