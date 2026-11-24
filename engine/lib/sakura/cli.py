"""Command-line entry points for sakura."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from datetime import UTC
from pathlib import Path
from types import SimpleNamespace
from typing import TYPE_CHECKING

from sakura.flow import exits

if TYPE_CHECKING:
    from sakura.flow.store import TaskStore

ENGINE_COMMANDS_HELP = """engine commands (each has its own --help):
  flow    start a task and follow it: start, status, approve, mark, report
  preset  see and choose how much checking a flow does: list, show, use, save
  engine  the rules of the lane flow: rules, set
  stack   start or stop the project's local stack; you run it, outside the cage
  board   show the posts on a project's board
  master  start the master, the one agent you chat with, in a terminal, or print its launch spec
  proposal accept or reject an agent's proposal for a knowledge folder
  box     run an agent task in a container box
  outside open lanes, add rows and ask questions from an agent session the engine did not start
  track   manage Tracks, links and decisions
  scheduled  scheduled Master tasks and their runs
  digest  the morning digest: run it now or on schedule, and see its status
  yield   give priority to your own agent sessions: on, off, status
  automations  engine verbs on a schedule: list, accept, enable, disable
  connectors  connect the accounts the digest reads: list, connect, disconnect
  setup   look for supported programs and their login folders
  keeper  keep lanes running without the app and resume them after a restart: install, uninstall, status
"""

LANE_SHELVED = "sakura lane is shelved: a lane is a flow task now; open one with sakura flow start."

# The `accounts` subcommands help lists; `set-ceiling` (the old name of `set-reserve`) is accepted but left out.
ACCOUNTS_COMMANDS = "{detect,list,add,rename,disable,enable,set-reserve,plan,extra,migrate}"


def main(argv: list[str]) -> int:
    """Run a sakura command."""
    if argv[:1] == ["box"]:
        from sakura import box_dispatch

        return box_dispatch.main(argv[1:])
    if argv[:1] == ["quota-hooks"]:
        from sakura.quota_enforcement import cli as quota_hooks_cli

        return quota_hooks_cli.main(argv[1:])
    if argv[:1] == ["flow"]:
        from sakura.flow import cli as flow_cli

        return flow_cli.main(argv[1:])
    if argv[:1] == ["board"]:
        from sakura.flow import board_cli

        return board_cli.main(argv[1:])
    if argv[:1] == ["master"]:
        from sakura.master import cli as master_cli

        return master_cli.main(argv[1:])
    if argv[:1] == ["proposal"]:
        from sakura import proposal_cli

        return proposal_cli.main(argv[1:])
    if argv[:2] == ["grants", "exec"]:
        from sakura import grants_exec

        return grants_exec.main(argv[2:])
    if argv[:1] == ["cage"]:
        from sakura.flow import cage_server

        return cage_server.main(argv[1:])
    if argv[:1] == ["stack"]:
        from sakura.flow import stack_cli

        return stack_cli.main(argv[1:])
    if argv[:1] == ["preset"]:
        from sakura.flow import preset_cli

        return preset_cli.main(argv[1:])
    if argv[:1] == ["engine"]:
        from sakura.flow import rules_cli

        return rules_cli.main(argv[1:])
    if argv[:1] == ["lane"]:
        print(LANE_SHELVED)
        return exits.USAGE
    if argv[:1] == ["quota"]:
        from sakura.quota import cli as quota_cli

        return quota_cli.main(argv[1:])
    if argv[:1] == ["setup"]:
        from sakura.setup import cli as setup_cli

        return setup_cli.main(argv[1:])
    if argv[:1] == ["cockpit"]:
        from sakura.cockpit import cli as cockpit_cli

        return cockpit_cli.main(argv[1:])
    if argv[:1] == ["outside"]:
        from sakura.outside import cli as outside_cli

        return outside_cli.main(argv[1:])
    if argv[:1] == ["track"]:
        from sakura.tracks import cli as track_cli

        return track_cli.main(argv[1:])
    if argv[:1] == ["scheduled"]:
        from sakura.scheduled import cli as scheduled_cli

        return scheduled_cli.main(argv[1:])
    if argv[:1] == ["digest"]:
        from sakura.digest import cli as digest_cli

        return digest_cli.main(argv[1:])
    if argv[:1] == ["yield"]:
        from sakura.flow import yield_person

        return yield_person.main(argv[1:])
    if argv[:1] == ["automations"]:
        from sakura.automations import cli as automations_cli

        return automations_cli.main(argv[1:])
    if argv[:1] == ["connectors"]:
        from sakura.connectors import cli as connectors_cli

        return connectors_cli.main(argv[1:])
    if argv[:1] == ["keeper"]:
        from sakura import keeper_cli

        return keeper_cli.main(argv[1:])
    parser = argparse.ArgumentParser(prog="sakura", epilog=ENGINE_COMMANDS_HELP,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    accounts_parser = sub.add_parser("accounts")
    accounts_sub = accounts_parser.add_subparsers(dest="accounts_command", metavar=ACCOUNTS_COMMANDS)
    accounts_sub.add_parser("detect")
    accounts_list = accounts_sub.add_parser("list")
    accounts_list.add_argument("--json", action="store_true", dest="accounts_json")
    accounts_add = accounts_sub.add_parser("add")
    accounts_add.add_argument("--provider", required=True)
    accounts_add.add_argument("--id", required=True, dest="account_id")
    accounts_add.add_argument("--config-dir", required=True)
    accounts_add.add_argument("--label")
    accounts_rename = accounts_sub.add_parser("rename")
    accounts_rename.add_argument("provider")
    accounts_rename.add_argument("account_id")
    accounts_rename.add_argument("label")
    for switch in ("disable", "enable"):
        switch_parser = accounts_sub.add_parser(switch)
        switch_parser.add_argument("provider")
        switch_parser.add_argument("account_id")
    accounts_migrate = accounts_sub.add_parser("migrate")
    accounts_migrate.add_argument("--write", action="store_true")
    for reserve_name in ("set-reserve", "set-ceiling"):  # set-ceiling: the old spelling, accepted and not listed
        accounts_reserve = accounts_sub.add_parser(reserve_name)
        accounts_reserve.add_argument("provider")
        accounts_reserve.add_argument("account_id")
        accounts_reserve.add_argument("percent")
    accounts_plan = accounts_sub.add_parser("plan")
    for name in ("provider", "account_id", "plan", "since"):
        accounts_plan.add_argument(name)
    accounts_plan.add_argument("--estimated", action="store_true")
    accounts_extra = accounts_sub.add_parser("extra")
    for name in ("provider", "account_id", "on", "amount", "currency"):
        accounts_extra.add_argument(name)
    accounts_extra.add_argument("--note", default="")
    for changing in ("rename", "disable", "enable", "set-reserve", "set-ceiling", "plan"):
        # The cockpit passes who asked; its action log records it, the registry does not.
        accounts_sub.choices[changing].add_argument("--actor", help=argparse.SUPPRESS)
    usage_parser = sub.add_parser("usage")
    usage_parser.add_argument("--json", action="store_true")
    usage_sub = usage_parser.add_subparsers(dest="usage_command")
    usage_sub.add_parser("archive")
    usage_backfill = usage_sub.add_parser("backfill-codex")
    usage_backfill.add_argument("--write", action="store_true")
    usage_quota = usage_sub.add_parser("quota")
    usage_quota.add_argument("--json", action="store_true", dest="quota_json")
    usage_estimate = usage_sub.add_parser("estimate")
    usage_estimate.add_argument("--account")
    source = usage_estimate.add_mutually_exclusive_group(required=True)
    source.add_argument("--tokens", type=_token_mix, metavar="CLASS=N[,CLASS=N]")
    source.add_argument("--plan", type=Path, metavar="FILE.md")
    usage_estimate.add_argument("--json", action="store_true", dest="quota_json")
    usage_weekly = usage_sub.add_parser("weekly")
    usage_weekly.add_argument("--week", type=_iso_week, metavar="YYYY-Www")
    usage_weekly.add_argument("--account")
    usage_weekly.add_argument("--json", action="store_true", dest="quota_json")
    usage_report = usage_sub.add_parser("report")
    usage_report.add_argument("--by-pr", action="store_true", required=True)
    usage_report.add_argument("--repo")
    usage_report.add_argument("--days", type=int)
    install_parser = sub.add_parser("install")
    install_parser.add_argument("path", nargs="?", default=".")
    install_parser.add_argument("--files-only", action="store_true",
                                help="write only the hook files inside the worktree: no Git config, no exclude file")
    uninstall_parser = sub.add_parser("uninstall")
    uninstall_parser.add_argument("path", nargs="?", default=".")
    test_parser = sub.add_parser("test")
    test_parser.add_argument("suite", nargs="?", choices=("fast", "focused", "full"), default="fast")
    integra_parser = sub.add_parser("integra")
    integra_parser.add_argument("--full", action="store_true")
    integra_parser.add_argument("numbers", nargs="+")
    status_parser = sub.add_parser("status")
    status_parser.add_argument("path", nargs="?", default=".")
    trust_parser = sub.add_parser("trust")
    trust_parser.add_argument("path", nargs="?", default=".")
    trust_codex_parser = sub.add_parser("trust-codex")
    trust_codex_parser.add_argument("path", nargs="?", default=".")
    trust_codex_parser.add_argument("--record", action="store_true")
    hook_parser = sub.add_parser("hook")
    hook_parser.add_argument("agent", choices=("claude", "codex"))
    hook_parser.add_argument("event")
    git_parser = sub.add_parser("git")
    git_parser.add_argument("hook", choices=("pre-commit", "pre-push"))
    git_parser.add_argument("remote_name", nargs="?")
    git_parser.add_argument("remote_url", nargs="?")
    run_parser = sub.add_parser("run")
    run_parser.add_argument("--heavy", action="store_true")
    run_parser.add_argument("--critical", action="store_true")
    run_parser.add_argument("--label")
    run_parser.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    try:
        return _run(args)
    except (ValueError, RuntimeError) as error:
        parser.error(str(error))
    return 2


def _run_accounts(args, accounts_command) -> int:
    from sakura.accounts import migrate

    handlers = {
        None: lambda: accounts_command.run_list(),
        "list": lambda: accounts_command.run_list(as_json=args.accounts_json),
        "detect": lambda: accounts_command.run_detect(),
        "add": lambda: accounts_command.run_add(args.provider, args.account_id, args.config_dir, args.label),
        "rename": lambda: accounts_command.run_rename(args.provider, args.account_id, args.label),
        "disable": lambda: accounts_command.run_set_enabled(args.provider, args.account_id, False),
        "enable": lambda: accounts_command.run_set_enabled(args.provider, args.account_id, True),
        "set-reserve": lambda: accounts_command.run_set_ceiling(args.provider, args.account_id, args.percent),
        "set-ceiling": lambda: accounts_command.run_set_ceiling(args.provider, args.account_id, args.percent),
        "plan": lambda: accounts_command.run_add_plan(
            args.provider, args.account_id, args.plan, args.since, args.estimated),
        "extra": lambda: accounts_command.run_add_extra(
            args.provider, args.account_id, args.on, args.amount, args.currency, args.note),
        "migrate": lambda: migrate.run_migrate(args.write),
    }
    return handlers[args.accounts_command]()


def _token_mix(text: str) -> dict[str, float]:
    """`opus=1000,sonnet=2.5` as weighted token counts per model class."""
    try:
        pairs = [item.split("=", 1) for item in text.split(",")]
        mix = {name.strip(): float(count) for name, count in pairs}
    except ValueError:
        raise argparse.ArgumentTypeError(f"expected CLASS=N[,CLASS=N], got {text!r}") from None
    if not all(mix) or any(count < 0 for count in mix.values()):
        raise argparse.ArgumentTypeError(f"expected CLASS=N with a class name and N >= 0, got {text!r}")
    return mix


def _iso_week(text: str) -> str:
    """`2026-W43` as given, when it is an ISO week."""
    from sakura.budget import weekly_window

    try:
        weekly_window.parse_week(text)
    except ValueError as error:
        raise argparse.ArgumentTypeError(str(error)) from None
    return text


def _run_usage(args: argparse.Namespace) -> int:
    from datetime import datetime

    from sakura.usage import command as usage_command

    if args.usage_command is None:
        return usage_command.run(args.json)
    from sakura.usage.archive import command as archive_command
    from sakura.usage.archive import settings

    config = settings.load()
    if args.usage_command == "archive":
        return archive_command.archive(config, archive_command.gh_runner, datetime.now(UTC))
    if args.usage_command == "backfill-codex":
        from sakura.usage.archive import backfill

        return backfill.run(config, args.write)
    if args.usage_command == "quota":
        return archive_command.quota_view(config, args.quota_json)
    if args.usage_command == "estimate":
        return _run_usage_estimate(args, config, archive_command)
    if args.usage_command == "weekly":
        return archive_command.weekly_view(config, args.week, args.account, args.quota_json, datetime.now(UTC))
    return archive_command.report_by_pr(config, args.repo, args.days)


def _run_usage_estimate(args: argparse.Namespace, config, archive_command) -> int:
    """`usage estimate`: a token mix of an account, or a Markdown plan under the flow config of the working directory."""
    from datetime import datetime

    now = datetime.now(UTC)
    if args.plan is None:
        if not args.account:
            raise ValueError("--tokens needs --account")
        return archive_command.estimate(config, args.account, args.tokens, args.quota_json, now)
    from sakura.flow import config as flow_config
    from sakura.usage.archive import settings as archive_settings

    if not archive_settings.config_exists():
        print(f"no usage archive configured: {archive_settings.DEFAULT_CONFIG} is missing", file=sys.stderr)
        return 1
    try:
        text = args.plan.read_text()
    except OSError as error:
        raise ValueError(f"cannot read the plan {args.plan}: {error.strerror or error}") from error
    return archive_command.estimate_plan(config, args.account, text, args.quota_json,
                                         now, flow_config.load(Path.cwd()))


def _run(args: argparse.Namespace) -> int:
    if args.command == "hook":
        from sakura import agent_hooks

        return agent_hooks.run(args.agent, args.event)
    if args.command == "git":
        from sakura import git_checks

        return git_checks.run_hook(args.hook, args.remote_name, args.remote_url)
    if args.command == "run":
        return _run_heavy(args)
    if args.command == "usage":
        return _run_usage(args)
    if args.command == "accounts":
        from sakura.accounts import command as accounts_command

        return _run_accounts(args, accounts_command)
    from sakura import (
        codex_trust,
        install,
        integration,
        profile,
        repo_hooks,
        repository,
        testing,
    )

    repo = repository.root(Path(getattr(args, "path", ".")))
    if args.command == "install":
        (install.install_files if args.files_only else install.install)(repo)
        print(f"sakura installed in {repo}")
        return 0
    if args.command == "uninstall":
        install.uninstall(repo)
        print(f"sakura Git hooks removed from {repo}")
        return 0
    if args.command == "test":
        return testing.run(repo, args.suite)
    if args.command == "integra":
        return integration.run(repo, args.numbers, args.full)
    if args.command == "trust":
        print(repo_hooks.trust(repo))
        return 0
    if args.command == "trust-codex" and args.record:
        print(f"recorded Codex hook trust {codex_trust.record(repo)}")
        return 0
    if args.command == "trust-codex":
        print(json.dumps(codex_trust.eligible_commands(repo), indent=2))
        return 0
    result = profile.load(repo)
    print(json.dumps({"effective": result.values, "layers": result.origins, "relaxations": result.relaxations}, indent=2, sort_keys=True))
    return 0


def _run_heavy(args: argparse.Namespace) -> int:
    from sakura import repository, slots
    from sakura.flow import config as flow_config

    command = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    if not args.heavy or not command:
        print("usage: sakura run --heavy [--critical] [--label TEXT] -- <command...>")
        return 2
    repo = repository.root(Path.cwd())
    stack_id = os.environ.get("SAKURA_FLOW_STACK", "")
    task_store = _flow_task_store(repo, stack_id)
    overrides = task_store.read_state().get("overrides") if task_store else None
    config = flow_config.load(repo, overrides=overrides) if overrides else flow_config.load(repo)
    values = config.values
    configured = values["load"]["slots"]
    count = slots.slot_count(os.environ, os.cpu_count() or 1, configured)
    label = args.label or Path(command[0]).name
    critical = args.critical or os.environ.get("SAKURA_FLOW_CRITICAL") == "1"
    runtime = SimpleNamespace(store=task_store, config=config) if task_store else None
    stop_file = task_store.stop_file if task_store else None
    budget = values["timeouts"]["evaluate"]
    queue = slots.SlotQueue(slots.default_root(), count)
    try:
        with queue.acquire(label, critical, budget, stop_file):
            _refresh_flow_activity(runtime, stack_id, force_touch=True)
            outcome = _run_heavy_child(command, values, runtime, stack_id, stop_file)
    except slots.SlotStopped:
        print(f"stopped while waiting for heavy-work slot: {label}")
        return 1
    except slots.SlotTimeout:
        print(f"timed out waiting for heavy-work slot: {label}")
        return 1
    return _heavy_exit_code(outcome, label)


def _run_heavy_child(command: list[str], values: dict, runtime, stack_id: str, stop_file: Path | None):
    from sakura.flow import process

    idle = values["timeouts"]["idle_seconds"]
    log_dir = runtime.store.operations_dir if runtime else Path.home() / ".cache/sakura/heavy"
    log_path = log_dir / f"heavy-{time.time_ns()}.raw"
    stream_output = _stream_log(log_path)

    def poll() -> None:
        _refresh_flow_activity(runtime, stack_id)
        stream_output()

    # `sakura heavy` runs inside whatever called it (a lane's own sandbox, where `ps` is refused): it keeps the
    # caller's process group, and the caller's supervision covers it.
    if runtime:
        outcome = process.external_tracked_run(runtime.store, command, cwd=Path.cwd(), env=dict(os.environ),
                                               log_path=log_path, budget_seconds=values["timeouts"]["evaluate"],
                                               idle_seconds=idle, stop_file=stop_file, on_poll=poll, own_group=False)
    else:
        outcome = process.monitored_run(command, cwd=Path.cwd(), env=dict(os.environ), log_path=log_path,
                                        budget_seconds=values["timeouts"]["evaluate"], idle_seconds=idle,
                                        stop_file=None, on_start=lambda _pid: None, on_poll=stream_output,
                                        own_group=False)
    stream_output()
    return outcome


def _stream_log(log_path: Path):
    offset = 0

    def stream() -> None:
        nonlocal offset
        with log_path.open("rb") as log:
            log.seek(offset)
            output = log.read()
            offset = log.tell()
        if output:
            sys.stdout.write(output.decode(errors="replace"))
            sys.stdout.flush()

    return stream


def _heavy_exit_code(outcome, label: str) -> int:
    if outcome.stopped or outcome.timed_out or outcome.stalled:
        reason = "stopped" if outcome.stopped else "timed out" if outcome.timed_out else "stalled"
        print(f"heavy command {reason}: {label}")
        return 1
    return outcome.exit_code if outcome.exit_code is not None else 1


def _flow_task_store(repo: Path, stack_id: str) -> TaskStore | None:
    task_id = os.environ.get("SAKURA_FLOW_TASK", "") or stack_id.partition(":")[0]
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,47}", task_id):
        return None
    from sakura.flow.store import TaskStore

    task_store = TaskStore(repo, task_id)
    return task_store if task_store.exists() else None


def _refresh_flow_activity(runtime, stack_id: str, force_touch: bool = False) -> None:
    if runtime is None:
        return
    task_id, separator, workspace = stack_id.partition(":")
    if not separator or not workspace or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,47}", task_id):
        return
    if runtime.store.task_id != task_id:
        return
    from sakura.flow import stacks
    from sakura.flow.store import TaskStore

    fresh_store = TaskStore(runtime.store.repo, task_id)
    fresh_runtime = SimpleNamespace(store=fresh_store, config=runtime.config)
    if force_touch:
        stacks.touch_store(fresh_store, stack_id)
    else:
        stacks.touch_if_due(fresh_runtime, stack_id)
