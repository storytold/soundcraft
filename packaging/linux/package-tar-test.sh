#!/usr/bin/env bash
# package.sh's FHS tar without a build: a Nightly tar ships the launcher as bin/soundcraft and
# the GUI beside it as bin/soundcraft-bin, so an unpacked distro tarball (also the Flatpak's
# input) starts the private profile too; the stable tar keeps the bare GUI. Stub binaries stand in
# for the app: nothing is compiled or launched.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

export CARGO_TARGET_DIR="$work/target"
export DIST="$work/dist"
mkdir -p "$CARGO_TARGET_DIR/release/examples" "$DIST"
for stub in soundcraft soundcraft-cli; do
  printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$stub"
  chmod 755 "$CARGO_TARGET_DIR/release/$stub"
done
# The example that writes soundcraft-cli's man page and completions.
cat >"$CARGO_TARGET_DIR/release/examples/man-and-completions" <<'DOCS'
#!/bin/sh
set -eu
mkdir -p "$1"
printf '.TH SOUNDCRAFT-CLI 1\n' >"$1/soundcraft-cli.1"
printf '# bash completion\n' >"$1/soundcraft-cli.bash"
printf '#compdef _soundcraft-cli soundcraft-cli\n' >"$1/_soundcraft-cli"
printf '# fish completion\n' >"$1/soundcraft-cli.fish"
DOCS
chmod 755 "$CARGO_TARGET_DIR/release/examples/man-and-completions"

nightly=0.0.1-nightly.20261124.1.1.g0123456789ab
# package.sh labels the tar after the host architecture unless CROSS_ARCH says otherwise, so the
# fixture reads the same label instead of assuming x86_64.
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) printf "unsupported test architecture %s\n" "$(uname -m)" >&2; exit 1 ;;
esac
SOUNDCRAFT_BUILD_CHANNEL=nightly-v1 SOUNDCRAFT_VERSION="$nightly" \
  SOUNDCRAFT_BUILD_SHA=0123456789ab0123456789ab0123456789ab0123 SOUNDCRAFT_BUILD_DATE=2026-11-24 \
  bash "$ROOT/packaging/linux/package.sh" --skip-build --formats tar >/dev/null
SOUNDCRAFT_VERSION=0.0.1 \
  bash "$ROOT/packaging/linux/package.sh" --skip-build --formats tar >/dev/null

extract() {
  mkdir -p "$work/$2"
  tar -xzf "$DIST/soundcraft-$1-linux-$arch.tar.gz" -C "$work/$2"
}
extract "$nightly" nightly
extract 0.0.1 stable

nightly_root="$work/nightly/soundcraft-$nightly-linux-$arch"
stable_root="$work/stable/soundcraft-0.0.1-linux-$arch"
fail() { printf '%s\n' "$1" >&2; exit 1; }

[[ -x "$nightly_root/bin/soundcraft" ]] || fail "the Nightly tar has no bin/soundcraft"
head -n 1 "$nightly_root/bin/soundcraft" | grep -qx '#!/bin/sh' || fail "bin/soundcraft is not the launcher"
grep -q 'launch-profile nightly-v1' "$nightly_root/bin/soundcraft" || fail "the launcher does not pass the profile"
cmp -s "$nightly_root/bin/soundcraft-bin" "$CARGO_TARGET_DIR/release/soundcraft" || fail "bin/soundcraft-bin is not the GUI"
[[ -x "$nightly_root/bin/soundcraft-bin" ]] || fail "bin/soundcraft-bin is not executable"
[[ -x "$nightly_root/bin/soundcraft-cli" ]] || fail "the Nightly tar lost soundcraft-cli"

cmp -s "$stable_root/bin/soundcraft" "$CARGO_TARGET_DIR/release/soundcraft" || fail "the stable tar no longer ships the bare GUI"
[[ ! -e "$stable_root/bin/soundcraft-bin" ]] || fail "the stable tar gained a launcher split"

echo "PASS: the Nightly FHS tar starts the GUI through its launcher and the stable tar is unchanged"
