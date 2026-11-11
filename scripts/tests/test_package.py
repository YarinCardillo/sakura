"""Packaging keeps runtime inputs and licences while excluding development files."""
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import package


class TestCarryDocuments(unittest.TestCase):
    def test_bundle_carries_the_same_legal_documents_as_flat_archives(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for name in package.DOCUMENTS:
                (root / name).write_text(f"contents of {name}\n")
            (root / "LICENSES").mkdir()
            (root / "LICENSES/GPL.txt").write_text("GPL fixture")
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "third_party_notices.py").write_text('print("copyright external@example.com " + chr(0x2014))')
            resources = root / "Sakura.app/Contents/Resources"
            with patch.object(package, "ROOT", root):
                package.carry_documents(resources)
            self.assertEqual((resources / "THIRD_PARTY_LICENSES").read_text(), "copyright external@example.com " + chr(0x2014) + "\n")
            self.assertEqual((resources / "LICENSES/GPL.txt").read_text(), "GPL fixture")
            for name in package.DOCUMENTS:
                self.assertEqual((resources / name).read_bytes(), (root / name).read_bytes())


class TestCarryService(unittest.TestCase):
    def test_engine_sources_and_lock_survive_without_local_dependencies_or_tests(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "source"
            for name in ("bin/box", "engine/package.json", "engine/package-lock.json", "engine/src/main.ts",
                         "engine/node_modules/local.js", "engine/test/local.test.ts", "tests/test_local.sh"):
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture\n")
            destination = root / "Application Support/service"
            with patch.object(package, "SERVICE", source), patch.object(package, "install_engine") as install:
                package.carry_service(destination)
            for name in ("bin/box", "engine/package-lock.json", "engine/src/main.ts"):
                self.assertTrue((destination / name).is_file(), name)
            for name in ("engine/node_modules", "engine/test", "tests"):
                self.assertFalse((destination / name).exists(), name)
            install.assert_called_once_with(destination / "engine")


if __name__ == "__main__":
    unittest.main()
