# Ordinary strip mute automation

Audio, instrument, Aux and Folder strips evaluate their existing mute lane at each sample represented by a render block. Values at or above 0.5 silence both the processed output and its pre-fader send copy. A mute and unmute inside one block therefore take effect without splitting that block into smaller processor calls.

The first mute lane, existing interpolation, and existing automation-mode policy remain authoritative. An empty lane or Off mode leaves ordinary audio unchanged. Static mute, solo suppression and static VCA mute remain dominant. Fader ramps, pan scheduling and hosted processor call sizes remain unchanged.

Mute reads use the current render-position clock. Signal alignment, Master/VCA automation and send-control policies retain their existing behavior.

Synthetic regressions cover interior and boundary transitions, different block partitions, pre/post-fader sends, Aux/Folder input paths, mode/empty-lane fallbacks, static/solo/VCA suppression, unchanged fader/pan scheduling, and an explicitly adopted mock hosted processor's call cadence. No installed hosted plugin is required.
