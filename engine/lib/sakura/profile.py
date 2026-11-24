"""Layered profile loading and rule-tightening validation."""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

from sakura import layers
from sakura.flow import gitutil


FLOW_TABLE = "flow"
# Branch names no task branch or branch pattern of a repository may reach (the integration branch is added).
PROTECTED_BRANCH_NAMES = ("main", "master")
TASK_PLACEHOLDER = "{task}"
# Branches the integration flow owns: the engine never pushes one as a task branch.
INTEGRATION_BRANCH_PREFIX = "sakura-integration/"
# Files the repository carries: untrusted, so they may only tighten a rule and never carry `relax`.
REPOSITORY_LAYERS = ("shared", "local")

DEFAULTS = {
    "integration_branch": "main",
    "protected_branches": ["main", "master"],
    "branch_pattern": "^(feat|fix|chore|refactor|docs|test|perf|security|hotfix)/[a-z0-9][a-z0-9._/-]*$",
    "require_signed_commits": False,
    "require_draft_pr": True,
    "ask_before_pr_ready": True,
    "tests": {"fast": [], "full": []},
    "migrations": {"alembic_versions": ""},
    "integration": {"setup": []},
}


@dataclass(frozen=True)
class EffectiveProfile:
    """Settings and the layer that supplied each setting."""

    values: dict
    origins: dict[str, str]
    relaxations: list[str]


class ProfileError(ValueError):
    """Raised when a profile is malformed or weakens a prior rule without consent."""


def load(repo: Path, home: Path | None = None) -> EffectiveProfile:
    """Load defaults, personal, shared and local profiles in precedence order."""
    home = home or Path.home()
    local = {key: value for key, value in _read(repo / ".sakura/local.toml").items() if key != FLOW_TABLE}
    stack = [
        ("personal", _read(home / ".config/sakura/profile.toml")),
        ("shared", _read(repo / ".sakura/profile.toml")),
        ("local", local),
    ]
    values, origins, relaxations = layers.merge(DEFAULTS, stack, _relaxes, ProfileError, "profile",
                                                tighten_only=REPOSITORY_LAYERS)
    _normalize(values)
    _require_safe_branch_pattern(values, origins, _host_protected_names(repo, stack[0]))
    return EffectiveProfile(values, origins, relaxations)


def protected_branch_names(settings: dict, *others: str) -> set[str]:
    """The names a task branch may never be: main, master, the profile's `protected_branches` and integration
    branch, and any `others` (such as the remote's default branch)."""
    return {*PROTECTED_BRANCH_NAMES, *settings["protected_branches"], settings["integration_branch"],
            *(name for name in others if name)}


def all_protected_names(repo: Path, *others: str, home: Path | None = None) -> set[str]:
    """Every name a task branch may never be: the merged profile's (a repository layer may have changed its integration
    branch) together with the host's own (its integration branch, its protected branches, the remote's default), and
    any `others`. The one place the engine asks, whether it checks a branch template or a push."""
    home = home or Path.home()
    host = _host_protected_names(repo, ("personal", _read(home / ".config/sakura/profile.toml")))
    return protected_branch_names(load(repo, home).values, *others) | host


def remote_default_branch(repo: Path) -> str:
    """The branch the remote's HEAD points at in the person's repository, or '' when it is not known."""
    try:
        done = gitutil.run(repo, "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
    except gitutil.GitError:
        return ""
    return done.stdout.strip().removeprefix("origin/") if done.returncode == 0 else ""


def _host_protected_names(repo: Path, personal: tuple[str, dict]) -> set[str]:
    """The protected names the host alone decides: its profile's integration branch and protected branches, main,
    master and the remote's default branch. A repository's own layers do not enter into it."""
    host, _, _ = layers.merge(DEFAULTS, [personal], _relaxes, ProfileError, "profile")
    _normalize(host)
    return protected_branch_names(host, remote_default_branch(repo))


def template_can_render_to(template: str, name: str) -> bool:
    """Whether a branch template with the task placeholder renders to `name` for some task id.

    A template without the placeholder is a fixed name, so it renders to `name` only when equal to it.
    """
    prefix, found, suffix = template.partition(TASK_PLACEHOLDER)
    if not found:
        return template == name
    return len(name) > len(prefix) + len(suffix) and name.startswith(prefix) and name.endswith(suffix)


def _require_safe_branch_pattern(values: dict, origins: dict[str, str], protected: set[str]) -> None:
    """A repository layer may set `branch_pattern` only when it matches none of the host's protected branch names."""
    if origins.get("branch_pattern") not in REPOSITORY_LAYERS:
        return
    try:
        compiled = re.compile(values["branch_pattern"])
    except (re.error, TypeError) as error:
        raise ProfileError(f"{origins['branch_pattern']} profile has an invalid branch_pattern: {error}") from error
    reached = sorted(name for name in protected if compiled.fullmatch(name))
    if reached:
        raise ProfileError(f"{origins['branch_pattern']} profile sets a branch_pattern that matches the protected "
                           f"branch {reached[0]!r}; a repository's pattern may not")

def _read(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        return tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ProfileError(f"cannot read {path}: {error}") from error


def _relaxes(key: str, old: object, new: object) -> bool:
    if key in ("integration_branch", "branch_pattern"):
        # Neutral: the integration branch only chooses the PR base, and a repository's branch_pattern is checked for the
        # protected names after the merge (`_require_safe_branch_pattern`).
        return False
    if key.startswith("require_") or key == "ask_before_pr_ready":
        return old is True and new is False
    if key == "protected_branches":
        return not set(old).issubset(set(new))
    if key.startswith("tests.") or key == "integration.setup":
        return not set(old).issubset(set(new))
    if key == "migrations.alembic_versions":
        return bool(old) and old != new
    return old != new


def _normalize(values: dict) -> None:
    branch = values["integration_branch"]
    protected = list(dict.fromkeys([*values["protected_branches"], branch]))
    values["protected_branches"] = protected
    for key in ("tests", "integration"):
        for value in values[key].values():
            if not isinstance(value, list):
                raise ProfileError(f"{key} values must be command lists")
