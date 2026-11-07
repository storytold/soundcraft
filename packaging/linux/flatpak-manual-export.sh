#!/usr/bin/env bash
# Manual Flatpak export fallback for environments where flatpak-builder cannot run
# (for example inside the RustRover Flatpak sandbox, where bwrap cannot create user
# namespaces and the org.freedesktop.Flatpak portal is unreachable).
#
# This reproduces the flatpak-builder build of packaging/linux/flatpak/*.yml without a
# sandbox: build the binaries natively, lay them out as an /app tree, finalize and export
# the app into a local OSTree repository, then bundle it as a single .flatpak file.
#
# It is the sandbox fallback, not the release path: release bundles come from
# packaging/linux/flatpak-bundle.sh and Flathub builds from the from-source manifest.
#
# Usage: packaging/linux/flatpak-manual-export.sh
#
# Requirements: cargo (or ~/.cargo/bin/cargo), flatpak (or the host flatpak at
# /run/host/usr/bin/flatpak, run through the host musl loader), and the freedesktop
# runtime/sdk matching RUNTIME_VERSION installed per-user from Flathub.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$ROOT"

APP_ID=ai.storyteller.soundcraft
RUNTIME_VERSION=26.08
FLATHUB=https://dl.flathub.org/repo/flathub.flatpakrepo
WORK="$ROOT/build-flatpak"
STAGE="$WORK/manual-stage"
REPO="$WORK/manual-repo"
OUT="$ROOT/dist/soundcraft-flatpak-x86_64.flatpak"

die() { echo "error: $*" >&2; exit 1; }

VERSION=$(awk '/^\[workspace.package\]/{p=1;next} /^\[/{if(p)exit} p&&$1=="version"{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)
[ -n "$VERSION" ] || die "cannot read [workspace.package] version from Cargo.toml"
DATE=$(date -u +%Y-%m-%d)

# --- flatpak command: the real one, or the host's via the host musl loader -----------
SHIMDIR=""
cleanup() { [ -n "$SHIMDIR" ] && rm -rf "$SHIMDIR"; }
trap cleanup EXIT

FLATPAK=()
if command -v flatpak >/dev/null 2>&1; then
  FLATPAK=(flatpak)
else
  HOST_FLATPAK=/run/host/usr/bin/flatpak
  HOST_LOADER=/run/host/usr/lib64/libc.so
  HOST_LIBPATH=/run/host/lib:/run/host/usr/lib:/run/host/usr/lib64:/run/host/lib64
  [ -x "$HOST_FLATPAK" ] || die "flatpak not found in PATH and no host flatpak at $HOST_FLATPAK"
  [ -x "$HOST_LOADER" ] || die "host musl loader not found at $HOST_LOADER"
  SHIMDIR=$(mktemp -d /tmp/fpx.XXXXXX)
  /bin/cp -f "$HOST_FLATPAK" "$SHIMDIR/flatpak"
  chmod +x "$SHIMDIR/flatpak"
  # build-export spawns /usr/libexec/flatpak-validate-icon, which does not exist inside
  # the sandbox. Point a byte-for-byte copy of the host flatpak at a script that runs the
  # host validator through the host loader. The replacement path must keep the byte count.
  if [ ! -x /usr/libexec/flatpak-validate-icon ]; then
    HELPER="$SHIMDIR/v"
    while [ "${#HELPER}" -lt 34 ]; do HELPER="$HELPER-"; done
    [ "${#HELPER}" -eq 34 ] || die "temporary path too long for icon-validator patch"
    python3 - "$SHIMDIR/flatpak" "$HELPER" <<'PY'
import sys
binary, helper = sys.argv[1], sys.argv[2]
old = b"/usr/libexec/flatpak-validate-icon\x00"
new = helper.encode() + b"\x00"
if len(old) != len(new):
    raise SystemExit("replacement path must be 34 bytes, got %d" % (len(new) - 1))
with open(binary, "r+b") as f:
    data = f.read()
    if data.count(old) != 1:
        raise SystemExit("validator path not found exactly once")
    f.seek(0)
    f.write(data.replace(old, new))
PY
    cat > "$HELPER" <<'SH'
#!/bin/sh
# Host icon validator through the host musl loader; --sandbox needs bwrap, which cannot
# create user namespaces inside the RustRover Flatpak, so drop that flag.
args=""
for a in "$@"; do
  [ "$a" = "--sandbox" ] && continue
  args="$args $a"
done
exec /run/host/usr/lib64/libc.so --library-path /run/host/lib:/run/host/usr/lib:/run/host/usr/lib64:/run/host/lib64 /run/host/usr/libexec/flatpak-validate-icon $args
SH
    chmod +x "$HELPER"
  fi
  FLATPAK=("$HOST_LOADER" --library-path "$HOST_LIBPATH" "$SHIMDIR/flatpak")
fi

# --- A. native release build ---------------------------------------------------------
echo "==> SoundCraft $VERSION: release build"
PATH="$HOME/.cargo/bin:$PATH" cargo build --release --locked -p soundcraft -p soundcraft-cli
if [ ! -x target/release/soundcraft ] || [ ! -x target/release/soundcraft-cli ]; then
  die "build produced no binaries"
fi

# --- B. /app layout ------------------------------------------------------------------
echo "==> staging /app tree in $STAGE"
rm -rf "$STAGE" "$REPO"
mkdir -p "$STAGE/files/bin" \
  "$STAGE/files/extensions/Plugins" \
  "$STAGE/files/share/applications" \
  "$STAGE/files/share/mime/packages" \
  "$STAGE/files/share/metainfo" \
  "$STAGE/files/share/doc/soundcraft"
install -Dm755 target/release/soundcraft "$STAGE/files/bin/soundcraft"
install -Dm755 target/release/soundcraft-cli "$STAGE/files/bin/soundcraft-cli"
install -Dm644 packaging/linux/ai.storyteller.soundcraft.desktop \
  "$STAGE/files/share/applications/$APP_ID.desktop"
install -Dm644 packaging/linux/ai.storyteller.soundcraft.mime.xml \
  "$STAGE/files/share/mime/packages/$APP_ID.xml"
install -Dm644 NOTICE "$STAGE/files/share/doc/soundcraft/NOTICE"
install -d "$STAGE/files/share/icons"
cp -a assets/app-icon/hicolor "$STAGE/files/share/icons/"
sed -e "s/@VERSION@/$VERSION/" -e "s/@DATE@/$DATE/" \
  packaging/linux/ai.storyteller.soundcraft.metainfo.xml.in \
  > "$STAGE/files/share/metainfo/$APP_ID.metainfo.xml"

# --- C. metadata ---------------------------------------------------------------------
# Matches the manifest's finish-args. The runtime/sdk are the ones this binary was built
# against (the sandbox SDK), not the 25.08 named in the committed manifest.
cat > "$STAGE/metadata" <<META
[Application]
name=$APP_ID
runtime=org.freedesktop.Platform/x86_64/$RUNTIME_VERSION
sdk=org.freedesktop.Sdk/x86_64/$RUNTIME_VERSION
command=soundcraft

[Context]
shared=network;ipc;
sockets=wayland;fallback-x11;pulseaudio;
devices=dri;
filesystems=xdg-music;~/.vst3:ro;~/.clap:ro;

[Environment]
VST3_PATH=/app/extensions/Plugins/vst3
CLAP_PATH=/app/extensions/Plugins/clap

[Extension org.freedesktop.LinuxAudio.Plugins]
directory=extensions/Plugins
version=$RUNTIME_VERSION
add-ld-path=lib
merge-dirs=vst3;clap
subdirectories=true
no-autodownload=true
META

# build-export refuses an unfinalized build dir; --no-exports keeps the real
# share/icons/hicolor layout in files/ (without it build-finish flattens the theme).
echo "==> finalizing metadata"
"${FLATPAK[@]}" build-finish --no-exports \
  --command=soundcraft \
  --runtime="org.freedesktop.Platform/x86_64/$RUNTIME_VERSION" \
  --sdk="org.freedesktop.Sdk/x86_64/$RUNTIME_VERSION" \
  --share=network --share=ipc \
  --socket=wayland --socket=fallback-x11 --socket=pulseaudio \
  --device=dri \
  --filesystem=xdg-music --filesystem='~/.vst3:ro' --filesystem='~/.clap:ro' \
  --env=VST3_PATH=/app/extensions/Plugins/vst3 \
  --env=CLAP_PATH=/app/extensions/Plugins/clap \
  "$STAGE" >/dev/null

# Export metadata for desktop integration. Icons are regular PNGs (build-export's icon
# validator does not read the scalable SVG here); the SVG still ships in files/ for the app.
mkdir -p "$STAGE/export/share/applications" "$STAGE/export/share/metainfo" \
  "$STAGE/export/share/icons/hicolor"
ln -sf "../../../files/share/applications/$APP_ID.desktop" \
  "$STAGE/export/share/applications/$APP_ID.desktop"
ln -sf "../../../files/share/metainfo/$APP_ID.metainfo.xml" \
  "$STAGE/export/share/metainfo/$APP_ID.metainfo.xml"
for png in "$STAGE"/files/share/icons/hicolor/*/apps/*.png; do
  rel=${png#"$STAGE/files/share/icons/hicolor/"}
  install -Dm644 "$png" "$STAGE/export/share/icons/hicolor/$rel"
done

# --- D. export and bundle ------------------------------------------------------------
# Note: build-export takes LOCATION DIRECTORY [BRANCH]; the app id comes from metadata.
echo "==> exporting to $REPO"
"${FLATPAK[@]}" build-export --arch=x86_64 "$REPO" "$STAGE" stable

mkdir -p "$ROOT/dist"
echo "==> bundling $OUT"
"${FLATPAK[@]}" build-bundle --runtime-repo="$FLATHUB" \
  "$REPO" "$OUT" "$APP_ID" stable

echo "==> wrote $OUT"
ls -lh "$OUT"
