#!/usr/bin/env bash
# Validate the staged Arch launcher without building or opening SoundCraft.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/source"
dst="$work/dist"
base=soundcraft-0.0.1-nightly.1-linux-x86_64
mkdir -p "$stage/bin" "$dst" "$work/extract"

# A local argv recorder stands in for the compiled binaries. It never opens a UI.
cat >"$stage/bin/soundcraft" <<'RECORDER'
#!/bin/sh
printf '<%s>\n' "$@"
RECORDER
chmod 755 "$stage/bin/soundcraft"
cp "$stage/bin/soundcraft" "$stage/bin/soundcraft-cli"

bash "$HERE/nightly-arch-tar.sh" "$stage" "$work/stage" "$dst" "$base"
tar -xzf "$dst/$base.tar.gz" -C "$work/extract"
bin="$work/extract/$base/bin"
# Simulate the existing Arch bin-tree copy and public launcher rename.
mkdir -p "$work/installed"
cp -R "$bin" "$work/installed/bin"
bin="$work/installed/bin"
mv "$bin/soundcraft" "$bin/soundcraft-nightly"
mv "$bin/soundcraft-cli" "$bin/soundcraft-nightly-cli"

[[ ! -e "$stage/bin/.soundcraft-nightly" ]]
cmp "$stage/bin/soundcraft" "$bin/.soundcraft-nightly/soundcraft"
cmp "$stage/bin/soundcraft-cli" "$bin/soundcraft-nightly-cli"
profile="${XDG_CONFIG_HOME:-${HOME:-}/.config}/soundcraft-nightly"
# A launch adds the Launcher's literal selector/root pair and the file delimiter; a version query or
# an already-formed protocol passes through untouched.
expect_launch() {
  expected="$(printf '<%s>\n' --launch-profile nightly-v1 --profile-root "$profile" -- "$@")"
  actual=$(PATH="$bin:$PATH" soundcraft-nightly "$@")
  [[ "$actual" == "$expected" ]] || {
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  }
}
expect_forwarded() {
  expected="$(printf '<%s>\n' "$@")"
  actual=$(PATH="$bin:$PATH" soundcraft-nightly "$@")
  [[ "$actual" == "$expected" ]] || {
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  }
}
expect_launch
expect_launch 'session with spaces.sc' '--other-name.sc'
expect_forwarded --version
expect_forwarded -V
expect_forwarded --launch-profile nightly-v1 --profile-root /launcher/profile --version

echo "PASS: Arch Nightly handoff preserves the binary layout and passes the Launcher profile"
