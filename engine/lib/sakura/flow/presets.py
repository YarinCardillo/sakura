"""Named presets: the shape of a flow (roles and models, builders, lanes, subagents, pace, reviewer, evaluator, reporter and its
language, cage).

A preset is a bundle of independent knobs. The built-ins live here; the person adds their own as
`<engine home>/presets/<name>.toml`, starting from a `base` preset (default `standard`) and changing only
the knobs it names. `sakura preset use <name>` records the host's choice in `<engine home>/chosen-preset`.

The config layers (`flow/config.py`) put the chosen preset under every config file, so a file or a flag can
still set a single knob or role.
"""

from __future__ import annotations

import os
import re
import tomllib
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path

from sakura import engine_home, file_cache

PRESETS_DIR = "presets"
CHOICE_FILE = "chosen-preset"
PRESET_SUFFIX = ".toml"
NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,31}")
DEFAULT_BASE = "standard"
# Which flow roles each model tier sets.
TIERS = {"lead": ("planner", "evaluator"), "worker": ("builder", "reporter"),
         "review": ("plan_reviewer", "code_reviewer")}
KNOB_LIMITS = {"builders": (1, 6), "lanes": (1, 6), "lanes_min": (1, 6), "subagents": (0, 8), "fix_rounds": (0, 6),
               "final_review_rounds": (0, 6), "pace_day_pct": (0, 100)}
# The steps of a lane and the reviews that are loops on them (`flow/pipeline.py`); a preset turns each on or off.
STEP_KNOBS = ("plan", "plan_review", "task_review", "final_review", "checks")
# The steps and reviews that can run on an account of their own, and the value that means the project's account.
ACCOUNT_STEPS = ("plan", "plan_review", "build", "task_review", "final_review", "checks")
PROJECT_ACCOUNT = "project"
FILE_KEYS = frozenset({"base", "description", "models", "builders", "lanes", "lanes_min", "subagents", "pace_day_pct",
                       "reviewer", "fix_rounds", "final_review_rounds", "evaluator", "reporter", "report_language", "cage",
                       "accounts", *STEP_KNOBS})
# The owner report's templates (`flow/owner_report.py`), by language.
REPORT_LANGUAGES = ("it", "en")
LEGACY = ("vanilla",)


class PresetError(ValueError):
    """Raised when a preset is unknown or its file is malformed."""


@dataclass(frozen=True)
class Preset:
    """One preset's knobs. `pace_day_pct` 0 means no daily pace."""

    name: str
    description: str
    models: dict[str, str] = field(default_factory=dict)
    # Parallel builders inside one lane (flow task); `lanes` is how many lanes the master may have open.
    builders: int = 3
    lanes: int = 3
    subagents: int = 2
    pace_day_pct: float = 0
    reviewer: bool = True
    fix_rounds: int = 2
    evaluator: bool = True
    # The reporter role writes the readable report and the owner report; off, the task report is the engine's section.
    reporter: bool = True
    report_language: str = "it"
    cage: str = "default"
    # The least number of lanes the engine keeps running while work waits (`lanes` is the most).
    lanes_min: int = 1
    # The steps of a lane. `plan` off builds from the lane's goal; `plan_review`, `task_review` and `final_review`
    # are the reviews on those steps (they run only while `reviewer` is on); `checks` is the project's own tests.
    plan: bool = True
    plan_review: bool = True
    task_review: bool = True
    final_review: bool = False
    final_review_rounds: int = 1
    checks: bool = True
    # The account each step runs on: a Leeway account id, or `project` for the project's own.
    step_accounts: dict[str, str] = field(default_factory=dict)

    def knobs(self) -> dict:
        """The `knobs` table of the flow config this preset sets."""
        return {"builders": self.builders, "lanes": self.lanes, "lanes_min": self.lanes_min, "subagents": self.subagents,
                "pace_day_pct": self.pace_day_pct, "reviewer": self.reviewer, "fix_rounds": self.fix_rounds,
                "evaluator": self.evaluator, "reporter": self.reporter, "report_language": self.report_language,
                "cage": self.cage, "final_review_rounds": self.final_review_rounds,
                **{knob: getattr(self, knob) for knob in STEP_KNOBS}}

    def layer(self) -> dict:
        """The config layer of this preset: its knobs, the role of every tier and the account of every step."""
        roles = {role: self.models[tier] for tier, members in TIERS.items() for role in members}
        accounts = {step: self.step_accounts.get(step, PROJECT_ACCOUNT) for step in ACCOUNT_STEPS}
        return {"knobs": self.knobs(), "roles": roles, "step_accounts": accounts}


_TEAM = {"lead": "claude:opus", "worker": "claude:sonnet", "review": "claude:sonnet"}
BUILTINS = {
    "solo": Preset("solo", "One agent, no subagents, no reviewer or evaluator; contract and gates stay.",
                   {tier: "claude:sonnet" for tier in TIERS}, builders=1, lanes=1, subagents=0, reviewer=False, evaluator=False,
                   plan=False, checks=False),
    "standard": Preset("standard", "Planner, parallel builders and a blocking reviewer; no evaluator.", dict(_TEAM),
                       evaluator=False),
    "verified": Preset("verified", "Standard plus an evaluator that proves each contract row with evidence.",
                       dict(_TEAM)),
    "vanilla": Preset("vanilla", "Legacy name: the standard team with no reviewer and no evaluator.", dict(_TEAM),
                      reviewer=False, evaluator=False),
    "lean": Preset("lean", "Plan, build, one review of the whole branch, deliver: no plan or task reviews, no checks.",
                   dict(_TEAM), evaluator=False, plan_review=False, task_review=False, final_review=True, checks=False),
}


def names(root: Path | None = None) -> list[str]:
    """Built-in names, then the host's own presets in name order; `root` is the engine home (default: the current one)."""
    folder = _root(root) / PRESETS_DIR
    own = sorted(path.stem for path in folder.glob(f"*{PRESET_SUFFIX}")) if folder.is_dir() else []
    return [*BUILTINS, *(name for name in own if name not in BUILTINS)]


def resolve(name: str, root: Path | None = None) -> Preset:
    """Return preset `name`: a built-in, or `<engine home>/presets/<name>.toml` over its base."""
    return _resolve(name, _root(root), ())


def chosen(root: Path | None = None) -> str:
    """The preset `sakura preset use` recorded for this host, or an empty string."""
    path = _root(root) / CHOICE_FILE
    return path.read_text().strip() if path.is_file() else ""


def choose(name: str, root: Path | None = None) -> Preset:
    """Record `name` as the host's preset after it resolves; returns it."""
    root = _root(root)
    preset = resolve(name, root)
    engine_home.ensure_private(root)
    temporary = root / f".{CHOICE_FILE}.{os.getpid()}"
    temporary.write_text(name + "\n")
    temporary.replace(root / CHOICE_FILE)
    return preset


def weakens(old: Preset, new: Preset) -> bool:
    """True when `new` turns off a review, the evaluator, the reporter, the plan or the checks that `old` runs."""
    return any(getattr(old, knob) and not getattr(new, knob)
               for knob in ("reviewer", "evaluator", "reporter", *STEP_KNOBS))


def listing(chosen: str, root: Path | None = None) -> list[dict]:
    """Every preset as `{name, label, description, chosen, legacy, usable}`; `chosen` is the name in force, empty for none.

    The description of a preset that cannot be used is the reason, which can name a file: a reader that sends the list to
    another machine replaces it (`usable` is false for it)."""
    rows = []
    for name in names(root):
        description, is_usable = _described(name, root)
        rows.append({"name": name, "label": label(name), "description": description, "chosen": name == chosen,
                     "legacy": name in LEGACY, "usable": is_usable})
    return rows


def describe(preset: Preset) -> dict:
    """Plain data for `sakura preset show`."""
    return {**asdict(preset), "label": label(preset.name)}


def label(name: str) -> str:
    """The name of a preset as a person reads it: `lean` is `Lean`, `night-shift` is `Night shift`."""
    return name.replace("-", " ").replace("_", " ").capitalize()


def _described(name: str, root: Path | None) -> tuple[str, bool]:
    """The description of preset `name` and whether it resolves, from one read (so the two never disagree)."""
    try:
        return resolve(name, root).description, True
    except PresetError as error:
        return f"unusable: {error}", False


def _root(root: Path | None) -> Path:
    return root if root is not None else engine_home.root()


def _resolve(name: str, root: Path, seen: tuple[str, ...]) -> Preset:
    if not isinstance(name, str) or not NAME.fullmatch(name):
        raise PresetError(f"preset name must match {NAME.pattern}; got {name!r}")
    path = root / PRESETS_DIR / f"{name}{PRESET_SUFFIX}"
    if name in BUILTINS:
        if path.exists():
            raise PresetError(f"{path} has the name of a built-in preset; rename it and set base = \"{name}\"")
        return BUILTINS[name]
    if name in seen:
        raise PresetError(f"preset {name} is its own base: {' -> '.join((*seen, name))}")
    data = _read(path, name)
    base = data.get("base", DEFAULT_BASE)
    return _apply(path, replace(_resolve(base, root, (*seen, name)), name=name), data)


def _read(path: Path, name: str) -> dict:
    if not path.is_file():
        known = ", ".join(BUILTINS)
        raise PresetError(f"unknown preset {name}: built-ins are {known}; add your own as {path}")
    try:
        data = file_cache.load_toml(path)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise PresetError(f"cannot read {path}: {error}") from error
    unknown = sorted(set(data) - FILE_KEYS)
    if unknown:
        raise PresetError(f"{path}: unknown keys {', '.join(unknown)}")
    return data


def _apply(path: Path, preset: Preset, data: dict) -> Preset:
    changes: dict = {key: data[key] for key in ("description", "builders", "lanes", "lanes_min", "subagents", "pace_day_pct",
                                                "reviewer", "fix_rounds", "final_review_rounds", "evaluator", "reporter",
                                                "report_language", "cage", *STEP_KNOBS)
                     if key in data}
    if "models" in data:
        changes["models"] = {**preset.models, **_models(path, data["models"])}
    if "accounts" in data:
        changes["step_accounts"] = {**preset.step_accounts, **_accounts(path, data["accounts"])}
    updated = replace(preset, **changes)
    _validate(path, updated)
    return updated


def _models(path: Path, models: object) -> dict[str, str]:
    if not isinstance(models, dict) or set(models) - set(TIERS):
        raise PresetError(f"{path}: models takes the tiers {', '.join(TIERS)}")
    if any(not isinstance(value, str) or not value for value in models.values()):
        raise PresetError(f"{path}: every model is a role value such as claude:sonnet or codex")
    return models


def _accounts(path: Path, accounts: object) -> dict[str, str]:
    if not isinstance(accounts, dict) or set(accounts) - set(ACCOUNT_STEPS):
        raise PresetError(f"{path}: accounts takes the steps {', '.join(ACCOUNT_STEPS)}")
    if any(not isinstance(value, str) or not value for value in accounts.values()):
        raise PresetError(f"{path}: every account is an account id, or {PROJECT_ACCOUNT}")
    return accounts


def _validate(path: Path, preset: Preset) -> None:
    for knob, (low, high) in KNOB_LIMITS.items():
        value = getattr(preset, knob)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
            raise PresetError(f"{path}: {knob} must be a number from {low} to {high}; got {value!r}")
        if knob != "pace_day_pct" and not isinstance(value, int):
            raise PresetError(f"{path}: {knob} must be a whole number; got {value!r}")
    if preset.lanes_min > preset.lanes:
        raise PresetError(f"{path}: lanes_min ({preset.lanes_min}) cannot be above lanes ({preset.lanes})")
    for knob in ("reviewer", "evaluator", "reporter", *STEP_KNOBS):
        if not isinstance(getattr(preset, knob), bool):
            raise PresetError(f"{path}: {knob} must be true or false")
    if preset.report_language not in REPORT_LANGUAGES:
        raise PresetError(f"{path}: report_language must be one of {', '.join(REPORT_LANGUAGES)}; "
                          f"got {preset.report_language!r}")
    for knob in ("description", "cage"):
        if not isinstance(getattr(preset, knob), str):
            raise PresetError(f"{path}: {knob} must be a string")
    if not NAME.fullmatch(preset.cage):
        raise PresetError(f"{path}: cage must name a cage policy matching {NAME.pattern}")
