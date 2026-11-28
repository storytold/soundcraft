---
title: Bounce your mix to a file
description: Export your SoundCraft session as a WAV, AIFF or FLAC file with File > Bounce Mix.
order: 40
verified: 0.7.0
---

Choose **File > Bounce Mix...**, set the file name, format and bit depth, and click **Bounce**. SoundCraft bounces the edit selection, or the whole session when nothing is selected.

## Steps

1. Optional: select the range you want to export. Leave nothing selected to bounce the whole session.
2. Choose **File > Bounce Mix...**. The **Bounce Mix** window opens.
3. In **File**, type the full path of the file to write, or click **Browse…**. The window suggests a name ending in `Bounce.wav` in the session's folder.
4. Pick a **Format**: WAV, AIFF or FLAC.
5. Pick a **Bit Depth**: 16-bit, 24-bit or 32f-bit. The window starts at 24-bit.
6. Tick **Normalize to -0.1 dBFS** if you want the bounce normalized to that peak level.
7. Click **Bounce**.

SoundCraft sets the file extension to match the format you chose.

## Other formats

Bounce writes WAV, AIFF and FLAC. MP3 and AAC export are not available yet.

## Bounce without the window

SoundCraft also ships a command-line tool, `soundcraft-cli`. Its `--bounce` option writes a file without opening the app. On Windows, `soundcraft-cli.exe` comes with the installer and the portable zip, next to `soundcraft.exe`. The Linux tarball has it in its `bin` folder. For macOS it is a separate download, `soundcraft-cli-<version>-macos-universal.zip`. The README shows an example.
