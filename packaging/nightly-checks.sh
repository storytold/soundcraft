#!/usr/bin/env bash
# The Nightly checks that need no compiler, macOS host or network: the synthetic suites, the
# tarball's launcher test, the FHS tar's launcher split, the Arch handoff's launcher test, and
# `bash -n` over every Nightly shell script. Run by .github/workflows/packaging-lint.yml and by
# hand before a push.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

python3 packaging/nightly-tests.py
bash packaging/linux/nightly-launch-test.sh
bash packaging/linux/package-tar-test.sh
bash packaging/linux/nightly-arch-tar-test.sh

for script in \
  packaging/nightly-common.sh \
  packaging/linux/package.sh \
  packaging/linux/flatpak-bundle.sh \
  packaging/linux/nightly.sh \
  packaging/linux/nightly-launch.sh \
  packaging/linux/nightly-arch-launch.sh \
  packaging/linux/nightly-arch-tar.sh \
  packaging/macos/nightly.sh \
  packaging/macos/nightly-lib.sh \
  packaging/macos/nightly-sign.sh \
  packaging/macos/nightly-verify.sh; do
  bash -n "$script"
done

echo "Nightly checks passed"
