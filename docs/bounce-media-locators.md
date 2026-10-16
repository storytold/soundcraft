# Bounce media locations

Native Bounce reserves every nonempty audio and video location currently named
by the session, including missing files, unsaved imports and recordings, and
generated sources not placed on a track. Relative names use the current session
folder, or the captured working directory when no session folder is set.

Destination write authority is checked before comparing media names. The
filesystem gateway resolves names through existing ancestors and symbolic links;
ordinary missing suffixes remain comparable. An unresolved locator fails before
encoding or staging, with a generic locator-validation error.

This validation inspects name metadata, including ancestors outside automation
roots. It never opens media contents or requires source-content read authority,
so decoded-session bounces retain write-only operation. Locators are processed
one at a time; Windows path validation uses bounded, lossless UTF-16 units.
Regular canonical disk and UNC paths remain supported; device, stream and
ambiguous disk-name forms are refused.

Native path bytes are preserved. Conservative ASCII case matching also reserves
case variants on case-sensitive filesystems, where it may refuse distinct names.
Filesystem-specific Unicode equivalence and the existing external directory
check/use race remain outside this name-comparison guarantee. Only locations
still present in session metadata can be reserved.

Streaming, normalization, channel mapping and progress retain their existing
behavior. Browser delivery keeps its existing in-memory download path. Existing
file publication and ownership checks remain a separate delivery boundary.
