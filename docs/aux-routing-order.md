# Aux and Folder routing order

Updated: 2026-10-14

Aux and Folder strips read audio from busses populated by earlier strips. The mixer prepares their execution order when the existing session-structure fingerprint changes, then reuses that order for subsequent blocks.

For an acyclic routing graph, preparation computes the longest producer path to every Aux or Folder input. Producers reach a bus before its consumers read it, even when tracks were created in reverse routing order or the chain contains more than eight returns. Output and send routes both create dependencies. Multiple routes from the same producer to the same bus contribute audio normally but count as one producer dependency. Strips at the same depth retain session order, preserving the previous ordering for ordinary short acyclic graphs.

The earlier ordering calculation capped every depth at eight. In a sufficiently deep reverse-created chain, that made a consumer tie with its producer and execute first. Because bus buffers are cleared each block, the consumer could receive silence. Exact acyclic depths remove that tie.

If any cycle exists, the whole graph retains the previous bounded depth calculation and deterministic session-order ties. Preparation does not reject a cyclic session or change its feedback policy. This increment does not alter PDC accounting, plugin processing or failure containment, render-helper APIs, pan coefficients, Master insert order, playback ownership, or device selection.

This is the routing-order increment of the remaining runtime work discussed in #394. Further runtime changes require independent treatment of prepared routing coefficients, workspace and instance lifetime, hosted automation delivery, graph latency, and explicit render failures. This increment does not claim to complete that rewrite or move all preparation off the audio callback.

Regression tests use synthetic source samples and public mixer APIs. They cover deep reverse-created chains, output/send dependencies, duplicate routes, branching, Folder consumers, callback-size invariance, edits on an existing mixer and preserved cyclic behavior with PDC enabled or disabled. The same public fixture can run against the previous main to demonstrate the depth-cap failure independently of the new preparation helper.

Revision history: 2026-10-14 — exact acyclic routing order with the existing whole-graph cyclic fallback.
