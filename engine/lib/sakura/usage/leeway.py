"""The `leeway` helper: the engine's one reader of usage and accounts, for every app.

Leeway reads each account's limits, forecasts where it lands at its reset and keeps the account book; the engine runs
its command line and never reads a provider's usage itself. The helper is found among three places, each asked for
`version --json`, and the newest copy whose contract the engine understands (status 5, advice 2, accounts 1) is used:
`SAKURA_LEEWAY` (the app sets it to the helper it ships), the `leeway` entry of `<engine home>/programs.toml`, and the
installed Leeway app. A copy runs only when `programs.untrusted` accepts it, as every program the engine starts outside
a cage, and under a fixed environment.

A failure to find or ask the helper is an error of its own kind and never a traceback for the person: callers say
"Usage is unavailable" and go on with the reserve rules.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from collections.abc import Callable, Mapping, Sequence
from datetime import datetime, timedelta
from pathlib import Path

from sakura import programs
from sakura.flow import pace

ENV_VARIABLE = "SAKURA_LEEWAY"
# Where the installed copies are looked for, separated like PATH; set (even to nothing) it replaces `INSTALLED_HELPERS`,
# which is how a child process of a test is kept from probing the real app.
INSTALLED_ENV = "SAKURA_LEEWAY_INSTALLED"
PROGRAMS_KEY = "leeway"
INSTALLED_HELPERS = ("/Applications/Leeway.app/Contents/Helpers/leeway",)
CONTRACT = {"status_schema": 5, "advice_schema": 2, "accounts_schema": 1}
STATUS_ROW_TEXT = ("id", "label", "provider", "config_dir")
STATUS_ROW_WINDOWS = ("five_hour", "seven_day")
BOOK_ROW_TEXT = ("id", "provider")
ACCOUNT_VERBS = ("rename", "hide", "show", "remove")
ADD_PROVIDERS = ("claude", "codex")
CALL_TIMEOUT_SECONDS = 20
VERSION_TIMEOUT_SECONDS = 10
# The helper is looked for again after this long; a status is reused for a few seconds, as pages ask often.
LOCATE_CACHE_SECONDS = 60
STATUS_CACHE_SECONDS = 5
NOT_FOUND_TEXT = "the leeway helper was not found"
# Readings older than Leeway's default polling interval are stale: one `refresh` is asked for before advice, waits at most
# `REFRESH_WAIT_SECONDS` (then the readings that are there are used) and is not asked again for `REFRESH_RETRY_SECONDS`.
STALE_AFTER = timedelta(minutes=15)
REFRESH_WAIT_SECONDS = 4
REFRESH_RETRY_SECONDS = 60


class LeewayError(RuntimeError):
    """The helper cannot be used."""


class LeewayNotFound(LeewayError):
    """No copy of the helper is found that runs and speaks the contract the engine understands."""


class LeewayUnavailable(LeewayError, pace.AdvisorUnavailable):
    """A call to the helper failed or answered something the engine cannot read."""


_located: dict[tuple, tuple[float, Path | None]] = {}
_answers: dict[tuple, tuple[float, object]] = {}
_refresh_asked_at: list[float] = []


def forget() -> None:
    """Drop what was found and answered, so the next call looks again (tests, and a changed helper)."""
    _located.clear()
    _answers.clear()
    _refresh_asked_at.clear()


def locate(env: Mapping[str, str], host_programs: Mapping[str, object]) -> Path:
    """The newest compatible helper among the candidates; raises LeewayNotFound.

    Newest is the larger `store_schema`, then the larger `version`. A copy the trust check refuses, that does not
    run, or that exits 3 (older than the data it would read) is skipped.
    """
    named = [env.get(ENV_VARIABLE), host_programs.get(PROGRAMS_KEY), *_installed(env)]
    found = [entry for candidate in dict.fromkeys(named) if isinstance(candidate, str) and candidate
             if (entry := _contract_of(candidate))]
    if not found:
        raise LeewayNotFound(NOT_FOUND_TEXT)
    return Path(max(found, key=lambda entry: entry[0])[1])


def _installed(env: Mapping[str, str]) -> tuple[str, ...]:
    if INSTALLED_ENV in env:
        return tuple(entry for entry in env[INSTALLED_ENV].split(os.pathsep) if entry)
    return INSTALLED_HELPERS


def helper() -> Path:
    """The helper for this process: `locate` over the process environment and `programs.toml`, kept for a minute."""
    key = (os.environ.get(ENV_VARIABLE), programs.host_entry(PROGRAMS_KEY))
    cached = _located.get(key)
    if cached and time.monotonic() - cached[0] < LOCATE_CACHE_SECONDS:
        if cached[1] is None:
            raise LeewayNotFound(NOT_FOUND_TEXT)
        return cached[1]
    try:
        found = locate(os.environ, {PROGRAMS_KEY: key[1]})
    except LeewayNotFound:
        _located[key] = (time.monotonic(), None)
        raise
    _located[key] = (time.monotonic(), found)
    return found


def is_available() -> bool:
    """Whether a compatible helper is found."""
    try:
        helper()
    except LeewayNotFound:
        return False
    return True


def status() -> dict:
    """`leeway status --json` (schema 5), kept for a few seconds."""
    return _cached(("status",), lambda: _run_json(["status", "--json"], schema=("schema_version", 5), shape=_is_status))


def newest_reading(document: Mapping) -> str | None:
    """The time of the newest reading among the accounts of a status document, as the helper wrote it; None when no
    account has been read. It is what the snapshot shows as `usage.as_of`, so the app can say how old the numbers are."""
    times = [(moment, row["as_of"]) for row in document.get("accounts", ()) if (moment := _moment(row.get("as_of")))]
    return max(times)[1] if times else None


def refresh_if_stale() -> bool:
    """Ask the helper for one `refresh` when the readings are older than the polling interval; True when it was asked.

    The wait is bounded: a refresh that does not answer in `REFRESH_WAIT_SECONDS`, or fails, is given up on and the
    readings that are there are used, so a lane start never waits on a probe. A copy of Leeway that does not hold the poll
    lock only leaves a request for the one that does, which takes it at its next check. It is not asked again for a minute.
    """
    try:
        document = status()
    except LeewayError:
        return False
    if not _is_stale(document) or _refresh_was_asked_lately():
        return False
    _refresh_asked_at[:] = [time.monotonic()]
    try:
        _run_json(["refresh", "--json"], schema=("schema_version", 5), shape=_is_status, timeout=REFRESH_WAIT_SECONDS)
    except LeewayError:
        return True
    _answers.clear()
    return True


def _is_stale(document: dict) -> bool:
    """Whether the newest reading is older than `STALE_AFTER` at the time of the document (its own clock, as Leeway tells
    the age); a book with no account has nothing to refresh."""
    if not document["accounts"]:
        return False
    newest, made = _moment(newest_reading(document)), _moment(document["generated_at"])
    return newest is None or made is None or made - newest > STALE_AFTER


def _refresh_was_asked_lately() -> bool:
    return bool(_refresh_asked_at) and time.monotonic() - _refresh_asked_at[0] < REFRESH_RETRY_SECONDS


def _moment(text: str | None) -> datetime | None:
    try:
        moment = datetime.fromisoformat(text) if text else None
    except ValueError:
        return None
    return moment if moment is not None and moment.tzinfo else None


def config_dir(account_id: str, provider: str) -> str | None:
    """The config folder of the `provider` account `account_id` that Leeway lists as usable (as the snapshot's accounts
    do); None when Leeway is absent or unreadable, or does not list such an account."""
    try:
        rows = status()["accounts"]
    except LeewayError:
        return None
    folders = [row["config_dir"] for row in rows if row["id"] == account_id and row["provider"] == provider]
    return folders[0] if folders and folders[0] else None


def claude_config_dir(account_id: str) -> str | None:
    """The config folder of the Claude account `account_id` that Leeway lists as usable (see `config_dir`)."""
    return config_dir(account_id, "claude")


def accounts(*, refresh: bool = False) -> list[dict]:
    """`leeway accounts list --json`: the account book in display order, hidden accounts included. `refresh` bypasses the cache."""
    if refresh:
        _answers.pop(("accounts",), None)
    return _cached(("accounts",), lambda: _book(_run_json(["accounts", "list", "--json"], schema=("schema_version", 1), shape=_is_book)))


def account_action(verb: str, account_id: str, value: str | None = None) -> list[dict]:
    """Run `leeway accounts <verb> <id> [value]` and return the updated book. Raises ValueError for another verb."""
    if verb not in ACCOUNT_VERBS:
        raise ValueError(f"an account verb is one of {', '.join(ACCOUNT_VERBS)}; got {verb!r}")
    _require_plain(account_id, "an account id")
    if value is not None:
        _require_plain(value, "a name")
    words = ["accounts", verb, account_id, *([value] if value is not None else []), "--json"]
    book = _book(_run_json(words, schema=("schema_version", 1), shape=_is_book))
    _answers.clear()
    return book


def account_add(folder: str, provider: str) -> list[dict]:
    """Run `leeway accounts add FOLDER --provider PROVIDER` and return the updated book. The helper reads the folder's
    login itself; the engine never does. Raises ValueError for another provider or a folder the helper's parser would
    read as an option."""
    if provider not in ADD_PROVIDERS:
        raise ValueError(f"an account provider is one of {', '.join(ADD_PROVIDERS)}; got {provider!r}")
    _require_plain(folder, "a folder")
    book = _book(_run_json(["accounts", "add", folder, "--provider", provider, "--json"],
                           schema=("schema_version", 1), shape=_is_book))
    _answers.clear()
    return book


def advise(repo: str, role: str, task_class: str, account: str | None, target_pct: int | None,
           candidates: Sequence[str] | None) -> dict:
    """`leeway advise`: whether and where the work may run now (advice schema 2)."""
    for name in (account, *(candidates or ())):
        if name:
            _require_plain(name, "an account name")
    refresh_if_stale()
    words = ["advise", "--repo", str(repo), "--role", role, "--task-class", task_class]
    words += ["--account", account] if account else []
    words += ["--target", str(target_pct)] if target_pct is not None else []
    words += ["--candidates", ",".join(candidates)] if candidates else []
    return _run_json(words, schema=("schema_version", 2), shape=_is_advice)


def _require_plain(text: str, what: str) -> None:
    """Refuse `text` that starts with a dash: the helper's parser has no `--` terminator, so it would read it as an option."""
    if text.startswith("-"):
        raise ValueError(f"{what} cannot start with a dash; got {text!r}")


def run_text(words: Sequence[str]) -> tuple[int, str]:
    """Run the helper with `words` as they are and return its exit code and output (for a command that passes through)."""
    path = helper()
    try:
        done = subprocess.run([str(path), *words], capture_output=True, text=True, stdin=subprocess.DEVNULL,
                              timeout=CALL_TIMEOUT_SECONDS, env=programs.environment("system", str(path)))
    except (OSError, subprocess.SubprocessError) as error:
        raise LeewayUnavailable(f"leeway {words[0]} did not answer ({type(error).__name__})") from None
    return done.returncode, done.stdout


def _contract_of(candidate: str) -> tuple[tuple[int, tuple[int, ...]], str] | None:
    if programs.untrusted(candidate):
        return None
    document = _version_of(candidate)
    if not document or any(document.get(name) != number for name, number in CONTRACT.items()):
        return None
    return (_order(document), candidate)


def _version_of(candidate: str) -> dict | None:
    try:
        done = subprocess.run([candidate, "version", "--json"], capture_output=True, text=True, stdin=subprocess.DEVNULL,
                              timeout=VERSION_TIMEOUT_SECONDS, env=programs.environment("system", candidate))
        document = json.loads(done.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        return None
    return document if done.returncode == 0 and isinstance(document, dict) else None


def _order(document: dict) -> tuple[int, tuple[int, ...]]:
    parts = str(document.get("version", "")).split(".")
    return int(document.get("store_schema", 0)), tuple(int(part) if part.isdigit() else 0 for part in parts)


def _run_json(words: list[str], schema: tuple[str, int], shape: Callable[[dict], bool],
              timeout: float = CALL_TIMEOUT_SECONDS) -> dict:
    """Run the helper and return its document: only an exit of zero, the schema asked for and a document that
    `shape` accepts are an answer, so no caller reads a failed reply or a field that is not there."""
    path = helper()
    try:
        done = subprocess.run([str(path), *words], capture_output=True, text=True, stdin=subprocess.DEVNULL,
                              timeout=timeout, env=programs.environment("system", str(path)))
        document = json.loads(done.stdout)
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        raise LeewayUnavailable(f"leeway {words[0]} did not answer ({type(error).__name__})") from None
    if done.returncode != 0 or not isinstance(document, dict) or document.get(schema[0]) != schema[1] or not shape(document):
        raise LeewayUnavailable(f"leeway {words[0]} answered in a form the engine does not read")
    return document


def _is_status(document: dict) -> bool:
    """A status has its time and a list of accounts, each with every field the engine reads, of the type it reads."""
    rows = document.get("accounts")
    return isinstance(document.get("generated_at"), str) and isinstance(rows, list) and all(
        isinstance(row, dict) and _is_status_row(row) for row in rows)


def _is_status_row(row: dict) -> bool:
    return (all(isinstance(row.get(name), str) for name in STATUS_ROW_TEXT) and _is_text_or_none(row, "as_of", present=True)
            and all(isinstance(row.get(name), dict) and _is_window(row[name]) for name in STATUS_ROW_WINDOWS)
            and isinstance(row.get("forecast"), dict) and _is_forecast(row["forecast"])
            and isinstance(row.get("stale", False), bool) and _is_text_or_none(row, "plan"))


def _is_window(window: dict) -> bool:
    return _is_number_or_none(window, "pct", present=True) and _is_text_or_none(window, "resets_at", present=True)


def _is_forecast(forecast: dict) -> bool:
    return _is_number_or_none(forecast, "seven_day_pct_at_reset") and _is_text_or_none(forecast, "exhausts_at")


def _is_number_or_none(document: dict, name: str, present: bool = False) -> bool:
    value = document.get(name)
    return (not present or name in document) and (value is None or (isinstance(value, (int, float)) and not isinstance(value, bool)))


def _is_text_or_none(document: dict, name: str, present: bool = False) -> bool:
    value = document.get(name)
    return (not present or name in document) and (value is None or isinstance(value, str))


def _is_book(document: dict) -> bool:
    rows = document.get("accounts")
    return isinstance(rows, list) and all(isinstance(row, dict) and _is_book_row(row) for row in rows)


def _is_book_row(row: dict) -> bool:
    return (all(isinstance(row.get(name), str) for name in BOOK_ROW_TEXT) and isinstance(row.get("hidden"), bool)
            and isinstance(row.get("folder_exists", True), bool) and _is_text_or_none(row, "label"))


def _is_advice(document: dict) -> bool:
    return isinstance(document.get("verdict"), str)


def _book(document: dict) -> list[dict]:
    rows = document.get("accounts")
    if not isinstance(rows, list):
        raise LeewayUnavailable("leeway accounts answered in a form the engine does not read")
    return rows


def _cached(key: tuple, load):
    cached = _answers.get(key)
    if cached and time.monotonic() - cached[0] < STATUS_CACHE_SECONDS:
        return cached[1]
    value = load()
    _answers[key] = (time.monotonic(), value)
    return value
