# Prepared Aux and Folder input routing

Aux and Folder tracks read an internal bus through a channel-conversion matrix.
The mix engine prepares this matrix during structural synchronization and keeps
one boxed, bounded matrix for each connected input. Unchanged inputs reuse their
coefficients on subsequent blocks and synchronizations.

The preparation key uses the input bus identity, its actual buffer format and
width, and the destination strip's effective format and width. Input, bus or
strip-format edits refresh the prepared conversion; disconnected inputs release
it. Rendering checks the effective metadata and uses the existing conversion
rules if malformed duplicate track identities make a prepared entry unsuitable.

Matching layouts pass through, a mono bus feeds both channels of a stereo return
at unity, and other layouts retain the existing fold, speaker and Ambisonics
coefficients. Sends, output pan automation, strip processing and latency remain
unchanged.

This removes repeated input-coefficient construction during steady rendering.
It does not establish a measured latency improvement or move synchronization
off the audio callback. Output/send preparation and other runtime changes remain
separate work.
