# Send pan and mute automation

Last reviewed: 2026-10-14. Last updated: 2026-10-14.

Bus sends read supported `SendPan` and `SendMute` automation during mixing. Pan automation
covers mono sources sent to ordinary stereo buses. Mute automation gates ordinary bus routes
using the existing channel-routing matrix.

## Reading and fallback

A valid nonempty lane in an automation mode that reads overrides the corresponding static send
control. A pan lane interpolates between knots and clamps finite values to -1 through 1. A mute
lane holds each knot's value, clamps to 0 through 1 and gates the send at values of at least 0.5.
Before the first knot, the reader holds its first value; at duplicate timestamps it uses the last
knot at that position.

Off mode, an absent or empty lane, nonfinite point values or decreasing timestamps use the
static control for the entire callback. This reader follows the existing mixer convention:
`lane.enabled` does not independently gate reading. Global lane-enable and suspension semantics
remain the existing mixer policy.

Each callback builds local forward cursors at the source-sample time represented by its send
buffer. Seeking, looping or rendering irregular chunks does not reuse a cursor from an earlier
callback. Reader initialization and per-frame lookup share that clock. Position, frame offset
and sample age are combined in widened arithmetic before clamping once to the supported bounds.

## Delay accounting

Audio send clocks account for processor-declared active insert latency and alignment already
applied to the selected buffer. Insert ages accumulate separately from the bounded delay-
compensation field; its safety cap does not shorten a send's clock. This assumes processors
report their delay accurately. Aux and folder sends also carry the upstream input bus's sample age, including
known ages propagated through nested Aux paths. Post-fader Aux buffers include their applied
alignment; the existing pre-fader Aux copy does not receive that alignment and uses its own age.
Pre-fader audio-track buffers do receive their existing pre-fader alignment.

Direct bus outputs contribute the age after any additional output delay. A send tap excludes
that later output delay, receiving processors and Master processing. Audio processing, routing,
delay-line placement and offline latency removal retain their existing behavior.

Bus age metadata is rebuilt with each block and introduces no new dynamic collection. Equal
known contribution ages retain a shared source clock. A sum of differently delayed contributors
has no single source age; mixed or overflowing age metadata retains the earlier render-position
clock for these send readers. This preserves the existing mixed-routing policy and does not
claim complete graph delay compensation. Empty bus inputs use zero upstream age. When delay
compensation is off, a known path still accounts for its actual processor delay.

## Routing compatibility

Without a qualifying pan lane, routing uses the current static matrix. Mono sends retain their
static send pan; stereo sends retain their left/right routing even when a stored static send pan
is nonzero. Automated stereo-send pan is outside this contribution.

A supported mute lane can override the static send mute, including unmuting it. Track mute and
the existing solo/VCA suppression still take precedence. Pre-fader sends use the existing
pre-fader buffer, post-fader sends use the existing processed buffer, and send level keeps its
current reader and timing policy.

Mute gating preserves the ordinary matrix for mono folds, stereo and supported surround widths.
Ambisonic routes and non-bus sends keep their existing handling. Main outputs, master processing,
audio delay compensation, hosted-plugin parameter delivery and routing-error policies are unchanged.

## Callback work and verification

Pan gains are computed per sample only for supported automated mono-to-stereo sends. Mute uses
a stepped gate at each sample. Static routing matrices and gated-path sparse taps are prepared outside the
sample loop; automated mono-pan routing needs no matrix. The new helpers use local cursors and fixed
stack storage without introducing heap allocation.

Lane validity is scanned once per callback over its stored points. Worst-case callback CPU and
the allocation behavior of the complete mixer remain unmeasured.

Synthetic regressions exercise independent PCM expectations, all pan laws, mute knots, static
fallback, pre/post-fader routing, send level, irregular chunks, seek/loop behavior, hostile
timestamps and short buffers. Engine cases cover command history and native save/reopen.
Static stereo compatibility checks use nonzero stored send-pan values and unchanged routing
references.

Latency regressions cover the unrelated silent Maximizer case, pan and mute source clocks,
Aux pre/post taps, nested upstream paths, subsequent output/processor delay, mixed-age fallback,
negative positions and reset/seek behavior with synthetic audio.
Unit regressions also check insert-age accounting beyond the delay-compensation cap and age overflow.

This contribution is limited to these send readers and bus-routing branches.

## Revision history

- 2026-10-14: Document supported bus-send automation, fallback, stereo compatibility and callback-work limits.
- 2026-10-14: Account for the actual delayed send tap and known upstream bus ages; retain mixed-age timing limits.
