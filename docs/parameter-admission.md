# Parameter setter admission

Mix caches a changed stored parameter target only when the processor's setter accepts it. A refused target remains eligible at the next existing parameter-application opportunity; a newer absolute target replaces it. Ordinary blocks are not subdivided. Aux and Master latency prepasses retain their existing same-position opportunities to retry.

CLAP and VST3 setters report admission to their existing fixed queues. Coalescing a target already in the queue remains possible when that queue is full. A refused new target does not update the adapter's accepted-value cache; VST3 also leaves its controller-synchronization value unchanged. Existing note admission/drop behavior is preserved. Audio Unit setters forward their existing native status before updating the value cache.

Setter acceptance is not processor acknowledgement. Processor failures, second-layer queue limits, catalog-wide batch delivery, parameters absent from the stored map, creation baselines and sample-exact hosted ramps remain separate concerns. Existing stored keys, defaults, mode rules and block-start automation schedules remain in use.

Tests adopt a controlled refusing processor and check its process-side receipt and PCM at ordinary 256/128-frame blocks, including an unaffected neighboring strip. Separate owned CLAP/VST3 fixtures expose 1025 parameters to exercise the existing 1024-entry queue boundary; processor-serialized receipts and affine PCM confirm subsequent consumption and retry. Tests discover only these owned fixtures and require no installed plugins. A successful enqueue alone is never used as proof of consumption.

Linux tests cannot exercise the macOS Audio Unit FFI. Forwarding its status is a source-level change whose native runtime behavior requires macOS validation. These tests do not establish physical-device realtime behavior or whole-renderer allocation freedom.
