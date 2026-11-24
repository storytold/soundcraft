#!/usr/bin/env python3
"""Run the synthetic packaging suites together; no app build or signing.

The Linux archive helper and packaging are covered by packaging/nightly-release-tests.py (its own
owner's checks); these are the macOS and Windows suites next to the shared caller contracts.
"""
import importlib.util
from pathlib import Path
import unittest


def main() -> int:
    here = Path(__file__).resolve().parent
    suite = unittest.TestSuite()
    for name, relative in (
        ("sound_plist_suite", "macos/nightly_plist_tests.py"),
        ("sound_mac_sign_suite", "macos/nightly_sign_tests.py"),
        ("sound_release_dispatch_suite", "nightly-release-tests.py"),
    ):
        spec = importlib.util.spec_from_file_location(name, here / relative)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        suite.addTests(unittest.defaultTestLoader.loadTestsFromModule(module))
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
