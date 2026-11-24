"""The rules of the lane flow: read them as the nodes of the flow, change one, keep the changes as a preset.

Every rule is a knob, a role or a step account of the flow config; nothing here is a second store. A change is
written to `<engine home>/rules.toml`, a layer of its own above the personal file, so a preset stays what it is and
`describe` can say `edited` until the changes are saved as a preset or dropped (`clear`, or choosing a preset).
The person's own change may weaken a gate, so the layer names every key it sets in its `relax` list.
"""

from __future__ import annotations

import os
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from sakura import engine_home, layers
from sakura.flow import (
    agent_accounts,
    config,
    pace,
    pace_accounts,
    pipeline,
    presets,
    rule_agents,
)
from sakura.flow.rules_nodes import NODES, REVIEW_NODES

CLAUDE_MODELS = ("claude:haiku", "claude:sonnet", "claude:opus")
BOOLEANS = {"true": True, "false": False}
FILE_MODE = 0o600
ALWAYS_ON_REASON = "build is always on: the least a lane does is build"
READ_ONLY_REASON = "is read-only: the guard hands out the cage and the grants to every step"


class RuleRefused(ValueError):
    """Raised, with a one-line reason, when a rule is unknown, read-only, or its value is not acceptable."""


@dataclass(frozen=True)
class RuleSpec:
    """One rule: where it sits, what it is called, and the config key it reads and writes."""

    id: str
    node: str
    label: str
    type: str
    key: str = ""
    role: str = ""
    read_only: bool = False
    kind: str = ""  # "model" and "effort" of a step's agent, "account" of a step: chosen with the others (`flow/rule_agents.py`)


def _on(node: str, key: str, label: str) -> RuleSpec:
    group = "reviews" if node in REVIEW_NODES else "steps"
    return RuleSpec(f"{group}.{node}.on", node, label, "bool", key)


def _model(node: str, role: str) -> RuleSpec:
    group = "reviews" if node in REVIEW_NODES else "steps"
    return RuleSpec(f"{group}.{node}.model", node, "Model", "choice", f"roles.{role}", role, kind="model")


def _effort(node: str, role: str) -> RuleSpec:
    group = "reviews" if node in REVIEW_NODES else "steps"
    return RuleSpec(f"{group}.{node}.effort", node, "Effort", "choice", f"efforts.{role}", role, kind="effort")


def _account(node: str) -> RuleSpec:
    group = "reviews" if node in REVIEW_NODES else "steps"
    return RuleSpec(f"{group}.{node}.account", node, "Account", "choice", f"step_accounts.{node}", kind="account")


SPECS = (
    RuleSpec("lanes_min", "start", "Lanes at least", "int", "knobs.lanes_min"),
    RuleSpec("lanes", "start", "Lanes at most", "int", "knobs.lanes"),
    RuleSpec("approve_spec", "start", "Ask before a lane builds", "bool", "approve_spec"),
    _on("plan", "knobs.plan", "Plan"), _model("plan", "planner"), _effort("plan", "planner"), _account("plan"),
    _on("plan_review", "knobs.plan_review", "Review the plan"), _model("plan_review", "plan_reviewer"),
    _effort("plan_review", "plan_reviewer"), _account("plan_review"),
    RuleSpec("steps.build.on", "build", "Build", "bool", read_only=True), _model("build", "builder"), _effort("build", "builder"), _account("build"),
    RuleSpec("builders", "build", "Builders at once", "int", "knobs.builders"),
    RuleSpec("subagents", "build", "Subagents", "int", "knobs.subagents"),
    _on("task_review", "knobs.task_review", "Review each task"), _model("task_review", "code_reviewer"),
    _effort("task_review", "code_reviewer"), _account("task_review"),
    RuleSpec("reviews.task_review.rounds", "task_review", "Rounds of changes", "int", "knobs.fix_rounds"),
    _on("final_review", "knobs.final_review", "Review the whole branch"), _account("final_review"),
    RuleSpec("reviews.final_review.rounds", "final_review", "Rounds of changes", "int", "knobs.final_review_rounds"),
    _on("checks", "knobs.checks", "Checks"), RuleSpec("evaluator", "checks", "Prove each row", "bool", "knobs.evaluator"),
    _model("checks", "evaluator"), _effort("checks", "evaluator"), _account("checks"),
    RuleSpec("reporter", "deliver", "Write the report", "bool", "knobs.reporter"),
    RuleSpec("report_language", "deliver", "Report language", "choice", "knobs.report_language"),
    RuleSpec("guard.cage", "guard", "Cage", "text", "knobs.cage", read_only=True),
    RuleSpec("guard.grants", "guard", "Grants", "list", read_only=True),
)
SPEC_BY_ID = {spec.id: spec for spec in SPECS}
# Rules that are the person's preferences, not part of how a lane runs: kept apart, they never edit the preset.
PREFERENCE_RULES = frozenset({"report_language"})
REVIEW_SWITCH = {"plan_review": "knobs.plan_review", "task_review": "knobs.task_review",
                 "final_review": "knobs.final_review"}


def describe(repo: Path | None, *, account_ids: Sequence[str] | None = None, accounts: Sequence | None = None) -> dict:
    """The effective rules of `repo` (the host's, without one) grouped by node, with where each value comes from.

    `agents` lists every account with the models its program lists (kept by `sakura engine models refresh`) and the efforts each
    takes; a step's model rule carries its `agent`: account, program, model, effort and what it may choose."""
    cfg = config.load(repo)
    known = _accounts(account_ids, accounts)
    agents = rule_agents.load(known, account_ids)
    ids = [account.id for account in agents.accounts]
    rules = [_describe(spec, cfg, agents) for spec in SPECS if not (spec.kind == "account" and len(ids) < 2)]
    nodes = [{"node": node, "rules": [rule for rule in rules if rule["node"] == node]} for node in NODES]
    return {"preset": cfg.values["preset"], "preset_label": presets.label(cfg.values["preset"]), "edited": is_edited(),
            "accounts_count": len(ids), "agents": _agent_accounts(agents),
            "pace_accounts": pace_accounts.everywhere(None),
            "nodes": [{"node": item["node"], "rules": [_public(rule) for rule in item["rules"]]} for item in nodes]}


def _accounts(account_ids: Sequence[str] | None, accounts: Sequence | None) -> Sequence:
    """The accounts a step may run on: those given, the plain Claude ids an older caller gives, else the registry's."""
    if accounts is not None:
        return accounts
    if account_ids is not None:
        return None
    return registered_accounts()


def _agent_accounts(agents) -> list[dict]:
    return [_agent_document(account, agents.catalogs.get(account.key), agents.statuses.get(account.key))
            for account in agents.accounts]


def _agent_document(account, catalog, status) -> dict:
    return {"id": account.id, "label": account.label, "provider": account.provider,
            "fetched_at": catalog.fetched_at if catalog else "", "source": catalog.source if catalog else "",
            "models": [model.document() for model in catalog.models] if catalog else [],
            **({"upstream_provider": account.upstream_provider} if account.provider == "goose" else {}),
            **_status_fields(status)}


def _status_fields(status) -> dict:
    shown = {"models_state": status.state if status else "missing", "models_note": status.note if status else ""}
    if status and status.reason:
        shown["models_reason"] = status.reason
    return shown


def set(rule_id: str, value: object, home: Path | None, *, account_ids: Sequence[str] | None = None,
        accounts: Sequence | None = None) -> dict:
    """Validate `value` for rule `rule_id` and keep it in the rules layer; returns the rule as it now reads.

    Raises RuleRefused, and keeps nothing, when the rule is unknown or read-only, the value is not acceptable or the
    config it would make does not load.
    """
    spec = _spec(rule_id)
    known = _accounts(account_ids, accounts)
    agents = rule_agents.load(known, account_ids)
    cfg = config.load(None, home)
    try:
        accepted = _accepted(spec, value, agents, cfg)
        edits = {spec.key: accepted, **_companions(spec, accepted, cfg, agents)}
    except rule_agents.AgentRefused as refusal:
        raise RuleRefused(f"{spec.id}: {refusal}") from refusal
    if spec.id in PREFERENCE_RULES:
        _keep_preference(spec.key, accepted, home)
        return {"rule": rule_id, "value": accepted, "edited": is_edited(home)}
    _write_checked(_without_presets_values(_merged_layer(_read_layer(home), edits), home), home)
    return {"rule": rule_id, "value": accepted, "edited": is_edited(home)}


def set_pace(level: str, home: Path | None) -> dict:
    """Keep the engine's pace `level` (`flow/pace.py`) in the layer of its own; the preset's edits are not touched.

    Raises RuleRefused, and keeps nothing, for a level that is not known.
    """
    if level not in pace.LEVELS:
        raise RuleRefused(f"pace must be one of {', '.join(pace.LEVELS)}; got {level!r}")
    _write_checked({"knobs": {"pace": level}}, home, config.PACE_FILE)
    return {"pace": level, "target_pct": pace.TARGETS[level]}


def clear(home: Path | None) -> None:
    """Drop the person's changes: the preset reads as it is again."""
    _layer_path(home).unlink(missing_ok=True)


def is_edited(home: Path | None = None) -> bool:
    """Whether the rules layer holds a change of how a lane runs; a preference an older engine left there is none."""
    preferences = {SPEC_BY_ID[rule_id].key for rule_id in PREFERENCE_RULES}
    layer = {key: value for key, value in _read_layer(home).items() if key != "relax"}
    return any(key not in preferences for key in layers.flatten(layer))


def save(name: str, home: Path | None) -> Path:
    """Keep the effective rules as the preset `name`, choose it, and drop the changes it now holds; returns its file.

    A preset names three model tiers, so the planner, the builder and the code reviewer stand for theirs.
    Raises RuleRefused for a bad or taken name.
    """
    root = engine_home.root(home)
    path = _preset_path(name, root)
    cfg = config.load(None, home)
    text = _preset_text(cfg, root)
    engine_home.ensure_private(root)
    path.parent.mkdir(mode=engine_home.DIRECTORY_MODE, exist_ok=True)
    _create(path, text)
    clear(home)
    presets.choose(name, root)
    return path


def registered_accounts() -> list:
    """The accounts the person can run a step on: Leeway's book, else the enabled accounts of the registry."""
    return agent_accounts.usable()


def registered_account_ids() -> list[str]:
    """The ids of the accounts the person can run a step on: the enabled accounts of the registry."""
    return sorted({account.id for account in registered_accounts()})


# Reading

def _describe(spec: RuleSpec, cfg: config.FlowConfig, agents) -> dict:
    rule = {"id": spec.id, "node": spec.node, "label": spec.label, "type": spec.type, "choices": _choices(spec, cfg, agents),
            "min": None, "max": None, "origin": cfg.origins.get(spec.key, "defaults"), "read_only": spec.read_only}
    if spec.type == "int":
        rule["min"], rule["max"] = presets.KNOB_LIMITS[spec.key.split(".")[1]]
    rule["value"] = _value(spec, cfg, agents)
    if spec.kind == "model":
        rule["agent"] = rule_agents.view(spec.node, cfg, agents)
    return rule


def _public(rule: dict) -> dict:
    return {key: value for key, value in rule.items() if key != "node"}


def _value(spec: RuleSpec, cfg: config.FlowConfig, agents=None) -> object:
    if spec.kind == "effort":
        return rule_agents.view(spec.node, cfg, agents)["effort"]
    built = pipeline.of_values(cfg.values)
    if spec.id == "steps.build.on":
        return True
    if spec.id.endswith(".on") and spec.id.split(".")[1] in pipeline.REVIEW_IDS:
        return built.review(spec.id.split(".")[1]).on
    if spec.id == "guard.grants":
        return _granted_classes(engine_home.root())
    return layers.get_key(cfg.values, spec.key)


def _choices(spec: RuleSpec, cfg: config.FlowConfig, agents) -> list:
    if spec.kind == "account":
        return [presets.PROJECT_ACCOUNT, *(account.id for account in agents.allowed())]
    if spec.kind == "model":
        return rule_agents.model_choices(spec.node, agents, cfg.values["step_accounts"].get(spec.node, ""))
    if spec.kind == "effort":
        return rule_agents.efforts_of(spec.node, cfg, agents)
    if spec.id == "report_language":
        return list(config.REPORT_LANGUAGES)
    return []


def _granted_classes(root: Path) -> list[str]:
    try:
        return sorted(tomllib.loads((root / "grants.toml").read_text()))
    except (OSError, tomllib.TOMLDecodeError):
        return []


# Changing

def _spec(rule_id: str) -> RuleSpec:
    spec = SPEC_BY_ID.get(rule_id)
    if spec is None:
        raise RuleRefused(f"unknown rule {rule_id!r}; `sakura engine rules` lists them")
    return spec


def _accepted(spec: RuleSpec, value: object, agents, cfg: config.FlowConfig) -> object:
    if spec.id == "steps.build.on":
        _require_true(_boolean(spec, value))
    if spec.read_only:
        raise RuleRefused(f"{spec.id} {READ_ONLY_REASON}")
    if spec.kind == "account":
        return _account_choice(spec, value, agents, cfg)
    if spec.kind == "model":
        return rule_agents.check_model(spec.node, str(value), cfg, agents)
    if spec.kind == "effort":
        return rule_agents.check_effort(spec.node, str(value), cfg, agents)
    parse = {"bool": _boolean, "int": _whole_number, "choice": _choice}[spec.type]
    return parse(spec, value)


def _require_true(value: bool) -> None:
    if not value:
        raise RuleRefused(ALWAYS_ON_REASON)


def _boolean(spec: RuleSpec, value: object) -> bool:
    found = value if isinstance(value, bool) else BOOLEANS.get(str(value).strip().lower())
    if found is None:
        raise RuleRefused(f"{spec.id} must be true or false; got {value!r}")
    return found


def _whole_number(spec: RuleSpec, value: object) -> int:
    low, high = presets.KNOB_LIMITS[spec.key.split(".")[1]]
    text = str(value).strip()
    if isinstance(value, bool) or not text.isdigit() or not low <= int(text) <= high:
        raise RuleRefused(f"{spec.id} must be a whole number from {low} to {high}; got {value!r}")
    return int(text)


def _choice(spec: RuleSpec, value: object) -> str:
    allowed = _choices(spec, None, None)
    if str(value) not in allowed:
        raise RuleRefused(f"{spec.id} must be one of {', '.join(allowed)}; got {value!r}")
    return str(value)


def _account_choice(spec: RuleSpec, value: object, agents, cfg: config.FlowConfig) -> str:
    if len(agents.accounts) < 2:
        raise RuleRefused(f"{spec.id} has a choice only with more than one account")
    account = rule_agents.check_account(spec.node, str(value), agents)
    if spec.node == "final_review" and account is not None and account.provider != config.roles_provider(cfg, "code_reviewer"):
        raise RuleRefused(f"{spec.id}: the whole-branch review runs the model of the code review: change that model first")
    return str(value)


def _companions(spec: RuleSpec, value: object, cfg: config.FlowConfig, agents) -> dict:
    """The other keys a change must carry: turning a review on while the reviewers are off turns on that review only; moving a
    step to the other program's account takes that program's model; a new model keeps the effort only when it takes it."""
    if spec.kind == "account" and spec.node != "final_review":
        return rule_agents.companions(spec.node, rule_agents.check_account(spec.node, str(value), agents), cfg, agents)
    if spec.kind == "model":
        return rule_agents.effort_after_model(spec.node, spec.role, str(value), cfg, agents)
    if spec.id == "reviews.plan_review.on" and value and not cfg.values["knobs"]["plan"]:
        raise RuleRefused("the plan review needs the plan step on; turn steps.plan.on on first")
    node = spec.id.split(".")[1] if spec.id.endswith(".on") else ""
    return review_companions(node, cfg.values["knobs"]) if value else {}


def review_companions(node: str, knobs: dict) -> dict:
    """What turning review `node` on carries while the reviewers are off: the reviewer switch on and the other reviews off."""
    if node not in pipeline.REVIEW_IDS or knobs["reviewer"]:
        return {}
    return {"knobs.reviewer": True, **{key: False for other, key in REVIEW_SWITCH.items() if other != node}}


# The layer file

def _keep_preference(key: str, value: object, home: Path | None) -> None:
    """Keep `key` in the preferences layer, and take it out of the rules layer where an older engine kept it."""
    kept = _read_layer(home, config.PREFERENCES_FILE)
    _write_checked(_merged_layer(kept, {key: value}), home, config.PREFERENCES_FILE)
    layer = _read_layer(home)
    if key not in layers.flatten({name: part for name, part in layer.items() if name != "relax"}):
        return
    *parents, leaf = key.split(".")
    table = layer
    for part in parents:
        table = table[part]
    del table[leaf]
    rest = {name: part for name, part in layer.items() if name != "relax"}
    if layers.flatten(rest):
        _write_checked(_merged_layer(rest, {}), home)
    else:
        clear(home)


def _layer_path(home: Path | None, name: str = config.RULES_FILE) -> Path:
    return engine_home.root(home) / name


def _read_layer(home: Path | None, name: str = config.RULES_FILE) -> dict:
    try:
        return tomllib.loads(_layer_path(home, name).read_text())
    except FileNotFoundError:
        return {}
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise RuleRefused(f"cannot read {_layer_path(home, name)}: {error}") from error


def _merged_layer(layer: dict, edits: dict) -> dict:
    merged = {key: value for key, value in layer.items() if key != "relax"}
    for key, value in edits.items():
        table = merged
        *parents, leaf = key.split(".")
        for part in parents:
            table = table.setdefault(part, {})
        table[leaf] = value
    flat = layers.flatten(merged)
    return {**merged, "relax": sorted(flat)}


def _without_presets_values(layer: dict, home: Path | None) -> dict:
    """`layer` without every key whose value is what the config gives without the rules layer (the preset's own, or a
    later layer's), and without its `relax` entry: a rule set back to that is no edit. An empty layer is an empty dict."""
    given = layers.flatten(config.load(None, home, without_rules=True).values)
    flat = layers.flatten({key: value for key, value in layer.items() if key != "relax"})
    kept = {key: value for key, value in flat.items() if key not in given or given[key] != value}
    pruned: dict = {}
    for key, value in kept.items():
        table = pruned
        *parents, leaf = key.split(".")
        for part in parents:
            table = table.setdefault(part, {})
        table[leaf] = value
    return {**pruned, "relax": sorted(kept)} if kept else {}


def _write_checked(layer: dict, home: Path | None, name: str = config.RULES_FILE) -> None:
    """Write the layer to file `name` (no file for an empty one), and put the old file back when the config it makes does
    not load."""
    path = _layer_path(home, name)
    before = path.read_text() if path.exists() else None
    if layer:
        _write_text(path, dump(layer))
    else:
        path.unlink(missing_ok=True)
    try:
        config.load(None, home)
    except config.FlowConfigError as error:
        if before is None:
            path.unlink(missing_ok=True)
        else:
            _write_text(path, before)
        raise RuleRefused(str(error)) from error


def _write_text(path: Path, text: str) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, FILE_MODE)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(text)
    temporary.replace(path)


def dump(layer: dict) -> str:
    """TOML text of a layer made of scalars, lists of text and tables of those."""
    scalars = [f"{key} = {_literal(value)}" for key, value in layer.items() if not isinstance(value, dict)]
    tables = [f"\n[{name}]\n" + "\n".join(f"{key} = {_literal(value)}" for key, value in table.items())
              for name, table in layer.items() if isinstance(table, dict)]
    return "\n".join(scalars) + "\n" + "\n".join(tables) + "\n"


def _literal(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return repr(value)
    if isinstance(value, list):
        return "[" + ", ".join(_literal(item) for item in value) + "]"
    return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


# Saving as a preset

def _preset_path(name: str, root: Path) -> Path:
    if not isinstance(name, str) or not presets.NAME.fullmatch(name):
        raise RuleRefused(f"the preset name must match {presets.NAME.pattern}; got {name!r}")
    if name in presets.BUILTINS:
        raise RuleRefused(f"{name} is the name of a built-in preset; choose another")
    path = root / presets.PRESETS_DIR / f"{name}{presets.PRESET_SUFFIX}"
    if path.exists():
        raise RuleRefused(f"the preset {name} already exists; choose another name")
    return path


def _preset_text(cfg: config.FlowConfig, root: Path) -> str:
    values, knobs, roles = cfg.values, cfg.values["knobs"], cfg.values["roles"]
    top = [f'description = "Saved from the rules of {values["preset"]}."', f"builders = {knobs['builders']}",
           f"lanes = {knobs['lanes']}", f"lanes_min = {knobs['lanes_min']}", f"subagents = {knobs['subagents']}",
           f"evaluator = {_literal(knobs['evaluator'])}", f"reporter = {_literal(knobs['reporter'])}",
           f"report_language = {_literal(knobs['report_language'])}", f"cage = {_literal(knobs['cage'])}"]
    models = ["", "[models]", f"lead = {_literal(roles['planner'])}", f"worker = {_literal(roles['builder'])}",
              f"review = {_literal(roles['code_reviewer'])}"]
    return "\n".join(top) + "\n" + pipeline.to_toml(pipeline.of_values(values)) + "\n".join(models) + "\n"


def _create(path: Path, text: str) -> None:
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, FILE_MODE)
    except FileExistsError:
        raise RuleRefused(f"the preset file {path.stem} already exists; choose another name") from None
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(text)
