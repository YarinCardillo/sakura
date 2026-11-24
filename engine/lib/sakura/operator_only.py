"""One check for the commands only the person runs: never an agent inside an engine role or its cage."""

from __future__ import annotations

import os
from collections.abc import Mapping

from sakura import engine_home
from sakura.flow import brakes


def is_inside_a_role(env: Mapping[str, str] | None = None) -> bool:
    """Whether an engine role or its cage runs this: its variables, or the cage itself, which an agent cannot unset (it
    cannot write the engine's trust root: `engine_home.sandboxed`)."""
    env = os.environ if env is None else env
    return bool(env.get(brakes.ROLE_ENV) or env.get(brakes.TASK_ENV)) or engine_home.sandboxed()
