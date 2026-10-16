# Float32 file preservation

**Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13

WAV/BWF/RF64 Float32 and AIFF-C `fl32` output retain every finite `f32` sample without
clipping to full scale. This includes positive and negative headroom above magnitude 1,
signed zero and subnormal values. File bytes use the container's PCM byte order: little-endian
for WAV and big-endian for AIFF-C. Float32 output does not apply integer dither.

NaN and positive or negative infinity are written as positive zero, matching the native
float decoder's existing policy. Streaming recording takes already preserved finite
headroom; their writer thread now applies the same nonfinite-to-zero policy before writing
samples. This does not change input callbacks or recording-ring ownership.

Integer encoding remains quantized and clipped to its representable range. Its independent
TPDF dither sequences and existing NaN/infinity handling are unchanged. FLAC remains an
integer format; it does not gain Float32 output.

Generated-source folder saves, Save Copy and `.scraftz` bundles use Float32 WAV audio, so
they retain finite headroom when reopened. Regression tests inspect actual interleaved
little- and big-endian payload bytes, check distinct six-channel values bit for bit, cover
nonfinite samples and integer clipping/dither, and inspect streamed take payloads as well as
folder and bundle reloads.

This change covers Float32 file sample preservation. Delivery sample-rate selection,
additional conversion and dither controls, and other mixdown options remain separate.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Rebased on the current shared PCM encoder and recording writer; clarified current-source behavior. |
| 2026-10-12 | Preserve finite Float32 file samples and consistently silence nonfinite samples; add payload, recording and session-persistence coverage. |
