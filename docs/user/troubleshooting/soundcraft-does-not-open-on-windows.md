---
title: SoundCraft does not open on Windows
description: What to try when SoundCraft flashes in Task Manager, closes right after starting or never shows a window on Windows 11.
order: 10
verified: 0.7.0
---

Install the [latest release](https://github.com/storytold/soundcraft/releases/latest): version 0.6.1 fixes the most common cause, a crash with `Parent device is lost` right after the window appears. If it still does not open, start it with the Vulkan graphics backend as shown below.

## Symptoms

- You start SoundCraft and nothing appears, or it shows in Task Manager for a moment and disappears.
- The window flashes and SoundCraft exits about a tenth of a second later.
- The log file ends with `Parent device is lost`.

## Fix

1. Check which version you have. For the installer, find SoundCraft in **Settings > Apps > Installed apps**. For the portable zip, the version is in the zip's file name. If it is older than 0.6.1, install the latest release. See [Install SoundCraft](../getting-started/install.md).
2. If 0.6.1 or newer still does not open, open PowerShell.
3. Go to the folder that contains `soundcraft.exe`. If you used the installer, that is `C:\Program Files\SoundCraft` (`C:\Program Files (x86)\SoundCraft` for the x86 installer). If you used the portable zip, it is the folder you unzipped.
4. Run these lines:

        $env:WGPU_BACKEND = "vulkan"
        & .\soundcraft.exe
        Remove-Item Env:WGPU_BACKEND

5. If SoundCraft opens, you can keep starting it this way. The last line restores the default for later launches in that window.

On Intel graphics (UHD, Iris), use `gl` in place of `vulkan`. Intel's Vulkan driver (`igvk64.dll`) crashed other Craft apps at start. `gl` is reported to work on Intel graphics in other Craft apps.

On AMD graphics do not set `WGPU_BACKEND` to `gl`: it can bring back the `atio6axx.dll` driver crash.

## Why this happens

On Windows, SoundCraft draws with DirectX 12 only. It leaves OpenGL out on purpose, because starting OpenGL can crash some graphics drivers (the README names AMD's `atio6axx.dll`) before the window appears.

Version 0.5.0 asked DirectX 12 to use a shader compiler file called `dxcompiler.dll`, which SoundCraft does not ship. On some PCs this ended with `Parent device is lost` right after the window appeared ([issue 185](https://github.com/storytold/soundcraft/issues/185)). Since 0.6.1, SoundCraft uses FXC, the shader compiler that is part of Windows, so it no longer needs that file. If 0.6.1 still does not open, the cause is something else, which is why the Vulkan backend is worth a try.

## Find the log file

The log is `%APPDATA%\SoundCraft\logs\soundcraft.log`. Attach it when you report a problem.

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or see [Report a problem](file-a-bug.md). Include your SoundCraft version, your graphics card and the end of the log file.
