"""Dependency notices cover the native release and directly adapted assets."""
import sys
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


class TestDirectAdaptations(unittest.TestCase):
    def test_adaptations_identify_every_file_and_both_icon_licences(self):
        import contextlib
        import io
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            notices.print_adaptations()
        text = output.getvalue()
        for name in ['ime', 'grid_view', 'overlay', 'keys', 'rows', 'mouse', 'pointer']:
            self.assertIn(f'crates/sakura-terminal/src/{name}.rs', text)
        for required in ['November 2026', 'ISC License', 'MIT License', 'Feather']:
            self.assertIn(required, text)


class TestNoticeInventory(unittest.TestCase):
    def test_source_inventory_lists_native_dependencies_and_required_adaptation_notices(self):
        import contextlib
        import io
        output = io.StringIO()
        crates = [("sample-crate", "1.0.0", "MIT", "https://example.com/source")]
        with patch.object(notices, "cargo_packages", return_value=crates):
            with contextlib.redirect_stdout(output):
                notices.print_inventory()
        text = output.getvalue()
        self.assertIn("sample-crate 1.0.0 (MIT)", text)
        self.assertNotIn("Restate", text)
        self.assertNotIn("npm packages", text)
        self.assertIn("THIRD_PARTY_LICENSES", text)
        self.assertNotIn("example.com/source", text)


if __name__ == "__main__":
    unittest.main()

class TestUnicodeLicences(unittest.TestCase):
    def test_legacy_redirected_stdout_preserves_unicode_licence_text_as_utf8(self):
        import io
        buffer = io.BytesIO()
        output = io.TextIOWrapper(buffer, encoding="cp1252")
        text = "Copyright example \u03bb \ufffd"
        collected = ({"sample": ["sample-crate"]}, {"sample": ("LICENSE", text)}, [])
        with patch.object(notices, "collect", return_value=collected):
            with patch.object(notices.sys, "stdout", output):
                notices.print_licenses()
                output.flush()
        self.assertIn(text.encode("utf-8"), buffer.getvalue())
