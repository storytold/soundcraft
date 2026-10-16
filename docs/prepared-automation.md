# Prepared automation reads

Reviewed: 2026-10-14.

`soundcraft_mix::PreparedSession` binds an immutable session to lookup tables for its
existing automation lanes. Preparation happens before publication to playback or before
offline rendering. The index contains references by original lane position; it copies
neither audio nor points nor plugin parameter strings.

Playback owns an `Arc<PreparedSession<'static>>`. Initialization and document updates
prepare the pair on the control side. The callback adopts that pair and uses the existing
retirement channel to return a replaced pair. The session and its index therefore share
one lifetime. An older held snapshot continues to describe its own document.

`MixEngine::render_prepared` consumes the pair. The range and bus render helpers prepare
a borrowed session once, and streamed Bounce holds one owned pair across its rendering
and optional normalization passes. Existing `MixEngine::render` and
`RangeRender::next_block` remain available for ordinary session reads.

## Compatibility

The index preserves the first lane for each parameter, including an empty or disabled
first lane. It preserves point order, duplicate points, parameter-key case and existing
nonfinite-value behavior. It does not normalize curves or add absent plugin parameter
keys. Automation modes, Trim, bypass and plugin setter cadence retain existing behavior.

The current control schedules remain:

- Ordinary volume interpolates gain between the first and last included sample of a block.
- Master volume retains the existing pre-fader order and next-sample block endpoint.
- Pan uses the existing global 64-sample grid.
- Mute and stored plugin parameters are read at block start.

The curve evaluator retains the existing `f32` interpolation operations. Only timestamp
differences that would overflow use widened subtraction. The Master endpoint uses
saturating arithmetic. Neither change alters representable ordinary timestamp results.

A same-rate document update preserves unread pending playback samples and its fractional
read position. Existing rate changes, main-format changes, instance preparation and
editor identity policies remain in use.

## Admission and fallback

The default requested index-storage budget is 32 MiB. The constructors accepting a
budget cap it at that default. Admission counts tracks and lane indices with checked
arithmetic; vector reservations are fallible. Allocator metadata and source-document
storage are outside that budget.

An exceeded budget or reservation failure keeps the same session with ordinary lane
lookups. A zero budget selects that fallback deterministically. Refusal neither retains
a previous document nor produces an empty render. Track lookups bind to addresses in
the paired immutable document, so reused track IDs cannot cross-bind lanes.

## Verification and limits

Synthetic Mix tests check index hits, first-lane behavior, budgets, timestamp bounds,
legacy PCM and hosted setter cadence. Engine tests exercise commands, Undo/Redo,
Save/Open, range rendering, streamed Float32 Bounce and two-pass normalization. Playback
tests exercise publication, held snapshots, same-rate pending audio, rate/format changes
and control-side retirement. These tests run through ordinary package test commands
without requiring installed audio plugins or a special automation environment variable.

This feature does not establish whole-renderer allocation freedom, a realtime deadline,
physical-device acceptance or complete automation writing/suspension policy. DSP,
host processing, routing, latency compensation, session generations and UI controls
remain outside this change.

The existing retirement channel uses `try_send`; its full/disconnected-channel disposal
policy is unchanged. This feature therefore does not guarantee control-side destruction
under channel saturation.
