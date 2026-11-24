"""Hook-time brakes of the flow engine, evaluated before the generic command guard.

Order: STOP, engine-owned files, operator commands, tmux, protected tests and contract,
scan allowlists, timeout inflation, evidence before claims. All but the first apply to
engine-launched sessions only (task id in the environment).
"""

from __future__ import annotations

import os
import re
from collections.abc import Iterator
from pathlib import Path
from typing import Callable, Mapping

from sakura import engine_home, guard
from sakura.flow import config, evidence, globs, inflation, writes
from sakura.flow.store import INBOX_DIR_NAME, TaskStore
from sakura.flow.writes import WriteIntent

TASK_ENV = "SAKURA_FLOW_TASK"
RUN_ENV = "SAKURA_FLOW_RUN"
ROLE_ENV = "SAKURA_FLOW_ROLE"
ENGINE_ROLE_REFUSAL = "operator commands cannot run inside an engine role"
PHASE_ENV = "SAKURA_FLOW_PHASE"
PRE_TOOL_USE = "PreToolUse"
HOOK_MARK_DIR_MODE = 0o700
BUILDER = "builder"
PROTECTED_GLOBS = [".sakura/**"]
OPERATOR_VERBS = ("dial", "role", "steer", "approve", "stop", "start", "resume", "recontract", "cleanup", "open",
                  "attach", "migrate-state", "mark", "reply", "restore-head", "agent-worker")
MODE_VERB = "mode"
LANE_WRITERS = ("open", "done", "say", "model")
COCKPIT_WRITERS = ("act",)
OUTSIDE_WRITERS = ("open", "row", "ask", "close", "inbox", "serve")
PROPOSAL_WRITERS = ("accept", "reject")
MASTER_WRITERS = ("spec", "serve")
DIGEST_WRITERS = ("run", "status", "install", "uninstall")
WRITERS_BY_GROUP = {"lane": LANE_WRITERS, "cockpit": COCKPIT_WRITERS, "preset": ("use",), "stack": ("up", "down"),
                    "cage": ("serve",), "outside": OUTSIDE_WRITERS,
                    "proposal": PROPOSAL_WRITERS, "master": MASTER_WRITERS, "digest": DIGEST_WRITERS}
# `agentkit` is the deprecated alias of `sakura`: it runs the same operator verbs, so it is refused the same way.
SAKURA_PROGRAMS = ("sakura", "agentkit")
TMUX_PROGRAM = "tmux"
TMUX_REASON = "tmux cannot run inside an engine role: it could reach the operator's terminal sessions"
SHELL_PROGRAMS = {"sh", "bash", "zsh", "dash", "ksh"}
NESTED_SHELL_MARKS = set(";&|()$")
MAX_NESTING = 3
# Digests, mode, deferred pages, lane and cockpit logs, personal and cockpit config.
OPERATOR_STATE_DIRS = (Path(".local/state/sakura"), Path(".config/sakura"))
# The engine home as a shell command may name it: its variable, and the tail of its default path, which
# has no space, so `Application\ Support` and other quotings of the folder above it still match.
ENGINE_HOME_NAMES = (engine_home.ENV_VARIABLE, "Sakura/engine")
OPERATOR_STATE_REASON = "engine-owned path: sakura operator state and config are not written from an engine role"
EXPANSION_CHARS = frozenset("$*?[{~")
PROGRAM_UNSAFE_CHARS = EXPANSION_CHARS | frozenset("(){}`")
TEST_PROGRAMS = frozenset({"[", "[["})
TOO_DEEP = "\0too-deep"
READER_PROGRAMS = frozenset({"echo", "printf", "grep", "rg", "cat", "ls", "head", "tail", "wc"})
# rg runs the command named by --pre on every file and --hostname-bin once; neither has a short form.
EXEC_FLAGS = ("--pre", "--hostname-bin")
REDIRECT_CHARS = frozenset("<>")
_OPERATOR_WORD = re.compile(r"(?<![\w-])(?:" + "|".join(OPERATOR_VERBS) + r"|" + MODE_VERB
                            + r"(?=[ \t]+[^\s;&|)]))(?![\w-])")
# Also matches inside socket paths such as /tmp/tmux-501/default; names like test_flow_tmux.py and
# tmux.conf (the word directly followed by a dot and a letter) are files, not the program.
_TMUX_WORD = re.compile(r"(?<!\w)tmux(?!\w)(?!\.[A-Za-z])", re.IGNORECASE)
_BACKTICK_PAIR = re.compile(r"`([^`]*)`")
_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
_FLOW_WORD = re.compile(r"(?<![\w-])flow(?![\w-])")
_WRITER_PATTERNS = tuple((re.compile(rf"(?<![\w-]){group}(?![\w-])"),
                         re.compile(r"(?<![\w-])(?:" + "|".join(writers) + r")(?![\w-])"))
                        for group, writers in WRITERS_BY_GROUP.items())
_VARIABLE_PROGRAM = re.compile(r"(?:^|[;&|(\n]\s*)\$")
READ_ONLY_REASON = "existing tests and the contract are read-only for builders; ask for a ruling"
INFLATION_REASON = "raising a timeout or adding a retry wrapper needs a ruling"
CLAIM_REASON = "read an evidence file before claiming that something passed or is done"


def decision(repo: Path, event: str, payload: dict, env: Mapping[str, str]) -> tuple[str, str] | None:
    """Return ("deny", reason) when a flow brake applies to this hook call, else None.

    Only sessions launched by the engine (task id in `env`) are subject to
    flow brakes. A Read of an evidence file marks the session as having seen
    evidence; that is the only side effect.
    """
    task_id = env.get(TASK_ENV)
    if not task_id or event != PRE_TOOL_USE:
        return None
    call = _Call(repo, TaskStore(repo, task_id), payload, env)
    if call.store.stop_file.exists():
        return ("deny", f"task {task_id} stopped by operator")
    reason = _first_reason(call)
    if reason:
        return ("deny", reason)
    call.note_evidence_reads()  # a refused command read nothing
    return None


def mark_hook_ran(repo: Path, env: Mapping[str, str]) -> None:
    """Touch the hook mark of the run named in `env` (operator state, out of a role's reach), so the runner can tell a run whose hooks never loaded."""
    task_id, run = env.get(TASK_ENV), env.get(RUN_ENV)
    if not task_id or not run:
        return
    mark = TaskStore(repo, task_id).hook_mark(run)
    for directory in (mark.parent.parent, mark.parent):
        directory.mkdir(parents=True, exist_ok=True, mode=HOOK_MARK_DIR_MODE)
        directory.chmod(HOOK_MARK_DIR_MODE)  # mkdir applies the mode to the leaf only, and the umask can narrow it
    mark.touch()


def _first_reason(call: "_Call") -> str | None:
    checks = (_engine_owned, _operator_command, _tmux_command, _protected, _allowlisted, _inflated, _unevidenced_claim)
    return next((reason for reason in (check(call) for check in checks) if reason), None)


def _engine_owned(call: "_Call") -> str | None:
    """The engine home (the task folder and its engine-owned files included) and operator state are never written."""
    inbox = call.root / INBOX_DIR_NAME
    targets = [Path(os.path.normpath(intent.path)) for intent in call.intents]
    if any(target == inbox or inbox in target.parents for target in targets):
        return f"engine-owned path: {INBOX_DIR_NAME}/ takes operator commands only"
    if call.payload.get("tool_name") == "Bash" and _names_operator_state(call):
        return OPERATOR_STATE_REASON
    operator_dirs = _operator_dirs()
    if any(target == root or root in target.parents for target in targets for root in operator_dirs):
        return OPERATOR_STATE_REASON
    return None


def _operator_dirs() -> list[Path]:
    """Operator state roots (the engine homes included) as written and as resolved, so a symlink cannot hide them."""
    roots = [Path.home() / relative for relative in OPERATOR_STATE_DIRS] + _engine_roots()
    return roots + [root.resolve() for root in roots]


def _engine_roots() -> list[Path]:
    """The trust root, and the engine home of this run when `SAKURA_ENGINE_HOME` names a valid one."""
    try:
        return [engine_home.root_for_trust(), engine_home.root()]
    except engine_home.EngineHomeError:
        return [engine_home.root_for_trust()]


def _names_operator_state(call: "_Call") -> bool:
    """Shell commands name these dirs through ~, $HOME and the like, which write targets do not expand.

    No exception for the role's own task folder: a role reads and writes only in its workspace (the exchange
    folder, `flow/exchange.py`), so any mention of an engine home is refused. This brake is a second layer
    and fails closed rather than parse the shell.
    """
    command = str((call.payload.get("tool_input") or {}).get("command") or "").casefold()
    return (any(str(relative).casefold() in command for relative in OPERATOR_STATE_DIRS)
            or any(name.casefold() in command for name in ENGINE_HOME_NAMES)
            or any(str(root).casefold() in command for root in _engine_roots()))


def _operator_command(call: "_Call") -> str | None:
    """Defense in depth: the CLI refuses operator verbs inside a role, this stops a shell that scrubs the role variable."""
    if not call.role or call.payload.get("tool_name") != "Bash":
        return None
    command = str((call.payload.get("tool_input") or {}).get("command") or "")
    verb = _invocation(command, MAX_NESTING, lambda tokens, _is_code: _operator_invocation(tokens))
    if verb == TOO_DEEP:
        return "operator command check: shell nesting too deep to inspect inside an engine role"
    if verb:
        return f"operator command `sakura {verb}` cannot run inside an engine role"
    if _mentions_operator_verb(command) and not _only_readers(command):
        return "possible operator command (sakura flow or lane with an operator verb) cannot run inside an engine role"
    return None


def _mentions_operator_verb(command: str) -> bool:
    """Fail closed where the parser cannot follow: variables, env options, interpreters, quoting, wrappers."""
    dequoted = " ; ".join(" ".join(tokens) for tokens in writes.segments(command))
    return _names_flow_verb(command) or _names_flow_verb(dequoted)


def _names_flow_verb(command: str) -> bool:
    if not (any(name in command for name in SAKURA_PROGRAMS) or _VARIABLE_PROGRAM.search(command)
            or _expanding_program(command)):
        return False
    if _FLOW_WORD.search(command) and _OPERATOR_WORD.search(command):
        return True
    return any(group.search(command) and writer.search(command) for group, writer in _WRITER_PATTERNS)


def _expanding_program(command: str) -> bool:
    """True when a segment's program is a glob, brace or tilde pattern the shell expands into another name."""
    return any(_program_expands(tokens) for tokens in writes.segments(command))


def _program_expands(tokens: list[str]) -> bool:
    """True when the program token may expand into another name.

    Only a single leading `~/` is exempt; the rest must be free of anything the shell expands or
    substitutes. A variable prefix is never safe: unquoted, it is word-split into more words.
    """
    start = guard.program_of(tokens)[1]
    if start >= len(tokens) or tokens[start] in TEST_PROGRAMS:
        return False
    return bool(set(tokens[start].removeprefix("~/")) & PROGRAM_UNSAFE_CHARS)


def _only_readers(command: str) -> bool:
    """True when every segment is a plain reader such as echo or grep: no nested shell, no exec flag, no redirection."""
    return all(_is_plain_reader(tokens) for tokens in writes.segments(command))


def _is_plain_reader(tokens: list[str]) -> bool:
    program, start = guard.program_of(tokens)
    if program not in READER_PROGRAMS or _nested_commands(tokens) or start:
        return False
    return not any(REDIRECT_CHARS & set(token) or _is_exec_flag(token) for token in tokens)


def _is_exec_flag(token: str) -> bool:
    return any(token == flag or token.startswith(flag + "=") for flag in EXEC_FLAGS)


def _tmux_command(call: "_Call") -> str | None:
    """Roles have no use for tmux, and a tmux call can reach the operator's own sessions.

    Denied as a parsed program in any wrapper, and, failing closed like the operator brake, wherever the
    word tmux appears in a command that is not only readers (wrappers such as nohup, timeout or xargs,
    interpreters with -c or -e text, socket paths) or the program token is one the shell may expand.
    """
    if not call.role or call.payload.get("tool_name") != "Bash":
        return None
    command = str((call.payload.get("tool_input") or {}).get("command") or "")
    found = _invocation(command, MAX_NESTING, _tmux_invocation)
    if found == TOO_DEEP:
        return "tmux check: shell nesting too deep to inspect inside an engine role"
    if found or (_mentions_tmux(command) and not _only_readers(command)):
        return TMUX_REASON
    return None


def _mentions_tmux(command: str) -> bool:
    """The word tmux in `command` as written or dequoted, or in any shell text nested in it."""
    return any(_TMUX_WORD.search(text) for text in _texts_of(command, MAX_NESTING))


def _texts_of(command: str, depth: int) -> Iterator[str]:
    yield command
    parsed = writes.segments(command)
    yield " ; ".join(" ".join(tokens) for tokens in parsed)
    for tokens in parsed if depth else ():
        for nested in _nested_commands(tokens):
            yield from _texts_of(nested, depth - 1)


def _invocation(command: str, depth: int, matcher: Callable[[list[str], bool], str], is_code: bool = True) -> str:
    """The first segment of `command` that `matcher` recognizes, also nested in `bash -c`, `eval` or `$(...)`.

    `is_code` tells the matcher whether the text is a command line (or a `bash -c` or `eval` body) or only
    the text of a plain argument such as `$HOME`, which the shell does not run.
    """
    for tokens in _shell_segments(command):
        hit = matcher(tokens, is_code)
        if hit:
            return hit
        texts = _nested_texts(tokens)
        if texts and not depth:
            return TOO_DEEP
        found = next((hit for hit in (_invocation(text, depth - 1, matcher, code) for text, code in texts) if hit), "")
        if found:
            return found
    return ""


def _shell_segments(command: str) -> list[list[str]]:
    """`writes.segments` with backtick substitution read as `$(...)`, so one hiding a program shows up as `$`."""
    return writes.segments(_BACKTICK_PAIR.sub(r"$(\1)", command).replace("`", "$("))


def _tmux_invocation(tokens: list[str], is_code: bool) -> str:
    """`tmux` when the segment's program is tmux, or may be (see `_may_run_tmux`) in code, behind env, command and path prefixes; else "".

    Plain argument text (`$HOME`, `$(pwd)`) is not run as written, so only a literal tmux program counts there.
    """
    return TMUX_PROGRAM if guard.program_of(tokens)[0] == TMUX_PROGRAM or (is_code and _may_run_tmux(tokens)) else ""


def _may_run_tmux(tokens: list[str]) -> bool:
    """True when the shell may turn the program into anything, so no name check can rule tmux out.

    That is an expanding program token (variable, glob, brace, tilde), or a prefix token that
    `guard.program_of` skips as an assignment but the shell would not read as `NAME=value`.
    """
    return _program_expands(tokens) or _skips_odd_assignment(tokens)


def _skips_odd_assignment(tokens: list[str]) -> bool:
    start = guard.program_of(tokens)[1]
    return any("=" in token and not token.startswith("-") and not _ASSIGNMENT.match(token) for token in tokens[:start])


def _operator_invocation(tokens: list[str]) -> str:
    """`flow <verb>`, `lane <writer>` or `cockpit act` for a `sakura` operator command, behind env, command and path prefixes; else "".

    `flow mode` counts as soon as any argument follows it (a value may hide behind a variable or glob);
    bare `flow mode` and `lane status` only read.
    """
    program, start = guard.program_of(tokens)
    arguments = tokens[start + 1:]
    if program not in SAKURA_PROGRAMS or not arguments or arguments[0] not in ("flow", *WRITERS_BY_GROUP):
        return ""
    words = [argument for argument in arguments[1:] if not argument.startswith("-")]
    verb, rest = (words[0], words[1:]) if words else ("", [])
    if arguments[0] in WRITERS_BY_GROUP:
        return f"{arguments[0]} {verb}" if verb in WRITERS_BY_GROUP[arguments[0]] else ""
    if verb == MODE_VERB:
        return f"flow {verb}" if rest else ""
    return f"flow {verb}" if verb in OPERATOR_VERBS else ""


def _nested_commands(tokens: list[str]) -> list[str]:
    return [text for text, _ in _nested_texts(tokens)]


def _nested_texts(tokens: list[str]) -> list[tuple[str, bool]]:
    """Shell text inside a segment as (text, is_code): plain arguments with shell marks, the bodies of
    substitutions in any token (which run), then `eval` and `-c` bodies."""
    program, start = guard.program_of(tokens)
    arguments = tokens[start + 1:]
    nested = [(token, False) for token in arguments if NESTED_SHELL_MARKS & set(token)]
    nested += [(body, True) for token in tokens for body in _substitution_bodies(token)]
    if program == "eval":
        nested.append((" ".join(arguments), True))
    elif program in SHELL_PROGRAMS:
        nested += [(after, True) for flag, after in zip(arguments, arguments[1:]) if flag.startswith("-") and "c" in flag[1:]]
    return nested


def _substitution_bodies(text: str) -> list[str]:
    """The command text of each outermost `$(...)` (parentheses balanced; unterminated runs to the end) and backtick pair in `text`."""
    bodies = _BACKTICK_PAIR.findall(text)
    position = text.find("$(")
    while position >= 0:
        end = _closing_parenthesis(text, position + 2)
        bodies.append(text[position + 2:end])
        position = text.find("$(", end)
    return bodies


def _closing_parenthesis(text: str, start: int) -> int:
    """Index of the `)` that closes a substitution whose body begins at `start`, or the text length when none does."""
    level = 1
    for index in range(start, len(text)):
        level += (text[index] == "(") - (text[index] == ")")
        if not level:
            return index
    return len(text)


def _protected(call: "_Call") -> str | None:
    if call.role != BUILDER or not call.intents:
        return None
    protected = call.protected_keys()
    for intent in call.intents:
        key = call.key(intent.path)
        if (key in protected or globs.matches_any(key, PROTECTED_GLOBS)) and not call.covered_by_ruling(key, intent):
            reason = INFLATION_REASON if inflation.inflates(intent.old, intent.new) else READ_ONLY_REASON
            return f"{reason}: {key}"
    return None


def _allowlisted(call: "_Call") -> str | None:
    if call.role != BUILDER or not call.intents:
        return None
    patterns = call.config_values()["brakes"]["scan_allowlists"]
    for intent in call.intents:
        key = call.key(intent.path)
        if (globs.matches_any(key, patterns) or intent.path.name in patterns) and not call.store.granted(key, "allowlist"):
            return f"scan allowlist {key} needs a ruling: fix false positives with the arbiter's approval"
    return None


def _inflated(call: "_Call") -> str | None:
    if call.role != BUILDER or not call.intents:
        return None
    patterns = call.config_values()["brakes"]["test_patterns"]
    protected = call.protected_keys()
    for intent in call.intents:
        key = call.key(intent.path)
        is_test = key in protected or globs.matches_any(key, patterns)
        if is_test and inflation.inflates(intent.old, intent.new) and not call.store.granted(key, "timeout"):
            return f"{INFLATION_REASON}: {key}"
    return None


def _unevidenced_claim(call: "_Call") -> str | None:
    session = str(call.payload.get("session_id") or "anonymous")
    for intent in call.intents:
        if evidence.is_claim_file(call.root, intent.path) and evidence.is_claim(intent.new):
            return None if evidence.is_marked(call.repo, session) else CLAIM_REASON
    return None


class _Call:
    """One hook call: who is calling, what it writes and the task data the checks need."""

    def __init__(self, repo: Path, store: TaskStore, payload: dict, env: Mapping[str, str]) -> None:
        self.repo, self.store, self.payload = repo.resolve(), store, payload
        self.role, self.phase = env.get(ROLE_ENV, ""), env.get(PHASE_ENV, "")
        self.root = store.root.resolve()
        self.intents: list[WriteIntent] = writes.extract(self.repo, payload)
        self._state: dict | None = None
        self._config: dict | None = None

    def note_evidence_reads(self) -> None:
        session = str(self.payload.get("session_id") or "anonymous")
        if any(evidence.is_evidence(self.root, path) for path in writes.read_paths(self.repo, self.payload)):
            evidence.mark(self.repo, session)

    def key(self, path: Path) -> str:
        """Repo-relative form of `path`, or its absolute form outside the repo."""
        try:
            return path.resolve().relative_to(self.repo).as_posix()
        except ValueError:
            return str(path.resolve())

    def protected_keys(self) -> set[str]:
        return {self.key(self.repo / entry) for entry in self._read_state()["protected_paths"]}

    def covered_by_ruling(self, key: str, intent: WriteIntent) -> bool:
        """A test_change grant covers any edit; a timeout grant covers an edit that only raises limits."""
        if self.store.granted(key, "test_change"):
            return True
        return self.store.granted(key, "timeout") and inflation.timeout_only_increase(intent.old, intent.new)

    def config_values(self) -> dict:
        if self._config is None:
            self._config = config.load(self.repo, overrides=self._read_state().get("overrides") or None).values
        return self._config

    def _read_state(self) -> dict:
        if self._state is None:
            self._state = self.store.read_state()
        return self._state
