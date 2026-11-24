"""Where the quota of one weekly window went: by project, role and model class, the share used outside Claude
Code, and the three most expensive tasks with what the same work would have cost one model class cheaper.

A weekly window is the account's own quota window: its reset time (`seven_day_resets_at` of the snapshots) minus
seven days up to the reset. Turns are Claude turns of the account inside it, in weighted tokens. Percentages
come from the calibrated ratios of the plan in force at the end of the window (`calibration.estimate`), and each
figure carries its basis and, where the calibration has a spread, a band.
"""

from __future__ import annotations

import sqlite3
from collections import defaultdict
from collections.abc import Iterable, Mapping
from dataclasses import asdict, dataclass
from datetime import datetime, timedelta, timezone, tzinfo
from pathlib import Path

from sakura.accounts import labels
from sakura.accounts.registry import Registry
from sakura.budget import history, preflight, routing, weekly_render
from sakura.budget.predict import Band as TokenBand
from sakura.budget.weekly_window import Window, pick_window, week_label
from sakura.flow import owner_report
from sakura.lanes import config as lane_config
from sakura.usage.archive import calibration
from sakura.usage.archive import settings as archive_settings
from sakura.usage.archive.settings import Settings

INTERACTIVE, LANE, UNKNOWN, FLOW, PULL_REQUEST = "interactive", "lane", "unknown", "flow", "pr"
TOP_TASKS = 3
RESOLUTION_PCT = 1.0
NO_RISE = "no attributed rise in the snapshots"
OUTSIDE_BASIS = "calibrated token ratios of the plan, whole UTC days"
IN_WINDOW = "datetime(t.ts) > datetime(:start) AND datetime(t.ts) <= datetime(:end)"
TURNS = f"""
SELECT t.session_id, t.cwd, c.repo, t.model, t.input_tokens, t.cache_write_tokens, t.cache_read_tokens,
       t.output_tokens FROM turns t LEFT JOIN cwds c ON c.cwd = t.cwd
WHERE t.provider = 'claude' AND t.account = :account AND {IN_WINDOW}
"""
PR_TURNS = f"""
SELECT t.repo, t.number, t.session_id, t.model, t.input_tokens, t.cache_write_tokens, t.cache_read_tokens,
       t.output_tokens FROM pr_turns t
WHERE t.provider = 'claude' AND t.account = :account AND {IN_WINDOW}
"""

Mix = dict[str, float]


@dataclass(frozen=True)
class Figure:
    """Weighted tokens and their share of the week. `pct`, `low` and `high` are None when no calibration applies."""

    tokens: float
    low: float | None
    pct: float | None
    high: float | None
    basis: str


@dataclass(frozen=True)
class Row:
    name: str
    figure: Figure


@dataclass(frozen=True)
class Cheaper:
    """The same weighted tokens priced one model class down. An upper bound of the saving: a cheaper model may
    need more tokens for the same work. `steps` are (from, to) classes; the percentages are None when the
    classes involved have no calibration.
    """

    steps: tuple[tuple[str, str], ...]
    pct_now: float | None
    pct_cheaper: float | None
    saving_pct: float | None
    basis: str
    now_band: tuple[float, float] | None = None
    cheaper_band: tuple[float, float] | None = None
    saving_band: tuple[float, float] | None = None


@dataclass(frozen=True)
class TopTask:
    """A flow task (`repo/task`) or a pull request (`owner/repo#n`) and what it spent."""

    kind: str
    name: str
    figure: Figure
    cheaper: Cheaper | None


@dataclass(frozen=True)
class Outside:
    """The weekly rise the snapshots show, and the part local turns do not explain (None without calibration)."""

    rise: float
    pct: float | None
    low: float | None
    high: float | None
    share: float | None
    unattributed: float
    basis: str


@dataclass(frozen=True)
class Weekly:
    account: str
    plan: str
    start: datetime
    end: datetime
    total: Figure
    by_project: list[Row]
    by_role: list[Row]
    by_class: list[Row]
    outside: Outside
    top_tasks: list[TopTask]


@dataclass(frozen=True)
class _Mixes:
    project: dict[str, Mix]
    role: dict[str, Mix]
    model: dict[str, Mix]
    task: dict[tuple[str, str], Mix]


@dataclass(frozen=True)
class _Converter:
    """Weighted tokens to percent of the week under one account's calibration, plan taken at `at`."""

    ratios: list[calibration.Ratio]
    usage: Settings
    account: str
    at: datetime

    def figure(self, mix: Mix) -> Figure:
        total = sum(mix.values())
        tokens = {name: TokenBand(weight, weight, weight) for name, weight in mix.items() if name != calibration.OTHER}
        if not tokens:
            return Figure(total, None, None, None, preflight.no_calibration(self.usage, self.account, self.at))
        band, basis = preflight.percent_band(tokens, self.ratios, self.usage, self.account, self.at)
        if band is None:
            return Figure(total, None, None, None, basis)
        return Figure(total, band.low, band.mid, band.high, basis)


def build(conn: sqlite3.Connection, settings: Settings, week_start: datetime, week_end: datetime, *, account: str,
          flow_repos: Iterable[Path] = (), lane_roots: Iterable[Path] = ()) -> Weekly:
    """The report of `account` for the turns with `week_start < ts <= week_end`.

    `flow_repos` are the repositories whose flow runs name the roles (the caller passes `registry.repos()`);
    `lane_roots` are the lane worktree roots, a turn under one is a `lane` turn.
    """
    at = week_end - timedelta(seconds=1)
    convert = _Converter(calibration.compute(conn, settings.plans), settings, account, at)
    sessions = {run.session_id: run for run in history.flow_runs(flow_repos)}
    mixes = _group(conn, account, Window(week_start, week_end), sessions, _lane_paths(lane_roots))
    period = archive_settings.period_at(settings.plans, account, at)
    return Weekly(
        account, period.plan if period else calibration.UNKNOWN_PLAN, week_start, week_end,
        convert.figure(_sum(mixes.model.values())), _rows(convert, mixes.project), _rows(convert, mixes.role),
        _rows(convert, mixes.model), _outside(conn, settings, account, Window(week_start, week_end)),
        _top_tasks(convert, mixes.task))


def to_json(report: Weekly) -> dict:
    """The report as plain data; times are ISO strings."""
    return _json_safe(asdict(report))


def _json_safe(value):
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, dict):
        return {key: _json_safe(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_safe(item) for item in value]
    return value


def lane_roots(path: Path | str | None = None) -> tuple[Path, ...]:
    """The worktree roots of the configured lane groups, none when there is no lane config.

    A lane config that exists but cannot be read raises LaneConfigError: a lane would be counted as interactive.
    """
    source = Path(path or lane_config.DEFAULT_PATH).expanduser()
    if not source.is_file():
        return ()
    return tuple(group.worktree_root for group in lane_config.load(source).values())


def write_pages(conn: sqlite3.Connection, settings: Settings, now: datetime, values: dict, *,
                flow_repos: Iterable[Path] = (), lane_roots: Iterable[Path] = (),
                tz: tzinfo | None = None, registry: Registry = labels.NO_ACCOUNTS) -> list[str]:
    """Write the page of the last completed window of each account, once: a page that exists is left alone.

    The page goes to the personal `report.owner_dir` resolved for the repository `sakura`. A window without
    turns writes nothing. Returns one line per page written. Raises ValueError for an account name that cannot
    be a file name. The page shows each account by its label from `registry`.
    """
    repos, roots = list(flow_repos), tuple(lane_roots)
    written: list[str] = []
    for account in _accounts(conn):
        window = pick_window(conn, account, None, now, tz)
        if window is None:
            continue
        year, number = week_label(window, tz)
        target = _page_path(values, account, year, number)
        if target.exists():
            continue
        report = build(conn, settings, window.start, window.end, account=account, flow_repos=repos, lane_roots=roots)
        if report.total.tokens:
            owner_report.write(target, weekly_render.page(report, number, registry))
            written.append(f"weekly report {account}: {target.name}")
    return written


def _page_path(values: dict, account: str, year: int, number: int) -> Path:
    if not owner_report.SAFE_SCALAR.fullmatch(account):
        raise ValueError(f"account {account!r} cannot be part of a page file name")
    return owner_report.path(values, weekly_render.PAGE_PROJECT, f"consumi-{account}-{year}-W{number:02d}")


def _accounts(conn: sqlite3.Connection) -> list[str]:
    return [row[0] for row in conn.execute(
        "SELECT DISTINCT account FROM quota_snapshots WHERE seven_day_resets_at IS NOT NULL"
        f" AND {archive_settings.CLAUDE_ACCOUNT_ONLY} ORDER BY account")]


def _lane_paths(roots: Iterable[Path]) -> frozenset[Path]:
    return frozenset(path for root in roots for path in (root, root.resolve()))


def _group(conn: sqlite3.Connection, account: str, window: Window, sessions: Mapping[str, history.Run],
           lanes: frozenset[Path]) -> _Mixes:
    params = {"account": account, "start": window.start.astimezone(timezone.utc).isoformat(),
              "end": window.end.astimezone(timezone.utc).isoformat()}
    mixes = _Mixes(*(defaultdict(lambda: defaultdict(float)) for _ in range(4)))
    for session, cwd, repo, model, *counts in conn.execute(TURNS, params):
        weight, name = calibration.weighted_tokens(*counts), calibration.model_class(model)
        run = sessions.get(session)
        role = run.role if run else LANE if _is_lane(cwd, lanes) else INTERACTIVE
        for group, key in ((mixes.project, repo or _last_part(cwd)), (mixes.role, role), (mixes.model, name)):
            group[key][name] += weight
        if run:
            mixes.task[(FLOW, f"{run.repo}/{run.task_id}")][name] += weight
    for repo, number, session, model, *counts in conn.execute(PR_TURNS, params):
        if session not in sessions:
            mixes.task[(PULL_REQUEST, f"{repo}#{number}")][calibration.model_class(model)] += (
                calibration.weighted_tokens(*counts))
    return mixes


def _is_lane(cwd: str | None, lanes: frozenset[Path]) -> bool:
    return cwd is not None and any(root in Path(cwd).parents for root in lanes)


def _last_part(cwd: str | None) -> str:
    return Path(cwd).name or UNKNOWN if cwd else UNKNOWN


def _sum(mixes: Iterable[Mix]) -> Mix:
    total: Mix = defaultdict(float)
    for mix in mixes:
        for name, weight in mix.items():
            total[name] += weight
    return total


def _rows(convert: _Converter, groups: Mapping[str, Mix]) -> list[Row]:
    rows = [Row(name, convert.figure(mix)) for name, mix in groups.items()]
    return sorted(rows, key=lambda row: (-row.figure.tokens, row.name))


def _top_tasks(convert: _Converter, tasks: Mapping[tuple[str, str], Mix]) -> list[TopTask]:
    ranked = sorted(tasks.items(), key=lambda item: (-sum(item[1].values()), item[0]))[:TOP_TASKS]
    return [TopTask(kind, name, convert.figure(mix), _cheaper(convert, mix)) for (kind, name), mix in ranked]


def _cheaper(convert: _Converter, mix: Mix) -> Cheaper | None:
    """The mix with every class moved one step down the ladder, None when no class has a step."""
    steps = tuple((name, _below(name)) for name in sorted(mix) if _below(name))
    if not steps:
        return None
    moved = _sum([{_below(name) or name: weight} for name, weight in mix.items()])
    now, cheaper = convert.figure(mix), convert.figure(moved)
    if now.pct is None:
        return Cheaper(steps, None, None, None, now.basis)
    if cheaper.pct is None:
        return Cheaper(steps, now.pct, None, None, cheaper.basis, (now.low, now.high))
    return Cheaper(steps, now.pct, cheaper.pct, now.pct - cheaper.pct, cheaper.basis, (now.low, now.high),
                   (cheaper.low, cheaper.high), (now.low - cheaper.high, now.high - cheaper.low))


def _below(name: str) -> str | None:
    ladder = routing.MODEL_LADDER
    return ladder[ladder.index(name) - 1] if name in ladder and ladder.index(name) > 0 else None


def _outside(conn: sqlite3.Connection, settings: Settings, account: str, window: Window) -> Outside:
    """The shares of the UTC days the window touches; its first and last day may hold hours outside it."""
    last = window.end - timedelta(seconds=1)
    days = (window.start.astimezone(timezone.utc).date(), last.astimezone(timezone.utc).date())
    shares = [s for s in calibration.outside_share(conn, settings.plans, days) if s.account == account]
    rise, unattributed = sum(s.rise for s in shares), sum(s.unattributed for s in shares)
    if not rise:
        return Outside(0.0, None, None, None, None, unattributed, NO_RISE)
    if any(s.share is None for s in shares):
        return Outside(rise, None, None, None, None, unattributed, preflight.no_calibration(settings, account, last))
    outside = sum(s.rise * s.share for s in shares)
    return Outside(rise, outside, max(outside - RESOLUTION_PCT, 0.0), outside + RESOLUTION_PCT,
                   outside / rise, unattributed, OUTSIDE_BASIS)
