# Safe Heal Separation

Last reviewed: 2026-10-14. Last updated: 2026-10-14.

`edit.heal` joins compatible ordinary audio clips at selected separation points. It plans the
operation without changing the document, then applies accepted joins as one undoable command.
The result retains `healed` and adds `skipped` entries with track, left/right clip IDs and a reason.

## Compatible pairs

The clips must be adjacent on the active playlist and continue the same source at consecutive
source offsets. Their gain must be equal and finite, mute states must agree, and stretch must
be exactly one. The seam must lie within the inclusive selected range. A point selection checks
the seam around that position using saturating arithmetic.

The left clip keeps its identity and presentation metadata. Selection of a removed right clip
moves to the surviving left clip, with duplicate selections removed. A chain of accepted joins
is one Undo/Redo step. Source audio remains shared and unchanged; Heal performs no file I/O,
encoding, resampling or audio rendering.

## Refusals and unchanged operations

Heal leaves incompatible material in place. It refuses non-audio content, nonpositive lengths,
negative positions or offsets, duplicate clip IDs, arithmetic overflow, noncontiguous source
positions, different sources, gain/mute mismatches, stretch, fades, gain envelopes, locks,
groups, loop identities and recognized per-clip processing or performance references.

Fades are refused even at the outside edges: removing a seam can change the renderer's
interpolation segments. Loop identities are refused so a later Unloop operation keeps its
original meaning. These conditions are reported rather than silently flattened or discarded.

When no pair is accepted, the command does not obtain mutable document access, sort the
playlist, replace its Arc, change its revision/dirty state, add an Undo step or consume pending
Redo. Normal command-journal handling continues; Heal does not introduce a separate journal policy.
In a mixed selection, accepted joins apply while refused pairs remain represented in the report.

## Command

```json
{"command":"edit.heal","params":{"tracks":[1],"start":120,"end":240}}
```

`tracks`, `start` and `end` use the existing edit-command selection defaults. The Edit menu,
keyboard shortcut and programmatic calls dispatch the same command.

Common refusal reasons include `not_contiguous`, `different_source`, `gain_mismatch`,
`mute_mismatch`, `unsupported_content`, `unsupported_stretch`, `unsupported_fade`,
`unsupported_envelope`, `locked`, `grouped`, `looped` and `clip_metadata`. Malformed data may
report `invalid_clip`, `duplicate_id` or `arithmetic_overflow`.

## Verification and scope

Synthetic regression cases cover mono/stereo sample equality, source offsets, gain/mute,
selection remapping, one Undo/Redo step, refusals that preserve pending Redo, malformed IDs,
checked arithmetic, unaligned fades and unchanged unsorted playlists. Independent in-memory
headroom checks compare rendered samples, original PCM bits and shared source identity.
Current loop-identity checks cover either or both sides of a seam.

This contribution changes only ordinary-audio Heal planning and command application. It does
not alter MIDI editing, mix/playback processing, file delivery or session formats.

## Revision history

- 2026-10-14: Document safe ordinary-audio Heal, explicit refusal reasons and unchanged-operation limits.
