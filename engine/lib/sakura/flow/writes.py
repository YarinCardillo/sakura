"""What a PreToolUse payload writes: file paths with the text removed and added."""

from __future__ import annotations

import re
import shlex
from dataclasses import dataclass
from pathlib import Path

from sakura import guard

PATCH_FILE = re.compile(r"^\*\*\* (?:Add|Update|Delete) File: (.+)$")
PATCH_MOVE = re.compile(r"^\*\*\* Move to: (.+)$")
PATCH_TOOLS = {"apply_patch", "ApplyPatch"}
READ_PROGRAMS = {"cat", "head", "tail", "less", "sed"}
SEPARATOR_CHARS = ";&|()"


@dataclass(frozen=True)
class WriteIntent:
    """One resolved file a tool call writes; `old` and `new` are the text removed and added."""

    path: Path
    old: str
    new: str


def extract(repo: Path, payload: dict) -> list[WriteIntent]:
    """Return every file the tool call writes; empty for tools that do not write."""
    tool, data = payload.get("tool_name"), payload.get("tool_input") or {}
    if tool == "Edit":
        return _intent(repo, data.get("file_path"), data.get("old_string"), data.get("new_string"))
    if tool == "MultiEdit":
        edits = [edit for edit in data.get("edits") or [] if isinstance(edit, dict)]
        return _intent(repo, data.get("file_path"), _join(edits, "old_string"), _join(edits, "new_string"))
    if tool == "Write":
        target = _resolve(repo, data.get("file_path"))
        return [WriteIntent(target, _disk_text(target), str(data.get("content") or ""))] if target else []
    if tool == "NotebookEdit":
        return _intent(repo, data.get("notebook_path"), "", data.get("new_source"))
    if tool in PATCH_TOOLS:
        return _patch_intents(repo, [value for value in data.values() if isinstance(value, str)])
    if tool == "Bash":
        command = str(data.get("command") or "")
        return [WriteIntent(path, "", command) for path in guard.write_targets(repo, command)]
    return []


def read_paths(repo: Path, payload: dict) -> list[Path]:
    """Return the files a Read call or a `cat`-style Bash command reads."""
    tool, data = payload.get("tool_name"), payload.get("tool_input") or {}
    if tool == "Read":
        target = _resolve(repo, data.get("file_path"))
        return [target] if target else []
    if tool != "Bash":
        return []
    tokens = _split(str(data.get("command") or ""))
    if not tokens or Path(tokens[0]).name not in READ_PROGRAMS:
        return []
    return [path for path in (_resolve(repo, token) for token in tokens[1:] if not token.startswith("-")) if path]


def segments(command: str) -> list[list[str]]:
    """Split a shell command line into the token lists of its simple commands.

    Separators (`;`, `&`, `&&`, `|`, `||`, parentheses, backticks, newlines) end a segment, also
    inside `$(...)`; an unbalanced quote falls back to a whitespace split.
    """
    lexer = shlex.shlex(command.replace("`", ";").replace("\n", ";"), posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    try:
        tokens = list(lexer)
    except ValueError:
        return [part.split() for part in re.split(r"[;&|()`\n]+", command) if part.strip()]
    result: list[list[str]] = [[]]
    for token in tokens:
        if token and set(token) <= set(SEPARATOR_CHARS):
            result.append([])
        else:
            result[-1].append(token)
    return [segment for segment in result if segment]


def _split(command: str) -> list[str]:
    try:
        return shlex.split(command)
    except ValueError:
        return command.split()


def _intent(repo: Path, raw: object, old: object, new: object) -> list[WriteIntent]:
    target = _resolve(repo, raw)
    return [WriteIntent(target, str(old or ""), str(new or ""))] if target else []


def _resolve(repo: Path, raw: object) -> Path | None:
    if not isinstance(raw, str) or not raw:
        return None
    path = Path(raw)
    return (path if path.is_absolute() else repo / path).resolve()


def _disk_text(path: Path) -> str:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return ""


def _join(edits: list[dict], key: str) -> str:
    return "\n".join(str(edit.get(key) or "") for edit in edits)


def _patch_intents(repo: Path, texts: list[str]) -> list[WriteIntent]:
    intents: list[WriteIntent] = []
    for text in texts:
        sections: list[tuple[Path | None, list[str], list[str]]] = []
        for line in text.splitlines():
            header, move = PATCH_FILE.match(line), PATCH_MOVE.match(line)
            if header or move:
                sections.append((_resolve(repo, (header or move).group(1).strip()), [], []))
            elif sections and line[:1] in "-+" and not line.startswith(("---", "+++")):
                sections[-1][1 if line[0] == "-" else 2].append(line[1:])
        intents += [WriteIntent(path, "\n".join(old), "\n".join(new)) for path, old, new in sections if path]
    return intents
