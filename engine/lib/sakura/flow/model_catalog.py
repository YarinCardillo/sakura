"""The models an account's program offers, with the efforts each accepts: read from the program itself, never written by hand.

Codex lists them through its app-server (`model/list`: id, display name, supported reasoning efforts, default). Claude Code
lists them in the answer to the `initialize` control request of its stream-json mode (`models`: the alias it accepts, the full
model it resolves to, display name, supported effort levels); there is no other machine-readable list.

Goose uses ACP. Claude and Codex have a separate ACP cache for cutover verification.

The lists are kept per account in `<engine home>/models/<provider>.<account>.json` with the date they were fetched, and read
from there: a lane, `rules describe` and every ask read the cache and never start a program. `refresh` is on demand
(`sakura engine models refresh`) and a failure of it leaves the cache as it was.

An account with no list yet (a fresh engine home) is asked for it in the background, once (`ensure`): the rules say the list is
being read, or in words why it could not be, and no lane waits for it. What the last attempt did is kept beside the list.
"""

from __future__ import annotations

import fcntl
import json
import os
import select
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from sakura import engine_home, programs
from sakura.accounts.providers import ADAPTERS, GOOSE
from sakura.accounts.registry import Account
from sakura.accounts.write import replace_atomically
from sakura.flow import process

CLAUDE, CODEX = "claude", "codex"
CACHE_DIR = "models"
FILE_MODE = 0o600
TIMEOUT_SECONDS = 25
STATE_SUFFIX = ".state.json"
RETRY_SECONDS = 300  # a read that failed is not asked again sooner
REFRESH_SECONDS = TIMEOUT_SECONDS + 35  # a read begun longer ago than this has died
REFRESHING, FAILED = "refreshing", "failed"
KEPT, STALE, UNAVAILABLE, MISSING = "kept", "stale", "unavailable", "missing"
HOST_VARIABLES = (engine_home.ENV_VARIABLE, "SAKURA_LEEWAY", "SAKURA_LEEWAY_INSTALLED", "CLAUDE_CODE_EXECUTABLE", "SAKURA_TMUX")
KIT_BIN = Path(__file__).resolve().parents[3] / "bin" / "sakura"
CLIENT = {"name": "sakura", "title": "Sakura", "version": "0"}
MAX_PAGES = 20
READ_BYTES = 1 << 16


class CatalogError(RuntimeError):
    """The program of an account could not list its models; the message says why."""


@dataclass(frozen=True)
class ModelOption:
    """One model an account's program accepts: the value to give it (`id`), the full model it stands for (`resolved`, for a
    Claude alias), its name, and the efforts it supports (`default_effort` is the program's own, "" when it names none)."""

    id: str
    label: str
    description: str = ""
    efforts: tuple[str, ...] = ()
    default_effort: str = ""
    is_default: bool = False
    resolved: str = ""

    def document(self) -> dict:
        return {"id": self.id, "label": self.label, "description": self.description, "efforts": list(self.efforts),
                "default_effort": self.default_effort, "is_default": self.is_default, "resolved": self.resolved}


@dataclass(frozen=True)
class Status:
    """Whether the models of an account are `kept`, being read (`refreshing`), could not be read (`unavailable`) or are not
    read yet (`missing`), with the words that say so for a person; empty for `kept`."""

    state: str
    note: str = ""
    reason: str | None = None


@dataclass(frozen=True)
class Catalog:
    """What the program of one account listed, and when and how."""

    provider: str
    account: str
    fetched_at: str
    source: str
    models: tuple[ModelOption, ...]
    upstream_provider: str | None = None

    def find(self, value: str) -> ModelOption | None:
        """The model a role value (`sonnet`, `claude-sonnet-5-5`, `gpt-6.2-sol`) names, by its id or its full model."""
        return next((model for model in self.models if value in (model.id, model.resolved)), None)


Exchange = Callable[[list[str], Mapping[str, str], list[dict], Callable[[dict], bool]], list[dict]]


def path(account: Account, root: Path | None = None, *, via_acp: bool = False) -> Path:
    """Locate the cache for the account and the selected catalog transport."""
    prefix = f"{account.provider}.{account.id}"
    if account.provider == GOOSE:
        prefix += f".{account.goose_provider or 'unconfigured'}"
    elif via_acp:
        prefix += ".acp"
    return engine_home.root(root) / CACHE_DIR / f"{prefix}.json"


def _backing(account: Account) -> object:
    if account.provider == GOOSE:
        from sakura.accounts.goose import lane_identity

        return [str(account.config_dir), lane_identity(account)]
    return str(account.config_dir)


def read(account: Account, root: Path | None = None, *, via_acp: bool = False) -> Catalog | None:
    """Read the account's kept models without starting a program."""
    try:
        document = json.loads(path(account, root, via_acp=via_acp).read_text())
        if (document["provider"], document["account"]) != (account.provider, account.id):
            return None
        if (via_acp or account.provider == GOOSE) and document.get("backing") != _backing(account):
            return None
        models = tuple(ModelOption(model["id"], model["label"], model.get("description", ""), tuple(model.get("efforts", ())),
                                   model.get("default_effort", ""), bool(model.get("is_default")), model.get("resolved", ""))
                       for model in document["models"])
        return Catalog(document["provider"], document["account"], document["fetched_at"], document["source"], models,
                       document.get("upstream_provider"))
    except (OSError, ValueError, KeyError, TypeError):
        return None


def refresh(account: Account, *, root: Path | None = None, program: str | None = None,
            exchange: Exchange | None = None, via_acp: bool = False) -> Catalog:
    """Fetch one account's models under a nonblocking lock and retain the last successful list on failure."""
    target = path(account, root, via_acp=via_acp)
    try:
        with _lock(target):
            return _refresh(account, target, program, exchange, via_acp)
    except OSError:
        raise CatalogError("catalog_storage_unavailable") from None


def _refresh(account, target, program, exchange, via_acp) -> Catalog:
    backing = _backing(account)
    _mark(account, target, REFRESHING, "", backing=backing)
    try:
        models, source = _fetch(account, target.parent.parent, program, exchange, via_acp)
        if backing != _backing(account):
            raise CatalogError("configuration_changed")
    except CatalogError as error:
        _mark(account, target, FAILED, str(error), backing=backing)
        raise
    catalog = Catalog(account.provider, account.id, datetime.now(UTC).isoformat(timespec="seconds"),
                      source, models, account.goose_provider)
    _keep(catalog, target, backing)
    _state_path(target).unlink(missing_ok=True)
    return catalog


def _fetch(account, root, program, exchange, via_acp):
    if via_acp or account.provider == GOOSE:
        from sakura.flow import acp_catalog

        return acp_catalog.fetch(account, root), "ACP session"
    fetch = {CLAUDE: _claude_models, CODEX: _codex_models}.get(account.provider)
    if fetch is None:
        raise CatalogError(f"{account.provider} accounts have no list of models")
    return fetch(account, program or (account.provider if exchange else _program(account)), exchange or _exchange)


def status(account: Account, root: Path | None = None, now: float | None = None, *, via_acp: bool = False) -> Status:
    """Describe catalog freshness without starting a program."""
    kept = read(account, root, via_acp=via_acp) is not None
    attempt = _attempt(account, path(account, root, via_acp=via_acp))
    age = (now if now is not None else time.time()) - attempt.get("at", 0)
    if attempt.get("state") == REFRESHING and age < REFRESH_SECONDS:
        return Status(REFRESHING, f"Reading the models of {account.label}.")
    if attempt.get("state") == FAILED or (kept and attempt.get("state") == REFRESHING):
        reason = str(attempt.get("reason") or "refresh_interrupted").rstrip(".")
        return Status(STALE if kept else UNAVAILABLE, f"The models of {account.label} could not be read: {reason}.", reason)
    if kept:
        return Status(KEPT)
    return Status(MISSING, f"The models of {account.label} have not been read yet.")


def ensure(accounts: list[Account], *, start: Callable[[], None] | None = None, now: float | None = None,
           root: Path | None = None) -> bool:
    """Start one background read (`start`, by default `sakura engine models refresh --missing`) when an account has no list and
    none is being read or failed lately; true when it was started. Never waits and never raises."""
    moment = now if now is not None else time.time()
    claimed = [_claim(account, root, moment) for account in accounts]
    if not any(claimed):
        return False
    (start or start_refresh)()
    return True


def _claim(account: Account, root: Path | None, moment: float) -> bool:
    if account.provider not in (CLAUDE, CODEX, GOOSE) or read(account, root) is not None:
        return False
    target = path(account, root)
    try:
        with _lock(target):
            if read(account, root) is not None or _recent(_attempt(account, target), moment):
                return False
            _mark(account, target, REFRESHING, "", moment)
            return True
    except (OSError, CatalogError):
        return False


def start_refresh() -> None:
    """Run `sakura engine models refresh --missing` detached from this process: a scrubbed environment plus the host's own
    choices (the engine home, Leeway, the Claude program, the interpreter), nothing else of this one."""
    from sakura import interpreter

    try:
        subprocess.Popen([str(KIT_BIN), "engine", "models", "refresh", "--missing"], start_new_session=True,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         env={**process.scrubbed_environment(os.environ), **{name: os.environ[name] for name in HOST_VARIABLES
                                                                              if name in os.environ}, **interpreter.environment()})
    except OSError as error:
        print(f"sakura: the models could not be read in the background: {error}", file=sys.stderr)


def _program(account: Account) -> str:
    """The engine's own copy of the account's program, found as every agent program is (never one a repository planted)."""
    try:
        return programs.path(account.provider, Path.cwd())
    except programs.ProgramError as error:
        raise CatalogError(f"{account.provider.title()} could not be found: {error}") from None


@contextmanager
def _lock(target: Path):
    target.parent.mkdir(mode=engine_home.DIRECTORY_MODE, parents=True, exist_ok=True)
    descriptor = os.open(target.with_suffix(".lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, FILE_MODE)
    with os.fdopen(descriptor, "rb") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise CatalogError("refresh_in_progress") from None
        yield


def _state_path(target: Path) -> Path:
    return target.with_suffix(STATE_SUFFIX)


def _attempt(account: Account, target: Path) -> dict:
    try:
        document = json.loads(_state_path(target).read_text())
        if (isinstance(document, dict) and type(document.get("at")) in (int, float)
                and document.get("backing", _backing(account)) == _backing(account)):
            return document
    except (OSError, ValueError):
        pass
    return {}


def _recent(attempt: dict, now: float) -> bool:
    age = now - attempt.get("at", 0)
    return (attempt.get("state") == REFRESHING and age < REFRESH_SECONDS) or (attempt.get("state") == FAILED and age < RETRY_SECONDS)


def _mark(account: Account, target: Path, state: str, reason: str, now: float | None = None,
          *, backing=None) -> None:
    document = {"state": state, "at": now if now is not None else time.time(),
                "reason": reason[:200], "backing": _backing(account) if backing is None else backing}
    replace_atomically(_state_path(target), json.dumps(document))


def refresh_all(accounts: list[Account], *, root: Path | None = None, **options) -> dict[str, str]:
    """`refresh` for every account; each answer is "ok" or why it failed. Never raises: one account's failure is not another's."""
    answers = {}
    for account in accounts:
        try:
            refresh(account, root=root, **options)
            answers[account.id] = "ok"
        except CatalogError as error:
            answers[account.id] = str(error)
    return answers


def _keep(catalog: Catalog, target: Path, backing: object) -> None:
    document = {"provider": catalog.provider, "account": catalog.account, "fetched_at": catalog.fetched_at,
                "source": catalog.source, "models": [model.document() for model in catalog.models],
                "upstream_provider": catalog.upstream_provider, "backing": backing}
    replace_atomically(target, json.dumps(document, indent=1))


# -- the two programs

def _claude_models(account: Account, program: str, exchange: Exchange) -> tuple[tuple[ModelOption, ...], str]:
    argv = [program, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]
    request = {"type": "control_request", "request_id": "sakura-models", "request": {"subtype": "initialize"}}
    answers = exchange(argv, _environment(account), [request], lambda event: event.get("type") == "control_response")
    listed = (answers[-1].get("response") or {}).get("response", {}).get("models") if answers else None
    if not isinstance(listed, list) or not listed:
        raise CatalogError("Claude Code did not list its models")
    models = tuple(ModelOption(str(item["value"]), str(item.get("displayName") or item["value"]), str(item.get("description", "")),
                               tuple(item.get("supportedEffortLevels") or ()), "", False, str(item.get("resolvedModel", "")))
                   for item in listed if isinstance(item, dict) and item.get("value") and item["value"] != "default")
    return models, "Claude Code initialize control request"


def _codex_models(account: Account, program: str, exchange: Exchange) -> tuple[tuple[ModelOption, ...], str]:
    models: list[ModelOption] = []
    cursor = None
    for page in range(MAX_PAGES):
        requests = [{"method": "initialize", "id": 1, "params": {"clientInfo": CLIENT}}, {"method": "initialized"},
                    {"method": "model/list", "id": 2, "params": {"includeHidden": False, "cursor": cursor}}]
        answers = exchange([program, "app-server"], _environment(account), requests, lambda event: event.get("id") == 2)
        result = next((event.get("result") for event in answers if event.get("id") == 2), None)
        if not isinstance(result, dict) or not isinstance(result.get("data"), list):
            raise CatalogError("Codex did not list its models")
        models += [_codex_option(item) for item in result["data"] if isinstance(item, dict) and not item.get("hidden")]
        cursor = result.get("nextCursor")
        if not cursor:
            break
    if not models:
        raise CatalogError("Codex did not list its models")
    return tuple(models), "Codex app-server model/list"


def _codex_option(item: dict) -> ModelOption:
    efforts = tuple(option["reasoningEffort"] for option in item.get("supportedReasoningEfforts") or []
                    if isinstance(option, dict) and option.get("reasoningEffort"))
    return ModelOption(str(item["model"]), str(item.get("displayName") or item["model"]), str(item.get("description", "")), efforts,
                       str(item.get("defaultReasoningEffort") or ""), bool(item.get("isDefault")))


def _environment(account: Account) -> dict[str, str]:
    """The environment of the program of `account`: its folder named, unless it is the default one (Claude Code keys its login by
    that variable and looks logged out when the default folder is named)."""
    adapter = ADAPTERS[account.provider]
    base = {key: value for key, value in os.environ.items() if key != adapter.env_var}
    env = process.scrubbed_environment(base)
    if account.config_dir.expanduser().resolve() != Path(adapter.default_dir).expanduser().resolve():
        env[adapter.env_var] = str(account.config_dir)
    return env


def _exchange(argv: list[str], env: Mapping[str, str], requests: list[dict], is_answer: Callable[[dict], bool]) -> list[dict]:
    """Start `argv`, send it `requests` (one JSON line each), read its JSON lines until one `is_answer` or the time is up, and
    end it. The program runs in an empty folder and is ended whatever happens."""
    with tempfile.TemporaryDirectory() as folder:
        try:
            child = subprocess.Popen(argv, cwd=folder, env=dict(env), stdin=subprocess.PIPE,
                                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        except OSError as error:
            raise CatalogError(f"{argv[0]} could not be started: {error.strerror or error}") from None
        try:
            return _converse(child, requests, is_answer)
        finally:
            child.kill()
            child.wait()


def _converse(child, requests: list[dict], is_answer: Callable[[dict], bool]) -> list[dict]:
    child.stdin.write("".join(json.dumps(request) + "\n" for request in requests).encode())
    child.stdin.flush()
    seen: list[dict] = []
    buffer = b""
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        ready, _, _ = select.select([child.stdout], [], [], 0.5)
        if not ready:
            continue
        chunk = os.read(child.stdout.fileno(), READ_BYTES)
        if not chunk:
            break
        buffer += chunk
        *lines, buffer = buffer.split(b"\n")
        for line in lines:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, dict):
                seen.append(event)
                if is_answer(event):
                    return seen
    raise CatalogError("the program did not answer in time" if time.monotonic() >= deadline else "the program ended without answering")
