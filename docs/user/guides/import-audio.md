---
title: Import audio
description: Bring WAV, AIFF, FLAC, MP3 and other audio files into a SoundCraft session.
order: 20
verified: 0.7.0
---

Choose **File > Import > Audio...**, pick the file and click **OK**. SoundCraft puts the audio on a new track.

## Steps

1. Open or create a session. See [Create a session and add tracks](create-a-session-and-add-tracks.md).
2. Choose **File > Import > Audio...**. The **Import Audio** window opens.
3. Type the path of the file, or click **Browse…** and pick it.
4. Click **OK**.

By default each imported file gets its own new track, and the clip starts at the beginning of the session.

## Which files work

| Reads                                                  | Notes                                       |
|--------------------------------------------------------|---------------------------------------------|
| WAV (including BWF and RF64), AIFF, AIFF-C, FLAC       | Also written by Bounce.                     |
| MP3, AAC and M4A, ALAC, Ogg Vorbis, CAF                | Read only.                                  |
| The audio of MP4 and MOV movies                        | Read only.                                  |

To bring in a MIDI file, choose **File > Import > MIDI...** instead.

## If it does not import

SoundCraft shows an error message instead of crashing when a file cannot be read. If a file that should work fails, report it. See [Report a problem](../troubleshooting/file-a-bug.md).
