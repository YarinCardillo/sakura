"""What a set of build tasks is expected to cost, from the usage archive: the shared core of the plan-phase
gate and of `sakura usage estimate --plan`."""

from __future__ import annotations

import sqlite3
from collections.abc import Callable, Mapping, Sequence
from dataclasses import replace
from datetime import datetime

from sakura.budget import account as budget_account
from sakura.budget import history, predict, preflight, routing
from sakura.budget.settings import BudgetSettings, for_account
from sakura.flow import config, registry, roles
from sakura.flow.plan import BuildTask
from sakura.usage.archive import settings as archive_settings
from sakura.usage.archive import store as archive_store

OPUS_CLASS = "opus"
NO_ARCHIVE = "no usage archive configured"
ARCHIVE_ERRORS = (OSError, sqlite3.Error, archive_settings.UsageConfigError, registry.RegistryError)


def for_tasks(cfg: config.FlowConfig, tasks: Sequence[BuildTask], settings: BudgetSettings, now: datetime,
              usage: archive_settings.Settings | None = None) -> preflight.Preflight:
    """The pre-flight of a whole flow of `tasks` under the flow config `cfg`.

    `usage` defaults to the host usage config. An empty `budget.account` is resolved from the engine's Claude
    config directory; an unresolvable account or an unreadable archive gives `preflight.unavailable`.
    """
    routes = {task.id: routing.route(routing.BUILDER, task.task_class, 0, cfg) for task in tasks}
    classes = _role_classes(cfg)
    return _estimate(settings, now, usage, lambda samples: predict.flow(tasks, routes, classes, samples))


def for_opus_run(task: BuildTask, settings: BudgetSettings, now: datetime,
                 usage: archive_settings.Settings | None = None) -> preflight.Preflight:
    """The pre-flight of one Opus run of `task`: what an escalation to Opus adds."""
    return _estimate(settings, now, usage, lambda samples: _opus_prediction(task, samples))


def _estimate(settings: BudgetSettings, now: datetime, usage: archive_settings.Settings | None,
              predict_from: Callable[[Sequence[history.Sample]], predict.FlowPrediction]) -> preflight.Preflight:
    if usage is None and not archive_settings.config_exists():
        return _unconfigured(settings)
    try:
        usage = usage or archive_settings.load()
        account = budget_account.resolve(settings.account, usage.sources, disabled=usage.disabled)
        named = for_account(replace(settings, account=account), usage.reserves.get(account))
        conn = archive_store.open_db(usage.db)
        try:
            prediction = predict_from(history.samples(conn, registry.repos()))
            return preflight.estimate(prediction, named.account, now, conn, named, usage)
        finally:
            conn.close()
    except budget_account.UnknownAccountError as error:
        return preflight.unavailable(f"quota unknown: {error}")
    except ARCHIVE_ERRORS as error:
        return preflight.unavailable(f"quota unknown: usage archive unreadable: {type(error).__name__}: {error}")


def _unconfigured(settings: BudgetSettings) -> preflight.Preflight:
    """No usage.toml: the budget is off, unless `budget.account` is set, which is a misconfiguration."""
    if settings.account:
        return preflight.unavailable(f"quota unknown: budget.account is {settings.account!r} but the usage archive "
                                     f"is not configured ({archive_settings.DEFAULT_CONFIG} is missing)")
    return preflight.off(NO_ARCHIVE)


def _opus_prediction(task: BuildTask, samples: Sequence[history.Sample]) -> predict.FlowPrediction:
    found = predict.task(predict.TaskShape(task.task_class, len(task.files), len(task.rows)), OPUS_CLASS, samples)
    by_class = {OPUS_CLASS: predict.Band(found.weighted, found.low, found.high)} if found.n else {}
    return predict.FlowPrediction(by_class, {task.id: found}, (), found.basis)


def _role_classes(cfg: config.FlowConfig) -> Mapping[str, str]:
    """The model class each role runs on; a role that is not Claude is `other` (it draws on no Claude quota)."""
    dial_model = config.DIAL[cfg.values["dial"]].claude_model
    classes: dict[str, str] = {}
    for role, value in cfg.values["roles"].items():
        spec = roles.parse_role(value)
        model = routing.route(role, None, 0, cfg).model or spec.model or dial_model
        classes[role] = history.class_of(model) if spec.provider == "claude" else history.OTHER
    return classes
