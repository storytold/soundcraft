# Bounce delivery sample rate

Last reviewed: 2026-10-13. Last updated: 2026-10-13.

`file.bounce_mix` accepts optional `sample_rate`, an integer JSON number from 8,000 through
768,000 Hz. Omitting it retains the session rate. The Bounce Mix dialog starts at the session
rate, offers common rates, and accepts a custom integer rate within the same bounds. The
session rate, source audio, clip positions, routing and saved plugin settings do not change.
The dialog omits the rate override when its value matches the current session rate, retaining
the existing default streaming path.

The mixer and plugins run at the session rate. The selected main mix or bus is optionally
folded to stereo, then converted once with the existing windowed-sinc converter. Conversion
precedes normalization, output-rate metering, encoding, integer quantization and existing
TPDF dither. The WAV channel mask still describes the delivered format. Equal-rate bounces
skip conversion and retain the prior encoded bytes and measurements. Bounded in-memory
compatibility helpers provide main-mix and named-source delivery without changing the session.

Results include actual `sample_rate`, `frames`, and `seconds = frames / sample_rate`, alongside
file bytes, peak, LUFS and true peak. Measurements describe the output-rate signal before
final quantization; they do not claim a post-encoding analyzer. Existing meter weights are
inferred from channel count (six channels use 5.1 weights, affecting 6.0); layout-aware LUFS
weighting remains separate. This change selects the measurement rate only. Conversion length is rounded
to the nearest whole frame, with a half frame rounded upward. A request that would round to
zero frames fails rather than producing an empty file.

An omitted rate override retains the current incremental writer and its existing output-format
limits. Explicit delivery-rate requests and in-memory delivery helpers validate the bounded
planar-buffer plan below, including when their explicit rate matches the session rate.

For explicit delivery planning, checked arithmetic verifies the range length and the existing
four-hour limit before rendering. Collected source buffers are bounded to 2^29 total samples, using their actual
width and at least stereo for its working path. Fold/conversion buffers must also fit that
aggregate limit while coexisting. Converted delivery is additionally bounded to 2^28 total
samples, matching the new fallible explicit-length conversion helper. Oversized requests
fail before rendering or writing a destination; returned frame/channel shapes are checked
again to prevent silently truncated delivery. Explicit delivery requests reject a declared Main
output or named bus wider than 16 channels, even when a stereo fold is requested, before the model can silently
substitute stereo for an unsupported Main format. Supported Main formats, including the
existing mono/missing-output stereo fallback, retain their behavior. These limits cover whole planar bounce buffers;
shared-renderer plugin latency/tail workspaces retain their existing bounds.

Changed-rate delivery yields between bounded kernel-construction, conversion and output
processing steps so cancellation remains available. It retains bounded planar buffers rather
than streaming SRC. Allocation, plugin processing and disk operations can still block; the
step limits do not provide a hard timing guarantee.

Tests cover default byte/stat equivalence, 44.1/48/96 kHz delivery in WAV/AIFF/FLAC, distinct
multichannel signals, bus and stereo-fold routing, exact frames and speaker masks, output-rate
metrics, one-frame half-rate rounding, malformed/huge requests with no writes or document
changes, sliced-conversion equivalence and cancellation, default streaming compatibility,
UI selection dispatch and current-source offscreen previews. Existing DSP SRC tests remain
unchanged. The current Bounce Mix file browser and other dialog controls are retained.

This change selects the Bounce Mix delivery rate. Stems, SRC quality selection, noise shaping,
master-fader dither placement, pull-up/down and other mixdown options remain separate.

## Revision history

- 2026-10-13: Rebased delivery-rate integration on current bounce and dialog paths; retained source-grounded limits.

- 2026-10-12: Added independently selected bounce delivery rate, bounded conversion and scoped
  command/UI regression coverage. No global parity/readiness estimate was changed.
