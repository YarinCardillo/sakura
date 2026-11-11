"""Release builds retain caller flags and remove workstation paths from compiler metadata."""
import importlib.util
import sys
import unittest
from pathlib import Path

BUILD = Path(__file__).resolve().parents[1] / "build.py"


class TestReleaseEnvironment(unittest.TestCase):
    def environment(self, values):
        spec = importlib.util.spec_from_file_location("sakura_build", BUILD)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.release_environment(values, Path("/home/developer"), Path("/home/developer/My App"))

    def test_workspace_and_home_are_remapped_as_whole_arguments_even_with_spaces(self):
        environment = self.environment({"PATH": "/usr/bin"})
        self.assertEqual(environment["PATH"], "/usr/bin")
        flags = environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
        self.assertIn("--remap-path-prefix=/home/developer=/build/home", flags)
        self.assertEqual(flags[-1], "--remap-path-prefix=/home/developer/My App=.")

    def test_a_separate_build_directory_is_remapped_before_the_source_directory(self):
        environment = self.environment({"CARGO_TARGET_DIR": "/home/developer/build-cache"})
        flags = environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
        self.assertIn("--remap-path-prefix=/home/developer/build-cache=/build/target", flags)
        self.assertEqual(flags[-1], "--remap-path-prefix=/home/developer/My App=.")

    def test_plain_caller_flags_are_preserved(self):
        environment = self.environment({"RUSTFLAGS": "-C debuginfo=1"})
        self.assertEqual(environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")[:2], ["-C", "debuginfo=1"])

    def test_encoded_caller_flags_keep_precedence_and_their_argument_boundaries(self):
        environment = self.environment({"RUSTFLAGS": "-C debuginfo=1", "CARGO_ENCODED_RUSTFLAGS": "--cfg\x1ffeature=\"my feature\""})
        self.assertEqual(environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")[:2], ["--cfg", 'feature="my feature"'])


if __name__ == "__main__":
    unittest.main()
