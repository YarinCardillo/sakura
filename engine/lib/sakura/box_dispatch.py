"""`sakura box ...`: hand the arguments, unchanged, to the Sakura service command."""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path
from typing import Mapping

ENV_VARIABLE = "SAKURA_BOX_BIN"
COMMAND_NAME = "box"
NOT_FOUND = 127


class BoxNotFound(RuntimeError):
    """Raised when neither SAKURA_BOX_BIN nor PATH names an executable service command."""


def resolve(environ: Mapping[str, str]) -> Path:
    """The service command: `SAKURA_BOX_BIN` when set, else `box` on `PATH`.

    Raises:
        BoxNotFound: the variable names no executable file, or it is unset and `PATH` has no `box`.
    """
    named = environ.get(ENV_VARIABLE, "")
    if named:
        path = Path(named).expanduser()
        if path.is_file() and os.access(path, os.X_OK):
            return path
        raise BoxNotFound(f"{ENV_VARIABLE}={named} is not an executable file")
    found = shutil.which(COMMAND_NAME, path=environ.get("PATH", ""))
    if found is None:
        raise BoxNotFound(f"no Sakura service command: set {ENV_VARIABLE} or put `{COMMAND_NAME}` on PATH")
    return Path(found)


def main(argv: list[str]) -> int:
    """Replace this process with the service command, passing `argv` as given; returns only on failure."""
    try:
        program = resolve(os.environ)
    except BoxNotFound as error:
        print(f"sakura box: {error}", file=sys.stderr)
        return NOT_FOUND
    sys.stdout.flush()
    sys.stderr.flush()
    try:
        os.execv(program, [str(program), *argv])
    except OSError as error:
        print(f"sakura box: cannot run {program}: {error.strerror}", file=sys.stderr)
    return NOT_FOUND
