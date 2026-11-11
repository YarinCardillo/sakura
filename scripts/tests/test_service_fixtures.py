"""Service fixtures remain executable with the system shell they document."""
from pathlib import Path
import os
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


class TestMacShell(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'darwin', 'macOS system bash parser')
    def test_trusted_program_inventory_parses_inside_command_substitution(self):
        text = (ROOT / 'service/tests/test_mac_trusted_programs.sh').read_text()
        snippet = text.split('NAMES="', 1)[1].split('\nCOUNT=', 1)[0]
        result = subprocess.run(['/bin/bash', '-c', 'NAMES="' + snippet + '\nprintf "%s" "$NAMES"'],
                                env=dict(os.environ, BOX_ROOT=str(ROOT / 'service'),
                                         SYSTEM_DIRECTORIES='/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin'),
                                text=True, capture_output=True, timeout=15, stdin=subprocess.DEVNULL)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, '')
        self.assertIn('bash', result.stdout.splitlines())
        self.assertGreaterEqual(len(result.stdout.splitlines()), 40)
