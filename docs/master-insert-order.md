# Master insert order

Last reviewed: 2026-10-14. Last updated: 2026-10-14.

Master tracks can explicitly place their existing insert chain after their volume fader.
The saved `master_insert_order` setting is `pre_fader` or `post_fader`.

## Compatibility and control

An omitted setting reads as `pre_fader`. Newly constructed mixers also default to `pre_fader`,
and that default is omitted from saved JSON. Existing sessions retain their insert-before-volume
order. No track is automatically migrated to post-fader processing.
Readers that do not implement this setting keep their previous order when opening an explicit
post-fader session; the new setting requires a reader that supports it.

Use the registered engine command through the CLI, control channel or MCP:

```json
{"command":"mix.master_insert_order","params":{"tracks":[1],"order":"post_fader"}}
```

Replace the example track ID with a Master track's ID. Set `order` to `pre_fader` to restore the
existing order. The command validates its parameters and all targets before changing the
document, refuses non-Master targets and applies a multi-track change as one Undo step. A valid
unchanged setting leaves document state and pending Redo intact.

The Master strip's insert-order selector offers Pre-fader and Post-fader choices and dispatches
the same command. Its displayed setting follows the session, including Undo and Redo.

## Processing

Pre-fader mode keeps the current sequence: copy the main mix into the Master strip, process
its active inserts, then apply volume while copying the strip back to the main mix.

Post-fader mode applies volume while copying the main mix into the Master strip, processes
the same insert chain and copies the result back without applying volume again. This matters
for nonlinear inserts, which receive a different signal level when the fader precedes them.

The explicit post-fader branch reads volume at the first and last included samples and
interpolates within the callback. Legacy mode retains its existing next-sample endpoint and
callback divisor. Both endpoints use bounded timestamp arithmetic. This does not introduce
sample-exact breakpoint segmentation or change the existing volume-reader policy.

The existing Master cascade, channel width, active/bypass handling, transport and parameter
delivery, insert failure containment, meters, delay compensation and metronome placement remain
in use. Ordinary track and aux insert order stays unchanged. No hosted-plugin per-sample
processing or broader playback rewrite is included.

## Verification and limits

Synthetic nonlinear DSP references distinguish gain-before-insert from insert-before-gain.
Coverage includes static and automated volume, serial inserts, multiple Masters, supported
widths, inactive/bypassed inserts, callback schedules, renderer reset and bounded endpoints.
Compatibility cases check legacy defaults and explicit serialization, command atomicity,
history/no-op behavior and native save/reopen.

The setting is exposed through the engine command and the Master strip's selector. UI interaction
tests exercise actual popup choices; offscreen captures check the displayed Pre and Post states.
Callback deadlines, allocation behavior of the complete renderer and physical-device playback
are not established by these tests.

## Revision history

- 2026-10-14: Add explicit Master insert order with legacy defaults and source-grounded processing limits.
