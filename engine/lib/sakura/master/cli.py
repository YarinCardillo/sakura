"""`sakura master`: start the terminal master, print its launch spec for the app, answer its permission requests, or
serve its tools.

`sakura master`, `sakura master spec --json` and `sakura master decide --json` are operator commands: refused inside an engine role and inside an agent
cage (the engine home is not writable there), as every command that writes the engine's files. Each writes the master's
cage first (`master/spec.py`), so a start always runs on files made for the preset chosen now. `decide` writes nothing:
it reads one ACP permission request on stdin and answers from the settings the last start wrote (`master/decide.py`).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Callable
from dataclasses import replace
from typing import IO

from sakura import engine_home
from sakura.flow import brakes, config, exits, presets
from sakura.master import decide as master_decide
from sakura.master import history
from sakura.master import spec as master_spec
from sakura.setup import apply as setup_apply

SANDBOX_REFUSAL = "operator commands cannot run inside an agent cage: the engine home is not writable here"
NO_CLAUDE = "no Claude Code program found: put it on PATH or name it in CLAUDE_CODE_EXECUTABLE"
USAGE_FAILURES = (master_spec.MasterRefused, config.FlowConfigError, presets.PresetError, engine_home.EngineHomeError)
ACCOUNT_HELP = "the Leeway account (id or label) the master runs on; the host's own account without it"
KNOBS_FROM_CONFIG = ("subagents", "cage")

Exec = Callable[[str, list[str], dict], None]


def main(argv: list[str], *, execute: Exec = os.execvpe, stdin: IO[bytes] | None = None) -> int:
    """Run `sakura master [spec --json | decide --json | serve POLICY]`; 2 for a refusal (printed), 0 when done.

    Without a verb the terminal master takes over this process (`execute` stands in for `os.execvpe`); `decide` reads
    its request from `stdin` (the process's own by default)."""
    if argv[:1] == ["serve"]:
        from sakura.master import server

        return server.main(argv[1:])
    args = _parser().parse_args(argv)
    if args.verb == "decide":
        return _decide(stdin or sys.stdin.buffer, args.slot, args.launch_id)
    try:
        _require_operator()
        if args.verb in ("acp", "history"):
            return _qualified_command(args, stdin or sys.stdin.buffer)
        if args.verb == "spec" and (selection := _qualified_selection(args)):
            return _acp_spec(args, *selection)
        account = _legacy_account(args)
        prepared = master_spec.prepare(engine_home.root(), _preset(), account=account)
    except USAGE_FAILURES as refusal:
        print(refusal)
        return exits.USAGE
    if args.verb == "spec":
        print(json.dumps(prepared.document(), indent=2))
        return exits.OK
    return _terminal(prepared, execute)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura master")
    parser.add_argument("--account", help=ACCOUNT_HELP)
    sub = parser.add_subparsers(dest="verb")
    spec = sub.add_parser("spec")
    spec.add_argument("--json", action="store_true", required=True, help="print the launch spec as JSON")
    spec.add_argument("--account", default=argparse.SUPPRESS, help=ACCOUNT_HELP)
    spec.add_argument("--harness", choices=("claude", "codex", "goose"))
    spec.add_argument("--acp", action="store_true")
    spec.add_argument("--slot", default="main")
    spec.add_argument("--model", default="")
    spec.add_argument("--effort", default="")
    spec.add_argument("--start-fresh", action="store_true")
    permission = sub.add_parser("decide")
    _json_option(permission)
    permission.add_argument("--slot")
    permission.add_argument("--launch-id")
    acp = sub.add_parser("acp")
    _json_option(acp)
    acp.add_argument("--slot", required=True)
    acp.add_argument("--launch-id", required=True)
    record = sub.add_parser("history")
    record.add_argument("operation", choices=("list", "import", "transcript", "record"))
    _json_option(record)
    record.add_argument("--slot", required=True)
    record.add_argument("--harness", choices=("claude", "codex"))
    record.add_argument("--account", default=argparse.SUPPRESS)
    record.add_argument("--conversation")
    record.add_argument("--launch-id")
    return parser


def _json_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--json", action="store_true", required=True)


def _legacy_account(args: argparse.Namespace) -> master_spec.MasterAccount | None:
    if args.verb != "spec" or args.harness is None:
        return master_spec.account_named(args.account) if args.account else None
    from sakura import programs
    from sakura.master import qualified

    try:
        account = qualified.account_for(args.harness, args.account)
    except programs.ProgramError as error:
        raise master_spec.MasterRefused(str(error)) from None
    return master_spec.MasterAccount(account.id, account.provider, str(account.config_dir))


def _qualified_selection(args: argparse.Namespace) -> tuple[str, str | None] | None:
    reference = setup_apply.master_account()
    harness, account = args.harness, args.account
    if not harness and not account and reference and reference["provider"] == "goose":
        harness, account = "goose", reference["id"]
    if args.acp or harness == "goose":
        if harness is None:
            raise master_spec.MasterRefused("choose a harness for the ACP launch")
        if account is None and reference and reference["provider"] == harness:
            account = reference["id"]
        return harness, account
    if args.start_fresh or args.model or args.effort:
        raise master_spec.MasterRefused("choose an ACP Master launch for session choices")
    return None


def _acp_spec(args: argparse.Namespace, harness: str, account: str | None) -> int:
    launch = master_spec.prepare_acp(engine_home.root(), _preset(), harness=harness, account_id=account, slot=args.slot,
                                     model=args.model, effort=args.effort, fresh=args.start_fresh)
    if not args.harness and not args.acp:
        document = master_spec.prepare(engine_home.root(), _preset()).document()
    else:
        document = {key: launch[key] for key in ("argv", "cwd", "env", "env_allow") if key in launch}
    print(json.dumps({**document, "acp": launch}))
    return exits.OK


def _qualified_command(args: argparse.Namespace, stdin: IO[bytes]) -> int:
    from sakura import programs
    from sakura.master import runtime

    home = engine_home.root()
    try:
        if args.verb == "history":
            result = _history(args, stdin, home)
        else:
            request = _read_document(stdin, runtime.MAX_PROMPT * 6 + 128, {"prompt"})
            if "prompt" in request and not isinstance(request["prompt"], str):
                raise history.HistoryError("master_invalid_prompt")
            result = runtime.operate(home, args.slot, args.launch_id, request.get("prompt"))
    except (history.HistoryError, programs.ProgramError) as error:
        result = {"outcome": "refused", "reason": str(error)}
    except OSError:
        result = {"outcome": "refused", "reason": "master_history_unavailable"}
    print(json.dumps(result))
    return exits.OK


def _history(args, stdin, home) -> dict:
    from sakura.master import qualified

    if args.operation == "list":
        return {"conversations": history.listed(home, args.slot)}
    if args.operation == "transcript" and args.conversation:
        return history.transcript(home, args.slot, args.conversation)
    if args.operation == "record" and args.launch_id:
        data = _read_document(stdin, 8192, {"session_id"})
        row = history.opened(home, args.slot, args.launch_id, data.get("session_id"))
        return history.public(row)
    if args.operation == "import" and args.harness:
        data = _read_document(stdin, 8192, {"session_id", "title"})
        binding = qualified.legacy_binding(args.harness, args.account)
        history.import_legacy(home, args.slot, binding, data.get("session_id"), data.get("title", ""))
        return {"outcome": "imported", "binding": binding.document()}
    raise history.HistoryError("master_invalid_history")


def _read_document(stdin: IO[bytes], max_bytes: int, keys: set[str]) -> dict:
    raw = stdin.read(max_bytes + 1)
    if len(raw) > max_bytes:
        raise history.HistoryError("master_request_too_large")
    try:
        data = json.loads(raw)
    except (ValueError, RecursionError):
        raise history.HistoryError("master_invalid_request") from None
    if not isinstance(data, dict) or data.keys() - keys:
        raise history.HistoryError("master_invalid_request")
    return data


def _decide(stdin: IO[bytes], slot: str | None = None, launch: str | None = None) -> int:
    """Print the engine's answer for the legacy or a slot-bound Master permission request."""
    try:
        _require_operator()
    except master_spec.MasterRefused as refusal:
        print(refusal)
        return exits.USAGE
    if slot is None and launch is None:
        decision = _answer(stdin.read(master_decide.MAX_REQUEST_BYTES + 1))
    else:
        from sakura.master import runtime

        try:
            request = _read_document(stdin, master_decide.MAX_REQUEST_BYTES, {"jsonrpc", "id", "method", "params", "sessionId", "toolCall", "options"})
            decision = runtime.permission(engine_home.root(), slot, launch, request)
        except (history.HistoryError, OSError) as error:
            decision = master_decide.Decision(master_decide.DENY, str(error))
    print(json.dumps(decision.document()))
    return exits.OK


def _answer(raw: bytes) -> master_decide.Decision:
    if len(raw) > master_decide.MAX_REQUEST_BYTES:
        return master_decide.oversized()
    try:
        request = master_decide.load_request(raw)
        home = engine_home.root()
        settings = json.loads(master_spec.settings_path(home).read_text())
    except master_decide.MalformedRequest as error:
        return master_decide.Decision(master_decide.DENY, str(error))
    except (OSError, ValueError) as error:
        return master_decide.Decision(master_decide.DENY, f"the master's settings cannot be read ({error}); "
                                                          f"start the master or run `sakura master spec --json`")
    return master_decide.decide(request, settings, master_spec.master_folder(home))


def _require_operator() -> None:
    if os.environ.get(brakes.ROLE_ENV):
        raise master_spec.MasterRefused(brakes.ENGINE_ROLE_REFUSAL)
    if engine_home.sandboxed():
        raise master_spec.MasterRefused(SANDBOX_REFUSAL)


def _preset() -> presets.Preset:
    """The chosen preset with the knobs the personal config sets over it: what the master's cage is made from."""
    chosen = presets.resolve(presets.chosen() or presets.DEFAULT_BASE)
    knobs = config.load(None).values["knobs"]
    return replace(chosen, **{name: knobs[name] for name in KNOBS_FROM_CONFIG})


def _terminal(prepared: master_spec.MasterSpec, execute: Exec) -> int:
    if not prepared.terminal_argv:
        print(NO_CLAUDE, file=sys.stderr)
        return exits.USAGE
    environment = {name: os.environ[name] for name in prepared.env_allow
                   if name in os.environ and name not in prepared.unset_env}
    os.chdir(prepared.cwd)
    cage_environment = prepared.options["env"]
    execute(prepared.terminal_argv[0], prepared.terminal_argv, {**environment, **cage_environment, **prepared.env})
    return exits.OK
