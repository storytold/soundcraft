# Optional external file providers

This web capability is **disabled by default**. Installing the WASM API does not register storage,
show provider recent files, or change local saving. The same-origin embedding host explicitly
registers one provider and one authenticated account namespace. Desktop and engine/CLI paths,
Save a Copy and exports retain their existing behavior. There is no bundled server, login,
filesystem mount or dependency on a private deployment.

The integration is a trusted same-origin capability, not a sandbox for arbitrary JavaScript.
The host authenticates users, filters every list/read/write by account, persists bytes durably,
and implements its destination dialog. The editor never interprets a resource ID as a path.
Do not put credentials in resource metadata.

## Protocol v1

The frozen API is available at window.soundcraftFileProviders. The bridge dispatches the
soundcraft-file-provider CustomEvent on window. Read the message type from event.detail.type
and accompanying fields (including bytes) from event.detail. The API exposes:

- registerProvider(providerId, label, namespace) -> connectionId. Identifiers use ASCII letters,
  digits, hyphen and underscore, at most 160 bytes; namespace identifies the authenticated account.
  Re-registering the exact identity is idempotent; an identity change requires unregistering first.
- unregisterProvider() and setEnabled(boolean), controlled by host configuration, not user preferences.
  Unregister refuses unresolved choices, captures and reads. Disabling does not discard pending work.
- setResources(connectionId, [{id,name,updatedAt?}]). A stale account connection is rejected.
  At most 100 unique entries, 255-byte simple filenames and 100-byte dates; no control characters.
- requestOpen(resourceId) -> requestId emits openRequest. The host returns
  openResource(requestId, filename, Uint8Array), or cancelOpen(requestId, error).
  The error argument must be a string of at most 1,024 UTF-8 bytes with no control characters.
  An invalid error leaves the read pending. Retry with a valid string.
  Only a current requested resource can be delivered; opened is emitted after project decoding and
  complete media restoration. Failed or dirty-session opens emit openFailed and preserve the editor.
- resolveSaveDialog(requestId, destination, filename), where destination is provider, device or cancel.
  Pass an explicit string for filename: use "" to keep the suggested name, including for cancel.
  Omitting it, null or undefined is rejected before resolving the choice, which remains retryable.
- saveResult(requestId, status, resourceId), status saved, failed, cancel or download. **Only saved**
  means a durable provider checkpoint; download never acknowledges persistence or clears dirty state.
  resourceId must also be an explicit string: use "" when no durable resource was created (failed,
  cancel or download). A saved result requires the actual valid resource ID. Omitted/null/undefined
  arguments reject the call without consuming the pending result. If the command queue is full,
  the result also stays pending. Retry with the same arguments once the queue has capacity.
- snapshot() reports connected/enabled, provider identity, resources, active document and pending work.

Every interactive Save and Save As emits saveDialog with requestId, documentId, revision, name,
saveAs and the existing resourceId where relevant. Repeated Save while pending is coalesced.
After the choice, the editor freezes native document state and all supported media. A provider
save event owns bytes (Uint8Array), bytesLength, destination=provider and the exact captured
revision. The host MUST acknowledge only after durable persistence. Later edits remain dirty.
If capture or encoding fails (including the 80 MiB bound), saveFailed is emitted instead with
requestId and error. The host should dismiss that request's progress indicator and show the error;
it must not keep waiting for a save event or acknowledge bytes that were never emitted.
A device choice uses the original native serializer and browser download writer; it keeps provider
dirty state unchanged. Device formats and resource-linking behavior remain those of the official web app.
Cancel/failure never marks the project clean. Request IDs include a random page nonce and a
monotonic serial; delayed, duplicated and cross-account results cannot acknowledge another save.

All identity-bearing events include providerId, namespace and connectionId. Provider Recent is a
separate native UI section from this device's paths, including on Home where the app has one.
A currently modified project must be saved or explicitly closed before opening a provider file.
Pending operations block replacement commands and request the browser's usual unload warning.
The host must not claim a successful save for a native-project JSON with missing external media.

Disabling stops new provider choices while retaining document bindings and pending work. Interactive
Save and Save As on a linked document or pending save are explicitly refused while disabled. Re-enable
the provider to save it, or use Save a Copy for a device download. This refusal does not deliver bytes,
clear dirty state or acknowledge a provider checkpoint. Unlinked documents keep their original local
save behavior.

## App payload

SoundCraft uses the existing native .scraftz ZIP: .scraft plus every source as lossless float WAV
under Audio Files/, including built-in Sampler sources. Plugin state is captured before freezing.
Missing/undecodable audio, decompression beyond the bound and video references are rejected before
replacement or a successful save event. The current web shell cannot restore referenced movies;
this limitation is explicit rather than silently dropping them. Native session/video workflows are
unchanged. Acknowledgements retain the captured Session checkpoint, so later edits stay dirty and
undoing to that checkpoint can become clean again. A saved provider session drops its previous
device path. Revert to Saved cannot read that older device copy.

Each provider operation is bounded to **80 MiB**. Restored audio also has an independent 80 MiB
budget for decoded floating-point samples across unique sources, with a per-file decode limit before
resampling. Media/path metadata and asset counts have separate bounds. Large projects can fail
capture, and local/native workflows remain available. Only a durable provider acknowledgement can
mark a provider revision saved.

## Validation

Source fixtures cover default-off behavior, account changes and stale connection lists, forged and
repeated acknowledgements, cancel/failure/download, bounded queues, resource aliases and correlated
opens. App fixtures cover bounded names, bundle integrity/media round trips and delayed-checkpoint
state. Run the normal repository unit/format/clippy/web-build checks before review. The embedding
host also needs browser integration tests for its authenticated persistence, dialog, media recovery
and failure handling. This feature does not claim that an arbitrary host authenticated or persisted
anything; those are responsibilities outside the editor.

Contribution credit: @alejocapo04.

## Saving before close or opening another session

With a provider enabled, Save in the Save Changes prompt uses the same destination chooser as menu Save, including in the browser. The continuation belongs to that request and the actual captured document/revision. Close, Quit or opening the pending session runs only after a durable provider ACK, while that exact revision is still current and no editor gesture, dialog, import, recording or bounce remains pending. Cancellation, encoding/write failure, a device download, or later edits keep the session open. A browser download does not prove durable provider storage. Programmatic SoundApp::run saves still honor their explicit native parameters and never open the chooser.

The browser checks unconsumed frame input before a saved-project continuation: text, keys,
pointer events, dropped files, held pointers and active editor gestures keep the project open.
This cancels only the close/open/quit continuation; the acknowledged checkpoint stays valid,
and edits applied by the following UI pass become dirty normally.
Direct provider opens use the same replacement guard. An active recording, queued import,
bounce, dialog, plugin state draft, editor gesture, held pointer or unconsumed frame input refuses
the open before stopping playback or installing a decoded session. The current take and session
remain available; retry opening after finishing and saving the pending work. Browser file reads
that finish after the session is replaced are discarded, including audio, MIDI and sampler reads.

## Delivery errors and ownership

Outgoing payload copying uses a catch-enabled JavaScript import. Recoverable copy or
`dispatchEvent` exceptions are logged without abandoning the remaining bounded publication
batch. A failed `save` transfers only its still-owned captured document/revision to the normal
application result consumer as `failed`, and queues a correlated `saveFailed` terminal for
bounded publication. The next logic frame releases that capture's checkpoint and close
continuation. It does not acknowledge persistence, commit editor drafts, clean newer edits or
change an account/resource binding. A later explicit Save can capture and retry the project.

An already accepted synchronous `saveResult` wins over a subsequent dispatch error; it cannot
also become a failed capture. The publisher requests a following logic frame for accepted
reentrant results as well as failures. Uncommitted plugin-editor state remains dirty in published
document metadata even when the captured engine snapshot was acknowledged.

Failed unresolved `saveDialog` and `openRequest` delivery releases only those still-owned
requests and queues their correlated `saveFailed`/`openFailed` terminals. A failed read is retired
before testing event-queue capacity, so a later callback from the same publication batch cannot
resurrect it. A full ordinary command queue rejects chooser resolution without consuming the
chooser; explicit cancellation remains available. Queued and currently dispatched terminals keep
their account until the publication attempt finishes, and nested publication leaves the outer
batch in charge. All retained failures belong to already admitted bounded operations.

Exceptions a browser swallows inside event listeners are not observable `dispatchEvent`
failures. A process abort or unrecoverable out-of-memory failure is outside this recovery path.
Hosts still implement authentication, storage durability and any network retry policy.

## Regression boundaries

Native regressions exercise full-queue and reentrant ownership, the actual app result consumer,
close continuation and plugin-state draft guards. Original synthetic audio is encoded to the
native `.scraftz` bundle and decoded again, checking every source sample.

The WASM binary's `file_providers::browser_tests::page_local_sound_provider_delivery_failure_preserves_audio_and_recovers`
fixture observes real `soundcraft-file-provider` window events, invokes the exported API and the
production provider runtime, injects recoverable copy/dispatch exceptions with JS-finally
restoration, retries a save, and reopens the emitted bundle through actual `openResource`
admission with its audio intact. It also checks the default local Save dialog, exercises the
actual Save Changes dialog in egui, verifies Close Session waits for the matching durable result,
and reopens the saved audio after closing. The fixture runs in a browser using an egui context
without a WebRunner, GPU, audio device, plugin window or backend. The injected JavaScript
RangeError exercises recoverable exceptions from the copy call. The fixture does not cover
unrecoverable allocation failure or DOM navigation.
