# Audio-complete session copies

Last reviewed: 2026-10-13. Last updated: 2026-10-13.

`session.save_copy` packages every declared audio source into a new SoundCraft session without
changing the open document, its saved path, dirty state, selection, Undo or Redo. Copy and its
preview are excluded from the recovery journal. Ordinary Save, Save As and autosave remain able
to preserve documents with missing audio.

Missing decoded audio blocks Copy before creating output directories or files. Restore or relink
the audio first. The copy includes alternate playlists and unused source rows retained in the
document. Audio clip references, mixer insert samples and instrument samples must resolve to
declared, available sources. Duplicate IDs, mismatched decoded dimensions, ragged channels,
unsupported rates and oversized metadata fail preflight. Same names and original paths are
allowed; copied media is distinguished by its actual content.

## Commands

```json
{"command":"session.save_copy_preview","params":{"path":"/sessions/Copy/Copy.scraft"}}
{"command":"session.save_copy","params":{"path":"/sessions/Copy/Copy.scraft","expected_revision":12}}
```

Preview accepts optional `path` and `expected_revision`; Copy requires `path` and additionally
accepts boolean `replace` (default false). Parameters must be an object with correctly typed,
known keys. A stale `expected_revision` fails before output. `replace: true` permits replacing
another session file; it does not permit replacing the open session's own file or its canonical aliases. Native replacement requirements take precedence
over optional-key validation; unsupported options still fail before any copy is written.

Preview returns `ready`, audio ready/missing/invalid counts, issues, and media rows. Successful
Copy retains `path` and `audio_files_written` and adds the effective `output_path`,
`audio_sources_packaged`, `audio_complete: true`, and `media`. Native audio rows say `written`
or `reused`; browser audio rows say `packaged`. Each gives its actual relative output path.
Video rows always say `external_unverified` and retain their external reference. All reports
state `full_session_portability: "not_assessed"`: plugin files and other external assets have
not been inventoried, even when there is no video.

## Native and browser behavior

Native destination inspection, media storage and session publication use the guarded I/O
layer. Copying loaded audio requires destination write authority without rereading original
audio files. Existing encoded-byte reuse and session/backup content reads require read authority;
without it, Copy writes fresh media under unused names instead of reusing existing bytes.
Own-file identity checks use authorized metadata. If the open file is outside both granted
roots, a fresh destination is allowed, but replacing an existing destination is blocked because
its identity cannot safely be compared with the open file.

Native Copy writes Float32 WAV at the current session rate, with byte-validated reuse of existing
media. It creates collision suffixes instead of replacing another session's audio. Symlink
session destinations, an Audio Files symlink directory, and encountered media symlinks are
rejected. With read authority, preflight also checks existing media-folder entries for symlinks. Session publication tries at most 100 unique staging names, creating its file exclusively,
writing and syncing that open file before renaming it to the authorized destination. Existing staging
paths are never opened for writing; an old fixed `.scraft.tmp` file or link is left untouched.
On a publication failure, best-effort cleanup removes staging only when its filesystem identity still matches
the file created by Copy; platforms without that identity check retain the error-stage artifact.
Unix hard-link aliases of the open session are also rejected.
A later encoding or disk I/O failure may leave additional audio without a committed session; a
whole-folder transaction is not implemented. Concurrent hostile filesystem replacement is not
covered by these preflight checks.

Browser Copy builds one `.scraftz` download through the existing delivery hook. The complete
bundle is validated before delivery; building it never changes the open document. Successful
delivery means the browser hook accepted the bytes, not that the download was retained. The
reader's 1 GiB uncompressed-data and 10,000-entry limits constrain Copy. Both routes allow at
most 9,999 declared audio sources and 9,999 external video references, source name/path fields
of at most 4,096 bytes, and at most 16 MiB of bounded serialized metadata. Source/buffer/session
rates must agree within 8,000–768,000 Hz. Float32 WAV channel/block/byte-rate fields and sample
counts are checked before output.

The dedicated Copy dialog shows the audio policy, blocked sources and external-video limit.
Edits preserve its typed destination but make the preview stale until Refresh Preview is
pressed. Successful New/Open/Close/Revert dismisses only the Copy dialog. A failed open leaves
it available. Escape cancels without copying, including when Enter is pressed in the same frame.
Available live plugin state is captured only into a detached export document; preview never
captures state or opens plugins.

## Verification and remaining scope

Synthetic tests move the copied folder, remove original media, reopen all declared sources, and
compare decoded audio and playlists. Tests also cover incomplete/malformed sessions, altered
same-size destination media, missing-media ordinary Save/recovery, Undo/Redo/journal preservation,
native destination aliases, strict bundles, delivery failure, and the actual dialog/UI dispatcher.

This feature verifies decoded-audio completeness. Output rate/depth/type conversion,
selected-time export and timing remap, exact original-file bytes, BWF metadata, video copying,
external plugin-file inventory and whole-copy rollback remain separate work.

The existing encoder clamps above-unity Float32 samples. Fixtures use finite samples within
unity and verify nonfinite sanitization without changing the original decoded audio.

## Revision history

- 2026-10-13: Rebase the focused Save Copy contribution on current main; document its
  audio-completeness contract and explicit limits.
