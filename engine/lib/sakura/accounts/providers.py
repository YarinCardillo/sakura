"""Per-provider conventions: where an account's files live and how its quota snapshots are keyed."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

# Codex plan limits and quota snapshots are stored under this prefix so they never mix with a Claude account of the same name.
CODEX_ACCOUNT_PREFIX = "codex:"
CLAUDE_ALTERNATE_DIRS = (".claude-work",)
GOOSE = "goose"
GOOSE_LOCAL_PROVIDER = "ollama"
GOOSE_NATIVE_DIRS = (".config/goose", "Library/Application Support/Block/goose")
SUBSCRIPTION_PROVIDERS = ("claude", "codex")


@dataclass(frozen=True)
class Adapter:
    """What the engine needs to know about a provider to find and key an account."""

    env_var: str
    default_dir: str
    transcript_subdir: str
    snapshot_key: Callable[[str], str]
    plan_choices: tuple[str, ...]  # the plan names `accounts plan` accepts for this provider


def _claude_snapshot_key(account_id: str) -> str:
    return account_id


def _codex_snapshot_key(account_id: str) -> str:
    return f"{CODEX_ACCOUNT_PREFIX}{account_id}"


def _goose_snapshot_key(account_id: str) -> str:
    return f"goose:{account_id}"


ADAPTERS = {
    "claude": Adapter("CLAUDE_CONFIG_DIR", "~/.claude", "projects", _claude_snapshot_key, ("pro", "max5x", "max20x")),
    "codex": Adapter("CODEX_HOME", "~/.codex", "sessions", _codex_snapshot_key, ("plus", "prolite", "pro")),
    GOOSE: Adapter("GOOSE_PATH_ROOT", "~/.goose", "data/sessions", _goose_snapshot_key, ()),
}
