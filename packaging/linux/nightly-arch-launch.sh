#!/bin/sh
set -eu

real_bin="$(dirname "$0")/.soundcraft-nightly/soundcraft"
exec "$real_bin" "$@"
