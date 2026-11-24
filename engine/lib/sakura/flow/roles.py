"""Role adapters: how each provider is launched and how its final message is read."""

from __future__ import annotations

import json
import re
import shlex
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from sakura import codex_trust, grants, programs
from sakura.flow import cage as cages, codex_profile, logread, run_log
from sakura.flow.config import REVIEW_ROLES, DialStep

Provider = Literal["claude", "codex", "goose", "exec", "none"]
PROVIDERS = ("claude", "codex", "goose", "exec", "none")
READ_ONLY_ROLES = REVIEW_ROLES
RESULT_LINE = re.compile(r"SAKURA_RESULT\s+(.*)")
EVALUATOR_TOOLS = "Bash,Read"
# The digest summarizer reads its input file and writes its output file, and has no other tool. It takes its prompt as
# plain text on stdin, so it has no live channel and none of the stream-json input flags.
DIGEST_TOOLS = "Read,Write"
# Isolation boundary of both providers: no personal settings, plugins, MCP servers, skills, hooks or exec
# rules. The person's instruction files (~/.claude/CLAUDE.md, ~/.codex/AGENTS.md) still load, by acceptance:
# they are the person's standards and carry no tools.
# Unattended roles load no Claude settings source (user, project or local): hooks merge across
# sources, so a tracked project hook would run unmediated next to the engine's. An empty value
# is its own argv element. The engine hooks and the cage come from the run's settings file in the task folder.
CLAUDE_ISOLATION = ("--setting-sources", "", "--strict-mcp-config", "--disable-slash-commands")
# Never `--dangerously-skip-permissions`: whatever would ask goes to the engine's prompt tool.
CLAUDE_PERMISSIONS = ("--permission-mode", "acceptEdits", "--permission-prompts", "host",
                      "--permission-prompt-tool", cages.PERMISSION_TOOL)
# The digest summarizer does not auto-accept edits in its working folder: it may write the one file its settings name.
DIGEST_PERMISSIONS = ("--permission-mode", "default", *CLAUDE_PERMISSIONS[2:])
NO_CAGE = "no cage was prepared for this run: the engine never launches an agent outside its cage"
# `--ignore-user-config` also drops the project trust Codex needs to load `.codex/hooks.json`, so the
# engine hooks are passed inline (see `_inline_hook_overrides`) and do not depend on the person's config.
# `--ignore-rules` skips user and project execpolicy .rules files.
CODEX_ISOLATION_CORE = ("--ignore-user-config", "--ignore-rules")
CODEX_ISOLATION = (*CODEX_ISOLATION_CORE, "-c", "features.hooks=true")
# Codex's shell tool runs commands in a login shell, which rebuilds PATH (the system `git` shim then writes a cache the
# profile does not allow), and passes only a core of its environment: the engine's own PATH, TMPDIR and Git settings
# must reach the commands, as they reach a Claude role's.
CODEX_SHELL = ("-c", "allow_login_shell=false", "-c", 'shell_environment_policy.inherit="all"')
# The digest summarizer reads untrusted text and has Claude's `--tools Read,Write` only: for Codex that is its shell tool off
# (its file edits stay), and a folder that is no repository.
CODEX_SUMMARIZER = ("--skip-git-repo-check", "-c", "features.shell_tool=false")
TRUST_HINT = "needs recorded hook trust: run sakura trust-codex --record"


class RoleRefused(RuntimeError):
    """Raised when a role may not be launched as configured."""


@dataclass(frozen=True)
class RoleSpec:
    """A parsed `provider[:detail]` role value."""

    provider: Provider
    model: str = ""
    command: str = ""


def parse_role(value: str) -> RoleSpec:
    """Parse `claude[:model]`, `codex[:model]`, `goose[:model]`, `exec:<command>` or `none`."""
    provider, _, detail = value.partition(":")
    if provider not in PROVIDERS:
        raise ValueError(f"unknown role provider in {value!r}; use one of {', '.join(PROVIDERS)}")
    if provider == "exec":
        if not detail.strip():
            raise ValueError("exec role needs a command: exec:<command>")
        return RoleSpec("exec", command=detail.strip())
    return RoleSpec(provider, model=detail.strip())


def codex_trust_problem(spec: RoleSpec, role: str, workdir: Path) -> str:
    """Return why a Codex role may not run in `workdir`, or an empty string.

    Every Codex role needs it, readers too: under `--ignore-user-config` Codex skips untrusted inline hooks unless
    `--dangerously-bypass-hook-trust` is given, and that bypass is only safe once the workspace hooks are exactly
    the engine's own.
    """
    if spec.provider != "codex":
        return ""
    if not codex_trust.is_recorded(workdir):
        return f"Codex {role} {TRUST_HINT}"
    hooks_problem = codex_trust.workspace_hooks_exact(workdir)
    return f"Codex {role} refused: {hooks_problem}" if hooks_problem else ""


def argv_for(
    spec: RoleSpec,
    role: str,
    dial: DialStep,
    workdir: Path,
    common_dir: Path,
    last_message: Path,
    max_budget_usd: float = 0.0,
    cage: cages.RunCage | None = None,
    resume_session: str = "",
) -> list[str]:
    """Build the command line for one run; the prompt goes on stdin. A Claude or Codex run needs its `cage`.

    With `resume_session` the run continues that session headless: a Claude run with `--resume`, a Codex run as
    `codex exec resume` (which takes no `-C`: its folder is the process's), both with the limits of a fresh run.

    An exec role runs under the cage's Seatbelt profile when one is given (the engine always gives one).
    """
    if spec.provider == "exec":
        argv = shlex.split(spec.command)
        return cage.wrap(argv) if cage else argv
    if spec.provider == "none":
        raise RoleRefused(f"role {role} is set to none")
    if spec.provider == "goose":
        raise RoleRefused("Goose requires a qualified ACP launch")
    if cage is None:
        raise RoleRefused(f"{spec.provider} {role}: {NO_CAGE}")
    argv = (_claude_argv(spec, role, dial, common_dir, max_budget_usd, cage, resume_session) if spec.provider == "claude"
            else _codex_argv(spec, role, dial, workdir, last_message, cage, resume_session))
    return [_pinned(argv[0], workdir), *argv[1:]]


def _pinned(program: str, workdir: Path) -> str:
    """The engine's own copy of the agent CLI (`claude`, `codex`), never one a repository put on PATH."""
    try:
        return programs.path(program, workdir)
    except programs.ProgramError as error:
        raise RoleRefused(str(error)) from error


def final_message(spec: RoleSpec, log_text: str, last_message: Path) -> str:
    """Extract the agent's final message from its log (or its `-o` file for Codex)."""
    if spec.provider == "codex":
        return _read_last_message(last_message)
    if spec.provider == "claude":
        return _claude_result(log_text)
    return log_text


def _read_last_message(path: Path) -> str:
    try:
        return logread.read_log(path)
    except FileNotFoundError:
        return ""
    except logread.UnreadableLog as error:
        raise RoleRefused(str(error)) from None


def parse_result(final: str) -> dict | None:
    """Return the JSON object on the last SAKURA_RESULT line, or None when absent or malformed."""
    matches = [RESULT_LINE.fullmatch(line.strip()) for line in final.splitlines()]
    matches = [match for match in matches if match]
    if not matches:
        return None
    try:
        parsed = json.loads(matches[-1].group(1))
    except ValueError:
        return None
    return parsed if isinstance(parsed, dict) else None


def _claude_argv(spec: RoleSpec, role: str, dial: DialStep, common_dir: Path, max_budget_usd: float,
                 cage: cages.RunCage, resume_session: str = "") -> list[str]:
    """The settings file carries the engine hooks (a tracked `disableAllHooks` cannot turn them off) and the cage."""
    is_digest = role == grants.DIGEST_CLASS
    argv = ["claude", "-p", *(["--resume", resume_session] if resume_session else []),
            *([] if is_digest else ["--input-format", "stream-json", "--replay-user-messages"]),
            "--output-format", "stream-json", "--verbose", *([] if is_digest else ["--include-partial-messages"]),
            "--model", spec.model or dial.claude_model, "--effort", dial.effort,
            *(DIGEST_PERMISSIONS if is_digest else CLAUDE_PERMISSIONS), *CLAUDE_ISOLATION, "--mcp-config", str(cage.mcp_config),
            "--settings", str(cage.settings)]
    if max_budget_usd > 0:
        argv += ["--max-budget-usd", str(max_budget_usd)]
    if role == "evaluator":
        argv += ["--tools", EVALUATOR_TOOLS]
    if is_digest:
        argv += ["--tools", DIGEST_TOOLS]
    return argv + ["--add-dir", str(common_dir)]


def _codex_argv(spec: RoleSpec, role: str, dial: DialStep, workdir: Path, last_message: Path,
                cage: cages.RunCage, resume_session: str = "") -> list[str]:
    """A Codex run of any role: the cage's permission profile (read-only for a reviewer, the workspace's writes for the
    others), approvals never, the engine's hooks and tool server; the final message comes back through `-o`.

    The digest summarizer has no hooks (its folder is no repository) and no tool server, so it needs no hook trust."""
    hooked = role != grants.DIGEST_CLASS
    problem = codex_trust_problem(spec, role, workdir) if hooked else ""
    if problem:
        raise RoleRefused(problem)
    argv = ["codex", "exec", *(["resume", resume_session] if resume_session else []),
            *(["--dangerously-bypass-hook-trust"] if hooked else CODEX_SUMMARIZER), "--json", "-c", 'approval_policy="never"',
            *cage.codex_args, "-c", f"model_reasoning_effort={dial.effort}", "-o", str(last_message)]
    if spec.model:
        argv += ["-m", spec.model]
    return argv + [*(CODEX_ISOLATION if hooked else CODEX_ISOLATION_CORE), *CODEX_SHELL,
                   *(_inline_hook_overrides() if hooked else []), *([] if resume_session else ["-C", str(workdir)]), "-"]


def _inline_hook_overrides() -> list[str]:
    """One `-c hooks.<Event>=[...]` pair per engine hook event, with the installer's exact commands."""
    argv: list[str] = []
    for event, command in zip(codex_trust.HOOK_EVENTS, codex_trust.canonical_commands()):
        entry = f'[{{hooks=[{{type="command",command={codex_profile.toml_string(command)}}}]}}]'
        argv += ["-c", f"hooks.{event}={entry}"]
    return argv


def hooks_expected(spec: RoleSpec) -> bool:
    """True when a run on `spec` must leave a hook mark once it used a tool: every Claude and Codex role gets the engine hooks inline."""
    return spec.provider in ("claude", "codex")


def ran_tools(spec: RoleSpec, log_text: str) -> bool:
    """True when the run's log holds a tool call: any Codex item or Claude block outside the text-only sets."""
    return any(entry.kind == run_log.TOOL for entry in run_log.entries(spec, log_text))


def _claude_result(log_text: str) -> str:
    final = ""
    for line in log_text.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if isinstance(event, dict) and event.get("type") == "result" and isinstance(event.get("result"), str):
            final = event["result"]
    return final
