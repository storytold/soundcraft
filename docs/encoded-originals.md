# Encoded originals for native audio imports

Updated: 2026-10-15

Native audio import retains the exact supplied file bytes separately from the decoded working audio. Edits, playback, rate conversion, clip placement and derived Float32 WAV files continue to use the existing audio path. The retained copy includes opaque container chunks and metadata that the decoder or working-audio encoder may omit.

## Storage and lifetime

An accepted import captures bytes in an exclusively created, private temporary directory. A versioned entry in the session's optional `originals` map records the source ID, SHA-256 checksum, byte count and generated relative path. Old sessions and generated audio can omit this map.

On Unix, temporary directories are created with mode `0700` and staged originals with mode `0600`, before any bytes are copied. A permissive process umask does not expose the staged file to group or other users. This private-creation rule applies to temporary originals; native session publication retains its existing file modes.

Save, Save As and Save Copy publish retained bytes under `Original Files/<sha256>.asset` in the native session folder. Open verifies the complete checksum before attaching an original to its source. Save Copy preserves the current document path, history and dirty state. It uses the same original bytes as the open document rather than rereading the external import path.

Document snapshots, Undo and Redo share leases on temporary originals. Cleanup waits for the last lease to expire, including snapshots held outside the engine. Releasing a lease only signals a bounded collector channel; filesystem cleanup runs on the control or collector thread. The collector removes only temporary files whose identities match its ownership records and attempts to remove its own empty directory. It leaves external imports, published project assets and replacements at previously owned paths alone. An orderly engine close also collects expired assets. Abrupt process termination can leave temporary files behind.

## Verification and failures

Capture and copy boundaries hash the complete bytes. An ordinary Save to the same published path checks the authenticated lease, file identity, length and modification/creation stamps without rehashing unchanged contents. Filesystems without a modification stamp fall back to hashing. The metadata cache assumes accurate filesystem stamps; it does not promise detection of concurrent changes or changes that deliberately preserve all checked metadata. Open and copy boundaries still verify the checksum.

Receipts accept only the supported version and exact generated relative path. Symlinked original files or sidecar directories are refused. Permanent paths remain subject to the existing automation read/write grants; authenticated private staging does not broaden command access to other files.

Missing, malformed or corrupt originals appear in Open's existing missing-media diagnostics while available decoded audio remains usable. Their receipts remain visible. Save and Save Copy refuse to silently publish a session missing a required original. Existing destination assets are verified before reuse and are never overwritten by the originals store.

Staging is bounded to 10,000 live assets and 8 GiB of retained bytes per scope; session publication applies the same limits. Save retains the existing per-file error behavior: a later failure writing decoded media or the session document may leave copied original files in the destination. The operation is not a transaction across all project files. If file identity cannot be established after an exclusive create, cleanup conservatively leaves that unverified path alone.

## Current scope

Retention applies to accepted native audio imports. Recording, generated audio and browser imports continue through their existing working-audio behavior. Browser byte/bundle APIs explicitly reject sessions containing original receipts, and native bundle export does the same. Native folder sessions are the supported preservation path; this prevents an incomplete bundle from silently dropping originals. Codec behavior, mixer/playback scheduling and UI layouts are outside this change.

## Regression coverage

Normal engine-package tests use an independently constructed PCM16 WAVE with BWF metadata, an odd-sized unknown chunk and an opaque chunk after the audio data. They assert both exact retained bytes and the expected decoded samples. Additional cases cover external-file changes, native save/open/copy, Undo/Redo and snapshot cleanup, corrupted/missing originals, path and symlink refusal, confined automation, legacy JSON and explicit bundle refusal. Module tests verify repeated Save avoids full hash reads and that cleanup or Save does not overwrite replacement files. An isolated Unix child with umask `022` checks the ordinary-mode control, private permissions before and during writes, exclusive creation, actual captured bytes/checksum and lease-controlled cleanup.

Revision history: 2026-10-14 — initial native encoded-original retention design and boundaries.

2026-10-15 — create temporary Unix originals with private permissions before writing; add isolated creation and lifecycle regression coverage.
