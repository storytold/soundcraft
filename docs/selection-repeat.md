# Selection Duplicate and Repeat

Last reviewed: 2026-10-14. Last updated: 2026-10-14.

`edit.duplicate`, `edit.repeat` and `edit.duplicate_extend` capture an arrangement selection
once and prepare a bounded immutable plan before changing the document. Each repetition uses
the original material and automation; it never reads a copy placed by an earlier iteration.
The clipboard is not used as temporary repeat state.

## Placement and selection

Targets are audio, MIDI or instrument tracks in supported arrangement views, using their active
playlists. Track targets are deduplicated. Repeat accepts an integer count from 1 through 999;
integral JSON floats are accepted for compatibility with the existing dialog. Invalid types,
fractions and out-of-range counts are rejected before document mutation.

In Slip, the entire destination time range is replaced, including copied silence. Existing
destination material cannot remain inside the selection's leading, interior or trailing gaps.
Outside material is retained when it can be represented safely.

In Shuffle, the original selection end is one insertion point. Later material on selected tracks
moves once by the aggregate inserted duration, up to the first locked barrier. That barrier and
all later material stay fixed. The complete insertion must fit before the barrier, and an insertion
point inside a crossfade is refused. A crossing plain-audio clip is split when safe. Unselected
tracks and alternate playlists are preserved.

Duplicate selects the copied range and its new clip IDs. Duplicate and Extend includes the
original range and the copy, retaining surviving selected IDs and adding copied IDs. Repeat
preserves the previous edit selection and surviving selected IDs. Linked timeline selection
follows the resulting edit selection. A successful operation is one Undo/Redo step.

## Automation and clip identity

With Automation Follows Edit enabled, each nonempty automation lane is captured from its original
curve. Copies include the start and final included sample values and interior breakpoints.
Boundary anchors preserve surrounding curves; Mute and SendMute retain stepped interpolation.
Shuffle also moves later automation points before the locked barrier. Original points at or after
the barrier stay fixed, with an anchor preserving the original curve from that boundary onward.
A preceding point that would move to or beyond the barrier is refused before mutation. Empty lanes
stay empty, and lane enabled states are retained. With the option disabled, automation lane objects
remain unchanged in either edit mode.

Whole valid clips preserve their stored audio/MIDI content, gain, fades, envelopes, stretch and
presentation metadata. Copies receive fresh IDs and recognized per-clip effect, elastic,
pitch/tempo/length metadata and original MIDI performances. Metadata matches complete IDs so a
similarly named ID or another playlist is not mistaken for the copied clip. This preserves stored
settings rather than a running effect's internal state. New IDs are reserved above known document
identities within the plan, without changing the live session during preflight. A cursor overlapping
existing clip, per-clip processing or live loop identities is refused.

Partial material is limited to safe plain-audio fragments. Processed partial clips, partial MIDI,
locked or grouped material that would be copied, split or moved, and touched loop identities are
refused atomically. Loop identities are not copied or split because that would change the meaning
of a later Unloop operation. Invalid timing, processing values, identities and unsupported
destination fragments are also rejected.

A refused operation preserves the document Arc, revision, dirty state, Undo/Redo, journal and
clipboard. General Cut, Paste, Clear, note-only editing and global song-time insertion use their
existing paths.

## Musical placement

`timebase: "samples"` explicitly selects exact sample spacing; `timebase: "ticks"` requests
musical placement. With no explicit timebase, a Bars|Beats main counter or explicit musical
boundaries requests musical placement.

Musical copies are placed from absolute original tick boundaries. The supported case has one
constant tempo, exactly round-tripping selection boundaries and equal sample widths at every
iteration. Fractional-width or variable-tempo mappings are rejected before mutation. Whole MIDI
notes, controllers and saved performances must fit their positive clip tick spans. Note-index
selection and MIDI Grabber merging require a separate policy.

## Bounds and verification

Planning limits include 4,096 tracks/targets, 100,000 clips/playlists, 1,000,000 aggregate
automation/MIDI/envelope events, 65,536 metadata entries, 128 MiB of estimated document payload,
and two million clip/metadata comparisons per planning phase. Repeated deep data and destination
fragments count toward the limits. Timeline placement, IDs and reservations use checked arithmetic.
These limits bound the proposed document plan; they do not bound total process memory, the
original session or the engine's copy-on-write snapshot.

Synthetic regressions exercise copied silence, Slip/Shuffle placement, automation curves and
follows-edit settings, selection/history, whole MIDI performances, processing metadata, invalid
inputs and musical boundaries. Audio oracles compare source samples and independent expected
mixer output. Mixer control timing remains the existing engine policy.

This contribution changes arrangement duplication only. It adds no mix/playback runtime,
originals store, file-delivery path, repository-wide contributor rule or UI implementation.

## Revision history

- 2026-10-14: Document immutable arrangement duplication, supported material and bounded refusal policies.
