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
