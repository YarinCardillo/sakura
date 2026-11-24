"""Command line of `sakura engine`: read the rules of the lane flow (`rules`) and change one (`set`)."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from sakura import engine_home, repository
from sakura.flow import (
    agent_accounts,
    brakes,
    config,
    exits,
    model_catalog,
    pace_accounts,
    rules,
)

ROLE_REFUSAL = "sakura engine set cannot run inside an engine role or an agent cage"
PACE_ROLE_REFUSAL = "sakura engine pace cannot run inside an engine role or an agent cage"
PACE_ACCOUNTS_ROLE_REFUSAL = "sakura engine pace-accounts cannot run inside an engine role or an agent cage"


def main(argv: list[str]) -> int:
    """Run a `sakura engine` command and return its exit code."""
    args = _parser().parse_args(argv)
    try:
        return {"rules": _rules, "set": _set, "pace": _pace, "pace-accounts": _pace_accounts, "models": _models}[args.command](args)
    except (rules.RuleRefused, pace_accounts.AccountsRefused, config.FlowConfigError, engine_home.EngineHomeError,
            repository.RepositoryError) as error:
        print(error)
        return exits.USAGE


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura engine")
    sub = parser.add_subparsers(dest="command", required=True)
    show = sub.add_parser("rules", help="the rules of the lane flow, by node, and the preset they come from")
    show.add_argument("--json", action="store_true")
    change = sub.add_parser("set", help="change one rule; the preset reads as edited until saved or switched")
    change.add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its action log records it
    change.add_argument("rule")
    change.add_argument("value")
    speed = sub.add_parser("pace", help="where each account lands at its reset: slowest, slow, normal, fast or fastest")
    speed.add_argument("--actor", help=argparse.SUPPRESS)
    speed.add_argument("level")
    allow = sub.add_parser("pace-accounts", help="the accounts pace may move this project's steps to, as a comma separated list")
    allow.add_argument("--actor", help=argparse.SUPPRESS)
    allow.add_argument("accounts")
    models = sub.add_parser("models", help="the models each account's program offers, kept from the program itself")
    models.add_argument("--actor", help=argparse.SUPPRESS)
    models.add_argument("action", nargs="?", choices=("refresh",), help="ask every program for its models again")
    models.add_argument("--missing", action="store_true", help="only the accounts with no list kept yet")
    models.add_argument("--json", action="store_true")
    models.add_argument("--acp", action="store_true", help="use the qualified ACP catalog")
    models.add_argument("--provider", choices=("claude", "codex", "goose"))
    models.add_argument("--account", help="read or refresh a single provider-scoped account")
    return parser


def _rules(args: argparse.Namespace) -> int:
    if not (os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed()):
        model_catalog.ensure(model_catalog_accounts())  # a list nobody read yet is read in the background, never waited for
    described = rules.describe(None)
    print(json.dumps(described, indent=2) if args.json else _lines(described))
    return exits.OK


def _lines(described: dict) -> str:
    edited = " (edited)" if described["edited"] else ""
    lines = [f"preset: {described['preset']}{edited}"]
    for node in described["nodes"]:
        lines.append(f"{node['node']}:")
        lines += [f"  {rule['id']} = {rule['value']}{'  [read-only]' if rule['read_only'] else ''}"
                  for rule in node["rules"]]
    return "\n".join(lines)


def _set(args: argparse.Namespace) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(ROLE_REFUSAL)
        return exits.USAGE
    done = rules.set(args.rule, args.value, None)
    print(f"{done['rule']} is now {done['value']}")
    return exits.OK


def _pace(args: argparse.Namespace) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(PACE_ROLE_REFUSAL)
        return exits.USAGE
    done = rules.set_pace(args.level, None)
    line = "no line, only the reserve stops a lane" if done["target_pct"] is None else f"{done['target_pct']}% of the week"
    print(f"pace is now {done['pace']} ({line})")
    return exits.OK


def _pace_accounts(args: argparse.Namespace) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(PACE_ACCOUNTS_ROLE_REFUSAL)
        return exits.USAGE
    kept = pace_accounts.set_allowed(Path.cwd(), pace_accounts.parse(args.accounts), None)
    print(f"pace may move this project's steps to: {', '.join(kept) if kept else 'no other account'}")
    return exits.OK


MODELS_ROLE_REFUSAL = "sakura engine models refresh cannot run inside an engine role or an agent cage"


def _models(args: argparse.Namespace) -> int:
    accounts = [account for account in model_catalog_accounts()
                if (not args.provider or account.provider == args.provider)
                and (not args.account or account.id == args.account)]
    if args.account and len(accounts) != 1:
        print("choose one enabled account by provider and id")
        return exits.USAGE
    if args.action == "refresh":
        if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
            print(MODELS_ROLE_REFUSAL)
            return exits.USAGE
        asked = [account for account in accounts if not args.missing or model_catalog.read(account, via_acp=args.acp) is None]
        print("\n".join(f"{account.label}: {_refreshed(account, via_acp=args.acp)}" for account in asked)
              or "no account to ask")
        return exits.OK
    listed = [{"id": account.id, "label": account.label, "provider": account.provider, **_kept(account, via_acp=args.acp)}
              for account in accounts]
    print(json.dumps({"accounts": listed}, indent=2) if args.json else _model_lines(listed))
    return exits.OK


def model_catalog_accounts() -> list:
    """The accounts a step may run on (`flow/agent_accounts.py`)."""
    return agent_accounts.usable()


def _refreshed(account, *, via_acp=False) -> str:
    try:
        model_catalog.refresh(account, **({"via_acp": True} if via_acp else {}))
    except model_catalog.CatalogError as error:
        return str(error)
    return "ok"


def _kept(account, *, via_acp=False) -> dict:
    catalog = model_catalog.read(account, via_acp=via_acp)
    status = model_catalog.status(account, via_acp=via_acp)
    return {"fetched_at": catalog.fetched_at if catalog else "", "source": catalog.source if catalog else "",
            "models": [model.document() for model in catalog.models] if catalog else [],
            "models_state": status.state, "models_reason": status.reason,
            **({"upstream_provider": account.goose_provider} if account.provider == "goose" else {})}


def _model_lines(listed: list[dict]) -> str:
    lines = []
    for account in listed:
        lines.append(f"{account['label']} ({account['provider']}), {account['fetched_at'] or 'never fetched'}:")
        lines += [f"  {model['id']}  {model['label']}  efforts {', '.join(model['efforts']) or '-'}" for model in account["models"]]
    return "\n".join(lines) or "no account"
