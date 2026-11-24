import subprocess
import tempfile
import unittest
from pathlib import Path


CHECK = Path(__file__).resolve().parents[1] / "check-comments.sh"


class CheckComments(unittest.TestCase):
    def check(self, source):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "sample.rs"
            path.write_text(source, encoding="utf-8")
            return subprocess.run([CHECK, path], capture_output=True, text=True, check=False)

    def test_rejects_each_pattern_in_line_comments(self):
        result = self.check(
            "// Explain the contract.\n" * 9
            + "fn work() {} // ---- WORKFLOW ----\n"
            + "// Step 3: dispatch\n"
            + "// A rocket \U0001f680\n"
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("sample.rs:1: comment exceeds 8 lines", result.stderr)
        self.assertIn("sample.rs:10: decorative separator", result.stderr)
        self.assertIn("sample.rs:11: numbered step", result.stderr)
        self.assertIn("sample.rs:12: emoji in comment", result.stderr)

    def test_ignores_comment_markers_inside_literals(self):
        result = self.check(
            'fn f<\'a>(x: &\'a str) { let _ = \'"\'; let _ = "\\"// Step 2"; }\n'
            + 'const A: &str = r##"a" #\n// Step 2\n/* \U0001f680 */\n"##;\n'
            + 'const B: &[u8] = br"// Step 2";\n'
            + 'const C: &std::ffi::CStr = cr"/* \U0001f680 */";\n'
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_allows_long_module_and_safety_comments(self):
        result = self.check(
            "//! Module documentation.\n" * 9
            + "mod inner {\n"
            + "/*! More documentation.\n" + "More facts.\n" * 9 + "*/\n"
            + "// SAFETY: The buffer outlives the call.\n"
            + "// A supporting detail.\n" * 9
            + "}\n"
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_nested_and_multiline_block_comments(self):
        result = self.check(
            "/* Outer /* Step 2: nested */ continuation */\n"
            + "/* Overview\n" + " * More prose.\n" * 8 + " */\n"
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("sample.rs:1: numbered step", result.stderr)
        self.assertIn("sample.rs:2: comment exceeds 8 lines", result.stderr)

    def test_eight_lines_are_allowed(self):
        result = self.check("/// API documentation.\n" * 8 + "pub fn work() {}\n")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
