#!/usr/bin/env python3
"""Synthetic tests for the flat Linux Nightly archive policy (packaging/nightly-archive.py).

No native build and no executable launch: generated fixtures are archived with the committed
helper, read back, and every refused shape is checked to leave no artifact behind.
"""
import importlib.util
import os
import tempfile
import tarfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("sound_nightly_archive", HERE / "nightly-archive.py")
ARCHIVE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ARCHIVE)


class ArchivePolicy(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        os.chmod(self.root, 0o700)
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)
        self.stage = self.root / "stage"
        self.stage.mkdir(mode=0o700)
        self.output = self.root / "archive.tar.gz"
        # The launcher is the entry point and the GUI sits beside it; both are required members.
        self.required = {
            "soundcraft",
            "soundcraft-bin",
            "soundcraft-nightly.png",
            "README.md",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "NOTICE",
            "ATTRIBUTION.md",
        }
        for name in self.required:
            (self.stage / name).write_bytes(b"generated fixture: " + name.encode())
        for name in ("soundcraft", "soundcraft-bin"):
            (self.stage / name).chmod(0o755)

    def test_launcher_gui_icon_and_notices_are_archived_executable(self):
        ARCHIVE.linux_archive(self.stage, self.output)
        with tarfile.open(self.output) as archive:
            members = {member.name: member for member in archive}
            self.assertEqual(set(members), self.required)
            self.assertTrue(all(member.isfile() and not member.pax_headers for member in members.values()))
            for name in ("soundcraft", "soundcraft-bin"):
                self.assertEqual(members[name].mode & 0o111, 0o111, f"{name} must stay executable")
            self.assertEqual(archive.extractfile("soundcraft").read(), (self.stage / "soundcraft").read_bytes())
        with open(self.output, "rb") as stream:
            self.assertEqual(stream.read(2), b"\x1f\x8b", "the archive is gzip-compressed")

    def test_each_required_member_is_required_before_output(self):
        for name in self.required:
            with self.subTest(name=name):
                content = (self.stage / name).read_bytes()
                (self.stage / name).unlink()
                with self.assertRaisesRegex(ValueError, "required"):
                    ARCHIVE.linux_archive(self.stage, self.output)
                self.assertFalse(self.output.exists())
                (self.stage / name).write_bytes(content)

    def test_unexpected_cli_marker_or_desktop_member_refuses(self):
        for name in ("soundcraft-cli", "portable.txt", "soundcraft.desktop"):
            with self.subTest(name=name):
                bad = self.stage / name
                bad.write_bytes(b"generated fixture")
                with self.assertRaisesRegex(ValueError, "unexpected"):
                    ARCHIVE.linux_archive(self.stage, self.output)
                self.assertFalse(self.output.exists())
                bad.unlink()

    def test_symlinked_member_refuses_before_output(self):
        icon = self.stage / "soundcraft-nightly.png"
        icon.unlink()
        icon.symlink_to(self.stage / "README.md")
        with self.assertRaisesRegex(ValueError, "regular file|aliases"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_directory_member_refuses_before_output(self):
        icon = self.stage / "soundcraft-nightly.png"
        icon.unlink()
        icon.mkdir()
        with self.assertRaisesRegex(ValueError, "regular file"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_case_colliding_notices_refuse_before_output(self):
        for name in ("OFL-Font.txt", "OFL-font.txt"):
            (self.stage / name).write_bytes(b"generated fixture")
        with self.assertRaisesRegex(ValueError, "case-colliding"):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertFalse(self.output.exists())

    def test_existing_archive_is_never_replaced(self):
        self.output.write_bytes(b"owned earlier artifact")
        with self.assertRaises(FileExistsError):
            ARCHIVE.linux_archive(self.stage, self.output)
        self.assertEqual(self.output.read_bytes(), b"owned earlier artifact")
