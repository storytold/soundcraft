# Playback publication ownership

Playback admits one prepared session publication at a time. The control caller retains one latest
unprepared snapshot while a previous publication is outstanding. Replacing that waiting snapshot
happens on the control caller. `Player::idle` collects completed publications and admits the latest
waiting snapshot. `try_update_session` returns `Ok(true)` for immediate publication, `Ok(false)`
for a coalesced snapshot, and an error when its playback owner is closed. Call publication and
collection methods from the control owner, as with plugin preparation and editor maintenance.

Hosted preparation runs only after admission credit is available. A deferred snapshot cannot
advance the plugin shadow, change the prepared sample rate, or close the accepted snapshot's
editor handles. Capture first attempts to publish the waiting snapshot after collection. While
a snapshot remains deferred, state and editor emissions are forwarded only when the accepted
track/slot/plugin/channel identity and prepared rate still agree with that snapshot. Editor GUI
maintenance continues for stale identities, but their gestures cannot edit replacement slots.
The identity rules use a borrowed Mix lookup with the existing hosted eligibility and instrument
width policy. Guarding a slot does not clone IDs or decode stored plugin state. Editor availability,
open requests and external knob updates use the same guard; stale windows remain closable and
GUI maintenance and the actual-open-window query continue to work.
Same-identity stored-state invalidation and document-generation pairing remain separate work.
Accepted publication retains the existing mixer synchronization and plugin
matching behavior; it does not recreate matching live processors.

The control caller allocates an envelope before sending a session. The callback puts the displaced
session, removed plugins, replaced output buffers, and any old sample-rate mixer in that envelope.
A bounded return channel carries the envelope back. Full or disconnected returns retain the same
envelope on the callback state, and further session installation waits until it can be returned. A separate bounded channel
holds the publication payload; its ordered transport marker carries no heavy resources. Stop,
Locate, recording and input commands remain serviceable under return backpressure. Playback
continues with the installed mix. Normal single-publication admission reserves return
capacity; the retained-return path also handles private-channel teardown and backpressure.

Compatible updates preserve unread rendered PCM, its fractional cursor, and the live mix engine.
A main-format change returns the old pending buffers and resets the pending cursor, including
changes between layouts of equal width.
A sample-rate change constructs the replacement mixer and returns the whole old mixer and old
pending buffers for destruction by the control owner. It resets the pending cursor even when the
channel count stays equal, so unread old-rate PCM is never consumed at the new session rate.

The device closure borrows audio state and device scratch from a shared slot retained by Player.
Its access uses `try_lock`, never a waiting lock. Only teardown takes a waiting lock on the control
owner. Shutdown first stops the stream or signals and joins the silent clock, then takes the audio
state from the slot and destroys it on the shutdown caller. A device closure released later owns
only a reference to an empty slot; shutdown also empties the allocated shared meter snapshot
before late closures release their shared handles. Failed stream construction reclaims its slot before trying a
fallback. The browser no-device path closes its unused receivers on the construction caller.

This contract covers session publication resources and final playback-state reclamation. It does
not make all callback work allocation-free: structural synchronization, strip and bus storage,
plugin-specific behavior, metering, and input-ring replacement retain their existing implementation.
Transport commands keep their existing channel and stay serviceable while a failed return is
retained and further session installation is deferred. There are no new routing, automation, latency-compensation or
plugin parameter-delivery policies in this change.
