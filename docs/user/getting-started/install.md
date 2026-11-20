---
title: Install SoundCraft
description: Download SoundCraft for Windows 11, macOS or Linux from GitHub, or install it with ArtCraft Launcher.
order: 10
verified: 0.7.0
---

Download the file for your system from the [latest release](https://github.com/storytold/soundcraft/releases/latest) and install it. In the file names, `<version>` stands for the version number, for example `0.7.0`. The release also lists a checksum for every file in `SHA256SUMS.txt`.

## Windows 11

1. Open the [latest release](https://github.com/storytold/soundcraft/releases/latest).
2. Download the installer for your PC from the table below. Most PCs (Intel and AMD, 64-bit) need `soundcraft-<version>-windows-x64.msi`.
3. Run the file and follow the installer. It installs for all users, so Windows may ask for administrator approval.
4. Start **SoundCraft** from the Start menu.

| PC                                  | Installer                                | Portable zip                                      |
|-------------------------------------|------------------------------------------|---------------------------------------------------|
| x64: Intel and AMD, 64-bit          | `soundcraft-<version>-windows-x64.msi`   | `soundcraft-<version>-windows-x64-portable.zip`   |
| arm64: Snapdragon and other Arm PCs | `soundcraft-<version>-windows-arm64.msi` | `soundcraft-<version>-windows-arm64-portable.zip` |
| x86: 32-bit                         | `soundcraft-<version>-windows-x86.msi`   | `soundcraft-<version>-windows-x86-portable.zip`   |

To run SoundCraft without installing, download the portable zip instead, unzip it and run `soundcraft.exe`.

If the window never appears, see [SoundCraft does not open on Windows](../troubleshooting/soundcraft-does-not-open-on-windows.md).

## macOS

1. Download `soundcraft-<version>-macos-universal.dmg`. It works on Apple silicon and Intel Macs. It is signed and notarized.
2. Open it and drag SoundCraft to Applications.

## Linux

Pick one file from the release. Replace `x86_64` with `aarch64` on Arm.

| File                                         | Notes                                                        |
|----------------------------------------------|--------------------------------------------------------------|
| `soundcraft-<version>-linux-x86_64.AppImage` | Runs on most distributions. Make it executable, then run it. |
| `soundcraft-<version>-linux-x86_64.deb`      | Debian and Ubuntu.                                           |
| `soundcraft-<version>-linux-x86_64.rpm`      | Fedora, RHEL and openSUSE.                                   |
| `soundcraft-<version>-linux-x86_64.flatpak`  | Sandboxed. Install with `flatpak install --user <file>`.     |
| `soundcraft-<version>-linux-x86_64.tar.gz`   | Unpack anywhere.                                             |

For RISC-V, the release has `soundcraft-<version>-linux-riscv64.tar.gz`, `.deb` and `.rpm`. There is also a FreeBSD tarball (`soundcraft-<version>-freebsd-x86_64.tar.gz`).

## In a browser

`soundcraft-web-<version>.zip` is a static site that runs in a modern browser. Host it on any static web server.

## Install with ArtCraft Launcher

ArtCraft Launcher lists SoundCraft, so you can install and update it from there. See the [ArtCraft Launcher docs](https://getartcraft.com/docs/craft-launcher).

## Still stuck?

Ask in [Discord](https://discord.gg/artcraft) or open an issue in the [issue tracker](https://github.com/storytold/soundcraft/issues).
