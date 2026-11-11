"""Tests of the release inputs: the version, the notes, the bundle's version, the cask."""
import plistlib
import json
from types import SimpleNamespace
from unittest.mock import patch
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import release_metadata  # noqa: E402
import write_cask  # noqa: E402

MANIFEST = '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.2.3"\nedition = "2021"\n\n[profile.release]\n'
CHANGELOG = "# Changelog\n\n## 1.2.3\n\n- Second.\n\n## 1.2.2\n\n- First.\n"


class TestReadVersion(unittest.TestCase):
    def test_read_version_matching_manifest_returns_it(self):
        self.assertEqual(release_metadata.read_version(MANIFEST, "1.2.3"), "1.2.3")

    def test_read_version_differing_from_manifest_is_refused(self):
        with self.assertRaisesRegex(ValueError, "differs from Cargo.toml"):
            release_metadata.read_version(MANIFEST, "1.2.4")

    def test_read_version_malformed_is_refused(self):
        with self.assertRaisesRegex(ValueError, "MAJOR.MINOR.PATCH"):
            release_metadata.read_version(MANIFEST, "v1.2.3")

    def test_read_version_ignores_versions_of_other_sections(self):
        other = '[package]\nversion = "9.9.9"\n\n' + MANIFEST
        self.assertEqual(release_metadata.read_version(other, "1.2.3"), "1.2.3")


class TestReleaseNotes(unittest.TestCase):
    def test_release_notes_known_version_returns_only_its_section(self):
        self.assertEqual(release_metadata.release_notes(CHANGELOG, "1.2.3"), "- Second.\n")

    def test_release_notes_missing_version_is_refused(self):
        with self.assertRaisesRegex(ValueError, "no section"):
            release_metadata.release_notes(CHANGELOG, "2.0.0")

    def test_release_notes_empty_section_is_refused(self):
        with self.assertRaisesRegex(ValueError, "is empty"):
            release_metadata.release_notes("## 1.0.0\n\n## 0.9.0\n- Old.\n", "1.0.0")


class TestCheckBundle(unittest.TestCase):
    def write_info(self, folder, short, build):
        path = Path(folder) / "Info.plist"
        with path.open("wb") as stream:
            plistlib.dump({"CFBundleShortVersionString": short, "CFBundleVersion": build}, stream)
        return path

    def test_check_bundle_reporting_the_version_passes(self):
        with tempfile.TemporaryDirectory() as folder:
            self.assertIn("1.2.3", release_metadata.check_bundle(self.write_info(folder, "1.2.3", "1.2.3"), "1.2.3"))

    def test_check_bundle_reporting_another_version_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaisesRegex(ValueError, "reports"):
                release_metadata.check_bundle(self.write_info(folder, "1.2.2", "1.2.2"), "1.2.3")


class TestWriteCask(unittest.TestCase):
    def cask_for(self, architecture):
        with tempfile.TemporaryDirectory() as folder:
            archive = Path(folder) / "Sakura-1.2.3.zip"
            archive.write_bytes(b"archive")
            destination = Path(folder) / "sakura.rb"
            write_cask.write_cask("1.2.3", archive, destination, architecture)
            return destination.read_text(), write_cask.checksum(archive)

    def test_write_cask_pins_version_and_checksum(self):
        cask, digest = self.cask_for("universal")
        self.assertIn('version "1.2.3"', cask)
        self.assertIn(f'sha256 "{digest}"', cask)
        self.assertNotIn("depends_on arch", cask)

    def test_write_cask_single_architecture_says_so(self):
        cask, _ = self.cask_for("arm64")
        self.assertIn("depends_on arch: :arm64", cask)

    def test_write_cask_unknown_architecture_is_refused(self):
        with self.assertRaisesRegex(ValueError, "Unknown architecture"):
            self.cask_for("riscv")


class TestDraftRelease(unittest.TestCase):
    def test_existing_draft_accepts_an_upload(self):
        with patch("release_metadata.subprocess.run", return_value=SimpleNamespace(stdout=json.dumps({"isDraft": True}))):
            self.assertIn("draft", release_metadata.require_draft("example/sakura", "v1.2.3"))

    def test_published_release_refuses_an_upload(self):
        with patch("release_metadata.subprocess.run", return_value=SimpleNamespace(stdout=json.dumps({"isDraft": False}))):
            with self.assertRaisesRegex(ValueError, "published"):
                release_metadata.require_draft("example/sakura", "v1.2.3")

    def test_missing_draft_state_refuses_an_upload(self):
        with patch("release_metadata.subprocess.run", return_value=SimpleNamespace(stdout="{}")):
            with self.assertRaises(ValueError):
                release_metadata.require_draft("example/sakura", "v1.2.3")


if __name__ == "__main__":
    unittest.main()
