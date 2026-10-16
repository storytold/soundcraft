# Track Bounce file delivery

Updated: 2026-10-13. This document describes SoundCraft's selected-track export implementation and its explicit limits.

`track.bounce` writes files separately from `track.commit`. It preserves the session document, source inventory, arrangement, mixer settings, Undo/Redo and recovery journal. Programmatic export requires an explicit destination. `track.bounce_preview` uses the same planner without rendering audio, opening processors or creating output files.

Track > Bounce…, Cmd+Alt+Shift+B and the track-header Bounce… item open the delivery dialog. A header action targets that track; menu and shortcut actions snapshot the selected tracks and time range. The dialog shows planned destinations, channel counts, delivered frame counts and any Instrument warm-up. The preview manifest also exposes total processed frames. A changed document invalidates its preview, and a replaced document closes it.

## Sources, ranges and processing

Active Audio and built-in Instrument tracks are supported when record arm and input monitoring are off. Exact declared source width is retained with Render Pan off; Render Pan on uses the existing routing matrix for the selected track's Main or internal Bus output width. Unsupported or ambiguous widths/routes are refused. Sends, downstream Aux/Bus/Master processing, metronome and live inputs are excluded.

Render Volume controls fader, readable volume/trim automation and referenced VCA gain. Phase inversion, static trim and applicable mute policy still apply. Render Pan controls the existing track pan policy. Other tracks' solo state does not change the selected source's export.

Explicit start/end positions take priority over the timeline selection. With no selection, source content determines the output range. Extra tail extends delivery after source input stops; no later source clip or note may enter that tail. Track inserts and instrument release continue against silence. Per-clip effect tails do not independently continue after source cutoff.

Instrument rendering prepares fresh state and processes earlier material needed at selection start before discarding the preroll. Preview exposes that processing cost. Audio effects start cold at selection start. This feature does not reconstruct earlier Audio effect history.

Only selected strip processors are instantiated. Active hosted plugins, unsupported MIDI controllers/stretch, lookahead automation and latency-producing effects are refused before delivery. Bypassed or inactive slots are not instantiated. Rendering is synchronous; progress reporting and cancellation are separate remaining work.

## Files and publication

A native single-track export uses `path`; a native batch uses an existing `dir` and sanitized track names with stable IDs. Extensions must match the requested format. Existing files, symlinks and collisions detected after preview are refused. Directories are not created automatically.

Native destination checks and publication use the guarded I/O layer. Automation write roots
apply before destination probes, staging writes and publication.

Native publication currently requires Unix and a hard-link-capable destination. Output is encoded to an exclusively created staging file, flushed, then published with an exclusive hard link. Cleanup checks device/inode identity before unlinking; externally replaced paths are retained and reported. These checks do not provide an atomic hostile-concurrency guarantee or an atomic whole-batch transaction. Unsupported publication fails without an overwrite fallback.

The browser path supports one named file through the existing download hook. Compilation and offscreen controls do not prove a live browser download. Windows native publication is unsupported.

WAV/AIFF support 16/24-bit integer and 32-bit float; FLAC supports 16/24-bit integer within its existing channel/rate limits. Rendering stays at session sample rate. Integer dither is optional. This feature does not change codec headroom policy or add sample-rate conversion. The existing
Float32 encoder still clamps over-range samples.

## Bounds and command parameters

Source, plugin, timing, shape and routing metadata checks, including finite-number validation, precede rendering and encoding. Limits include 128 requested tracks, 10,000 active-playlist clips per source, bounded automation/gain metadata, 128 Instrument notes per source and a checked 512 MiB aggregate processing estimate. Instrument preroll is charged even when discarded. Workspace estimates are conservative policies, not physical-memory or realtime deadline guarantees.

```json
{
  "tracks": [1], "path": "/existing/folder/Voice.wav",
  "format": "wav", "bit_depth": "24", "dither": true,
  "render_volume": true, "render_pan": false,
  "start": 48000, "end": 96000, "tail_seconds": 0.5,
  "expected_revision": 7
}
```

Use the same parameters with `track.bounce_preview` to inspect the plan. Export and preview do not enter Undo or the recovery journal.

## Remaining capabilities

Aux/Master and hosted processing, latency-producing effects, real-time/hardware paths, multiple output paths, Windows native delivery, grouped mono files, sample-rate conversion, additional formats, naming presets, import-after-bounce and progress/cancellation remain open. Tests use original synthetic SoundCraft fixtures; they do not establish output equivalence to another DAW.

## Revision history

- 2026-10-13: Refresh the focused Track Bounce contribution on current upstream main, use safe preroll arithmetic and document SoundCraft behavior without manual citations or stale validation claims.
