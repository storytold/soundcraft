# Checked mixed offline windows

The main-mix, bus and pre-fader track render APIs return `Result`. Literal
inverted ranges, unrepresentable lengths, unknown selected buses or tracks,
and unsupported output widths are refused before plugin preparation. Valid
negative starts and zero-length output retain their existing behavior.

In-memory mixed output keeps the existing channel-summed sample budget.
Oversized requests fail instead of returning shortened audio. Output buffers
use fallible reservation; block appends and the final result require exact
channel and frame lengths. Pre-fader mono rendering budgets its stereo
intermediate and folds the existing left buffer in place.

Streaming rendering checks timeline advancement and the actual compensation
extension. An error ends that render instance; later calls return the same
error without processing more audio. Ordinary block cadence, latency
compensation and successful PCM retain their existing behavior.

Bounce checks destination write authority before window diagnostics and validates
its static request before staging. Constructor, iteration and normalization-pass
errors propagate. Commit checks its tail arithmetic and preflights every selected
target's static window before rendering any target. Failed rendering precedes
source registration, original-track visibility changes and undo publication.

These mixed-render checks are separate from clip-only rendering, explicit preroll,
source-cutoff and tail policies, routing-graph compensation and live workspace
ownership. Those operations retain their existing implementations.
