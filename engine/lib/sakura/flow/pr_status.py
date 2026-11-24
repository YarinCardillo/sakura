"""Read the GitHub state of a pull request."""

import json
import subprocess
from pathlib import Path

from sakura import programs


def read(url: str, repo: Path | None = None) -> str:
    """The state `gh` reports for the pull request, or '' when there is no URL, no safe `gh` or no answer.

    `repo` (the working directory by default) is the repository whose folders `gh` may not come from."""
    if not url:
        return ""
    try:
        gh = programs.path("gh", repo)
        result = subprocess.run([gh, "pr", "view", url, "--json", "state"], capture_output=True, text=True,
                                check=False, env=programs.environment("gh", gh), cwd=repo or Path.cwd(), timeout=10)
    except (OSError, programs.ProgramError, subprocess.TimeoutExpired):
        return ""
    if result.returncode:
        return ""
    try:
        return json.loads(result.stdout).get("state", "")
    except (AttributeError, TypeError, ValueError):
        return ""
