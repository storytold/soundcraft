# Reused send-routing coefficients

The mix engine retains one boxed channel-conversion matrix for each send slot
within the nominal send limit. Rendering compares the effective source and bus
formats, actual buffer widths, pan law, stereo pan and resolved surround fields.
Unchanged keys reuse coefficients; changed keys update the existing matrix.

The strip's pointer table stays bounded at the nominal send-slot count. Sends
beyond that count continue through the existing uncached conversion. The table
also covers slot movement and send-vector growth that do not change the routing
fingerprint. Cached entries hold no destination or sample-buffer references.

Gain, mute, target lookup and pre/post buffer selection remain live. Mono sends
keep their own pan, stereo sends keep hard left/right routing, and explicit
surround settings take precedence over following the track's surround position.

The first eligible use of a cached slot may allocate its matrix on the rendering
owner. Warm unchanged sends construct no additional matrices; this does not
establish a measured latency improvement or move preparation off the callback.
Output conversion, Aux input preparation, automation, PDC and other runtime work
remain separate.
