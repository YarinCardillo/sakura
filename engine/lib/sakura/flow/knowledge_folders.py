"""The knowledge folders of a project as the engine uses them: the settings' folders, checked against the cage.

`sakura/knowledge.py` holds the rules of a folder; this module adds what only the cage knows: the secret paths, the
engine's own folders and every place a role can write, none of which a knowledge folder may be, hold or lie inside.
Cages (to read them), the proposal tool (to know where a proposal goes) and `sakura proposal accept` (to write it) all
get the folders from here, so they agree.
"""

from __future__ import annotations

import os
from collections.abc import Iterable
from pathlib import Path

from sakura import engine_home, knowledge
from sakura.flow import cage


def of(values: dict, home: Path, workspace: Path | None = None, extra: Iterable[Path] = ()) -> dict[str, Path]:
    """The checked folders by name for settings `values`; `workspace` and `extra` are further folders a role can write.

    Raises knowledge.KnowledgeRefused naming the setting. Nothing is read when no folder is configured.
    """
    if not any((values.get("knowledge") or {}).values()):
        return {}
    policy = cage.load_policy(values["knobs"]["cage"])
    secrets = (path for path in cage.secret_paths(policy, home, workspace or home) if "*" not in path)
    engine = (engine_home.root(), engine_home.root_for_trust(), engine_home.account_root())
    forbidden = [Path(os.path.realpath(path)) for path in (*secrets, *engine, *cage.writable_roots(), *extra)]
    return knowledge.configured(values, home, forbidden)
