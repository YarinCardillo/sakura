"""The digest's host file, `<engine home>/digest.toml`, and the folders the digest writes.

The file is read only from the engine home and never from a repository: a repository is written by agents, and the
file names programs the engine runs as the person. It must be owned by the person and closed to everyone else
(mode 0600), like every other host file that names a program.
"""

from __future__ import annotations

import json
import os
import re
import stat
import tomllib
import urllib.parse
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home
from sakura.budget import account as budget_account
from sakura.flow import gates
from sakura.usage import leeway
from sakura.usage.archive import settings as usage_settings

CONFIG_FILE = "digest.toml"
DIGEST_FOLDER = "digest"
LATEST_FILE = "latest.json"
STATE_FILE = "state.json"
LOCK_FILE = ".lock"
FILE_MODE = 0o600
FOLDER_MODE = 0o700
DEFAULT_AT = "07:30"
DEFAULT_MODEL = "sonnet"
DEFAULT_LANGUAGE = "en"
DEFAULT_LOOKBACK_HOURS = 24
MAX_LOOKBACK_HOURS = 24 * 7
DEFAULT_TIMEOUT_SECONDS = 120
MAX_TIMEOUT_SECONDS = 900
MAX_SOURCES = 20
MAX_ARGUMENTS = 64
MAX_ARGUMENT_CHARACTERS = 1000
MAX_FEED_URL = 500
TOP_KEYS = frozenset({"at", "model", "language", "lookback_hours", "account", "source"})
SOURCE_KEYS = frozenset({"id", "run", "feed", "connector", "enabled", "timeout_seconds", "label", "reads"})
MAX_LABEL, MAX_READS = 60, 160
SOURCE_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
TIME_OF_DAY = re.compile(r"([01]\d|2[0-3]):([0-5]\d)")
# `provider:model` as a role value is, or a bare name: a Claude model, as before (`codex` and `claude` alone name the program).
MODEL_NAME = re.compile(r"(?:(?:claude|codex):)?[a-z0-9][a-z0-9._-]{0,63}")
ACCOUNT_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
LANGUAGE = re.compile(r"[a-z]{2,3}(-[A-Za-z0-9]{2,8})?")
FEED_SCHEMES = ("https://", "http://")


class ConfigError(ValueError):
    """Raised, with a one-line reason, when the digest file is missing, unsafe or not valid."""


@dataclass(frozen=True)
class Source:
    """One place the digest looks: a program to run (`run`, absolute program first), a feed URL, or a connector (by its
    id, see `sakura connectors`), exactly one of them. A source that is not `enabled` stays in the file and is skipped."""

    id: str
    run: tuple[str, ...] = ()
    feed: str = ""
    timeout_seconds: int = DEFAULT_TIMEOUT_SECONDS
    connector: str = ""
    enabled: bool = True
    label: str = ""  # the name a person reads for the source, and what it reads; both optional (`digest/source_words.py`)
    reads: str = ""


@dataclass(frozen=True)
class DigestConfig:
    """The whole file: when to run, which model, the language of the text, how far back the first run looks, and the
    Claude account the summarizer runs on ("" is the default account, as for any role without `budget.account`)."""

    at: str
    model: str
    language: str
    lookback_hours: int
    sources: tuple[Source, ...]
    account: str = ""

    @property
    def active_sources(self) -> tuple[Source, ...]:
        """The sources that are switched on, in file order."""
        return tuple(source for source in self.sources if source.enabled)

    @property
    def minute_of_day(self) -> int:
        """The configured time as minutes after midnight, local time."""
        hours, minutes = self.at.split(":")
        return int(hours) * 60 + int(minutes)


def config_path() -> Path:
    """`digest.toml` in the engine home; not created."""
    return engine_home.root() / CONFIG_FILE


def digest_folder() -> Path:
    """The folder of the digest's own files in the engine home; not created."""
    return engine_home.root() / DIGEST_FOLDER


def load(path: Path | None = None, *, home: Path | None = None) -> DigestConfig:
    """Read and check the digest file (the engine home's by default); `home` stands in for the account home.

    A file named by `path` that lies in a repository is refused; the engine home's own file is the one trusted place.
    Raises ConfigError for a file open to others, unreadable, unknown keys or a bad value.
    """
    _require_host_file(path or config_path(), is_engine_home=path is None)
    path = path or config_path()
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError:
        raise ConfigError(f"no digest configuration: create {path} (mode 0600)") from None
    except (OSError, tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        raise ConfigError(f"cannot read {path}: {error}") from error
    return parse(data, home=home)


def load_or_default() -> DigestConfig:
    """The engine home's digest file, or the defaults with no sources when there is none yet."""
    try:
        return load()
    except ConfigError:
        if config_path().exists():
            raise
        return parse({})


def save(settings: DigestConfig, path: Path | None = None) -> None:
    """Write `settings` as the digest file (the engine home's by default), mode 0600, keeping the file it replaces as
    `digest.toml.bak`. Comments of the old file are not kept; the backup has them."""
    path = path or config_path()
    backup = path.with_name(path.name + ".bak")
    if path.is_file():
        gates.write_atomically(backup, path.read_bytes())
        os.chmod(backup, FILE_MODE)
    gates.write_atomically(path, dump(settings).encode())
    os.chmod(path, FILE_MODE)


def dump(settings: DigestConfig) -> str:
    """The digest file's text for `settings`; every string is a JSON string, which TOML reads the same."""
    lines = [f"at = {json.dumps(settings.at)}", f"model = {json.dumps(settings.model)}",
             f"language = {json.dumps(settings.language)}", f"lookback_hours = {settings.lookback_hours}"]
    if settings.account:
        lines.append(f"account = {json.dumps(settings.account)}")
    for source in settings.sources:
        lines += ["", "[[source]]", f"id = {json.dumps(source.id)}", *_source_lines(source)]
    return "\n".join(lines) + "\n"


def _source_lines(source: Source) -> list[str]:
    lines = ([f"run = {json.dumps(list(source.run))}"] if source.run else
             [f"feed = {json.dumps(source.feed)}"] if source.feed else [f"connector = {json.dumps(source.connector)}"])
    if source.timeout_seconds != DEFAULT_TIMEOUT_SECONDS:
        lines.append(f"timeout_seconds = {source.timeout_seconds}")
    lines += [f"{name} = {json.dumps(value)}" for name, value in (("label", source.label), ("reads", source.reads)) if value]
    if not source.enabled:
        lines.append("enabled = false")
    return lines


def parse(data: dict, *, home: Path | None = None) -> DigestConfig:
    """The configuration of an already parsed document; raises ConfigError."""
    unknown = sorted(set(data) - TOP_KEYS)
    if unknown:
        raise ConfigError(f"{CONFIG_FILE}: unknown key {unknown[0]!r}; the keys are {', '.join(sorted(TOP_KEYS))}")
    sources = _sources(data.get("source", []), home)
    return DigestConfig(_matching("at", data.get("at", DEFAULT_AT), TIME_OF_DAY, "a time like 07:30"),
                        _matching("model", data.get("model", DEFAULT_MODEL), MODEL_NAME, "a model name"),
                        _matching("language", data.get("language", DEFAULT_LANGUAGE), LANGUAGE, "a code like en or it"),
                        _lookback(data.get("lookback_hours", DEFAULT_LOOKBACK_HOURS)), sources,
                        _account(data.get("account", "")))


def _require_host_file(path: Path, *, is_engine_home: bool) -> None:
    if not is_engine_home and _is_inside_repository(path):
        raise ConfigError(f"{path} lies in a repository: the digest file is read from the engine home only")
    try:
        info = os.stat(path)
    except FileNotFoundError:
        return
    except OSError as error:
        raise ConfigError(f"cannot read {path}: {error}") from error
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
        raise ConfigError(f"{path} must be a regular file you own")
    if info.st_mode & 0o077:
        raise ConfigError(f"{path} is open to other users (mode {info.st_mode & 0o777:o}); run chmod 600 on it")


def _is_inside_repository(path: Path) -> bool:
    """Whether a folder of `path` holds a `.git` entry: a file there could have been written by an agent."""
    real = Path(os.path.realpath(path))
    return any((folder / ".git").exists() for folder in real.parents)


def _sources(entries: object, home: Path | None) -> tuple[Source, ...]:
    if not isinstance(entries, list) or not all(isinstance(entry, dict) for entry in entries):
        raise ConfigError(f"{CONFIG_FILE}: source must be a list of [[source]] tables")
    if len(entries) > MAX_SOURCES:
        raise ConfigError(f"{CONFIG_FILE}: {len(entries)} sources; the limit is {MAX_SOURCES}")
    found = tuple(_source(entry, home) for entry in entries)
    ids = [source.id for source in found]
    if len(set(ids)) != len(ids):
        raise ConfigError(f"{CONFIG_FILE}: source ids must be distinct")
    return found


def _source(entry: dict, home: Path | None) -> Source:
    unknown = sorted(set(entry) - SOURCE_KEYS)
    if unknown:
        raise ConfigError(f"{CONFIG_FILE}: source has unknown key {unknown[0]!r}; the keys are "
                          f"{', '.join(sorted(SOURCE_KEYS))}")
    source_id = _matching("source id", entry.get("id"), SOURCE_ID, "lowercase letters, digits and hyphens")
    if sum(key in entry for key in ("run", "feed", "connector")) != 1:
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} needs exactly one of run or feed, or a connector")
    timeout = _timeout(source_id, entry.get("timeout_seconds", DEFAULT_TIMEOUT_SECONDS))
    enabled = entry.get("enabled", True)
    if not isinstance(enabled, bool):
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} enabled must be true or false")
    words = {"label": _words(source_id, entry, "label", MAX_LABEL), "reads": _words(source_id, entry, "reads", MAX_READS)}
    if "run" in entry:
        return Source(source_id, run=_program(source_id, entry["run"], home), timeout_seconds=timeout, enabled=enabled, **words)
    if "connector" in entry:
        connector = _matching("source connector", entry["connector"], SOURCE_ID, "a connector id")
        return Source(source_id, connector=connector, timeout_seconds=timeout, enabled=enabled, **words)
    return Source(source_id, feed=_feed_url(source_id, entry["feed"]), timeout_seconds=timeout, enabled=enabled, **words)


def _words(source_id: str, entry: dict, key: str, limit: int) -> str:
    """The source's `label` or `reads`: one printable line of at most `limit` characters, or an empty string when absent."""
    text = entry.get(key, "")
    if not isinstance(text, str) or (key in entry and not text.strip()) or len(text) > limit or not text.isprintable():
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} {key} must be one printable line of at most {limit} characters")
    return text.strip()


def _program(source_id: str, run: object, home: Path | None) -> tuple[str, ...]:
    if not isinstance(run, list) or not run or not all(isinstance(word, str) for word in run):
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} run must be a list of words, the program first")
    if len(run) > MAX_ARGUMENTS or any(len(word) > MAX_ARGUMENT_CHARACTERS or "\0" in word for word in run):
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} run has too many or too long words")
    program = _expanded(run[0], home)
    if not os.path.isabs(program):
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} run must start with an absolute program path "
                          f"(or ~/...), not {run[0]!r}")
    return (program, *run[1:])


def _expanded(program: str, home: Path | None) -> str:
    if not program.startswith("~/"):
        return program
    return str((home or Path(os.path.expanduser("~"))) / program[2:])


def _feed_url(source_id: str, feed: object) -> str:
    if (not isinstance(feed, str) or not feed.startswith(FEED_SCHEMES) or len(feed) > MAX_FEED_URL
            or re.search(r"\s", feed) or not feed.isprintable()):
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} feed must be one http(s) URL of at most "
                          f"{MAX_FEED_URL} characters")
    if "@" in urllib.parse.urlsplit(feed).netloc:
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} feed must not carry credentials (user:password@); "
                          "a token the feed needs goes in the query or in a program of your own")
    return feed


def _timeout(source_id: str, value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= MAX_TIMEOUT_SECONDS:
        raise ConfigError(f"{CONFIG_FILE}: source {source_id!r} timeout_seconds must be 1 to {MAX_TIMEOUT_SECONDS}")
    return value


def _lookback(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= MAX_LOOKBACK_HOURS:
        raise ConfigError(f"{CONFIG_FILE}: lookback_hours must be 1 to {MAX_LOOKBACK_HOURS}")
    return value


def _account(value: object) -> str:
    """The account name, checked against the engine's own registry (the one `budget.account` is checked against)."""
    if value == "":
        return ""
    name = _matching("account", value, ACCOUNT_NAME, "an account name")
    try:
        usage = usage_settings.load()
        budget_account.apply({}, name, usage.sources, usage.disabled)
    except budget_account.UnknownAccountError as error:
        if leeway.claude_config_dir(name) is None:  # the apps send Leeway's account ids
            raise ConfigError(f"{CONFIG_FILE}: account {error}") from error
    except usage_settings.UsageConfigError as error:
        raise ConfigError(f"{CONFIG_FILE}: account {error}") from error
    return name


def _matching(name: str, value: object, pattern: re.Pattern, meaning: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise ConfigError(f"{CONFIG_FILE}: {name} must be {meaning}; got {str(value)[:40]!r}")
    return value
