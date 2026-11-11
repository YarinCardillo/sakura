"""Exercise the local release pipeline without build tools, credentials, signing services or publication."""
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
DITTO = '''import pathlib, shutil, sys, zipfile
args = sys.argv[1:]
source, destination = map(pathlib.Path, args[-2:])
if "-c" in args:
    with zipfile.ZipFile(destination, "w") as archive:
        for item in source.rglob("*"):
            if item.is_file():
                archive.write(item, pathlib.Path(source.name) / item.relative_to(source))
elif "-x" in args:
    with zipfile.ZipFile(source) as archive:
        archive.extractall(destination)
else:
    shutil.copytree(source, destination, dirs_exist_ok=True)
'''


class TestLocalRelease(unittest.TestCase):
    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def fixture(self, base):
        repository, tools, app = base / "repository", base / "tools", base / "prebuilt/Sakura.app"
        (repository / "scripts").mkdir(parents=True)
        tools.mkdir()
        (app / "Contents/MacOS").mkdir(parents=True)
        for name in ["release.sh", "release_metadata.py", "write_cask.py"]:
            shutil.copy2(ROOT / "scripts" / name, repository / "scripts" / name)
        (repository / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        (repository / "CHANGELOG.md").write_text("## 0.1.0\n\n- First release.\n")
        (repository / ".gitignore").write_text("dist/\n__pycache__/\n*.pyc\n")
        with (app / "Contents/Info.plist").open("wb") as stream:
            plistlib.dump({"CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "0.1.0"}, stream)
        (app / "Contents/MacOS/sakura").write_bytes(b"fixture executable")
        for name, body in [("codesign", "exit 0"), ("lipo", "printf arm64"), ("security", "exit 1"),
                           ("gh", 'touch "$PUBLICATION_MARKER"; exit 1')]:
            self.executable(tools / name, "#!/bin/sh\n" + body + "\n")
        self.executable(tools / "ditto", f"#!{sys.executable}\n" + DITTO)
        return repository, tools, app

    def test_clean_checkout_creates_its_cask_directory_and_never_publishes(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            repository, tools, app = self.fixture(base)
            home = base / "home"
            home.mkdir()
            environment = dict(os.environ, HOME=str(home), GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
                               PATH=str(tools) + os.pathsep + os.environ["PATH"], SAKURA_APP_BUNDLE=str(app),
                               PUBLICATION_MARKER=str(base / "published"))
            for args in [["init", "-q", str(repository)], ["-C", str(repository), "add", "."],
                         ["-C", str(repository), "-c", "user.name=fixture", "-c", "user.email=fixture@example.com",
                          "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]]:
                subprocess.run(["git", *args], env=environment, check=True, timeout=15, stdin=subprocess.DEVNULL)
            self.assertFalse((repository / "Casks").exists())
            result = subprocess.run(["bash", "scripts/release.sh", "0.1.0"], cwd=repository, env=environment,
                                    capture_output=True, text=True, timeout=30, stdin=subprocess.DEVNULL)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((repository / "Casks/sakura.rb").is_file())
            self.assertEqual((repository / "dist/releases/0.1.0/signature.txt").read_text(), "ad-hoc\n")
            self.assertFalse((base / "published").exists())


    def test_explicit_ad_hoc_signing_never_probes_the_keychain(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            _, tools, _ = self.fixture(base)
            self.executable(tools / "security", '#!/bin/sh\ntouch "$PROBE_MARKER"\nexit 1\n')
            source = (ROOT / "scripts/release.sh").read_text()
            function = 'can_sign_for_distribution() {' + source.split('can_sign_for_distribution() {', 1)[1].split('\narchitecture_of()', 1)[0]
            script = 'with_deadline() { shift; "$@"; }\n' + function + '\nif can_sign_for_distribution; then exit 4; fi'
            environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"],
                               IDENTITY='-', PROFILE='unused', CALL_DEADLINE_SECONDS='1', PROBE_MARKER=str(base / 'probe'))
            subprocess.run(['bash', '-c', script], env=environment, check=True, timeout=5, stdin=subprocess.DEVNULL)
            self.assertFalse((base / 'probe').exists())


if __name__ == "__main__":
    unittest.main()
