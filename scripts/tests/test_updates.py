"""Offline checks of Sparkle acquisition, bundle metadata and release behavior."""

import base64
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
PUBLIC_KEY = base64.b64encode(bytes(32)).decode()
REPOSITORY = "https://github.com/example/app"


class UpdateScriptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, TMPDIR=str(self.root))
        for name in list(self.env):
            if name.startswith("SAKURA_"):
                self.env.pop(name)
        self.env["SAKURA_BUILD_DIR"] = str(self.root / "build")

    def run_script(self, name, *args, **env):
        return subprocess.run(
            ["bash", str(ROOT / "scripts" / name), *map(str, args)],
            env=dict(self.env, **env), capture_output=True, text=True, timeout=15,
        )

    def metadata(self, **env):
        plist = self.root / "Info.plist"
        plist.write_bytes(plistlib.dumps({"CFBundleVersion": "1.2.3"}))
        result = subprocess.run(
            ["python3", str(ROOT / "scripts" / "update-metadata.py"), str(plist)],
            env=dict(self.env, **env), capture_output=True, text=True, timeout=10,
        )
        return result, plistlib.loads(plist.read_bytes())

    def test_wrong_archive_hash_refuses_before_extraction(self):
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        curl = fake_bin / "curl"
        curl.write_text('#!/bin/bash\nwhile [[ $# -gt 0 ]]; do\n'
                        'if [[ $1 == --output ]]; then shift; printf tampered > "$1"; fi\n'
                        'shift\ndone\n')
        curl.chmod(0o755)
        result = self.run_script("fetch-sparkle.sh", SAKURA_BUILD_DIR=str(self.root / "build"),
                                 PATH=f"{fake_bin}:{self.env['PATH']}")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256", result.stderr)
        self.assertFalse(list((self.root / "build").rglob("Sparkle.framework")))



    def test_bundle_without_environment_has_no_sparkle_keys(self):
        result, info = self.metadata()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("SUFeedURL", info)
        self.assertNotIn("SUPublicEDKey", info)
        self.assertIn("without updates", result.stdout)

    def test_bundle_keys_are_written_as_data_not_interpolated_xml(self):
        url = "https://example.com/appcast.xml?a=1&b=2"
        result, info = self.metadata(SAKURA_UPDATE_FEED_URL=url, SAKURA_UPDATE_PUBLIC_KEY=PUBLIC_KEY)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(info["SUFeedURL"], url)
        self.assertEqual(info["SUPublicEDKey"], PUBLIC_KEY)
        self.assertFalse(info["SUEnableSystemProfiling"])
        self.assertFalse(info["SUAutomaticallyUpdate"])

    def test_partial_or_invalid_metadata_is_rejected(self):
        for env in [dict(SAKURA_UPDATE_FEED_URL="https://example.com/feed"),
                    dict(SAKURA_UPDATE_PUBLIC_KEY=PUBLIC_KEY),
                    dict(SAKURA_UPDATE_FEED_URL="http://example.com/feed", SAKURA_UPDATE_PUBLIC_KEY=PUBLIC_KEY),
                    dict(SAKURA_UPDATE_FEED_URL="https://example.com/feed", SAKURA_UPDATE_PUBLIC_KEY="invalid")]:
            with self.subTest(env=env):
                result, _ = self.metadata(**env)
                self.assertNotEqual(result.returncode, 0)

    def test_release_without_account_skips_feed_and_never_runs_a_key_tool(self):
        out = self.root / "release"
        out.mkdir()
        result = self.run_script("generate-appcast.sh", "1.2.3", self.root / "tools", out, REPOSITORY)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SAKURA_SPARKLE_ACCOUNT is not set", result.stdout)
        self.assertFalse((out / "appcast.xml").exists())


    def test_previous_feed_is_required_after_first_update_release(self):
        result = self.run_script("generate-appcast.sh", "1.2.3", self.root / "tools", self.root / "out", REPOSITORY,
                                 SAKURA_SPARKLE_ACCOUNT="test-account")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("previous appcast", result.stderr.lower())

    def make_feed_tools(self):
        tools = self.root / "tools"
        tools.mkdir()
        sign = tools / "sign_update"
        sign.write_text('#!/bin/bash\nprintf "%s\\n" "$*" >> "$TOOL_LOG"\n'
                        'if [[ $* != *--verify* ]]; then printf public-signature; fi\n')
        generate = tools / "generate_appcast"
        generate.write_text('#!/bin/bash\nprintf "%s\\n" "$*" >> "$TOOL_LOG"\n'
                            'for out; do :; done\ncp "$NEW_FEED" "$out/appcast.xml"\n')
        for tool in (sign, generate):
            tool.chmod(0o755)
        return tools

    def test_feed_signs_zip_and_preserves_all_previous_items_without_overwriting_input(self):
        previous = self.root / "previous.xml"
        old = self.feed("1.0.0")
        previous.write_text(old)
        new = self.root / "new.xml"
        new.write_text(self.feed("1.2.3"))
        out = self.root / "out"
        out.mkdir()
        (out / "Sakura-1.2.3.zip").write_bytes(b"stapled archive")
        log = self.root / "tool-log"
        result = self.run_script("generate-appcast.sh", "1.2.3", self.make_feed_tools(), out,
                                 REPOSITORY,
                                 SAKURA_SPARKLE_ACCOUNT="test-account", SAKURA_PREVIOUS_APPCAST=str(previous),
                                 TOOL_LOG=str(log), NEW_FEED=str(new))
        self.assertEqual(result.returncode, 0, result.stderr)
        generated = (out / "appcast.xml").read_text()
        self.assertIn("1.0.0", generated)
        self.assertIn("1.2.3", generated)
        self.assertEqual(previous.read_text(), old)
        self.assertEqual((out.parent / "appcast.xml").read_text(), generated)
        calls = log.read_text().splitlines()
        self.assertIn("--account test-account -p", calls[0])
        self.assertIn("--verify", calls[1])
        self.assertIn("--maximum-versions 0", calls[2])
        self.assertIn("--download-url-prefix https://github.com/example/app/releases/download/v1.2.3/", calls[2])

    def test_feed_reads_the_previous_appcast_kept_beside_the_release_folders(self):
        releases = self.root / "dist/releases"
        out = releases / "1.2.3"
        out.mkdir(parents=True)
        (releases / "appcast.xml").write_text(self.feed("1.2.3"))
        result = self.run_script("generate-appcast.sh", "1.2.3", self.root / "no-tools", out,
                                 REPOSITORY, SAKURA_SPARKLE_ACCOUNT="test-account")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already exists", result.stderr)

    def test_reusing_an_existing_feed_build_is_rejected_before_signing(self):
        previous = self.root / "previous.xml"
        previous.write_text(self.feed("1.2.3"))
        result = self.run_script("generate-appcast.sh", "1.2.3", self.root / "no-tools", self.root / "out", REPOSITORY,
                                 SAKURA_SPARKLE_ACCOUNT="test-account", SAKURA_PREVIOUS_APPCAST=str(previous))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already exists", result.stderr)

    def test_feed_preserves_distinct_previous_items_sharing_a_build_version(self):
        import xml.etree.ElementTree as ET
        previous = self.root / "previous.xml"
        tree = ET.fromstring(self.feed("1.0.0"))
        channel = tree.find("channel")
        second = ET.fromstring(ET.tostring(channel.find("item")))
        second.find("enclosure").set("url", "https://example.com/other-architecture.zip")
        channel.append(second)
        previous.write_bytes(ET.tostring(tree))
        target = self.root / "appcast.xml"
        target.write_text(self.feed("1.2.3"))
        result = subprocess.run(["python3", str(ROOT / "scripts/appcast-history.py"), "merge", str(previous), str(target)],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(ET.parse(target).findall("channel/item")), 3)

    @staticmethod
    def feed(version):
        return ('<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">'
                f'<channel><item><sparkle:version>{version}</sparkle:version>'
                f'<enclosure url="https://example.com/{version}.zip" sparkle:edSignature="old-signature"/>'
                '</item></channel></rss>')


    def test_sparkle_signing_alone_signs_only_the_framework_in_documented_order(self):
        app = self.root / "Sakura.app"
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        log = self.root / "signed"
        signer = fake_bin / "codesign"
        signer.write_text('#!/bin/bash\nprintf "%s\\n" "$*" >> "$SIGN_LOG"\n')
        signer.chmod(0o755)
        result = self.run_script("sign-sparkle.sh", app, "Release identity", SIGN_LOG=str(log),
                                 PATH=f"{fake_bin}:{self.env['PATH']}")
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = log.read_text().splitlines()
        expected = ["XPCServices/Installer.xpc", "XPCServices/Downloader.xpc", "Autoupdate", "Updater.app",
                    "Contents/Frameworks/Sparkle.framework"]
        self.assertEqual(len(lines), len(expected))
        for line, path in zip(lines, expected):
            self.assertTrue(line.endswith(path), line)
            self.assertIn("--options runtime --timestamp ", line)

    def test_embedding_copies_framework_license_and_keys_into_an_existing_app(self):
        project = self.root / "project"
        scripts = project / "scripts"
        scripts.mkdir(parents=True)
        for name in ("embed-sparkle.sh", "update-metadata.py"):
            shutil.copy(ROOT / "scripts" / name, scripts)
        sparkle = self.root / "sparkle"
        (sparkle / "Sparkle.framework").mkdir(parents=True)
        (sparkle / "Sparkle.framework/marker").write_text("framework")
        (sparkle / "LICENSE").write_text("MIT")
        (scripts / "fetch-sparkle.sh").write_text(f'#!/bin/bash\nprintf "%s\\n" "{sparkle}"\n')
        app = self.root / "Sakura.app"
        (app / "Contents").mkdir(parents=True)
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleVersion": "1.2.3"}))
        result = subprocess.run(["bash", str(scripts / "embed-sparkle.sh"), str(app)],
                                env=dict(self.env, SAKURA_UPDATE_FEED_URL="https://example.com/appcast.xml",
                                         SAKURA_UPDATE_PUBLIC_KEY=PUBLIC_KEY),
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((app / "Contents/Frameworks/Sparkle.framework/marker").read_text(), "framework")
        self.assertEqual((app / "Contents/Resources/Sparkle-LICENSE").read_text(), "MIT")
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        self.assertEqual(info["SUPublicEDKey"], PUBLIC_KEY)

    @unittest.skipUnless(shutil.which("lipo") and shutil.which("clang"), "requires Xcode tools")
    def test_arm64_embed_thins_sparkle_and_its_nested_executables(self):
        sparkle = self.root / "sparkle"
        framework = sparkle / "Sparkle.framework"
        nested = framework / "Versions/B/Updater.app/Contents/MacOS"
        nested.mkdir(parents=True)
        (sparkle / "LICENSE").write_text("MIT")
        parts = []
        for arch in ("arm64", "x86_64"):
            part = self.root / arch
            subprocess.run(["clang", "-arch", arch, "-x", "c", "-c", "-o", str(part), "-"],
                           input=b"int main(void) { return 0; }\n", check=True, timeout=10)
            parts.append(str(part))
        universal = nested / "Updater"
        subprocess.run(["lipo", "-create", "-output", str(universal), *parts],
                       check=True, timeout=10)
        universal.chmod(0o755)
        (framework / "Updater").symlink_to(universal.relative_to(framework))
        app = self.root / "Sakura.app"
        (app / "Contents").mkdir(parents=True)
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleVersion": "1.2.3"}))
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ("embed-sparkle.sh", "update-metadata.py"):
            shutil.copy(ROOT / "scripts" / name, scripts)
        (scripts / "fetch-sparkle.sh").write_text("exit 17\n")
        result = subprocess.run(
            ["bash", str(scripts / "embed-sparkle.sh"), str(app), str(sparkle), "arm64"],
            env=self.env, capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        embedded = app / "Contents/Frameworks/Sparkle.framework/Updater"
        self.assertTrue(embedded.is_symlink())
        self.assertTrue(os.access(embedded, os.X_OK))
        archs = subprocess.check_output(["lipo", "-archs", str(embedded)], text=True, timeout=10)
        self.assertEqual(archs.strip(), "arm64")
        self.assertLess(embedded.stat().st_size, universal.stat().st_size)



if __name__ == "__main__":
    unittest.main()
