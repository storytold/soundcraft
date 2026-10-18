# Reusable clip definitions

`clip.capture` stores detached copies of complete Audio, MIDI, or Video clips in the session.
It leaves the original timeline clips and their names unchanged. Definitions can be listed,
placed repeatedly, saved, and opened even after their original timeline instances are deleted.

## Commands

| Command | Parameters | Result |
|---|---|---|
| `clip.capture` | `{clip?: id, clips?: [id], name?: string}`; omitted targets use the edit selection | Capture one definition per complete clip. |
| `clip.definitions` | `{}` | List the stored definitions without changing the document or undo history. |
| `clip.place_definition` | `{definition: id, track?: id\|name, at?: position}` | Place a fresh instance; the default destination is a new compatible track. |
| `clip.clear_definitions` | `{definitions: [id]}` | Remove definitions while keeping already placed clips and their media. |

Positions accept the usual samples, seconds, or time-string forms described in
[the control protocol](control-protocol.md). Omitted `at` uses the selection start.
The existing Capture menu command and shortcut use `clip.capture`. The Clip List's Reusable
section shows saved definitions with their Audio, MIDI, or Video type, sharing the existing
case-insensitive filter. Right-click a row to place it on a new track, place it on the selected
compatible track, or remove that definition. Placement uses the selection start. Removing a
definition keeps placed clips and media; Undo restores it. All commands also remain available
through the command API. Definition drag-and-drop is outside this increment.

Explicit `clip` or `clips` targets capture their entire contents, independently of the edit range.
Selection-driven capture uses selected clips, or intersecting clips on selected tracks. A nonempty
range that cuts through a selected clip is rejected instead of silently widening the selection.
Capturing a partial range and placing an instance over another clip are unsupported.

## Independent instances

A definition retains its media reference or MIDI sequence, source offset, sample length, clip gain,
gain envelope, fades including curvature, mute, rating, sync point, color, and stretch.
Recognized clip-effect settings, effect bypass, elastic original references, and saved MIDI
performance are retained and rebound to the new clip ID on placement. Arbitrary edit-value keys
are not copied.

Each placement allocates a fresh clip ID. Editing the original clip, a definition's placed instance,
or another instance does not modify the other snapshots. Group and loop links and edit/time locks
are cleared on the new instance. Audio definitions place on Audio tracks, MIDI definitions on MIDI
or Instrument tracks, and Video definitions on Video tracks. Incompatible destinations and overlaps
return errors before changing the document, including overlaps with locked clips.

Lengths remain sample based. MIDI events retain their sequence ticks and follow the destination
tempo within that fixed sample window, matching existing clip movement. Placement does not
convert the stored window into a musical duration. Audio placement requires a valid decoded
source buffer; an offline audio definition can still be saved and listed. Video definitions retain
declared video sources without requiring a movie decoder during placement.

## Persistence and history

Sessions with definitions write format version 2. New ordinary sessions without definitions retain
format version 1. Readers that only understand version 1 reject version 2 rather than discarding
the definition store. Invalid definitions are rejected before ordinary session sanitization,
media decoding, or save-side media writes.

Save, Save As, Save Copy, bundles, and autosave retain declared media used only by definitions.
Bundles package audio; movies retain their existing path references, without adding video-file
portability in this increment.
`clip.clear` refuses to remove an audio source referenced by a definition, including a saved
elastic original, even when `with_clips` is enabled. Remove the definition explicitly first.
Clearing definitions does not delete media or timeline instances. Session templates clear the
definition store along with their existing media-free timeline cleanup.

Capture, placement, and definition removal each use a single undo step. Invalid requests leave
the document, selection, dirty state, and redo history unchanged. Stored names, IDs, positions,
metadata, and cloned content have validation and allocation limits.

## Verification

Ordinary Model and Engine regression tests cover independent Audio/MIDI/Video instances,
metadata rebinding, invalid requests, session-format compatibility, media ownership, save and
bundle round trips, autosave recovery, and template cleanup. Widget tests exercise filtering,
real placement/removal clicks, and keyboard Undo. PCM comparisons use synthesized
audio and the existing mix renderer. No installed audio plugins or external media are fixtures.
