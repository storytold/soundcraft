#!/bin/sh
set -eu

if [ -z "${SOUNDCRAFT_CONFIG_DIR+x}" ]; then
  export SOUNDCRAFT_CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/soundcraft-nightly"
fi
real_bin="$(dirname "$0")/.soundcraft-nightly/soundcraft"
exec "$real_bin" "$@"
