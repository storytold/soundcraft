# Track Commit channel-format integrity

**Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13

Track Commit retains the source track's exact `ChannelFormat`, including surround, height and
ambisonic layouts. Channel count alone cannot identify a layout: 5.1 and 6.0 both have six
channels, while Quad, LCRS and first-order ambisonics all have four.

The shared pre-fader renderer already returns multichannel audio in the track's own layout.
Commit copies that format to the new audio track and checks the returned channel count and
frame count before adding the source or hiding the original track. Formats wider than the
renderer’s 16-channel limit are rejected before rendering. The fixed two-second tail uses
checked arithmetic; overflowing ranges or requests beyond the renderer’s sample limit return
an error. A command committing several tracks rolls back all of them if any selected track
fails. Current routing and mixer inheritance remain unchanged.

The regression fixtures use distinct bipolar signals on each channel across all 30 declared
formats within the 16-channel limit, with an insert applied before committing. The four higher
ambisonic orders are rejected without replacing the source track. Additional cases check
bus/send routing, undo/redo, session save/reopen and float WAV export, including its output
speaker mask.

Mono uses a two-channel frame limit because the existing renderer temporarily targets stereo
before folding back to mono. This frame limit does not bound total process memory. The shared
mono renderer’s -3 dB center-pan gain followed by `SQRT_2` compensation produces approximately
+0.0103 dB gain; the signal fixture accounts for that existing processing behavior.

Session track metadata retains the exact layout. Generic source WAV saving still infers its
channel mask from channel count. This change preserves the shared renderer, the fixed tail
and the existing commit options.

Implementation: `crates/engine/src/io/render.rs::commit_track` and
`crates/mix/src/offline.rs::render_track_pre_fader`. Regression fixtures:
`crates/engine/tests/commit_multichannel.rs`.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Refresh against current main; document source-supported format checks, existing renderer limits and regression fixtures. |
| 2026-10-12 | Preserve exact committed formats and reject incomplete or unsupported-width renders; add channel-signal, routing, persistence/export and rollback coverage. |
