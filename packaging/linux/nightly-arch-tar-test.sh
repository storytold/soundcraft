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
expect() {
  expected="$1"
  shift
  actual=$(PATH="$bin:$PATH" soundcraft-nightly "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}
expect "$(printf '<%s>\n' 'session with spaces.sc' '--other-name.sc')" \
  'session with spaces.sc' '--other-name.sc'
expect '<--version>' --version
expect '<-V>' -V

echo "PASS: Arch Nightly handoff stages binary and preserves launch arguments"
