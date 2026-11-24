"""The tree ships no stray backups."""

import subprocess
from pathlib import Path

KIT = Path(__file__).resolve().parents[1]


def test_no_backup_file_is_tracked():
    tracked = subprocess.run(["git", "-C", str(KIT), "ls-files"], capture_output=True, text=True, check=True).stdout.split("\n")

    assert [name for name in tracked if name.endswith((".bak", ".orig", ".rej", "~"))] == []
