#!/bin/sh
# The SoundCraft Nightly launcher for the Arch handoff (packaging/linux/nightly-arch-tar.sh): the
# real GUI stays in .soundcraft-nightly/, and this starts it with the Launcher's private profile,
# as every other Nightly package does.
set -eu

bin="$(dirname "$0")/.soundcraft-nightly/soundcraft"
profile="${XDG_CONFIG_HOME:-${HOME:-}/.config}/soundcraft-nightly"

# The Launcher's protocol, and a bare version query, pass through untouched.
case "${1-}" in
  --launch-profile | --profile-root | --version | -V) exec "$bin" "$@" ;;
esac
exec "$bin" --launch-profile nightly-v1 --profile-root "$profile" -- "$@"
