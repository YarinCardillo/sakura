"""Agent-tool command guards and guarded-file fingerprints."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
from contextlib import contextmanager
from pathlib import Path

from sakura import engine_home, profile
from sakura.flow import gitutil

_WRITE_PROGRAMS = {"rm", "mv", "cp", "tee", "touch", "truncate", "chmod", "chown", "install"}
# Claude Code writes these under the workspace's `.claude` while its sandbox is on (the engine cage turns it on).
TOOL_RECORDS = frozenset({".cc-writes"})
_GIT_OPTIONS_WITH_VALUES = {"-C", "--git-dir", "--work-tree", "--namespace", "--config-env", "--exec-path", "--super-prefix"}
_GH_OPTIONS_WITH_VALUES = {"--repo", "-R", "--hostname", "--config"}
_SILENT_REDIRECT = re.compile(r"(?:^|\s)(?:\d?>>?|&>>?)\s*/dev/null\b")
_IN_PLACE_PROGRAMS = {"sed", "perl"}
_SILENT_CHECK_REASON = "silent check: print every check's outcome"
_SCAN_BYPASS_REASON = "secret scan bypass is refused; fix the false positive with a narrow allowlist"


def command_decision(repo: Path, command: str) -> tuple[str, str] | None:
    """Return a hook decision and reason for an unsafe shell command."""
    settings = profile.load(repo).values
    tokens = _tokens(command)
    early = _silent_check_reason(command) or _scan_bypass_reason(tokens)
    if early:
        return "deny", early
    if _unsupported_shell(command):
        return "deny", "unsupported shell syntax is refused"
    reason = _bypass_reason(command, tokens)
    if reason:
        return "deny", reason
    reason = _github_reason(tokens, settings)
    if reason:
        return reason
    if _writes_guarded(repo, command):
        return "deny", "writing sakura guarded files is refused"
    return None


def write_targets(repo: Path, command: str) -> list[Path]:
    """Return the resolved paths a shell command writes: redirects, write programs and `sed -i`/`perl -i`."""
    names = re.findall(r">{1,2}\s*([^\s;&|]+)", command)
    tokens = _tokens(command)
    program = Path(tokens[0]).name if tokens else ""
    arguments = [token for token in tokens[1:] if not token.startswith("-")]
    if program in _WRITE_PROGRAMS:
        names.extend(arguments)
    elif program in _IN_PLACE_PROGRAMS and any(_is_in_place_flag(token) for token in tokens[1:]):
        names.extend(arguments[1:] if program == "sed" and len(tokens) > 2 else arguments[-1:])
    return [(Path(name) if Path(name).is_absolute() else repo / name).resolve() for name in names]


def _is_in_place_flag(token: str) -> bool:
    return token.startswith("-") and not token.startswith("--") and "i" in token[1:] or token.startswith("--in-place")


def _silent_check_reason(command: str) -> str | None:
    return _SILENT_CHECK_REASON if "&&" in command and _SILENT_REDIRECT.search(command) else None


def _scan_bypass_reason(tokens: list[str]) -> str | None:
    program, start = program_of(tokens)
    if program != "git":
        return None
    for token in tokens[:start]:
        key, _, value = token.partition("=")
        if key == "SKIP" or (key == "GITLEAKS_ENABLE" and value.lower() == "false"):
            return _SCAN_BYPASS_REASON
    return None


def _tokens(segment: str) -> list[str]:
    try:
        return shlex.split(segment, comments=False)
    except ValueError:
        return segment.split()


def _bypass_reason(segment: str, tokens: list[str]) -> str | None:
    if any(token.partition("=")[0].upper().startswith("GIT_CONFIG_") for token in tokens):
        return "GIT_CONFIG overrides are refused"
    program, start = program_of(tokens)
    if program != "git":
        return None
    options = tokens[start:]
    if any(_matches_long_option(token, "--config-env") for token in options):
        return "git configuration overrides are refused"
    for key, value in _git_configs(options):
        if key in {"core.hookspath", "commit.gpgsign"} and (key == "core.hookspath" or value.lower() == "false"):
            return "git hook and signing configuration overrides are refused"
    subcommand = _git_subcommand(options)
    if subcommand in {"commit", "push"} and (
        any(_matches_long_option(option, "--no-verify") for option in options)
        or (subcommand == "commit" and _has_short_option(options, "n"))
    ):
        return "--no-verify is refused"
    if subcommand == "config" and any("core.hookspath" in token.lower() for token in options):
        return "changing core.hooksPath is refused"
    return None


def _git_configs(tokens: list[str]) -> list[tuple[str, str]]:
    found: list[tuple[str, str]] = []
    for index, token in enumerate(tokens):
        if token == "-c":
            if index + 1 == len(tokens):
                continue
            key, sep, value = tokens[index + 1].partition("=")
            if sep:
                found.append((key.lower(), value))
        elif token.startswith("-c"):
            key, sep, value = token[2:].partition("=")
            if sep:
                found.append((key.lower(), value))
    return found


def _matches_long_option(option: str, canonical: str) -> bool:
    name = option.partition("=")[0]
    return name.startswith("--") and canonical.startswith(name)


def _has_short_option(tokens: list[str], flag: str) -> bool:
    return any(token.startswith("-") and not token.startswith("--") and flag in token[1:] for token in tokens)


def _git_subcommand(tokens: list[str]) -> str:
    index = 1
    while index < len(tokens) and tokens[index].startswith("-"):
        option = tokens[index]
        if option == "-c" or option in _GIT_OPTIONS_WITH_VALUES or any(
            _matches_long_option(option, value) for value in _GIT_OPTIONS_WITH_VALUES
        ):
            if "=" in option:
                index += 1
            else:
                index += 2
        else:
            index += 1
    return tokens[index] if index < len(tokens) else ""


def _github_reason(tokens: list[str], settings: dict) -> tuple[str, str] | None:
    program, start = program_of(tokens)
    args = _gh_arguments(tokens[start + 1 :])
    if program == "gh" and args[:2] == ["pr", "create"] and settings["require_draft_pr"]:
        if not _draft_enabled(args[2:]):
            return "deny", "open pull requests as drafts"
    if program == "gh" and args[:2] == ["pr", "ready"] and settings["ask_before_pr_ready"]:
        return "ask", "ask the user before marking a pull request ready"
    return None


def _unsupported_shell(command: str) -> bool:
    return any(token in command for token in ("$(`", "$(", "`", ";", "&&", "||", "|", "&", "<(", ">(", "\n"))


def program_of(tokens: list[str]) -> tuple[str, int]:
    index = 0
    while index < len(tokens) and "=" in tokens[index] and not tokens[index].startswith("="):
        index += 1
    while index < len(tokens) and Path(tokens[index]).name in {"env", "command"}:
        index += 1
        while index < len(tokens) and (tokens[index].startswith("-") or "=" in tokens[index]):
            index += 2 if tokens[index] == "-u" else 1
    return (Path(tokens[index]).name if index < len(tokens) else "", index)


def _draft_enabled(options: list[str]) -> bool:
    for option in options:
        if option in {"--draft", "-d"}:
            return True
        if _matches_long_option(option, "--draft") and "=" in option:
            return option.partition("=")[2].lower() in {"1", "true", "yes"}
    return False


def _gh_arguments(args: list[str]) -> list[str]:
    index = 0
    while index < len(args) and args[index].startswith("-"):
        option = args[index]
        if option in _GH_OPTIONS_WITH_VALUES or any(_matches_long_option(option, value) for value in _GH_OPTIONS_WITH_VALUES):
            index += 1 if "=" in option else 2
        else:
            index += 1
    return args[index:]


def guarded_paths(repo: Path) -> list[Path]:
    """Return files whose local enforcement configuration agents cannot edit."""
    kit = Path(__file__).resolve().parents[2]
    home = Path.home()
    return [
        kit / "bin",
        kit / "git-hooks",
        kit / "lib",
        repo / ".sakura",
        repo / ".claude",
        repo / ".claude/settings.json",
        repo / ".codex/hooks.json",
        repo / ".codex/config.toml",
        home / ".claude/settings.json",
        home / ".codex/hooks.json",
        home / ".codex/config.toml",
    ]


def fingerprint(repo: Path) -> dict[str, str]:
    """Hash guarded files for a post-command authoritative integrity check."""
    values: dict[str, str] = {}
    for target in guarded_paths(repo):
        files = _files(target)
        for file in files:
            values[str(file)] = _file_fingerprint(file)
        if not files:
            values[str(target)] = "missing"
    values.update(_git_config(repo))
    return values


def save_snapshot(repo: Path, session: str) -> None:
    """Save a guarded-file snapshot below the common Git directory."""
    target = _snapshot_path(repo, session)
    with _locked(target):
        if not target.exists():
            target.write_text(json.dumps({"repo": str(repo.resolve()), "baseline": fingerprint(repo), "failed": False}, sort_keys=True))


def changed_since_snapshot(repo: Path, session: str) -> bool:
    """Fail closed when a snapshot is unavailable or any guarded file changed."""
    target = _snapshot_path(repo, session)
    with _locked(target):
        try:
            state = json.loads(target.read_text())
            snapshot_repo = Path(state.get("repo", repo)).resolve()
            current = fingerprint(snapshot_repo)
            differences = sorted(key for key in state["baseline"].keys() | current.keys()
                                 if state["baseline"].get(key) != current.get(key))
            changed = state["failed"] or bool(differences)
        except (OSError, ValueError, KeyError):
            return True
        if changed and not state["failed"]:
            state["failed"] = True
            state["changed"] = differences
            target.write_text(json.dumps(state, sort_keys=True))
        return changed


def any_snapshot_changed(repo: Path) -> bool:
    """Return whether any retained agent session has a changed guard baseline."""
    directory = engine_home.kit_dir(repo) / "fingerprints"
    return any(changed_since_snapshot(repo, path.stem) for path in directory.glob("*.json"))


def _snapshot_path(repo: Path, session: str) -> Path:
    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", session or "anonymous")
    return engine_home.kit_dir(repo) / "fingerprints" / f"{safe}.json"


def _writes_guarded(repo: Path, segment: str) -> bool:
    return any(_is_guarded(repo, target) for target in write_targets(repo, segment))


def _is_guarded(repo: Path, target: Path) -> bool:
    resolved = (target if target.is_absolute() else repo / target).resolve()
    return any(guarded.resolve() == resolved or guarded.resolve() in resolved.parents for guarded in guarded_paths(repo))


def _files(path: Path) -> list[Path]:
    if path.exists() or path.is_symlink():
        if not path.is_dir() or path.is_symlink():
            return [path]
        children = [child for child in path.rglob("*") if not _is_tool_record(child)]
        # A `.claude` folder that holds only Claude Code's own records is the same as no folder: its sandbox
        # creates it in the middle of a run, and nothing in it configures anything.
        return [path, *children] if children or path.name != ".claude" else []
    return []


def _is_tool_record(path: Path) -> bool:
    """A file Claude Code itself writes under `.claude` while its sandbox runs; it configures nothing."""
    return path.name in TOOL_RECORDS and path.parent.name == ".claude"


def _file_fingerprint(path: Path) -> str:
    return hashlib.sha256(_fingerprint_node(path, set())).hexdigest()


def _fingerprint_node(path: Path, ancestors: set[tuple[int, int]]) -> bytes:
    metadata = os.lstat(path)
    value = f"{metadata.st_mode & 0o7777}:{path.is_symlink()}:".encode()
    if path.is_symlink():
        target = os.readlink(path).encode()
        try:
            return value + target + _fingerprint_node(path.resolve(strict=True), ancestors)
        except (OSError, RuntimeError):
            return value + target + b"unresolved"
    identity = (metadata.st_dev, metadata.st_ino)
    if identity in ancestors:
        return value + b"cycle"
    if path.is_file():
        return value + path.read_bytes()
    if not path.is_dir():
        return value
    child_ancestors = ancestors | {identity}
    children = sorted((child for child in path.iterdir() if not _is_tool_record(child)), key=lambda child: child.name)
    return value + b"".join(child.name.encode() + _fingerprint_node(child, child_ancestors) for child in children)


def _git_config(repo: Path) -> dict[str, str]:
    try:
        done = gitutil.run(repo, "config", "--show-origin", "--includes", "--list", text=False, hardened=False,
                           person_config=True)
    except gitutil.GitError:
        return {"git-config:unavailable": "unavailable"}
    if done.returncode:
        return {"git-config:unavailable": "unavailable"}
    entries: dict[bytes, list[bytes]] = {}
    for line in done.stdout.splitlines():
        setting = line.rsplit(b"\t", 1)[-1]
        key = setting.partition(b"=")[0].lower()
        if not _is_upstream_tracking_setting(key):
            entries.setdefault(key, []).append(line)
    return {f"git-config:{key.decode(errors='replace')}": hashlib.sha256(b"\n".join(sorted(lines))).hexdigest()
            for key, lines in entries.items()}


def _is_upstream_tracking_setting(key: bytes) -> bool:
    return key.startswith(b"branch.") and key.endswith((b".remote", b".merge"))


@contextmanager
def _locked(target: Path):
    target.parent.mkdir(parents=True, exist_ok=True)
    lock = target.with_suffix(".lock")
    descriptor = os.open(lock, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        import fcntl

        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)
