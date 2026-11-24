"""Layered flow configuration: defaults, personal, shared, local and flag overrides."""

from __future__ import annotations

import copy
import os
import re
import tomllib
from functools import partial
from urllib.parse import urlparse
from dataclasses import dataclass, field
from pathlib import Path

from sakura import engine_home, file_cache, layers, repository
from sakura.flow import controls, flow_paths, pace, pipeline, presets
from sakura.budget import routing as budget_routing
from sakura.budget import settings as budget_settings
from sakura import profile
from sakura.profile import FLOW_TABLE

# Budget thresholds a layer weakens by lowering (floors) or by raising (alert and age limits).
BUDGET_FLOORS = ("budget.reserve_week_pct", "budget.reserve_five_hour_pct")
BUDGET_CEILINGS = ("budget.warn_flow_week_pct", "budget.ask_flow_week_pct", "budget.outside_share_alert",
                   "budget.snapshot_max_age_minutes")
CHOSEN_LAYER = "chosen"
PRESET_ORIGIN = "preset:"
WEAKENABLE_KNOBS = ("knobs.reviewer", "knobs.evaluator", "knobs.reporter",
                    *(f"knobs.{knob}" for knob in presets.STEP_KNOBS))
# The layer `sakura engine set` writes (`flow/rules.py`): the person's own edits of a preset, in a file of their own.
RULES_LAYER = "rules"
RULES_FILE = "rules.toml"
# The engine's pace (`flow/pace.py`) is not part of a preset: it lives in a layer of its own, which dropping the edits
# of a preset leaves alone.
PACE_LAYER = "pace"
PACE_FILE = "pace.toml"
# The person's preferences that are not part of how a lane runs (the report's language): changing one leaves the
# preset unedited, and dropping the edits of a preset leaves them alone.
PREFERENCES_LAYER = "preferences"
PREFERENCES_FILE = "preferences.toml"
OLD_DAY_PACE_KNOB = "knobs.pace_day_pct"
OLD_DAY_PACE_WARNING = ("knobs.pace_day_pct is ignored: pace is now where each account lands at its reset; "
                        "set it with `sakura engine pace`")
# Knobs an explicit dial sets, with the dial field behind each.
DIAL_KNOBS = {"builders": "builders_cap", "fix_rounds": "fix_rounds"}
# Knobs that spend more when raised; the pace loosens when raised or turned off (0).
SPENDING_KNOBS = ("knobs.builders", "knobs.lanes", "knobs.subagents")
PACE_KNOB = "knobs.pace_day_pct"
CAGE_KNOB = "knobs.cage"
# Claude model families from cheapest to strongest; any other model is treated as the strongest (fail closed).
MODEL_TIERS = ("haiku", "sonnet", "opus")
FAMILIARITY_LEVELS = ("new", "familiar", "expert")
ROLE_PROVIDERS = ("claude", "codex", "goose", "exec", "none")
# The roles that only read: their agent runs read-only whatever its provider. Every other role writes its own work.
REVIEW_ROLES = ("plan_reviewer", "code_reviewer")
CODEX_PROVIDER = "codex"
EFFORT = re.compile(r"[a-z][a-z0-9_-]{0,19}")
WEAKENABLE_ROLES = ("plan_reviewer", "code_reviewer", "evaluator")
OPUS_PROVIDER = "claude"
REPOSITORY_LAYERS = ("shared", "local")
# The project facts a repository layer (shared or local) may declare: its commands (approved and caged before they
# run), its task branch template (under the protected-name rules), its app, its tests and contract facts. Everything
# else (roles, presets, dial, knobs, routes, budget, cage, checks lists, the stack commands, which run outside the
# sandbox, pace, lanes, builders, timeouts, and the app's origin, whose port the cage opens) is the host's; a
# repository's `relax` is refused by the layer merge.
REPOSITORY_KEYS = frozenset({
    "workspace.branch", "workspace.setup", "app.start", "app.stop", "app.ready_url", "app.ready_seconds",
    "tests.focused", "tests.full", "tests.e2e", "tests.viewports", "contract.required_ids",
    "spec.source", "brakes.test_patterns", "brakes.test_only_markers", "brakes.scan_allowlists", "report.project"})
NO_ROLE = "none"
COMMAND_KEY_PREFIXES = ("tests.", "workspace.setup", "app.start", "app.stop")
HOST_COMMAND_PREFIXES = ("stack.up", "stack.down", "checks.approved", "checks.remembered")
SILENT_CHECK_PATTERN = re.compile(r"(&>|[12]?>)\s*/dev/null")
FLAGS_LAYER = "flags"
NOTIFY_PREFIX = "notify."
GATES_PREFIX = "gates."
GATE_MODES = ("day", "night", "auto")
DAY_WINDOW = re.compile(r"(?P<start>([01]\d|2[0-3]):[0-5]\d)-(?P<end>([01]\d|2[0-3]):[0-5]\d)")
NOTIFY_LAYER = "personal"
KNOWLEDGE_SHARED_KEY = "knowledge.shared"
OWNER_DIR_KEY = "report.owner_dir"
OWNER_DIRS_KEY = "report.owner_dirs"
STATUS_DIR_KEY = "report.status_dir"
OWNER_DIR_PLACEHOLDER = "{owner_dir}"
# The owner report templates (`flow/owner_report.py`); kept here as names so config needs no report import.
REPORT_LANGUAGES = presets.REPORT_LANGUAGES
# Keys that moved: a layer that still sets one is told where the setting lives now.
MOVED_KEYS = {("report", "language"): "report.language moved: the report language is the knob "
                                      "knobs.report_language (it or en), in a config file's [knobs] table or as "
                                      "report_language in a preset"}
NOTIFY_FILE = "<engine home>/config.toml"
COCKPIT_TABLE = "cockpit"
LOCAL_OVERLAY = ".sakura/local.toml"
LOOPBACK_HOSTS = ("127.0.0.1", "::1", "localhost")
SURFACE_NAME = re.compile(r"[A-Za-z0-9._-]{1,32}")
ACCOUNT_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")


@dataclass(frozen=True)
class PresetSteps:
    """Which optional review steps a preset runs."""

    plan_review: bool
    code_review: bool
    evaluate: bool
    # The default keeps the three-field form older callers build; the plan step is read from the pipeline
    # (`flow/pipeline.py`), which also derives these.
    final_review: bool = False


@dataclass(frozen=True)
class DialStep:
    """Resource budget of one dial position."""

    claude_model: str
    effort: str
    builders_cap: int
    fix_rounds: int


@dataclass(frozen=True)
class FlowConfig:
    """Effective flow settings, the layer behind each key and consented relaxations."""

    values: dict
    origins: dict[str, str]
    relaxations: list[str]
    # Settings that are read but ignored, in words for the person (a retired knob that a file still sets).
    warnings: list[str] = field(default_factory=list)


class FlowConfigError(ValueError):
    """Raised when flow configuration is malformed or weakens a rule without consent."""


def _steps_of_knobs(knobs: dict) -> PresetSteps:
    built = pipeline.of_knobs(knobs)
    # Checks off means no evaluation either; the evaluator setting itself is kept for when checks are on again.
    return PresetSteps(built.review("plan_review").on, built.review("task_review").on,
                       bool(knobs["evaluator"]) and built.step("checks").on, built.review("final_review").on)


PRESETS = {name: _steps_of_knobs(preset.knobs()) for name, preset in presets.BUILTINS.items()}

DIAL = {
    "save": DialStep("haiku", "low", 1, 1),
    "lean": DialStep("sonnet", "low", 2, 2),
    "balanced": DialStep("sonnet", "medium", 3, 2),
    "strong": DialStep("opus", "high", 4, 3),
    "power": DialStep("opus", "xhigh", 6, 4),
}

DEFAULTS = {
    "preset": "standard",
    "dial": "balanced",
    "familiarity": "familiar",
    "approve_spec": True,
    "roles": {
        "planner": "claude:opus",
        "plan_reviewer": "claude:sonnet",
        "builder": "claude:sonnet",
        "code_reviewer": "claude:sonnet",
        "evaluator": "claude:opus",
        "reporter": "claude:sonnet",
    },
    # The effort a role runs at when the person chose one for its step ("": the dial's own, `DialStep.effort`).
    "efforts": {"planner": "", "plan_reviewer": "", "builder": "", "code_reviewer": "", "evaluator": "", "reporter": ""},
    "builders": {"min": 1, "max": 4},
    "knobs": {**presets.BUILTINS["standard"].knobs(), "pace": pace.DEFAULT_LEVEL, "lane_pace": ""},
    "step_accounts": {step: presets.PROJECT_ACCOUNT for step in presets.ACCOUNT_STEPS},
    "stack": {"up": "", "down": ""},
    "checks": {"approved": [], "remembered": [], "loopback_ports": [], "local_sockets": False},
    "spec": {"source": "file"},
    "app": {"start": "", "stop": "", "ready_url": "", "ready_seconds": 60, "origin": ""},
    "contract": {"required_ids": [], "flow_paths": list(flow_paths.DEFAULT)},
    "tests": {"focused": [], "full": [], "e2e": [], "viewports": ["1440x900", "390x844"]},
    "workspace": {"root": f"{engine_home.ENGINE_PLACEHOLDER}-workspaces/{{repo}}", "branch": "feat/flow-{task}", "setup": []},
    "brakes": {
        "test_patterns": ["tests/**", "test/**", "e2e/**", "**/test_*.py", "**/*_test.*", "**/*.test.*", "**/*.spec.*"],
        "test_only_markers": [],
        "scan_allowlists": [".gitleaksignore", ".gitleaks.toml", ".secrets.baseline"],
    },
    "surface": {"tmux": True, "session_prefix": "flow-", "cockpit_session": "flow"},
    "report": {"owner_dir": f"{engine_home.ENGINE_PLACEHOLDER}/reports/{{repo}}", "owner_dirs": {}, "project": "",
               "status_dir": f"{OWNER_DIR_PLACEHOLDER}/stato"},
    "notify": {"ntfy_keychain": "", "ntfy_url": "https://ntfy.sh"},
    "knowledge": {"shared": "", "project": ""},
    "gates": {"mode": "auto", "day": "08:00-23:00"},
    "limits": {"claude_max_budget_usd": 0.0},
    "budget": {
        "account": "", "reserve_week_pct": 25, "reserve_five_hour_pct": 30, "warn_flow_week_pct": 5,
        "ask_flow_week_pct": 10, "outside_share_alert": 0.4, "night": "23:00-07:00",
        "snapshot_max_age_minutes": 30,
        "routes": {"mechanical": "haiku", "integration": "sonnet", "design": "sonnet:high", "reviewer": "sonnet",
                   "reporter": "sonnet", "intake": "haiku"},
    },
    "load": {"slots": 0, "idle_stack_seconds": 900, "critical": [], "nice": 10},
    "timeouts": {
        "idle_seconds": 600, "intake": 900, "contract": 1800, "plan": 1800, "plan_review": 1200,
        "build": 5400, "review": 1800, "calibrate": 2700, "evaluate": 2700, "arbiter": 900, "report": 1200,
    },
}


def load_personal(home: Path | None = None) -> FlowConfig:
    """Defaults and the personal layer only, for jobs that run without a repository (the usage archive)."""
    return load(None, home)


def load(repo: Path | None, home: Path | None = None, overrides: dict | None = None,
         trusted: bool = False, without_rules: bool = False, *, folder_missing: bool = False) -> FlowConfig:
    """Load defaults, personal, shared, project, local and flag layers in precedence order.

    The personal and project layers live in the engine home, outside the repository; no `repo` skips the
    shared, project and local layers. `trusted` (Git hooks and stamp writers) reads them from the trust
    root, which `SAKURA_ENGINE_HOME` cannot move. `home` stands in for the account home in tests. `without_rules` leaves
    out the person's rules layer: what the preset and the other layers give alone. A missing main worktree
    loads its project layer by path and has no shared or local layer.
    """
    base = engine_home.root_for_trust() if trusted else engine_home.root(home)
    local = _read(repo / LOCAL_OVERLAY).get(FLOW_TABLE, {}) if repo and not folder_missing else {}
    personal = _read(base / engine_home.CONFIG_NAME)
    owner_dirs = _take_owner_dirs(personal)
    stack = [("personal", personal)]
    chosen = presets.chosen(base)
    if chosen:
        # `sakura preset use` is the person's own word, so moving to a weaker preset needs no `relax`.
        stack.append((CHOSEN_LAYER, {"preset": chosen, layers.RELAX_KEY: ["preset"]}))
    stack += [
        (RULES_LAYER, {} if without_rules else _read(base / RULES_FILE)),
        (PACE_LAYER, _read(base / PACE_FILE)),
        (PREFERENCES_LAYER, _read(base / PREFERENCES_FILE)),
        ("shared", _read(repo / ".sakura/flow.toml") if repo and not folder_missing else {}),
        ("project", _read_project(repo, base, folder_missing=folder_missing) if repo else {}),
        ("local", local),
    ]
    if overrides:
        stack.append((FLAGS_LAYER, overrides))
    loaded = _merged(stack, owner_dirs, base)
    if repo and not folder_missing:
        _require_safe_task_branch(loaded, repo, home)
    return loaded


def _require_safe_task_branch(loaded: FlowConfig, repo: Path, home: Path | None) -> None:
    """A repository layer may set `workspace.branch` only to a template with the task placeholder that can never
    render to a protected branch (main, master, the integration branch)."""
    layer = loaded.origins.get("workspace.branch")
    if layer not in REPOSITORY_LAYERS:
        return
    template = loaded.values["workspace"]["branch"]
    names = profile.all_protected_names(repo, home=home)
    if profile.TASK_PLACEHOLDER not in template:
        raise FlowConfigError(f"workspace.branch in the {layer} layer must contain {profile.TASK_PLACEHOLDER}; got {template!r}")
    reached = sorted(name for name in names if profile.template_can_render_to(template, name))
    if reached:
        raise FlowConfigError(f"workspace.branch {template!r} in the {layer} layer can render to the protected "
                              f"branch {reached[0]!r}; only a host layer may set that")


def defaults() -> FlowConfig:
    """The built-in values alone, for a reader that must keep working when the personal file does not load."""
    return _merged([], {}, engine_home.account_root())


def _merged(stack: list[tuple[str, dict]], owner_dirs: dict, root: Path) -> FlowConfig:
    """Merge `stack` over the defaults and the preset it names; `root` is the engine home that holds host presets."""
    for name, layer in stack:
        _reject_moved_keys(layer)
        _reject_cockpit_settings(layer)
        _reject_silent_layer(layer)
        _reject_personal_only(name, layer)
        _require_repository_keys(name, layer)
        if "preset" in layer:
            _preset(layer["preset"], root)
    preset = _preset(_preset_name(stack), root)
    base = _with_preset(preset)
    values, origins, relaxations = layers.merge(base, stack, partial(_relaxes, root), FlowConfigError, "flow config",
                                                tighten_only=REPOSITORY_LAYERS)
    _label_preset_origins(origins, preset)
    _apply_dial(values, origins)
    values["report"]["owner_dirs"] = owner_dirs
    _normalize(values)
    _validate(values)
    return FlowConfig(values, origins, relaxations, _old_day_pace_warnings(values))






def _old_day_pace_warnings(values: dict) -> list[str]:
    """One warning when a file or a preset still sets the retired daily pace to a share; it has no effect."""
    return [OLD_DAY_PACE_WARNING] if values["knobs"]["pace_day_pct"] else []


def _reject_moved_keys(layer: dict) -> None:
    """Refuse a key that moved, naming its new place (one name per meaning)."""
    for (table, key), message in MOVED_KEYS.items():
        if isinstance(layer.get(table), dict) and key in layer[table]:
            raise FlowConfigError(message)


def _preset_name(stack: list[tuple[str, dict]]) -> str:
    """The preset the highest layer names, else the default."""
    named = [layer["preset"] for _, layer in stack if "preset" in layer]
    return named[-1] if named else DEFAULTS["preset"]


def _preset(name: object, root: Path) -> presets.Preset:
    try:
        return presets.resolve(name, root)  # type: ignore[arg-type]
    except presets.PresetError as error:
        raise FlowConfigError(str(error)) from error


def _with_preset(preset: presets.Preset) -> dict:
    """The defaults with the preset's knobs and roles in place: the base every config file layers on."""
    base = copy.deepcopy(DEFAULTS)
    for key, value in layers.flatten(preset.layer()).items():
        layers.set_key(base, key, value)
    return base


def _label_preset_origins(origins: dict[str, str], preset: presets.Preset) -> None:
    """Name the preset behind its knobs, and behind each role it moves off the default.

    A role the preset leaves at its default stays `defaults`, so the budget router may still place it.
    """
    defaults = layers.flatten(DEFAULTS)
    for key, value in layers.flatten(preset.layer()).items():
        if origins[key] == "defaults" and (key.startswith("knobs.") or value != defaults[key]):
            origins[key] = f"{PRESET_ORIGIN}{preset.name}"


def _apply_dial(values: dict, origins: dict[str, str]) -> None:
    """An explicit dial sets `builders` and `fix_rounds`, unless a config layer set that knob itself."""
    if origins.get("dial", "defaults") == "defaults":
        return
    step = DIAL.get(values["dial"])
    for knob, field_name in DIAL_KNOBS.items():
        if step and _from_preset(origins[f"knobs.{knob}"]):
            values["knobs"][knob] = getattr(step, field_name)
            origins[f"knobs.{knob}"] = f"dial:{values['dial']}"


def _from_preset(origin: str) -> bool:
    return origin == "defaults" or origin.startswith(PRESET_ORIGIN)


def steps(values: dict) -> PresetSteps:
    """Return the steps the knobs turn on, with a step off when its role is `none`.

    A bare `values` without knobs (a caller holding only a preset name) reads the built-in preset's steps.
    """
    knobs = values.get("knobs")
    preset = _steps_of_knobs(knobs) if knobs else PRESETS[values["preset"]]
    roles = values["roles"]
    return PresetSteps(
        preset.plan_review and roles["plan_reviewer"] != NO_ROLE,
        preset.code_review and roles["code_reviewer"] != NO_ROLE,
        preset.evaluate and roles["evaluator"] != NO_ROLE,
        preset.final_review and roles["code_reviewer"] != NO_ROLE,
    )


def knobs(values: dict) -> dict:
    """The effective knobs of a loaded config."""
    return values["knobs"]


def _read_project(repo: Path, base: Path, *, folder_missing: bool) -> dict:
    """The per-project layer under engine folder `base`, with the commands the person remembered (`checks.remembered`).

    A folder outside Git has no project and so no such layer.
    """
    try:
        project = engine_home.project_id_for_path(repo) if folder_missing else engine_home.project_id(repo)
    except repository.RepositoryError:
        return {}
    layer = _read(base / project / engine_home.CONFIG_NAME)
    remembered = _read(base / project / engine_home.REMEMBERED_CHECKS_NAME).get("checks", {})
    return {**layer, "checks": {**layer.get("checks", {}), **remembered}} if remembered else layer


def _read(path: Path) -> dict:
    try:
        info = path.stat()
    except (FileNotFoundError, NotADirectoryError):
        return {}
    except OSError:
        if not path.exists():
            return {}
        raise
    try:
        return file_cache.load_toml(path, info)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise FlowConfigError(f"cannot read {path}: {error}") from error


def _as_list(value: object) -> list:
    return value if isinstance(value, list) else [value]


def _relaxes(root: Path, key: str, old: object, new: object) -> bool:
    if key == "preset":
        return _preset_weakens(root, old, new)
    if key in WEAKENABLE_KNOBS:
        return old is True and new is False
    if key in SPENDING_KNOBS:
        return _numbers(old, new) and new > old
    if key == PACE_KNOB:
        return _numbers(old, new) and old > 0 and (new == 0 or new > old)
    if key.startswith("roles."):
        return key.split(".")[1] in WEAKENABLE_ROLES and new == NO_ROLE and old != NO_ROLE
    if key == "approve_spec":
        return old is True and new is False
    if key.startswith(("tests.", "brakes.")):
        return not set(_as_list(old)).issubset(set(_as_list(new)))
    if key in BUDGET_FLOORS or key in BUDGET_CEILINGS:
        return _budget_weakens(key in BUDGET_FLOORS, old, new)
    return False






def _budget_weakens(is_floor: bool, old: object, new: object) -> bool:
    """A floor weakens when lowered, a ceiling when raised; a non-number is left to validation."""
    if any(isinstance(value, bool) or not isinstance(value, (int, float)) for value in (old, new)):
        return False
    return new < old if is_floor else new > old


def _preset_weakens(root: Path, old: object, new: object) -> bool:
    """Moving from preset `old` to `new` loosens a gate or spends more; a name that does not resolve counts as both.

    Every knob and role `new` changes goes through the same checks as a config key, and a role moved to a stronger
    model (or one that cannot be ranked) or another cage policy count too.
    """
    try:
        before = layers.flatten(presets.resolve(old, root).layer())  # type: ignore[arg-type]
        after = layers.flatten(presets.resolve(new, root).layer())  # type: ignore[arg-type]
    except presets.PresetError:
        return True
    return any(_preset_change_weakens(root, key, before[key], value) for key, value in after.items()
               if value != before[key])


def _preset_change_weakens(root: Path, key: str, old: object, new: object) -> bool:
    if key == CAGE_KNOB:
        return True
    if key.startswith("roles.") and _model_rank(new) > _model_rank(old):
        return True
    return _relaxes(root, key, old, new)


def _model_rank(spec: object) -> int:
    """The tier of a role's model: haiku, sonnet, opus; `none` ranks lowest and anything unknown highest."""
    text = str(spec).strip().lower()
    if text == NO_ROLE:
        return -1
    provider, _, model = text.partition(":")
    tiers = [index for index, tier in enumerate(MODEL_TIERS) if provider == OPUS_PROVIDER and tier in model]
    return tiers[0] if tiers else len(MODEL_TIERS)


def _numbers(*values: object) -> bool:
    return all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in values)


def _normalize(values: dict) -> None:
    for key, value in values["tests"].items():
        values["tests"][key] = _as_list(value)
    values["load"]["critical"] = _as_list(values["load"]["critical"])


def roles_provider(cfg: "FlowConfig", role: str) -> str:
    """The program a role runs on (`claude`, `codex`, `exec`, `none`)."""
    return str(cfg.values["roles"][role]).partition(":")[0]


def providers_for(role: str) -> tuple[str, ...]:
    """The providers a role may run on: every one, for every role (the person chooses per step)."""
    return ROLE_PROVIDERS


def _validate(values: dict) -> None:
    _require_choice("dial", values["dial"], tuple(DIAL))
    _validate_knobs(values["knobs"])
    _validate_step_accounts(values["step_accounts"])
    for key in ("up", "down"):
        if not isinstance(values["stack"][key], str):
            raise FlowConfigError(f"stack.{key} must be a command string")
    for key in ("approved", "remembered"):
        commands = values["checks"][key]
        if not isinstance(commands, list) or not all(isinstance(item, str) and item.split() for item in commands):
            raise FlowConfigError(f"checks.{key} must be a list of commands")
    _validate_loopback_ports(values["checks"]["loopback_ports"])
    if type(values["checks"]["local_sockets"]) is not bool:
        raise FlowConfigError("checks.local_sockets must be true or false")
    _reject_control_characters(values)
    _require_choice("familiarity", values["familiarity"], FAMILIARITY_LEVELS)
    for role, spec in values["roles"].items():
        _require_choice(f"roles.{role}", str(spec).split(":", 1)[0], providers_for(role))
    for role, effort in values["efforts"].items():
        if not isinstance(effort, str) or (effort and not EFFORT.fullmatch(effort)):
            raise FlowConfigError(f"efforts.{role} must be an effort level such as low or high, or empty; got {effort!r}")
    _validate_surface(values["surface"])
    _validate_report(values["report"])
    _validate_knowledge(values["knowledge"])
    _validate_ntfy_url(values["notify"]["ntfy_url"])
    _validate_gates(values["gates"])
    _validate_app_origin(values["app"]["origin"])
    if values["app"]["origin"] and values["builders"]["max"] != 1:
        raise FlowConfigError(
            "app.origin needs builders.max = 1: every workspace would need the same port block")
    _validate_required_contract_ids(values["contract"]["required_ids"])
    try:
        flow_paths.require_rooted(values["contract"]["flow_paths"])
    except flow_paths.FlowPathError as error:
        raise FlowConfigError(str(error)) from error
    _validate_budget(values)
    load = values["load"]
    for key in ("slots", "idle_stack_seconds", "nice"):
        if type(load[key]) is not int:
            raise FlowConfigError(f"load.{key} must be an integer")
    if load["slots"] < 0 or load["idle_stack_seconds"] < 0 or not 0 <= load["nice"] <= 19:
        raise FlowConfigError("load.slots and load.idle_stack_seconds must be non-negative; load.nice must be 0..19")
    if any(not isinstance(task, str) for task in load["critical"]):
        raise FlowConfigError("load.critical must be a list of task ids")
    _reject_silent_layer(values)


def _reject_control_characters(values: dict) -> None:
    """A command the engine runs or shows the person holds no control character (tab and newline included), so what
    the person reads before an approval or an unsandboxed run is what runs."""
    flat = layers.flatten(values)
    for key in sorted(flat):
        if key.startswith(COMMAND_KEY_PREFIXES) or key.startswith(HOST_COMMAND_PREFIXES):
            for command in _as_list(flat[key]):
                if isinstance(command, str) and any(controls.is_control(character) for character in command):
                    raise FlowConfigError(f"{key} holds a control character: {controls.escaped(command)}")


def _validate_loopback_ports(ports: object) -> None:
    """`checks.loopback_ports`: the dependent services (a database, a cache) a caged check may reach on loopback."""
    if not isinstance(ports, list) or not all(type(port) is int and 1 <= port <= 65535 for port in ports):
        raise FlowConfigError("checks.loopback_ports must be a list of port numbers from 1 to 65535")


def _validate_step_accounts(accounts: dict) -> None:
    for step, account in accounts.items():
        if not isinstance(account, str) or not (account == presets.PROJECT_ACCOUNT or ACCOUNT_ID.fullmatch(account)):
            raise FlowConfigError(f"step_accounts.{step} must be an account id or {presets.PROJECT_ACCOUNT}; got {account!r}")


def _validate_knobs(knobs: dict) -> None:
    for knob, (low, high) in presets.KNOB_LIMITS.items():
        value = knobs[knob]
        whole = knob != "pace_day_pct"
        if isinstance(value, bool) or not isinstance(value, int if whole else (int, float)) or not low <= value <= high:
            raise FlowConfigError(f"knobs.{knob} must be a {'whole ' if whole else ''}number from {low} to {high}; got {value!r}")
    for knob in ("reviewer", "evaluator", "reporter", *presets.STEP_KNOBS):
        if not isinstance(knobs[knob], bool):
            raise FlowConfigError(f"knobs.{knob} must be true or false")
    _require_choice("knobs.pace", knobs["pace"], pace.LEVELS)
    _require_choice("knobs.lane_pace", knobs["lane_pace"], (*pace.LANE_LEVELS, ""))  # "": the engine's pace
    if knobs["lanes_min"] > knobs["lanes"]:
        raise FlowConfigError(f"knobs.lanes_min ({knobs['lanes_min']}) cannot be above knobs.lanes ({knobs['lanes']})")
    _require_choice("knobs.report_language", knobs["report_language"], REPORT_LANGUAGES)
    if not isinstance(knobs["cage"], str) or not presets.NAME.fullmatch(knobs["cage"]):
        raise FlowConfigError(f"knobs.cage must name a cage policy matching {presets.NAME.pattern}")






def _require_repository_keys(layer_name: str, layer: dict) -> None:
    """A repository layer declares project facts only (REPOSITORY_KEYS); any other key is the host's and is refused,
    named, so a repository's own files never touch a gate, a role, a budget or the cage, even through a side key."""
    if layer_name not in REPOSITORY_LAYERS:
        return
    refused = sorted(key for key in layers.flatten(layer) if key != layers.RELAX_KEY and key not in REPOSITORY_KEYS)
    if refused:
        raise FlowConfigError(f"{refused[0]} may only be set by the host (personal, project, preset or flags), not in "
                              f"the {layer_name} layer; a repository declares only {', '.join(sorted(REPOSITORY_KEYS))}")


def _reject_silent_layer(layer: dict) -> None:
    for key, commands in layers.flatten(layer).items():
        if key.startswith(COMMAND_KEY_PREFIXES):
            _reject_silent_checks(key, commands)


def _reject_cockpit_settings(layer: dict) -> None:
    """Cockpit trust settings pick who may control the engine, so no flow layer may carry them."""
    if COCKPIT_TABLE in layer or any(key.startswith(f"{COCKPIT_TABLE}.") for key in layers.flatten(layer)):
        raise FlowConfigError("cockpit settings belong in ~/.config/sakura/cockpit.toml only")


def _reject_personal_only(layer_name: str, layer: dict) -> None:
    """Notify, gate and report-directory settings pick a destination or silence pages, so only the personal file may set them.

    The flag layer is the operator's own command line, so it may still pass `report.owner_dir` and `report.status_dir`.
    """
    if layer_name == NOTIFY_LAYER:
        return
    for key in layers.flatten(layer):
        if _is_personal_only(layer_name, key):
            raise FlowConfigError(
                f"{key} may only be set in the personal layer ({NOTIFY_FILE}), not in the {layer_name} layer")


def _is_personal_only(layer_name: str, key: str) -> bool:
    if key.startswith((NOTIFY_PREFIX, GATES_PREFIX, f"{OWNER_DIRS_KEY}.")):
        return True
    return key in (OWNER_DIR_KEY, STATUS_DIR_KEY, KNOWLEDGE_SHARED_KEY) and layer_name != FLAGS_LAYER


def _take_owner_dirs(personal: dict) -> dict:
    """Remove and return the personal `[report.owner_dirs]` table: its keys are repository names, unknown to the layer merge."""
    report = personal.get("report")
    taken = report.pop("owner_dirs", {}) if isinstance(report, dict) else {}
    return taken if isinstance(taken, dict) else {"": taken}


def _validate_surface(surface: dict) -> None:
    if type(surface["tmux"]) is not bool:
        raise FlowConfigError("surface.tmux must be true or false")
    for key in ("session_prefix", "cockpit_session"):
        if not isinstance(surface[key], str) or not SURFACE_NAME.fullmatch(surface[key]):
            raise FlowConfigError(f"surface.{key} must be 1 to 32 characters of A-Z a-z 0-9 . _ -; got {surface[key]!r}")


def _validate_report(report: dict) -> None:
    owner_dir = report["owner_dir"]
    if not isinstance(owner_dir, str) or not _is_absolute(owner_dir):
        raise FlowConfigError(f"report.owner_dir must be an absolute path (a leading ~ is allowed); got {owner_dir!r}")
    status_dir = report["status_dir"]
    if not isinstance(status_dir, str) or not _is_absolute(status_dir.replace(OWNER_DIR_PLACEHOLDER, owner_dir)):
        raise FlowConfigError(f"report.status_dir must be an absolute path (a leading ~ is allowed); got {status_dir!r}")
    if not isinstance(report["project"], str):
        raise FlowConfigError("report.project must be a string")
    for name, directory in report["owner_dirs"].items():
        if not name or not isinstance(directory, str) or not _is_absolute(directory):
            raise FlowConfigError(f"report.owner_dirs.{name} must be an absolute path (a leading ~ is allowed); got {directory!r}")


def _validate_knowledge(knowledge: dict) -> None:
    for name, folder in knowledge.items():
        if not isinstance(folder, str) or (folder and not _is_absolute(folder)):
            raise FlowConfigError(f"knowledge.{name} must be an absolute path (a leading ~ is allowed) or empty; got {folder!r}")


def _is_absolute(template: str) -> bool:
    """Whether a directory setting is absolute once `~` and `{engine}` are expanded."""
    return os.path.isabs(os.path.expanduser(template.replace(engine_home.ENGINE_PLACEHOLDER, "/engine")))


def _validate_ntfy_url(url: object) -> None:
    parsed = urlparse(str(url))
    if parsed.scheme == "https" and parsed.hostname:
        return
    if parsed.scheme == "http" and parsed.hostname in LOOPBACK_HOSTS:
        return
    raise FlowConfigError(f"notify.ntfy_url must be https://, or http:// for localhost only; got {url!r}")


def _validate_gates(gates: dict) -> None:
    _require_choice("gates.mode", gates["mode"], GATE_MODES)
    window = DAY_WINDOW.fullmatch(gates["day"]) if isinstance(gates["day"], str) else None
    if window is None or window["start"] == window["end"]:
        raise FlowConfigError(f"gates.day must be HH:MM-HH:MM with different times; got {gates['day']!r}")


def _validate_budget(values: dict) -> None:
    try:
        budget_settings.load(values)
        budget_routing.load_routes(values)
    except (budget_settings.BudgetSettingsError, budget_routing.RoutingError) as error:
        raise FlowConfigError(str(error)) from error


def _validate_app_origin(url: object) -> None:
    if url == "":
        return
    try:
        parsed = urlparse(str(url))
        port = parsed.port
    except ValueError:
        port = None
        parsed = urlparse("")
    if (parsed.scheme != "http" or parsed.hostname not in LOOPBACK_HOSTS or port is None or not 1 <= port <= 65535
            or parsed.path or parsed.query or parsed.fragment or parsed.username or parsed.password):
        raise FlowConfigError("app.origin must be an explicit http loopback origin without a path")


def _validate_required_contract_ids(ids: object) -> None:
    if (not isinstance(ids, list) or any(not isinstance(item, str) or not item for item in ids)
            or len(ids) != len(set(ids))):
        raise FlowConfigError("contract.required_ids must be a list of unique non-empty strings")


def _require_choice(key: str, value: object, allowed: tuple) -> None:
    if value not in allowed:
        raise FlowConfigError(f"{key} must be one of {', '.join(allowed)}; got {value!r}")


def _reject_silent_checks(key: str, commands: object) -> None:
    for command in _as_list(commands):
        if "&&" in str(command) and SILENT_CHECK_PATTERN.search(str(command)):
            raise FlowConfigError(f"{key} has a silent check (output hidden before &&): {command}")
