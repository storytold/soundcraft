# Batch track naming

**Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13

Select tracks, then choose **Batch Rename Tracks** from the Track menu or the track context menu
in Edit, Mix or Track List. The shortcut is **Option+Shift+R** on macOS and **Alt+Shift+R** elsewhere.
A context action on a selected track retains the selected group; on an unselected track it selects
that track alone. Numbering follows session track order, including explicit command targets.

Enable the stages you need. They run in this order:

1. **Replace:** literal Find/Replace, or clear the existing name.
2. **Trim:** remove Unicode scalar values from the beginning/end, including spaces; each count is 0–1024.
3. **Add:** insert prefix/suffix text.
4. **Numbering:** decimal numbers at the beginning/end; choose start, nonnegative increment,
   minimum digit width and a separator that is empty or one Unicode scalar. The separator is omitted
   when the existing text is empty.

Leading and trailing whitespace is trimmed after the stages. Review the old/new name preview and
click Apply. All final names must be nonempty and unique,
with at most 64 Unicode scalar values. Existing case-sensitive uniqueness rules are retained.
Names are rejected rather than truncated. Final-set validation permits swaps and cycles between
selected tracks, but a collision with an unselected track rejects the entire batch. Apply creates
one Undo step and changes only track names; IDs, routing, folders, groups, playlists and media stay intact.
Cancel, invalid requests, previews and no-op applications do not change the document or Undo history.
If the session changes while the dialog is open, reopen it to review current targets. Successful
New/Open/Close/Revert dismisses this dialog; failed operations and saves retain it.

Programmatic callers use `track.batch_rename_preview` and `track.batch_rename` with the same parameters:

```json
{"tracks":[12,7],"expected_revision":42,"replace":{"find":"Audio","with":"Drum"},"trim":{"beginning":0,"end":0},"add":{"prefix":"Live_","suffix":""},"numbering":{"position":"end","start":1,"increment":1,"places":2,"separator":"_"}}
```

Omit `tracks` to use the selected tracks, or pass a single `track` ID/name. Do not pass both.
Resolve names before applying; IDs avoid ambiguity in the existing case-insensitive lookup fallback.
Pass the preview's revision back as `expected_revision` to reject stale application. Preview returns
`valid`, `changed`, `revision` and rows containing `track`, `old`, `name` and `error`; it does not journal.
Apply validates again and returns the same result shape. Counts are strict integers; numbering uses
checked unsigned 64-bit arithmetic and 1–20 digit padding. At most 4096 targets, 256 UTF-8 bytes per
Find/replacement/prefix/suffix, and 1024 bytes per existing/intermediate name are accepted. Trimming
counts scalars rather than user-perceived grapheme clusters; combining marks can be separated.

This is a bounded track workflow. Range cropping, middle insertion, numbering at arbitrary positions,
alphabetic numbering, regex, presets, clip/file renaming, and Score/MIDI context menu entries remain
future work. It does not rename I/O buses or synchronize related bus labels.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Rebased the bounded preview/Apply workflow on current command and UI integration. |
| 2026-10-12 | Added atomic batch naming with preview, validation and one Undo step. |
