---
title: Report a problem
description: What to include when you report a SoundCraft bug, and where to post it.
order: 20
verified: 0.7.0
---

Open an issue in the [SoundCraft issue tracker](https://github.com/storytold/soundcraft/issues), or ask in [Discord](https://discord.gg/artcraft). Attach the log file.

## What to include

- Your SoundCraft version and your system, for example Windows 11 x64.
- What you did, step by step, and what you expected.
- What happened instead, with the error message if there was one.
- The log file (below).

## The log file

SoundCraft writes its log to `logs/soundcraft.log` in its settings folder. On Windows that is `%APPDATA%\SoundCraft\logs\soundcraft.log`. On macOS it is `~/Library/Application Support/SoundCraft/logs/` and on Linux `~/.config/soundcraft/logs/`.

The log records crash reports, audio devices that failed to open, plugins that refused their saved state and files that failed to open. Each start moves the previous log to `soundcraft.1.log`, so if SoundCraft crashed, attach `soundcraft.1.log` after you start it again.

## Before you post

Search the [open issues](https://github.com/storytold/soundcraft/issues) first. Your problem may already be there, and a comment with your details helps.
