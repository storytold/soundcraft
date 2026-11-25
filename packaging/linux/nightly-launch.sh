#!/bin/sh
# The entry point inside the plain Nightly tarball (packaging/linux/nightly.sh installs it as
# `soundcraft`, beside the GUI as `soundcraft-bin`; package.sh's Nightly tar ships the same pair
# under bin).
#
# A Nightly build only starts with its private profile, and every package format passes the
# Launcher's literal protocol: `--launch-profile nightly-v1 --profile-root <dir> -- <files>`. The
# profile holds ui.json, Autosave/, Presets/ and logs/ outside stable's settings folder, and outside
# this unpacked copy, which the app's own fence would refuse.
set -eu

here="$(dirname "$(readlink -f "$0")")"
bin="$here/soundcraft-bin"
profile="${XDG_CONFIG_HOME:-${HOME:-}/.config}/soundcraft-nightly"

# The Launcher's protocol, and a bare version query, pass through untouched: the first already
# names its profile, and the second must answer without one.
case "${1-}" in
  --launch-profile | --profile-root | --version | -V) exec "$bin" "$@" ;;
esac
exec "$bin" --launch-profile nightly-v1 --profile-root "$profile" -- "$@"
