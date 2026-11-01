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

API: window.soundcraftFileProviders; CustomEvent: soundcraft-file-provider. The frozen API exposes:

- registerProvider(providerId, label, namespace) -> connectionId. Identifiers use ASCII letters,
  digits, hyphen and underscore, at most 160 bytes; namespace identifies the authenticated account.
  Re-registering the exact identity is idempotent; an identity change requires unregistering first.
- unregisterProvider() and setEnabled(boolean), controlled by host configuration, not user preferences.
  Unregister refuses unresolved choices, captures and reads. Disabling does not discard pending work.
- setResources(connectionId, [{id,name,updatedAt?}]). A stale account connection is rejected.
  At most 100 unique entries, 255-byte simple filenames and 100-byte dates; no control characters.
- requestOpen(resourceId) -> requestId emits openRequest. The host returns
  openResource(requestId, filename, Uint8Array), or cancelOpen(requestId, error).
  Only a current requested resource can be delivered; opened is emitted after project decoding and
  complete media restoration. Failed or dirty-session opens emit openFailed and preserve the editor.
- resolveSaveDialog(requestId, destination, filename), where destination is provider, device or cancel.
  Pass an explicit string for filename: use "" to keep the suggested name, including for cancel.
  Omitting it, null or undefined is rejected before resolving the choice, which remains retryable.
- saveResult(requestId, status, resourceId), status saved, failed, cancel or download. **Only saved**
  means a durable provider checkpoint; download never acknowledges persistence or clears dirty state.
  resourceId must also be an explicit string: use "" when no durable resource was created (failed,
  cancel or download). A saved result requires the actual valid resource ID. Omitted/null/undefined
  arguments reject the call without consuming the pending result.
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

## App payload

SoundCraft uses the existing native .scraftz ZIP: .scraft plus every source as lossless float WAV
under Audio Files/, including built-in Sampler sources. Plugin state is captured before freezing.
Missing/undecodable audio, decompression beyond the bound and video references are rejected before
replacement or a successful save event. The current web shell cannot restore referenced movies;
this limitation is explicit rather than silently dropping them. Native session/video workflows are
unchanged. Acknowledgements retain the captured Session checkpoint, so later edits stay dirty and
undoing to that checkpoint can become clean again.

Each provider operation is bounded to **80 MiB**. Media/path metadata and decoded asset counts are
bounded independently. Large projects fail before a save event; local/native workflows remain
available. No cloud project is marked saved merely because serialization or a download completed.

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
remain available; retry opening after finishing and saving the pending work.
