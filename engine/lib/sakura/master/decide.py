"""The engine's answer to one ACP permission request of the master (`sakura master decide --json`).

The app forwards each `session/request_permission` request to the engine and shows the person only what the engine
answers `ask`. The engine never answers `allow`: Claude Code applies the master's allow rules itself before it asks, so
a request that reaches the app is one those rules did not grant, and matching an allow rule here a second time could
only differ from Claude Code's own (a URL host, a path spelling) in the dangerous direction. The answer comes from the
settings the engine wrote for the master (`master/spec.py`), never from a second policy:

- `deny`: a deny rule of the settings matches, the cage forbids the call (a write outside the master's folder or into a
  protected path, a read inside a denied folder), a Glob or Grep pattern reaches outside the working folder, or a
  WebFetch address is not a plain http(s) address;
- `ask`: everything else, so it reaches the person.

Paths are compared canonically: links resolved, and case folded when the volume ignores case (or when that is unknown).
A request that cannot be read (not JSON, wrong method, no tool name, oversized) is denied with the reason, never asked.
The deny rule forms understood are the ones the engine writes: a tool name, `Tool(specifier)` with a Bash command or
`command *` prefix, a path pattern for the read and edit tools and `domain:<host>` for WebFetch. A deny rule whose
specifier is not understood matches (the call is denied).
"""

from __future__ import annotations

import json
import os
import re
import unicodedata
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

from sakura.flow import cage_server, globs

MAX_REQUEST_BYTES = 1024 * 1024
METHOD = "session/request_permission"
DENY, ASK = "deny", "ask"
PATH_KEYS = ("file_path", "notebook_path", "path")
OPTIONAL_PATH_TOOLS = ("Glob", "Grep", "LS")
EDIT_RULE, READ_RULE = "Edit", "Read"
MCP_PREFIX = "mcp__"
SPECIFIER_RULE = re.compile(r"([A-Za-z0-9_.\-]+)(?:\((.*)\))?", re.DOTALL)
SIMPLE_COMMAND = re.compile(r"[A-Za-z0-9._/+ -]{1,4096}")
LS_FLAGS = frozenset("aAldFhrt1")
BASH_INPUT_FIELDS = frozenset({"command", "cwd", "description", "timeout"})
BASH_SEPARATORS = re.compile(r"&&|\|\||[;|&\n`()]|\$\(")
GLOB_KEYS = {"Glob": "pattern", "Grep": "glob"}
PATH_SEGMENTS = re.compile(r"[\\/]")
URL_SCHEMES = ("http://", "https://")
CASE_SENSITIVE_NAME = "PC_CASE_SENSITIVE"
REASON_LIMIT = 200


class MalformedRequest(ValueError):
    """Raised when the request is not one the engine can judge; the message is the reason given to the caller."""


@dataclass(frozen=True)
class Request:
    """The tool the master wants to use and the input it gave it."""

    tool: str
    tool_input: dict
    paths: tuple[str, ...]


@dataclass(frozen=True)
class Touched:
    """The absolute paths a call touches: as written, and with every link resolved."""

    written: tuple[str, ...] = ()
    resolved: tuple[str, ...] = ()

    @property
    def both(self) -> tuple[str, ...]:
        return (*self.written, *self.resolved)


@dataclass(frozen=True)
class Decision:
    """`deny` or `ask`, with the reason in plain words."""

    decision: str
    reason: str

    def document(self) -> dict:
        """The decision as plain data: what `sakura master decide --json` prints."""
        return {"decision": self.decision, "reason": self.reason}


def oversized() -> Decision:
    """The decision for a request longer than `MAX_REQUEST_BYTES`."""
    return Decision(DENY, f"the request is larger than {MAX_REQUEST_BYTES} bytes")


def load_request(raw: bytes) -> Request:
    """Parse one ACP request (the whole JSON-RPC message, or its params). Raises MalformedRequest."""
    try:
        message = json.loads(raw.decode("utf-8"))
    except ValueError as error:
        raise MalformedRequest(f"the request is not JSON text: {error}") from error
    if not isinstance(message, dict):
        raise MalformedRequest("the request is not a JSON object")
    if "method" in message and message["method"] != METHOD:
        raise MalformedRequest(f"the request is not a {METHOD} request")
    params = message.get("params", message)
    call = params.get("toolCall") if isinstance(params, dict) else None
    if not isinstance(call, dict):
        raise MalformedRequest("the request has no toolCall")
    tool, tool_input = _tool_name(call), call.get("rawInput", {})
    if not isinstance(tool_input, dict):
        raise MalformedRequest("the tool call's rawInput is not an object")
    if tool == "Bash" and any("commandActions" in part or "command_actions" in part for part in (params, call)):
        raise MalformedRequest("the command approval has unsupported action metadata")
    return Request(tool, tool_input, _paths(tool, tool_input))


def scoped(message: dict, grants: list[str]) -> Decision:
    """Allow qualified connections to ask only for an engine-granted MCP tool."""
    try:
        request = load_request(json.dumps(message).encode())
    except (MalformedRequest, ValueError):
        return Decision(DENY, "master_invalid_permission")
    tool = request.tool.removeprefix("mcp__sakura__")
    if tool not in grants or tool == "permission":
        return Decision(DENY, "master_tool_outside_grant")
    return Decision(ASK, "master_tool_granted")


def decide(request: Request, settings: dict, cwd: Path) -> Decision:
    """The decision for `request` under the master's `settings` (its `--settings` file) working in `cwd`."""
    fold = _folder(cwd)
    paths = _touched(request, cwd, fold)
    permissions = settings.get("permissions") or {}
    for rule in permissions.get("deny") or ():
        if _rule_matches(str(rule), request, paths, cwd, fold):
            return Decision(DENY, f"denied by the master's rule {_short(rule)}")
    if request.tool == "Bash":
        problem = _bash_problem(request.tool_input, cwd)
        if problem:
            return Decision(DENY, problem)
    forbidden = (_cage_forbids(request, paths, (settings.get("sandbox") or {}).get("filesystem") or {}, cwd, fold)
                 or _pattern_escapes(request) or _address_refused(request))
    if forbidden:
        return Decision(DENY, forbidden)
    return Decision(ASK, f"no deny rule of the master covers {request.tool}")


# -- the request


def _tool_name(call: dict) -> str:
    meta = call.get("_meta")
    claude = meta.get("claudeCode") if isinstance(meta, dict) else None
    name = claude.get("toolName") if isinstance(claude, dict) and "toolName" in claude else call.get("name")
    if not isinstance(name, str) or not name.strip() or len(name) > REASON_LIMIT:
        raise MalformedRequest("the tool call does not name its tool (`_meta.claudeCode.toolName` or `name`)")
    return name


def _paths(tool: str, tool_input: dict) -> tuple[str, ...]:
    if tool not in cage_server.WRITE_TOOLS and tool not in cage_server.READ_TOOLS:
        return ()
    found = [tool_input[key] for key in PATH_KEYS if key in tool_input]
    if not all(isinstance(path, str) and path and "\0" not in path for path in found):
        raise MalformedRequest("a path of the tool call is not a non-empty text")
    if not found and tool not in OPTIONAL_PATH_TOOLS:
        raise MalformedRequest(f"the {tool} call names no path")
    return tuple(found)


def _touched(request: Request, cwd: Path, fold: Callable[[str], str]) -> Touched:
    """Every path the call touches, canonical; the working folder for a search that names none."""
    named = request.paths or ((str(cwd),) if request.tool in OPTIONAL_PATH_TOOLS else ())
    written = [os.path.normpath(os.path.join(cwd, path)) for path in named]
    return Touched(tuple(fold(path) for path in written), tuple(fold(os.path.realpath(path)) for path in written))


def _folder(cwd: Path) -> Callable[[str], str]:
    """How names are folded for comparison: case-folded unless the working folder's volume is case sensitive."""
    return _casefold if _case_insensitive(cwd) else _same


def _case_insensitive(path: Path) -> bool:
    """Whether the volume of `path` ignores case; True when that cannot be told (the safe answer: more matches)."""
    probe = Path(os.path.realpath(path))
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent
    try:
        return os.pathconf(probe, CASE_SENSITIVE_NAME) == 0
    except (OSError, ValueError):
        return True


def _casefold(text: str) -> str:
    return unicodedata.normalize("NFC", text).casefold()


def _same(text: str) -> str:
    return text


def _short(rule: object) -> str:
    return str(rule)[:REASON_LIMIT]


# -- the cage


def _cage_forbids(request: Request, paths: Touched, filesystem: dict, cwd: Path, fold: Callable[[str], str]) -> str:
    """The reason when the sandbox's filesystem block forbids the call, else an empty string."""
    if request.tool in cage_server.WRITE_TOOLS:
        return _write_forbidden(paths, filesystem, cwd, fold)
    if request.tool in cage_server.READ_TOOLS:
        return _read_forbidden(paths, filesystem, fold)
    return ""


def _write_forbidden(paths: Touched, filesystem: dict, cwd: Path, fold: Callable[[str], str]) -> str:
    protected = [pattern for entry in filesystem.get("denyWrite") or () for pattern in _folder_patterns(entry, fold)]
    if any(globs.matches_any(path, protected) for path in paths.both):
        return "the master's cage protects that path from writes"
    roots = [fold(os.path.realpath(root)) for root in (cwd, *(filesystem.get("allowWrite") or ()))]
    if not all(any(_inside(path, root) for root in roots) for path in paths.resolved):
        return "the master writes only inside its own folder"
    return ""


def _read_forbidden(paths: Touched, filesystem: dict, fold: Callable[[str], str]) -> str:
    """A path inside a denied folder is forbidden unless a narrower allowed entry lies between (the narrower rule wins)."""
    allowed = [fold(entry) for entry in filesystem.get("allowRead") or ()]
    for path in paths.both:
        for entry in filesystem.get("denyRead") or ():
            if (globs.matches_any(path, _folder_patterns(entry, fold))
                    and not _narrower_allowed(path, fold(entry), allowed, fold)):
                return "the master's cage does not let it read that path"
    return ""


def _narrower_allowed(path: str, denied: str, allowed: list[str], fold: Callable[[str], str]) -> bool:
    return any(entry.startswith(denied.rstrip("/") + "/") and globs.matches_any(path, _folder_patterns(entry, fold))
               for entry in allowed)


def _folder_patterns(entry: str, fold: Callable[[str], str]) -> list[str]:
    entry = fold(entry)
    return [entry, entry.rstrip("/") + "/**"]


def _inside(path: str, root: str) -> bool:
    return path == root or path.startswith(root.rstrip("/") + "/")


def _pattern_escapes(request: Request) -> str:
    """The reason when a Glob or Grep pattern names a place outside the working folder: absolute, home or `..`."""
    key = GLOB_KEYS.get(request.tool)
    if key is None or key not in request.tool_input:
        return ""
    pattern = request.tool_input[key]
    if not isinstance(pattern, str):
        return f"the {request.tool} {key} pattern is not text"
    if pattern.startswith(("/", "\\", "~")) or ".." in PATH_SEGMENTS.split(pattern):
        return f"the {request.tool} {key} pattern reaches outside the working folder"
    return ""


def _address_refused(request: Request) -> str:
    """The reason when a WebFetch address is not a plain http(s) address that every parser reads the same way."""
    if request.tool != "WebFetch":
        return ""
    url = request.tool_input.get("url")
    if not isinstance(url, str) or not url.lower().startswith(URL_SCHEMES):
        return "the WebFetch address is not an http or https address"
    if "\\" in url or "@" in url or any(char.isspace() or unicodedata.category(char) == "Cc" for char in url):
        return "the WebFetch address has a backslash, an @, a space or a control character"
    return ""


# -- the deny rules


def _rule_matches(rule: str, request: Request, paths: Touched, cwd: Path, fold: Callable[[str], str]) -> bool:
    parsed = SPECIFIER_RULE.fullmatch(rule.strip())
    if parsed is None:
        return True
    name, specifier = parsed.groups()
    if not _tool_matches(name, request.tool):
        return False
    if specifier is None:
        return True
    judged = _specifier_matches(specifier, request, paths, cwd, fold)
    return True if judged is None else judged


def _tool_matches(rule_tool: str, tool: str) -> bool:
    if rule_tool == tool:
        return True
    if rule_tool == EDIT_RULE:
        return tool in cage_server.WRITE_TOOLS
    if rule_tool == READ_RULE:
        return tool in cage_server.READ_TOOLS
    return rule_tool.startswith(MCP_PREFIX) and tool.startswith(rule_tool + "__")


def _specifier_matches(specifier: str, request: Request, paths: Touched, cwd: Path,
                       fold: Callable[[str], str]) -> bool | None:
    """Whether the specifier covers the call; None when this tool has no specifier form the engine understands."""
    tool = request.tool
    if tool == "Bash":
        command = request.tool_input.get("command")
        return _bash_matches(specifier, command) if isinstance(command, str) else None
    if tool in cage_server.WRITE_TOOLS or tool in cage_server.READ_TOOLS:
        return _path_matches(specifier, paths, cwd, fold)
    if tool == "WebFetch":
        return _domain_matches(specifier, request.tool_input.get("url"))
    return None


def _bash_problem(tool_input: dict, root: Path) -> str:
    if set(tool_input) - BASH_INPUT_FIELDS:
        return "the command approval has unsupported execution options"
    cwd = _bash_folder(tool_input.get("cwd", str(root)), root)
    if cwd is None:
        return "the command working directory must be canonical and inside the master's folder"
    command = tool_input.get("command")
    if not isinstance(command, str) or not SIMPLE_COMMAND.fullmatch(command) or not command.strip():
        return "the engine cannot verify this shell command"
    program, *arguments = command.split()
    if (program in {"pwd", "true"} and not arguments) or (program == "git" and arguments == ["status"]):
        return ""
    if program == "ls" and all(_ls_argument(word, cwd) for word in arguments):
        return ""
    return "the engine cannot verify this shell command"


def _bash_folder(value: object, root: Path) -> Path | None:
    if not isinstance(value, str) or not os.path.isabs(value):
        return None
    try:
        folder = Path(value).resolve(strict=True)
        if str(folder) == value and folder.is_dir() and folder.is_relative_to(root.resolve(strict=True)):
            return folder
    except (OSError, ValueError, RuntimeError):
        pass
    return None


def _ls_argument(word: str, cwd: Path) -> bool:
    if word.startswith("-"):
        return len(word) > 1 and set(word[1:]) <= LS_FLAGS
    path = cwd / word
    try:
        resolved = path.resolve(strict=True)
    except (OSError, ValueError, RuntimeError):
        return False
    return resolved == path and resolved.is_relative_to(cwd)


def _bash_matches(specifier: str, command: str) -> bool:
    """Whether any part of the command (split at every operator and substitution) matches the rule."""
    parts = [" ".join(part.split()) for part in BASH_SEPARATORS.split(command) if part.strip()]
    return any(_command_matches(specifier, part) for part in parts)


def _command_matches(specifier: str, part: str) -> bool:
    if specifier.endswith(" *"):
        prefix = specifier[:-2]
        return part == prefix or part.startswith(prefix + " ")
    return part == specifier


def _path_matches(specifier: str, paths: Touched, cwd: Path, fold: Callable[[str], str]) -> bool:
    pattern = fold(_absolute_pattern(specifier, cwd))
    return any(globs.matches_any(path, [pattern]) for path in paths.both)


def _absolute_pattern(specifier: str, cwd: Path) -> str:
    """The rule's path pattern as an absolute pattern: `//x` is `/x`, `/x`, `./x` and `x` lie in the working folder."""
    if specifier.startswith("//"):
        return specifier[1:]
    if specifier.startswith("~/"):
        return os.path.expanduser(specifier)
    return os.path.join(cwd, specifier.lstrip("/").removeprefix("./"))


def _domain_matches(specifier: str, url: object) -> bool | None:
    domain = specifier.removeprefix("domain:")
    if domain == specifier:
        return None
    host = urlsplit(url).hostname if isinstance(url, str) else None
    if not host:
        return False
    if domain.startswith("*."):
        return host.endswith(domain[1:])
    return host == domain


def unattended(message: dict, settings: dict, cwd: Path) -> dict:
    """Apply the cage decision and decline every interactive permission fallback."""
    try:
        request = load_request(json.dumps(message).encode())
        decide(request, settings, cwd)
    except (MalformedRequest, OSError, ValueError):
        pass
    params = message.get("params")
    options = params.get("options") if isinstance(params, dict) else None
    for option in options if isinstance(options, list) else ():
        if (isinstance(option, dict) and option.get("kind") in ("reject_once", "reject_always")
                and isinstance(option.get("optionId"), str)):
            return {"outcome": {"outcome": "selected", "optionId": option["optionId"]}}
    return {"outcome": {"outcome": "cancelled"}}
