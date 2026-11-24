"""The Python the engine runs under, handed to every `bin/sakura` it starts so a re-entry never looks one up on PATH.

A tmux window gets the engine's fixed PATH, and the first `python3` on it can be an old system one without
`tomllib`. `bin/sakura` therefore prefers `SAKURA_PYTHON`, and the engine sets it to its own interpreter.
"""

from __future__ import annotations

import os
import sys

ENV_NAME = "SAKURA_PYTHON"
# A test (or a proof run) starts the engine under a stand-in interpreter that names a throwaway trust root
# (`tests/fixtures/trust_root_python`): these variables carry it, and every child of the engine must get them too.
TEST_VARIABLES = ("SAKURA_TEST_TRUST_ROOT", "SAKURA_TEST_LIB")
TEST_INTERPRETER = "trust_root_python"  # the stand-in's file name: no other program is ever handed on this way
MINIMUM_VERSION = (3, 11)


class InterpreterError(RuntimeError):
    """Raised when the interpreter the engine runs under is older than the engine supports."""


def pinned() -> str:
    """The absolute real path of the interpreter this process runs under; raises when it is too old."""
    if sys.version_info[:2] < MINIMUM_VERSION:
        needed = ".".join(map(str, MINIMUM_VERSION))
        raise InterpreterError(f"sakura needs Python {needed} or later; this is {sys.version.split()[0]}")
    return os.path.realpath(sys.executable)


def environment() -> dict[str, str]:
    """The variables that make a started `bin/sakura` run under this interpreter.

    Normally that is the engine's own interpreter. A process started under the test interpreter (the one that names a
    throwaway trust root, `SAKURA_TEST_TRUST_ROOT`) hands that interpreter and its variables on instead, so every child of
    the engine, a role, a hook or a worker, keeps its trust records in the same throwaway root. Only the stand-in of that
    name is handed on; the engine's environment is the person's own, so this opens nothing a role could reach."""
    handed = handed_test_interpreter()
    if handed:
        return {ENV_NAME: handed, **{name: os.environ[name] for name in TEST_VARIABLES if name in os.environ}}
    return {ENV_NAME: pinned()}


def handed_test_interpreter() -> str | None:
    """The path of the test interpreter this process was started under and hands on (see `environment`), or None."""
    inherited = os.environ.get(ENV_NAME)
    if inherited and os.path.basename(inherited) == TEST_INTERPRETER and os.environ.get(TEST_VARIABLES[0]):
        return inherited
    return None
