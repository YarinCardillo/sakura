"""The update feed and Sparkle signing inside the release flow, without credentials or network."""
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import sign_bundle  # noqa: E402

MACHO = bytes.fromhex("cffaedfe") + b"fixture"
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
XCRUN = '''#!/bin/sh
case "$2" in
  submit) printf '{"id":"submission"}\\n' ;;
  wait) printf '{"id":"submission","status":"Accepted"}\\n' ;;
esac
'''
FEED = ('<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>'
        '<sparkle:version>0.1.0</sparkle:version><enclosure url="https://example.com/0.1.0.zip"/>'
        '</item></channel></rss>')


UPDATE_KEYS = {"SUFeedURL": "https://example.com/appcast.xml", "SUPublicEDKey": "AAAA"}


def write_bundle_part(path, executable, info=None):
    """Lay out a code container whose main executable is a Mach-O image."""
    base = path / "Contents/MacOS" if path.suffix != ".framework" else path
    plist = path / "Contents/Info.plist" if path.suffix != ".framework" else path / "Resources/Info.plist"
    base.mkdir(parents=True, exist_ok=True)
    plist.parent.mkdir(parents=True, exist_ok=True)
    plist.write_bytes(plistlib.dumps(dict(info or {}, CFBundleExecutable=executable)))
    (base / executable).write_bytes(MACHO)


class TestSigningTargets(unittest.TestCase):
    def layout(self, base):
        app = base / "Sakura.app"
        write_bundle_part(app, "sakura")
        frameworks = app / "Contents/Frameworks"
        write_bundle_part(frameworks / "Sparkle.framework", "Sparkle")
        version = frameworks / "Sparkle.framework/Versions/B"
        (version / "XPCServices").mkdir(parents=True)
        write_bundle_part(version / "XPCServices/Downloader.xpc", "Downloader")
        (version / "Autoupdate").write_bytes(MACHO)
        write_bundle_part(version / "Updater.app", "Updater")
        write_bundle_part(frameworks / "Other.framework", "Other")
        (app / "Contents/Resources").mkdir()
        (app / "Contents/Resources/helper").write_bytes(MACHO)
        return app

    def test_signing_targets_skip_everything_inside_sparkle_framework(self):
        with tempfile.TemporaryDirectory() as folder:
            app = self.layout(Path(folder))
            targets = sign_bundle.signing_targets(app)
            relative = [target.relative_to(app).as_posix() for target in targets]
            self.assertEqual([name for name in relative if "Sparkle.framework" in name], [])
            self.assertIn("Contents/Frameworks/Other.framework", relative)
            self.assertIn("Contents/Resources/helper", relative)
            self.assertEqual(targets[-1], app)

    def test_signing_targets_keep_signing_a_framework_that_is_not_sparkle(self):
        with tempfile.TemporaryDirectory() as folder:
            app = self.layout(Path(folder))
            names = {target.name for target in sign_bundle.signing_targets(app)}
            self.assertLessEqual({"Other.framework", "helper"}, names)


class TestReleaseFeed(unittest.TestCase):
    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def fixture(self, base, info=None):
        repository, tools, app = base / "repository", base / "tools", base / "prebuilt/Sakura.app"
        scripts = repository / "scripts"
        scripts.mkdir(parents=True)
        tools.mkdir()
        for name in ["release.sh", "release_metadata.py", "write_cask.py", "sign_bundle.py", "sign-sparkle.sh",
                     "generate-appcast.sh", "appcast-history.py", "release.entitlements"]:
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        sparkle = base / "sparkle"
        (sparkle / "bin").mkdir(parents=True)
        self.executable(scripts / "fetch-sparkle.sh", f'#!/bin/bash\nprintf "%s\\n" "{sparkle}"\n')
        self.executable(sparkle / "bin/sign_update",
                        '#!/bin/bash\nif [[ $* != *--verify* ]]; then printf signature; fi\n')
        self.executable(sparkle / "bin/generate_appcast", f'#!/bin/bash\nfor out; do :; done\n'
                        f'cp "{base}/feed.xml" "$out/appcast.xml"\n')
        (base / "feed.xml").write_text(FEED)
        (repository / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        (repository / "CHANGELOG.md").write_text("## 0.1.0\n\n- First release.\n")
        (repository / ".gitignore").write_text("dist/\n__pycache__/\n*.pyc\n")
        write_bundle_part(app, "sakura", {"CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "0.1.0",
                                          **(info or {})})
        for name, body in [("codesign", 'printf "%s\\n" "$*" >> "$SIGN_LOG"'), ("lipo", "printf arm64"),
                           ("security", 'echo "1) HASH \\"Developer ID Application: Fixture\\""'),
                           ("spctl", "exit 0"), ("gh", "exit 1"), ("xcrun", None)]:
            self.executable(tools / name, XCRUN if body is None else "#!/bin/sh\n" + body + "\n")
        self.executable(tools / "ditto", f"#!{sys.executable}\n" + DITTO)
        return repository, tools, app

    def release(self, base, info=None, **extra):
        repository, tools, app = self.fixture(base, info)
        home = base / "home"
        home.mkdir()
        environment = dict(os.environ, HOME=str(home), GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
                           PATH=str(tools) + os.pathsep + os.environ["PATH"], SAKURA_APP_BUNDLE=str(app),
                           SIGN_LOG=str(base / "signed"), **extra)
        for name in list(environment):
            if name.startswith("SAKURA_") and name not in extra and name != "SAKURA_APP_BUNDLE":
                environment.pop(name)
        for args in [["init", "-q", str(repository)], ["-C", str(repository), "add", "."],
                     ["-C", str(repository), "-c", "user.name=fixture", "-c", "user.email=fixture@example.com",
                      "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]]:
            subprocess.run(["git", *args], env=environment, check=True, timeout=15, stdin=subprocess.DEVNULL)
        result = subprocess.run(["bash", "scripts/release.sh", "0.1.0"], cwd=repository, env=environment,
                                capture_output=True, text=True, timeout=60, stdin=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result, repository / "dist/releases/0.1.0", (base / "signed").read_text().splitlines()

    def test_notarised_release_with_account_publishes_the_feed_and_signs_sparkle_first(self):
        with tempfile.TemporaryDirectory() as folder:
            result, out, signed = self.release(Path(folder), UPDATE_KEYS, SAKURA_SPARKLE_ACCOUNT="sakura",
                                               SAKURA_FIRST_UPDATE_RELEASE="1")
            self.assertEqual((out / "signature.txt").read_text(), "developer-id-notarized\n")
            self.assertTrue((out / "appcast.xml").is_file())
            self.assertIn("appcast.xml", (out / "SHA256SUMS-macos.txt").read_text())
            self.assertIn("Sakura-0.1.0.zip", (out / "SHA256SUMS-macos.txt").read_text())
            self.assertIn("appcast.xml", (out / "publish-draft.sh").read_text())
            framework = next(index for index, line in enumerate(signed) if line.endswith("Sparkle.framework"))
            application = next(index for index, line in enumerate(signed)
                               if "--entitlements" in line and line.endswith("Sakura.app"))
            self.assertLess(framework, application)
            self.assertTrue(all("Developer ID Application" in line for line in signed[:framework + 1]))

    def test_notarised_release_without_account_says_the_feed_was_skipped(self):
        with tempfile.TemporaryDirectory() as folder:
            result, out, _ = self.release(Path(folder))
            self.assertIn("SAKURA_SPARKLE_ACCOUNT is not set", result.stdout)
            self.assertFalse((out / "appcast.xml").exists())
            self.assertNotIn("appcast.xml", (out / "SHA256SUMS-macos.txt").read_text())
            self.assertNotIn("appcast.xml", (out / "publish-draft.sh").read_text())

    def test_ad_hoc_release_with_account_makes_no_feed(self):
        with tempfile.TemporaryDirectory() as folder:
            result, out, _ = self.release(Path(folder), UPDATE_KEYS, SAKURA_SPARKLE_ACCOUNT="sakura",
                                         SAKURA_SIGN_IDENTITY="-")
            self.assertEqual((out / "signature.txt").read_text(), "ad-hoc\n")
            self.assertFalse((out / "appcast.xml").exists())
            self.assertIn("update feed", result.stdout.lower())

    def test_release_with_account_refuses_a_bundle_without_update_keys(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(AssertionError) as raised:
                self.release(Path(folder), SAKURA_SPARKLE_ACCOUNT="sakura")
            self.assertIn("no update feed or public key", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
