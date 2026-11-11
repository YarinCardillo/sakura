"""CI fixtures exercise the shipped service with the repository and temporary layout it needs."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class TestConfinementFixture(unittest.TestCase):
    def seed_script(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        heading = "      - name: A throwaway repository for the self-test project\n        run: |\n"
        body = workflow.split(heading, 1)[1].split("\n      - ", 1)[0]
        return "\n".join(line[10:] for line in body.splitlines()) + "\n"

    def test_ci_repository_can_create_the_profile_task_without_docker(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            home, shims = base / "person", base / "shims"
            home.mkdir()
            shims.mkdir()
            docker = shims / "docker"
            docker.write_text('#!/bin/sh\nprintf called > "$DOCKER_MARKER"\nexit 99\n')
            docker.chmod(0o755)
            script = base / "seed.sh"
            script.write_text(self.seed_script())
            environment = dict(os.environ, HOME=str(home), AGENTBOX_HOME=str(base / "data"),
                               PATH=str(shims) + os.pathsep + os.environ["PATH"], DOCKER_CONTEXT="orbstack",
                               DOCKER_MARKER=str(base / "docker-called"), GIT_CONFIG_GLOBAL=os.devnull,
                               GIT_CONFIG_NOSYSTEM="1")
            subprocess.run(["bash", "-e", str(script)], env=environment, check=True, timeout=15, stdin=subprocess.DEVNULL)
            box = ROOT / "service/bin/box"
            try:
                result = subprocess.run([str(box), "new", "ci-fixture", "selftest", "--where", "mac"], env=environment,
                                        capture_output=True, text=True, timeout=30, stdin=subprocess.DEVNULL)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                source = Path(environment["AGENTBOX_HOME"]) / "selftest-source"
                self.assertTrue((source / "package.json").is_file())
                self.assertTrue((source / "package-lock.json").is_file())
                self.assertFalse((base / "docker-called").exists())
            finally:
                if (ROOT / "service/engine/package.json").is_file():
                    subprocess.run([str(box), "engine", "stop"], env=environment, timeout=20, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


class TestServiceSuite(unittest.TestCase):
    def test_service_step_uses_a_spaced_temporary_folder_outside_the_checkout(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        heading = "      - name: Service suite (with the stand-in Docker the suite carries)\n"
        block = workflow.split(heading, 1)[1].split("\n      - ", 1)[0]
        run = block.split("        run: ", 1)[1]
        script = run.splitlines()[0]
        if script == "|":
            script = "\n".join(line[10:] for line in run.splitlines()[1:] if line.startswith("          "))
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            checkout, runner, default = base / "checkout", base / "runner-temp", base / "default-temp"
            stand_in = checkout / "service/tests/run.sh"
            stand_in.parent.mkdir(parents=True)
            runner.mkdir()
            default.mkdir()
            stand_in.write_text('#!/bin/bash\nprintf "%s" "$TMPDIR" > "$RESULT_FILE"\ntest -d "$TMPDIR"\n')
            result = base / "result"
            environment = dict(os.environ, RUNNER_TEMP=str(runner), TMPDIR=str(default), RESULT_FILE=str(result))
            subprocess.run(["bash", "-e", "-c", script], cwd=checkout, env=environment, check=True,
                           timeout=10, stdin=subprocess.DEVNULL)
            actual = Path(result.read_text())
            self.assertEqual(actual, runner / "Application Support")
            self.assertTrue(actual.is_dir())
            self.assertFalse(actual.is_relative_to(checkout))


if __name__ == "__main__":
    unittest.main()

class TestReleasePermissions(unittest.TestCase):
    def test_builds_have_no_write_token_and_only_the_upload_step_receives_one(self):
        release = (ROOT / '.github/workflows/release.yml').read_text()
        jobs = release.split('jobs:\n', 1)[1]
        self.assertNotIn('GH_TOKEN', release.split('jobs:\n', 1)[0])
        build = jobs.split('  build:\n', 1)[1].split('\n  upload:', 1)[0]
        self.assertIn('contents: read', build)
        self.assertNotIn('contents: write', build)
        self.assertNotIn('GH_TOKEN', build)
        self.assertNotIn('gh release', build)
        self.assertIn('actions/upload-artifact@', build)
        upload = jobs.split('  upload:\n', 1)[1]
        self.assertEqual(release.count('contents: write'), 1)
        self.assertEqual(release.count('GH_TOKEN:'), 1)
        self.assertIn('actions/download-artifact@', upload)
        self.assertNotIn('checkout@', upload)
        self.assertNotIn('python3 scripts/', upload)
        self.assertIn('isDraft', upload)
        self.assertIn('GH_TOKEN', upload.split('      - name: Upload to the draft', 1)[1])
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertNotIn('GH_TOKEN', ci)
        self.assertNotIn('contents: write', ci)

class TestNativeOnlyRelease(unittest.TestCase):
    def test_release_docs_and_workflows_do_not_offer_the_excluded_engine(self):
        for name in ['README.md', 'THIRD_PARTY_NOTICES', '.github/workflows/ci.yml', '.github/workflows/release.yml']:
            text = (ROOT / name).read_text()
            self.assertNotIn('service/engine', text, name)
            self.assertNotIn('Restate', text, name)

    def test_each_zed_adaptation_has_the_prominent_modification_date(self):
        for name in ['ime', 'grid_view', 'overlay', 'keys', 'rows', 'mouse', 'pointer']:
            text = (ROOT / f'crates/sakura-terminal/src/{name}.rs').read_text()
            self.assertIn("//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.", text)

class TestHostedWorkspaceAccess(unittest.TestCase):
    def test_hosted_fixture_grants_only_the_task_worktree_to_the_container_uid(self):
        text = (ROOT / '.github/workflows/ci.yml').read_text()
        block = text.split('      - name: Writable throwaway workspace for the container UID', 1)[1].split('\n      - ', 1)[0]
        self.assertIn('$AGENTBOX_HOME/work/ci', block)
        self.assertIn('u:1000:rwX', block)
        self.assertIn('sudo setfacl -R -m', block)
        self.assertIn('sudo find', block)
        self.assertNotIn('chmod 777', block)
        self.assertNotIn('setfacl -R -m u:1000:rwX "$AGENTBOX_HOME"', block)
