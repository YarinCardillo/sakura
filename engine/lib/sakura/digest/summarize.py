"""The summarizer: a light model condenses the collected items inside a disposable cage.

The model's only input is `collected.jsonl` and its only output is `out.json`; it has the Read and Write tools and
nothing else (no shell, no web, no MCP server beyond the engine's own permission prompt, no subagent), and the cage's
Seatbelt and permission rules are written by the engine (`flow/cage.py`) like those of every other role. What it
reads is untrusted text, so what it writes is untrusted too: the engine reads `out.json` through a descriptor it opens
without following a link, checks it against the fixed schema, and refuses any link whose URL is not in the input.
Whatever the outcome, the run folder is removed afterwards.
"""

from __future__ import annotations

import json
import os
import shutil
import string
import tempfile
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path

from sakura import beneath, engine_home, grants, programs
from sakura.budget import account as budget_account
from sakura.digest import config, intake, schema
from sakura.flow import cage, pinned_roots, process, roles, step_accounts
from sakura.flow.config import DialStep
from sakura.flow.render import strip_controls
from sakura.usage import leeway
from sakura.usage.archive import quota as usage_quota
from sakura.usage.archive import settings as usage_settings

PROMPT = Path(__file__).with_name("prompt.md")
COLLECTED_FILE, OUTPUT_FILE, LOG_FILE = "collected.jsonl", "out.json", "run.log"
FOLDER_PREFIX = "sakura-digest-"
MAX_INPUT_BYTES = 300_000
MAX_OUTPUT_BYTES = 200_000
RUN_SECONDS, IDLE_SECONDS = 600, 300
MAX_BUDGET_USD = 1.0
EFFORT = "low"
FAILURE_TAIL = 300
RUN_NAME = "digest"

Launch = Callable[..., process.ProcessOutcome]


class SummarizeError(RuntimeError):
    """Raised, with a one-line reason, when the summarizer could not produce a valid output; nothing is published."""


@dataclass(frozen=True)
class RunFolder:
    """The disposable folders of one run: `input` (read only to the role), `output` (its working folder, the only
    place it writes) and `tmp` (its tmp), all under `root`, which only the engine writes."""

    root: Path
    input: Path
    output: Path
    tmp: Path


def summarize(results: list[intake.SourceResult], settings: config.DigestConfig, window: schema.Window, *,
              launch: Launch = process.monitored_run, home: Path | None = None) -> schema.ModelOutput:
    """The model's validated output for the items of `results`; an empty output when there are none, with no model run.

    Raises SummarizeError when the run fails or its output breaks the schema or links to an address not in the input.
    """
    lines = collected_lines(results)
    if not lines:
        return schema.ModelOutput()
    folder = _new_folder()
    try:
        return _run(folder, lines, settings, window, launch, home)
    finally:
        _remove(folder)


def fit(results: list[intake.SourceResult]) -> list[intake.SourceResult]:
    """`results` with every source that does not fit the model's input withheld whole.

    Sources go in the order of their newest item, each whole or not at all, until the input limit is reached. A source
    left out delivers nothing and its state does not move (see `intake.too_much_new`); one that fits is returned as it
    is. A source bigger than the whole input never fits: it has to be narrowed."""
    order = sorted(range(len(results)), key=lambda index: max((item.at for item in results[index].items), default=""),
                   reverse=True)
    room, withheld = MAX_INPUT_BYTES, set()
    for index in order:
        size = sum(len(_line(results[index].report.id, item)) + 1 for item in results[index].items)
        if size > room:
            withheld.add(index)
        else:
            room -= size
    return [intake.too_much_new(result.report.id, result.report.skipped, len(result.items), 0)
            if index in withheld else result for index, result in enumerate(results)]


def collected_lines(results: list[intake.SourceResult]) -> list[str]:
    """The input file's lines: every item, newest first, until the input limit is reached."""
    rows = [(item.at, result.report.id, item) for result in results for item in result.items]
    lines, size = [], 0
    for _, source, item in sorted(rows, key=lambda row: row[0], reverse=True):
        line = _line(source, item)
        if size + len(line) + 1 > MAX_INPUT_BYTES:
            break
        lines.append(line)
        size += len(line) + 1
    return lines


def _line(source: str, item: schema.Item) -> str:
    return json.dumps({"source": source, "id": item.id, "title": item.title, "url": item.url, "at": item.at,
                       "text": item.text})


def _run(folder: RunFolder, lines: list[str], settings: config.DigestConfig, window: schema.Window, launch: Launch,
         home: Path | None) -> schema.ModelOutput:
    beneath.write_bytes(folder.input, COLLECTED_FILE, ("\n".join(lines) + "\n").encode(), config.FILE_MODE)
    run_cage = _prepared_cage(folder, home)
    argv = _argv(settings.model, folder, run_cage)
    provider = role_spec(settings.model).provider
    env = {**_account_environment(settings.account, provider), **run_cage.env}
    outcome = launch(argv, cwd=folder.output, env=env, log_path=folder.root / LOG_FILE, budget_seconds=RUN_SECONDS,
                     idle_seconds=IDLE_SECONDS, stop_file=None, on_start=lambda pid: None,
                     stdin_text=_prompt(folder, settings, window))
    if outcome.exit_code != 0:
        raise SummarizeError(_failure(outcome, folder))
    return _validated(folder, {json.loads(line)["url"] for line in lines} - {""})


def role_spec(model: str) -> roles.RoleSpec:
    """The program and model a digest `model` names: `claude:haiku`, `codex:gpt-6.2-sol`, `codex` or `claude` alone, and a
    bare name is a Claude model (the file's long-standing form)."""
    provider, separator, name = model.partition(":")
    if separator:
        return roles.RoleSpec(provider, model=name)
    return roles.RoleSpec(model) if model in ("claude", "codex") else roles.RoleSpec("claude", model=model)


def _account_environment(account: str, provider: str = "claude") -> dict[str, str]:
    """The role's environment on the configured account of its program, by the mechanism every role uses
    (`budget.account` for Claude, Leeway's account ids for either program); no account keeps the default one."""
    environment = process.scrubbed_environment(os.environ, provider)
    if not account:
        return environment
    if provider == "codex":
        folder = leeway.config_dir(account, provider)
        if folder is None:
            raise SummarizeError(f"the Codex account {account} is not one Leeway lists")
        return step_accounts.point_at(environment, folder, provider)
    try:
        usage = usage_settings.load()
        return budget_account.apply(environment, account, usage.sources, usage.disabled)
    except budget_account.UnknownAccountError as error:
        folder = leeway.claude_config_dir(account)  # the apps send Leeway's account ids
        if folder is None:
            raise SummarizeError(str(error)) from error
        return usage_quota.claude_config_env(environment, Path(folder))
    except usage_settings.UsageConfigError as error:
        raise SummarizeError(str(error)) from error


def _new_folder() -> RunFolder:
    root = Path(tempfile.mkdtemp(prefix=FOLDER_PREFIX, dir=os.path.realpath(tempfile.gettempdir())))
    try:
        pinned_roots.pin(root)
        names = [pinned_roots.pin(pinned_roots.make_folder(root, name)) for name in ("in", "out", "tmp")]
    except (OSError, pinned_roots.RootReplaced) as error:
        shutil.rmtree(root, ignore_errors=True)
        raise SummarizeError(f"cannot make the run folder: {error}") from error
    return RunFolder(root, *names)


def _remove(folder: RunFolder) -> None:
    for path in (folder.root, folder.input, folder.output, folder.tmp):
        pinned_roots.release(path)
    shutil.rmtree(folder.root, ignore_errors=True)


def _prepared_cage(folder: RunFolder, home: Path | None) -> cage.RunCage:
    roots = tuple(dict.fromkeys((engine_home.root(), engine_home.root_for_trust(), engine_home.account_root())))
    site = cage.RunSite(config.digest_folder(), RUN_NAME, grants.DIGEST_CLASS, folder.input, folder.output,
                        roots, None, tmp_dir=folder.tmp, knowledge={"input": str(folder.input)}, hooks=False,
                        output_file=folder.output / OUTPUT_FILE)
    try:
        return cage.prepare(site, cage.Policy(cage.DEFAULT_POLICY), 0, home)
    except cage.CageError as error:
        raise SummarizeError(f"cannot write the cage: {error}") from error


def _argv(model: str, folder: RunFolder, run_cage: cage.RunCage) -> list[str]:
    try:
        spec = role_spec(model)
        return roles.argv_for(spec, grants.DIGEST_CLASS, DialStep(spec.model, EFFORT, 0, 0),
                              folder.output, folder.output, folder.root / "last.txt", MAX_BUDGET_USD, run_cage)
    except (roles.RoleRefused, programs.ProgramError) as error:
        raise SummarizeError(str(error)) from error


def _prompt(folder: RunFolder, settings: config.DigestConfig, window: schema.Window) -> str:
    template = string.Template(PROMPT.read_text())
    return template.substitute(language=settings.language, since=window.since, until=window.until,
                               collected_path=folder.input / COLLECTED_FILE, output_path=folder.output / OUTPUT_FILE)


def _failure(outcome: process.ProcessOutcome, folder: RunFolder) -> str:
    reason = ("timed out" if outcome.timed_out else "stalled" if outcome.stalled else "was stopped" if outcome.stopped
              else f"exited with status {outcome.exit_code}")
    try:
        tail = (folder.root / LOG_FILE).read_bytes()[-FAILURE_TAIL:].decode("utf-8", errors="replace")
    except OSError:
        tail = ""
    return strip_controls(f"the summarizer {reason}: {' '.join(tail.split())}".strip(": "))


def _validated(folder: RunFolder, allowed_urls: set[str]) -> schema.ModelOutput:
    try:
        text = beneath.read_capped(folder.output, OUTPUT_FILE, MAX_OUTPUT_BYTES).decode("utf-8")
    except FileNotFoundError:
        raise SummarizeError(f"the summarizer wrote no {OUTPUT_FILE}") from None
    except (OSError, UnicodeDecodeError) as error:
        raise SummarizeError(f"{OUTPUT_FILE} cannot be read: {type(error).__name__}") from None
    try:
        output = schema.parse_model_output(text)
    except schema.SchemaError as error:
        raise SummarizeError(f"{OUTPUT_FILE} breaks the schema: {error}") from error
    stray = sorted(output.urls() - allowed_urls)
    if stray:
        raise SummarizeError(f"{OUTPUT_FILE} links to an address that is not in the input: {stray[0][:80]}")
    return output
