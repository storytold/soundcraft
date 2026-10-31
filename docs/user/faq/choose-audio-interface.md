---
title: Can I choose my audio interface?
description: Yes. Pick the audio system, output and input device, sample rate and buffer size in Setup > Playback Engine. The downloads have no ASIO support.
order: 30
verified: 0.7.0
---

Yes. Choose **Setup > Playback Engine...**, pick your interface and click **Apply**. The downloads from the release page do not include ASIO support on Windows.

## Pick your interface

1. Choose **Setup > Playback Engine...**.
2. Pick the **Audio system**, then the **Output device** and **Input device**. Each list starts with **Default**, which follows your system's sound settings.
3. Optional: pick a **Sample rate** and a **Buffer size**. **Device default** leaves them to the device.
4. Click **Apply**. SoundCraft reopens the audio devices with the new settings.

If you plug in an interface while the window is open, click **Refresh devices**. **Revert** undoes changes you have not applied yet. The lower part of the window shows the output and input that are open right now. The input is opened the first time you record or monitor.

The choice is saved for this computer, not in the session. If the device is unplugged later, or does not offer the rate or buffer size you picked, SoundCraft falls back to the system default.

## ASIO on Windows

ASIO can only be turned on by building SoundCraft yourself, because it needs Steinberg's ASIO SDK. On Windows the downloads use WASAPI. A request for an audio driver option is open as [issue 213](https://github.com/storytold/soundcraft/issues/213).
