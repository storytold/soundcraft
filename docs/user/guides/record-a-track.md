---
title: Record a track
description: Record-enable an audio track in SoundCraft, pick its input, and record. What happens to a take if SoundCraft crashes.
order: 30
verified: 0.7.0
---

Click the record-enable button in the track header, then press the Record button or `Ctrl+Space` (`Cmd+Space` on macOS). Press Stop (`Space`) to finish.

## Steps

1. Add an audio track if you have none. See [Create a session and add tracks](create-a-session-and-add-tracks.md).
2. In the track's header, click the dark red button with a dot. Its tooltip is **Record enable**, and it lights up when the track is armed. With the track selected, `Shift+R` does the same.
3. Optional: click the **I** button in the header to hear the input through the track (**Input monitoring**).
4. Press `Ctrl+Space` (`Cmd+Space` on macOS), or click the Record button in the transport controls.
5. Play or sing. SoundCraft records from the current playhead.
6. Press `Space` to stop. The status line reports how many clips were recorded.

If no track is record-enabled, SoundCraft does not record. The status line says: No track is record-enabled: click the R button on the track to record. If the session has no tracks yet, it says: Nothing to record on yet: add a track with Track › New…, then click its R button.

## If SoundCraft crashes while recording

SoundCraft writes each take to a WAV file in the session's `Audio Files` folder while you record. Until you first save the session, the takes go to the `Autosave` folder in SoundCraft's settings folder.

The next time you start SoundCraft, the **Recover Unsaved Work** window says the session has an unfinished recording. Click **Recover** to put the takes back on their tracks, then save the session. Recovery fixes each take file so it includes all the audio that reached the disk. The last moments before the crash may still be missing.

If you open a take file in another program without recovering it first, up to the last second of it may be missing, because SoundCraft updates the file's length only about once a second while it records.

## Punch in and loop recording

- Select a time range first and SoundCraft records into that selection and trims the take back to it.
- Turn on **Options > Pre/Post-Roll** to start recording a little before the selection and run on after it.
- Turn on **Options > Loop Record** with a selection to record the range over and over, one take each pass.

## Which input is used

SoundCraft records from the **Input device** chosen in **Setup > Playback Engine...**, which is your system's default input unless you pick another one. See [Can I choose my audio interface?](../faq/choose-audio-interface.md).

Each track records from the input shown on its channel strip in the Mix window, **In 1** unless you change it. Click it and pick **In 1** to **In 8** to record from another channel of the device. A stereo track records that channel and the next one.

## If nothing is recorded

The status line shows **Cannot record:** followed by the reason. Include it when you [report a problem](../troubleshooting/file-a-bug.md).
