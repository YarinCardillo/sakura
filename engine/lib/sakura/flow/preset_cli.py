"""Command line of `sakura preset`: list the presets, show one, choose the host's."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

from sakura import engine_home, repository
from sakura.flow import brakes, config, exits, presets, rules, task_repository

ROLE_REFUSAL = "sakura preset use cannot run inside an engine role or an agent cage"
SAVE_ROLE_REFUSAL = "sakura preset save cannot run inside an engine role or an agent cage"


def main(argv: list[str]) -> int:
    """Run a `sakura preset` command and return its exit code."""
    args = _parser().parse_args(argv)
    try:
        return {"list": _list, "show": _show, "use": _use, "save": _save}[args.command](args)
    except (presets.PresetError, config.FlowConfigError, engine_home.EngineHomeError, rules.RuleRefused) as error:
        print(error)
        return exits.USAGE


def summary(values: dict) -> str:
    """One ledger line with the effective preset, its knobs and the role of each step."""
    knobs = values["knobs"]
    pace = knobs["pace"]
    on = {True: "on", False: "off"}
    roles = " ".join(f"{role}={spec}" for role, spec in values["roles"].items())
    return (f"preset {values['preset']}: builders {knobs['builders']}, lanes {knobs['lanes']}, subagents {knobs['subagents']}, "
            f"reviewer {on[knobs['reviewer']]}, evaluator {on[knobs['evaluator']]}, fix rounds {knobs['fix_rounds']}, "
            f"pace {pace}, cage {knobs['cage']}; roles {roles}")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="sakura preset")
    sub = parser.add_subparsers(dest="command", required=True)
    listing = sub.add_parser("list")
    listing.add_argument("--json", action="store_true", help="every preset with its label, description and whether it is chosen")
    show = sub.add_parser("show", help="a preset's knobs; without a name, the effective preset of this repository")
    show.add_argument("name", nargs="?")
    show.add_argument("--json", action="store_true")
    use = sub.add_parser("use", help="make a preset the host default, dropping the changes made with `sakura engine set`")
    save = sub.add_parser("save", help="keep the effective rules as a preset of your own and choose it")
    for command in (use, save):
        command.add_argument("--actor", help=argparse.SUPPRESS)  # the cockpit says who asked; its action log records it
        command.add_argument("name")
    return parser


def _list(args: argparse.Namespace) -> int:
    chosen = config.load(None).values["preset"]  # what `sakura engine rules` reports: the choice, else the default
    listed = presets.listing(chosen)
    if args.json:
        print(json.dumps(listed))
        return exits.OK
    for entry in listed:
        print(f"{'*' if entry['chosen'] else ' '} {entry['name']:<12} {entry['description']}")
    return exits.OK


def _show(args: argparse.Namespace) -> int:
    if args.name:
        payload = presets.describe(presets.resolve(args.name))
    else:
        payload = _effective()
    print(json.dumps(payload, indent=2) if args.json else _lines(payload))
    return exits.OK


def _effective() -> dict:
    """The preset the config of this repository (or the host, outside one) resolves to, knob by knob."""
    try:
        repo: Path | None = task_repository.operator_repository(Path("."))
    except repository.RepositoryError:
        repo = None
    cfg = config.load(repo)
    origins = {key: cfg.origins[key] for key in cfg.origins if key.startswith(("knobs.", "roles.")) or key == "preset"}
    return {"preset": cfg.values["preset"], "knobs": cfg.values["knobs"], "roles": cfg.values["roles"],
            "origins": origins}


def _lines(payload: dict) -> str:
    if "origins" in payload:
        lines = [f"preset: {payload['preset']} ({payload['origins'].get('preset', 'defaults')})"]
        lines += [f"{knob}: {value} ({payload['origins'][f'knobs.{knob}']})" for knob, value in payload["knobs"].items()]
        lines += [f"role {role}: {spec} ({payload['origins'][f'roles.{role}']})" for role, spec in payload["roles"].items()]
        return "\n".join(lines)
    return "\n".join(f"{key}: {value}" for key, value in payload.items())


def _use(args: argparse.Namespace) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(ROLE_REFUSAL)
        return exits.USAGE
    preset = presets.choose(args.name)
    rules.clear(None)
    print(f"preset {preset.name} is the host default ({preset.description})")
    return exits.OK


def _save(args: argparse.Namespace) -> int:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(SAVE_ROLE_REFUSAL)
        return exits.USAGE
    rules.save(args.name, None)
    print(f"preset {args.name} holds the rules of this host and is the host default")
    return exits.OK
