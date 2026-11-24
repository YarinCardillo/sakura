"""Programs and commands a role may not start, found by name in a shell command.

The Claude cage denies them twice: by a Bash rule and in its Seatbelt profile. Codex's own sandbox does not (measured
with 0.171: `osascript`, `open`, `launchctl` and `security` all start in its workspace-write profile), so a Codex role's
hook refuses a command that starts one. This is a check of the words of the command, not a sandbox: a program started
through an expansion (`$(...)`, a variable) or a script the role wrote is not seen. It stands in for the Seatbelt rule
and is named as a weaker guarantee for Codex roles."""

from __future__ import annotations

import os
import re
import shlex
from collections.abc import Sequence

# `ps`, `pbpaste` and `pbcopy` too: measured in a Claude lane (2.1.323), its sandbox refuses ps, process info and the
# pasteboard but lets the securityd and LaunchServices lookups through.
PROGRAMS = ("osascript", "open", "launchctl", "tailscale", "docker", "kill", "pkill", "killall", "security",
            "ps", "pbpaste", "pbcopy")
COMMANDS = ("git push",)
OPERATORS = {";", "&&", "||", "|", "&", "|&", "(", ")", "\n"}
WRAPPERS = {"env", "command", "exec", "nohup", "time", "sudo", "xargs", "nice", "timeout", "builtin", "stdbuf"}
SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "csh", "tcsh", "fish"}
EVALUATORS = {"eval"}
# A backslash before a newline joins the lines, unless that backslash is itself escaped.
CONTINUATION = re.compile(r"(?<!\\)((?:\\\\)*)\\\n")
DURATION = re.compile(r"\d+[smhd]?")
MAX_NESTING = 3
MAX_WRAPPED_WORDS = 200
TOO_DEEP = "shells, eval and wrappers nested more than 3 deep cannot be checked and are refused"
NOT_ALLOWED = " is not allowed in a role's commands"
GIT_OPTIONS_WITH_VALUES = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"}


def reason(command: str, programs: Sequence[str] = PROGRAMS, commands: Sequence[str] = COMMANDS, depth: int = 0) -> str | None:
    """Why `command` may not run (the denied program or command it starts), or None. A command that cannot be read
    as shell words is left to the other guards."""
    try:
        lexer = shlex.shlex(CONTINUATION.sub(r"\1", command).replace("\n", " ; "), posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        words = list(lexer)
    except ValueError:
        return None
    for start in _command_starts(words):
        found = _denied_at(words[start:], programs, commands, depth)
        if found:
            return found
    return None


def _command_starts(words: list[str]) -> list[int]:
    starts = [0] if words else []
    starts += [index + 1 for index, word in enumerate(words[:-1]) if set(word) <= set(";&|()") and word in OPERATORS]
    return starts


def _denied_at(words: list[str], programs: Sequence[str], commands: Sequence[str], depth: int) -> str | None:
    index = _after_wrappers(words)
    if index >= len(words):
        return None
    program = _name(words[index])
    rest = words[index + 1:]
    if program in programs:
        return program + NOT_ALLOWED
    if index and (found := _inside_wrapper(words[:index], rest, programs, commands, depth)):
        return found
    for denied in commands:
        if _starts_command(program, rest, denied):
            return denied + NOT_ALLOWED
    if program in EVALUATORS or program in SHELLS:
        if depth >= MAX_NESTING:
            return TOO_DEEP
        return reason(" ".join(rest), programs, commands, depth + 1) if program in EVALUATORS else _inside_shell(rest, programs, commands, depth)
    return None


def _inside_wrapper(prefix: list[str], rest: list[str], programs: Sequence[str], commands: Sequence[str], depth: int) -> str | None:
    """The full check at every word of the rest of a command a wrapper starts: a wrapper's option takes a value of its own
    (`env -u X`, `nice -n 5`, `timeout -s KILL`), which this check does not model, so any word may be the program (a few
    harmless commands, such as `xargs echo kill`, are refused with it)."""
    if not any(_name(word) in WRAPPERS for word in prefix):
        return None
    segment = rest[:next((i for i, word in enumerate(rest) if word in OPERATORS), len(rest))]
    if len(segment) > MAX_WRAPPED_WORDS:  # fail closed: too long to look through
        return f"a command a wrapper starts may have at most {MAX_WRAPPED_WORDS} words"
    if depth >= MAX_NESTING:  # fail closed: a chain this deep costs more to look through than any real command needs
        return TOO_DEEP
    return next((found for i in range(len(segment)) if (found := _denied_at(segment[i:], programs, commands, depth + 1))), None)


def _after_wrappers(words: list[str]) -> int:
    """Where the program starts: past `VAR=value` words, wrapper programs and, once a wrapper was seen, its options and numbers."""
    index, wrapped = 0, False
    while index < len(words):
        word = words[index]
        if _name(word) in WRAPPERS:
            wrapped = True
        elif not (_is_assignment(word) or (wrapped and (word.startswith("-") or DURATION.fullmatch(word)))):
            break
        index += 1
    return index


def _name(word: str) -> str:
    """The program a word names: its last path element, in lower case (the volume ignores case)."""
    return os.path.basename(word).lower()


def _is_assignment(word: str) -> bool:
    name, separator, _ = word.partition("=")
    return bool(separator) and name.isidentifier()


def _starts_command(program: str, rest: list[str], denied: str) -> bool:
    """Whether `program rest...` is the two-word command `denied` (`git push`), options of the first word skipped."""
    first, _, subcommand = denied.partition(" ")
    if program != first.lower():
        return False
    skip = 0
    for word in rest:
        if skip:
            skip -= 1
        elif word in GIT_OPTIONS_WITH_VALUES:
            skip = 1
        elif not word.startswith("-"):
            return word.lower() == subcommand
    return False


def _inside_shell(rest: list[str], programs: Sequence[str], commands: Sequence[str], depth: int) -> str | None:
    for position, word in enumerate(rest[:-1]):
        if word.startswith("-") and not word.startswith("--") and "c" in word:
            return reason(rest[position + 1], programs, commands, depth + 1)
    return None
