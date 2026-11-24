"""Platform packages preserve the supplied desktop icons and application identity."""
import configparser
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import package


class TestPlatformArchives(unittest.TestCase):
    def make_archive(self, system, root):
        binary = root / "built-app"
        binary.write_bytes(b"executable fixture")
        binary.chmod(0o755)
        with patch.object(package, "carry_service"), patch.object(package, "carry_documents"):
            return package.package(system, "0.1.2", binary, root / "output")

    def test_linux_archive_carries_every_original_hicolor_size(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive("linux", Path(temporary))
            with tarfile.open(archive) as packed:
                prefix = archive.name.removesuffix(".tar.gz")
                for size in (16, 32, 48, 64, 128, 256, 512):
                    with self.subTest(size=size):
                        name = f"{prefix}/share/icons/hicolor/{size}x{size}/apps/sakura.png"
                        source = package.ROOT / f"assets/icons/linux/sakura-{size}.png"
                        self.assertEqual(packed.extractfile(name).read(), source.read_bytes())
                self.assertTrue(packed.getmember(f"{prefix}/sakura").mode & 0o111)

    def test_linux_desktop_names_the_theme_icon_and_matches_the_window_class(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive("linux", Path(temporary))
            with tarfile.open(archive) as packed:
                prefix = archive.name.removesuffix(".tar.gz")
                desktop = configparser.ConfigParser(interpolation=None)
                desktop.read_string(packed.extractfile(f"{prefix}/sakura.desktop").read().decode())
                entry = desktop["Desktop Entry"]
                self.assertEqual(entry["Icon"], "sakura")
                self.assertEqual(entry["StartupWMClass"], "sakura")
                self.assertEqual(entry["Exec"], "sakura")

    def test_windows_archive_still_carries_the_executable_without_linux_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.make_archive("windows", Path(temporary))
            with zipfile.ZipFile(archive) as packed:
                self.assertEqual(packed.namelist(), [f"{archive.stem}/sakura.exe"])
                self.assertEqual(packed.read(packed.namelist()[0]), b"executable fixture")


if __name__ == "__main__":
    unittest.main()
