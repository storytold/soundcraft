# Autosave recovery isolation

Last updated: 2026-10-13.

Each open document uses an atomically reserved recovery directory. Same-named documents and
sources with the same numeric ID therefore do not share recovery media. The desktop writer
uses that exact directory, together with its existing lock, `session.scraft`, origin and
recording-plan files. Unsaved recordings use the same namespace; a unique temporary directory
provides a fallback when no autosave directory is configured.

The document's lifetime is host state. Successful New, Open, Close and Revert operations start
another lifetime; edits, undo/redo, Save As and Save Copy retain it. Failed commands retain it
as well. No session-format migration or serialized recovery identity is introduced.

Current main's background writer, flush behavior and successful-save cleanup are preserved.
A physical reservation can be renewed after cleanup without changing the document's logical
lifetime. Recoverable folders continue to use the current one-level discovery layout. Desktop startup
recovery uses locked `session.scraft` folders. The legacy named-file regression covers the
separate `latest_autosave()` helper.

Available pooled audio is encoded as Float32 WAV before admitting a reused recovery candidate.
A source ID, cached pointer or existing filename alone is insufficient. The candidate must
match the encoded bytes; otherwise a fresh filename stores the available signal. Media is
stored before the recovery session is replaced. Original media is left unchanged. Existing
cleanup and recovery-discard behavior still apply.

Encoding and byte verification run on the background worker and can consume CPU, temporary
memory and disk I/O for large sessions. Unchanged candidates avoid repeated media writes.
This change does not add a retention policy, unavailable-media repair or a new worker queue.
A temporary fallback does not add automatic discovery outside the configured recovery root.

Regression scenarios cover separate same-name documents, reused source IDs, changed cached
WAVs, document transitions and failed commands, legacy helper lookup and unsaved recording paths.
Recovery PCM is checked before cleanup and saved/copy PCM through the existing save/open APIs.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Preserve the current background writer and cleanup while connecting exclusive document directories and verified media reuse to its recovery format. |
| 2026-10-12 | Add per-document recovery directories and exact-byte media reuse. |
