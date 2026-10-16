# Relinking missing audio

> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13 · **Change:** minor (current loader and autosaver integration)

The native app's **File > Session Audio Health** report offers **Relink Audio…** for clips whose
source metadata exists but whose audio is not loaded. Select the original file with Browse or
enter its path. A successful relink restores every clip and alternate take using that source;
clip offsets, timeline positions, lengths, source channel count, sample rate and BWF time reference
remain unchanged. Undo and Redo restore the previous media/path state.
Audio Health refreshes after relink, Undo or Redo. Successfully creating, opening, closing or
reverting a session discards an open relink dialog and its consent; failed commands and saves do not.

Newly imported and saved sources carry an optional `pcm-sha256-v1:` identity. It is SHA-256 over
exact decoded f32 sample bits in planar channel order, using explicit little-endian dimensions,
lengths and sample bits. Imported media is compared at the source/session rate, using the same
offline sample-rate conversion as import. Matching filenames, sizes or durations are insufficient.
A known content mismatch cannot be accepted by the legacy confirmation option.

Older session JSON may omit `audio_identity`. These sources still load normally when their
metadata matches. Manual relinking requires an explicit **I have checked that this is the
original audio** confirmation: matching metadata cannot establish content identity for a legacy
source. Channel count, frame count and converted sample rate must still match. A successful
confirmed relink adds an identity for subsequent saves and verification.

The commands `media.relink` and its alias `source.relink` accept:

```json
{"source": 12, "path": "/absolute/path/original.wav", "allow_unverified": false}
```

They never open a dialog. An error leaves the document and undo history unchanged. Success
returns `source`, canonical absolute `path`, `identity_verified`, and `legacy_confirmation`.
Relinking rejects loaded sources, unknown source IDs, unsupported identity versions, nonregular
files, files over 1 GiB, empty/corrupt audio, and excessive sample-rate conversion allocations.
Configured automation read roots also apply to the selected path. Filesystem-path relinking is
native only; the web command returns an explicit unsupported error.

Save and bundle identities describe the actual encoded-and-decoded saved audio. The current
Float32 WAV encoder clamps samples to [-1, 1] and maps NaN to zero; infinities become the
corresponding endpoint. This feature preserves that codec behavior. PCM identity does not
establish raw file-byte identity or lossless out-of-range saves. Decoder or sample-rate conversion
changes can cause conservative verification failures for an original file; a known mismatch
still cannot be overridden. Session opening checks a stored identity and reports changed audio
as unavailable rather than silently loading it.

The background autosaver caches a numbered private audio copy and its encoded PCM identity for
each source's audio buffer. Reusing that buffer for the same source reuses the same identity. Relinking to a confirmed
existing file references that file and removes an obsolete private recovery copy through the
existing cleanup path. The older `io::save_copy` helper compares encoded bytes before reusing a
source-ID filename and chooses a free filename when bytes differ. Encoding, decoding and hashing
run outside the audio callback and add save/import work; large-file performance needs separate
measurement.

This implements user-selected audio relinking. It does not search folders automatically, process
a batch, repair missing source metadata, relink video/AAF/OMF, or recover an identity absent from
the session metadata.
The implementation uses SoundCraft's source/clip model and existing audio decoder and conversion
paths.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-13 | minor | Rebased on the current loader and background autosaver; documented current Float32 WAV behavior |
| 2026-10-12 | major | Created: user-selected missing-audio validation, legacy consent, identity persistence and document-lifecycle behavior |
