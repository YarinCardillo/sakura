"""Dependency notices accept cargo's multi-root output and npm's production graph."""
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import third_party_notices as notices


class TestCargoPackages(unittest.TestCase):
    def test_blank_lines_between_workspace_roots_do_not_break_the_inventory(self):
        output = "sakura-app v0.1.0|AGPL-3.0-only|\nserde v1.0.0|MIT OR Apache-2.0|https://github.com/serde-rs/serde\n\nsakura-core v0.1.0|AGPL-3.0-only|\nserde v1.0.0|MIT OR Apache-2.0|https://github.com/serde-rs/serde (*)\n"
        with patch.object(notices.subprocess, "run", return_value=SimpleNamespace(stdout=output)):
            self.assertEqual(notices.cargo_packages(), [
                ("serde", "1.0.0", "MIT OR Apache-2.0", "https://github.com/serde-rs/serde")])


class TestEnginePackages(unittest.TestCase):
    def test_production_dependencies_include_scoped_packages_and_exclude_test_tools(self):
        with tempfile.TemporaryDirectory() as folder:
            engine = Path(folder)
            lock = {"packages": {"": {"name": "engine"},
                "node_modules/@restatedev/restate-sdk": {"version": "1.17.4", "license": "MIT"},
                "node_modules/vitest": {"version": "5.1.0", "dev": True}}}
            (engine / "package-lock.json").write_text(json.dumps(lock))
            with patch.object(notices, "ENGINE", engine):
                packages = notices.engine_packages()
            self.assertEqual([(entry[0], entry[1]) for entry in packages], [("@restatedev/restate-sdk", "1.17.4")])


class TestNoticeInventory(unittest.TestCase):
    def test_source_inventory_lists_dependencies_without_copying_upstream_prose(self):
        import contextlib
        import io
        output = io.StringIO()
        crates = [("sample-crate", "1.0.0", "MIT", "https://example.com/source")]
        npm = [("@restatedev/restate-sdk", "1.17.4", "MIT", "https://example.com/package", Path("cache"))]
        with patch.object(notices, "cargo_packages", return_value=crates), patch.object(notices, "engine_packages", return_value=npm):
            with contextlib.redirect_stdout(output):
                notices.print_inventory()
        text = output.getvalue()
        self.assertIn("sample-crate 1.0.0 (MIT)", text)
        self.assertIn("@restatedev/restate-sdk 1.17.4 (MIT)", text)
        self.assertIn("Business Source License 1.1", text)
        self.assertIn("THIRD_PARTY_LICENSES", text)
        self.assertNotIn("example.com/source", text)


if __name__ == "__main__":
    unittest.main()
