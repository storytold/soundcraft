# Output-routing coefficient reuse

The mixer retains one boxed channel-conversion matrix for each non-Master strip
that routes to Main or a bus. Each existing pan segment checks the effective
source and destination formats and buffer widths, pan law, resolved stereo pan
values, and all fields of the optional surround position. Matching inputs reuse
the coefficients; changed inputs refresh the same allocation.

The destination is resolved on every routing call. Moving a strip between buses
with the same geometry can reuse coefficients while writing to the newly selected
bus. Fader, mute, source samples, and destination buffers remain live inputs to
the existing processing path.

Pan automation retains its timeline-aligned 64-sample grid. Coefficient reuse
preserves that schedule, the existing channel mappings and folds, and the order
of sends, output delay, and output routing.

First eligible use may allocate the per-strip box on the render owner. This
change establishes coefficient reuse rather than a whole-renderer allocation or
latency guarantee.

Regression coverage uses synthetic audio, analytic channel expectations, live
session edits, block partition comparisons, and actual matrix-construction
counters. The public PCM fixture is also run against the unchanged base before
the candidate is validated.
