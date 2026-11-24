"""Recognize direct codex exec invocations without executing shell input."""

from __future__ import annotations

import shlex
from dataclasses import dataclass
from pathlib import Path

SEPARATORS = frozenset(";&|()\n")
VALUE_OPTIONS = {"-m", "--model", "-c", "--config", "-C", "--cd", "--profile", "-p"}


@dataclass(frozen=True)
class Launch:
    """The routing inputs of one direct Codex process."""

    model: str | None
    env: dict
    repo: Path

    @property
    def is_resolved(self) -> bool:
        """Whether account and directory routing use literal, inspectable paths."""
        values = (str(self.repo), self.env.get("CODEX_HOME", ""), self.env.get("HOME", ""))
        return all(not value or (Path(value).is_absolute() and "$" not in value and "`" not in value)
                   for value in values)


def launches(command: str, env: dict, cwd: str) -> list[Launch]:
    """Track simple shell directory/environment changes before each direct launch."""
    local, repo, result = dict(env), Path(cwd), []
    for tokens in _segments(command):
        if tokens[0] == "cd":
            target = tokens[-1] if len(tokens) > 1 else local.get("HOME", str(Path.home()))
            repo = _directory(target, repo, local)
            continue
        if tokens[0] in {"export", "unset"}:
            _environment(tokens, local)
            continue
        args, current = _prefix(tokens, local)
        if not args:
            local = current
        elif Path(args[0]).name == "codex":
            launch = _exec(args[1:], current, repo)
            if launch:
                result.append(launch)
    return result


def _segments(command: str):
    lexer = shlex.shlex(command, posix=True, punctuation_chars=";&|()\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    segment = []
    for token in [*lexer, ";"]:
        if token and set(token) <= SEPARATORS:
            if segment:
                yield segment
                segment = []
        else:
            segment.append(token)


def _environment(tokens: list[str], env: dict) -> None:
    for token in tokens[1:]:
        if tokens[0] == "unset":
            env.pop(token, None)
        elif "=" in token:
            key, value = token.split("=", 1)
            env[key] = value


def _prefix(tokens: list[str], env: dict) -> tuple[list[str], dict]:
    local, index = dict(env), 0
    while index < len(tokens):
        token = tokens[index]
        if token == "env":
            index += 1
            while index < len(tokens) and tokens[index].startswith("-"):
                flag = tokens[index]
                if flag in {"-i", "--ignore-environment"}:
                    local.clear()
                elif flag in {"-u", "--unset"} and index + 1 < len(tokens):
                    index += 1
                    local.pop(tokens[index], None)
                index += 1
        elif token == "command":
            index += 1
            while index < len(tokens) and tokens[index] in {"--", "-p"}:
                index += 1
        elif "=" in token and not token.startswith("-"):
            key, value = token.split("=", 1)
            local[key] = value
            index += 1
        else:
            break
    return tokens[index:], local


def _exec(args: list[str], env: dict, repo: Path) -> Launch | None:
    model, is_exec, index = None, False, 0
    while index < len(args):
        token = args[index]
        if token == "--":
            break
        if token in VALUE_OPTIONS:
            if index + 1 >= len(args):
                break
            value = args[index + 1]
            if token in {"-m", "--model"}:
                model = value
            elif token in {"-C", "--cd"}:
                repo = _directory(value, repo, env)
            index += 2
            continue
        if token.startswith("--model="):
            model = token.split("=", 1)[1]
        elif token.startswith("--cd="):
            repo = _directory(token.split("=", 1)[1], repo, env)
        elif token in {"exec", "e"}:
            is_exec = True
        elif not token.startswith("-"):
            break
        index += 1
    return Launch(model, env, repo) if is_exec else None


def _directory(raw: str, base: Path, env: dict) -> Path:
    expanded = raw
    if expanded == "~" or expanded.startswith("~/"):
        expanded = str(Path(env.get("HOME", str(Path.home()))) / expanded[2:])
    path = Path(expanded)
    return (path if path.is_absolute() else base / path).resolve()
