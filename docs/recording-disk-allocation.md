# Recording disk allocation

Last updated: 2026-10-13.

`setup.disk_allocation` chooses where new audio takes are written. A per-track setting
overrides the session setting. Without either setting, takes use `Audio Files` under the
session folder (or the recovery folder for an unsaved session).

```json
{"command":"setup.disk_allocation","params":{"path":"Recorded Audio"}}
{"command":"setup.disk_allocation","params":{"track":2,"path":"/Volumes/Recording/Takes"}}
{"command":"setup.disk_allocation","params":{"track":2,"path":null}}
```

Relative paths resolve against the session folder. Absolute paths name an existing
directory, such as a folder selected on another recording volume. SoundCraft refuses the
recording plan if an armed track's absolute destination is unavailable. It does not silently
redirect that track to the session folder. File creation and disk-write failures are reported
by the take writer. Resetting a track's allocation with `null` restores the session setting;
resetting the session setting restores `Audio Files`.

Paths retain their exact spelling, including spaces, and are saved with the session. The
settings command rejects empty, NUL-containing, non-string, or over-32768-byte paths rather
than silently shortening them. Existing take files are never overwritten. Normal session
saving retains its existing media-gathering behavior; allocation controls new take locations.

This connects the existing command to capture planning. It does not add automatic
round-robin volume selection, volume roles, free-space reservation, or a dedicated allocation
editor. Automated tests exercise path precedence, persistence, synthetic take import, reset,
and refusal of an unavailable folder. Physical recording across two volumes and full-volume
behavior require hardware and filesystem runtime checks.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Document destination precedence, path validation and remaining runtime checks. |
| 2026-10-12 | Connect persisted session/track allocations to take planning, reject unavailable absolute folders, and document remaining volume-management work. |
