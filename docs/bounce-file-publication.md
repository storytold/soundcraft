# Bounce file publication

Native Bounce Mix delivery requires an unused destination. Existing files,
directories and symbolic links, including dangling links, are refused. A file
created by another writer before publication wins; Bounce does not replace it.

Each job creates its own exclusive sibling staging file. The former fixed
`<name>.part` path is left alone, so concurrent jobs and cancellation cannot
truncate or remove another job's work. Completed encoded bytes are flushed and
synced before a no-replace publication operation exposes the final file.

The staging guard retains the initial write authority and also checks the
current caller's authority. Publication and cleanup validate the canonical
parent and held-file identity. Cancellation discards buffered output and removes
only a verified owned stage. If ownership or write authority cannot be verified,
the stage is retained and its path is logged. A published destination is never
removed by this cleanup.

These path checks retain the filesystem gateway's existing limitation: an
external process that replaces a directory between a check and its operation
can still race it. This does not establish a stronger filesystem sandbox.

Rendering, normalization, channel mapping, progress and incremental execution
retain their existing schedules. Browser delivery continues through the existing
in-memory download hook. Exact PCM comparisons and native filesystem regression
fixtures cover successful output, collisions, cancellation, initialization
failures, replaced staging paths and retained authority.
