# CI plugin discovery

`cargo xtask ci` isolates plugin discovery in its Cargo child processes. CLAP and
VST3 use explicitly empty search roots, Audio Unit discovery requests only Apple's
system manufacturer (`appl`), and optional installed-plugin smoke switches are
removed. The six quality gates and repository-owned plugin fixtures still run.

`SOUNDCRAFT_CLAP_SEARCH_PATHS` and `SOUNDCRAFT_VST3_SEARCH_PATHS` are exclusive,
OS-separated path lists. When present, they replace standard locations and the
legacy additive path variables. An empty list disables discovery of that format.
When absent, ordinary application discovery retains its existing search paths.

`SOUNDCRAFT_AU_MANUFACTURERS` restricts Audio Unit enumeration to comma-separated,
exact four-byte printable ASCII codes before querying component names or versions.
At most 64 distinct codes are admitted. An empty, invalid or over-budget policy
disables enumeration; an absent policy retains normal discovery. Set policies
before first discovery; registry caching and explicit rescan behavior are retained.

These policies govern discovery, not sandboxing or explicit plugin loading.
Other xtask commands retain the caller's environment. Unix command regressions
exercise the actual CI executable with an owned Cargo probe; cross-platform host
tests cover the selectors without relying on installed plugins.
