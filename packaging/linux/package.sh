#!/usr/bin/env bash
# Build and package SoundCraft for Linux (<arch> is x86_64 or aarch64, or riscv64 when cross-compiling):
#
#   $DIST/soundcraft-<version>-linux-<arch>.AppImage  any distro with glibc >= the build host's
#   $DIST/soundcraft-<version>-linux-<arch>.AppImage.zsync  delta updates (needs zsyncmake)
#   $DIST/soundcraft-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/soundcraft-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/soundcraft-<version>-linux-<arch>.tar.gz    plain FHS-style tree (bin/, share/)
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar"]
#
# Needs: cargo; nfpm for deb/rpm (https://nfpm.goreleaser.com); appimagetool for the AppImage
# (downloaded into $CARGO_TARGET_DIR if missing). Build on an old distro (CI: Ubuntu 22.04,
# glibc 2.35) so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli,
# zsyncmake (the zsync package) for the AppImage's .zsync.
#
# Cross-compiling (CI's linux-riscv64 job, tar.gz only): set CROSS_ARCH (riscv64), CROSS_TARGET
# (the Rust target), CROSS_COMPILE (the binutils prefix, for strip) and optionally EMULATOR
# (e.g. qemu-riscv64) to smoke-test the CLI and run the example that writes its man pages and
# completions.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=ai.storyteller.soundcraft

SKIP_BUILD=0
FORMATS="appimage deb rpm tar"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  riscv64) DEB_ARCH=riscv64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export SOUNDCRAFT_MAINTAINER="${SOUNDCRAFT_MAINTAINER:-SoundCraft maintainers <soundcraft@storyteller.ai>}"
BASENAME="soundcraft-$VERSION-linux-$ARCH"

echo "==> SoundCraft $VERSION for Linux $ARCH ($FORMATS)"

# The man-and-completions example is built with the binaries: it writes soundcraft-cli's man pages
# and shell completions below.
if [[ "$SKIP_BUILD" = 0 ]]; then
  if [[ -n "${CROSS_TARGET:-}" ]]; then
    (cd "$ROOT" && cargo build --release --locked -p soundcraft -p soundcraft-cli \
      --bins --example man-and-completions --target "$CROSS_TARGET")
  else
    (cd "$ROOT" && cargo build --release --locked -p soundcraft -p soundcraft-cli \
      --bins --example man-and-completions)
  fi
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/soundcraft" "$STAGE/usr/bin/soundcraft"
install -Dm755 "$BIN/soundcraft-cli" "$STAGE/usr/bin/soundcraft-cli"
"${CROSS_COMPILE:-}strip" "$STAGE/usr/bin/soundcraft" "$STAGE/usr/bin/soundcraft-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$SOUNDCRAFT_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/soundcraft"
copy_docs "$STAGE/usr/share/doc/soundcraft"
# soundcraft-cli's man pages and shell completions, from its clap definition, written by the
# example built above. A cross build runs it through $EMULATOR, like the --version check at the
# end, and leaves them out without one. zsh reads site-functions on Fedora and openSUSE; the deb
# also links the completion into Debian's vendor-completions (nfpm.yaml).
CLI_DOCS="$BIN/examples/man-and-completions"
if [[ -n "${CROSS_TARGET:-}" ]] && [[ -z "${EMULATOR:-}" ]]; then
  warn "no EMULATOR to run the $CROSS_TARGET build, so soundcraft-cli's man pages and completions are left out"
else
  if [[ ! -x "$CLI_DOCS" ]]; then
    echo "error: $CLI_DOCS is missing: build it with the binaries (without --skip-build)" >&2
    exit 1
  fi
  if [[ -n "${CROSS_TARGET:-}" ]]; then
    "$EMULATOR" "$CLI_DOCS" "$WORK/cli-docs"
  else
    "$CLI_DOCS" "$WORK/cli-docs"
  fi
  install -Dm644 -t "$STAGE/usr/share/man/man1" "$WORK"/cli-docs/*.1
  install -Dm644 "$WORK/cli-docs/soundcraft-cli.bash" "$STAGE/usr/share/bash-completion/completions/soundcraft-cli"
  install -Dm644 "$WORK/cli-docs/_soundcraft-cli" "$STAGE/usr/share/zsh/site-functions/_soundcraft-cli"
  install -Dm644 "$WORK/cli-docs/soundcraft-cli.fish" "$STAGE/usr/share/fish/vendor_completions.d/soundcraft-cli.fish"
fi

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() {
  local format="$1"
  case " $FORMATS " in *" $format "*) return 0 ;; *) return 1 ;; esac
}

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  # Without the completions (a cross build with no EMULATOR) the deb's zsh symlink would dangle,
  # so its entry is dropped too.
  DROP_ZSH_LINK=
  [[ -f "$STAGE/usr/share/zsh/site-functions/_soundcraft-cli" ]] || DROP_ZSH_LINK='\|^  - src: /usr/share/zsh/site-functions/|,/packager: deb/d;'
  sed "${DROP_ZSH_LINK}s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPIMAGETOOL_VERSION=1.9.1
  case "$ARCH" in
    x86_64) APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
    aarch64) APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
    *) APPIMAGETOOL_SHA256='' ;;
  esac
  # appimagetool otherwise fetches the unpinned type2-runtime `continuous` build and embeds it.
  APPIMAGE_RUNTIME_VERSION=20251108
  case "$ARCH" in
    x86_64) APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64) APPIMAGE_RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
    *) APPIMAGE_RUNTIME_SHA256='' ;;
  esac
  if [[ -z "$APPIMAGE_RUNTIME_SHA256" ]]; then
    echo "no pinned AppImage runtime for $ARCH; build the AppImage on x86_64 or aarch64" >&2
    exit 2
  fi
  APPDIR="$WORK/SoundCraft.AppDir"
  cp -R "$STAGE" "$APPDIR"
  ln -s usr/bin/soundcraft "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  verify_sha256() {
    local sha="$1" file="$2"
    printf '%s  %s\n' "$sha" "$file" | sha256sum -c -
  }
  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [[ -z "$TOOL" ]]; then
    if [[ -z "$APPIMAGETOOL_SHA256" ]]; then
      echo "no pinned appimagetool for $ARCH; set APPIMAGETOOL or put appimagetool on PATH" >&2
      exit 2
    fi
    TOOL="$CARGO_TARGET_DIR/appimagetool-$APPIMAGETOOL_VERSION-$ARCH.AppImage"
    if [[ ! -x "$TOOL" ]]; then
      download="$TOOL.download"
      curl --proto '=https' -fsSL -o "$download" \
        "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$ARCH.AppImage"
      verify_sha256 "$APPIMAGETOOL_SHA256" "$download"
      chmod 755 "$download"
      mv "$download" "$TOOL"
    fi
    verify_sha256 "$APPIMAGETOOL_SHA256" "$TOOL"
  fi
  RUNTIME="$CARGO_TARGET_DIR/appimage-runtime-$APPIMAGE_RUNTIME_VERSION-$ARCH"
  if [[ ! -f "$RUNTIME" ]]; then
    download="$RUNTIME.download"
    curl --proto '=https' -fsSL -o "$download" \
      "https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_VERSION/runtime-$ARCH"
    verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$download"
    mv "$download" "$RUNTIME"
  fi
  verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$RUNTIME"
  # Absolute, because appimagetool runs in $DIST below (CARGO_TARGET_DIR or APPIMAGETOOL may be
  # relative).
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  RUNTIME="$(cd "$(dirname "$RUNTIME")" && pwd)/$(basename "$RUNTIME")"
  # A .zsync left by an earlier run would hide a missing zsyncmake and describe another file.
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate, AppImageLauncher and the like read it from the file and
  # fetch only the blocks that changed in a newer release, through the .zsync published next to
  # each AppImage on GitHub Releases. `latest` is the newest published release that is not a
  # pre-release. A fork's builds point at its own releases through GITHUB_REPOSITORY.
  REPO="${GITHUB_REPOSITORY:-storytold/soundcraft}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|soundcraft-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either. With zsyncmake on the host (CI installs the zsync
  # package) appimagetool also writes the .zsync, into its working directory, hence the cd.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream --runtime-file "$RUNTIME" -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [[ -s "$OUT.zsync" ]]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

if [[ -z "${CROSS_TARGET:-}" ]]; then
  "$STAGE/usr/bin/soundcraft-cli" --version
elif [[ -n "${EMULATOR:-}" ]]; then
  "$EMULATOR" "$STAGE/usr/bin/soundcraft-cli" --version
fi
echo "==> done"
ls -lh "$DIST"
