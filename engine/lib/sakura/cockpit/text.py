"""The snapshot as fixed-width text lines; pure, and every value passes through `strip_controls`."""

from __future__ import annotations

from sakura.cockpit.accounts import AccountRow
from sakura.cockpit.items import GroupRow, Item
from sakura.cockpit.snapshot import Snapshot
from sakura.flow.progress import Progress
from sakura.flow.waits import Wait
from sakura.flow.render import strip_controls
from sakura.usage.aggregate import ProviderWeek
from sakura.usage.report import NOT_AVAILABLE

MARK_SELECTED = ">"
MARK_NONE = " "
INDENT = "    "
ELLIPSIS = "..."
BLANK = "-"
DEFERRED_SHOWN = 5
DEFERRED_EXPLANATION = "deferred at night (no phone page is sent at night), listed here until the next day starts"
EVENTS_SHOWN = 10


def render(snapshot: Snapshot, width: int, selected: tuple | None) -> list[str]:
    """Return the cockpit as lines no wider than `width`; the row whose key is `selected` starts with `>`."""
    lines = [*_header(snapshot), *_problems(snapshot), *_usage(snapshot)]
    lines += _section("WAITING", [_wait(wait) for wait in snapshot.visible_waits])
    lines += _section("GROUPS", [_group(row, selected) for row in snapshot.groups])
    lines += _section("ITEMS", [line for item in snapshot.items for line in _item(item, selected)])
    lines += _section("EVENTS", [_event(event) for event in snapshot.events[-EVENTS_SHOWN:]])
    return [_fit(line, width) for line in lines]


def _header(snapshot: Snapshot) -> list[str]:
    load = snapshot.load
    average = " ".join(f"{value:.2f}" for value in load["loadavg"])
    lines = [f"sakura cockpit  mode: {_clean(snapshot.mode)}  slots {load['slots_in_use']}/{load['slots_total']}"
             f"  queue {len(load['queue'])}  load {average}"]
    if snapshot.deferred:
        lines.append(f"{len(snapshot.deferred)} {DEFERRED_EXPLANATION}:")
        lines += [_deferred(entry) for entry in snapshot.deferred[:DEFERRED_SHOWN]]
        if len(snapshot.deferred) > DEFERRED_SHOWN:
            lines.append(f"{INDENT}and {len(snapshot.deferred) - DEFERRED_SHOWN} more")
    return lines


def _problems(snapshot: Snapshot) -> list[str]:
    return [f"! {_clean(problem)}" for problem in snapshot.problems]


def _deferred(entry: dict) -> str:
    parts = (entry.get("at", ""), entry.get("task", ""), entry.get("event", ""), entry.get("message", ""))
    return f"{INDENT}{_cells(*parts)}"


def _usage(snapshot: Snapshot) -> list[str]:
    return [provider_line(entry) for entry in snapshot.usage.providers]


def provider_line(entry: ProviderWeek) -> str:
    """One provider's week as a line: utilization, projection at reset and tokens."""
    utilization = f"{entry.utilization * 100:.1f}%" if entry.utilization is not None else NOT_AVAILABLE
    tokens = f"{entry.totals.total:,}" if entry.totals is not None else NOT_AVAILABLE
    ahead = f", {entry.projection.at_reset * 100:.1f}% at reset" if entry.projection else ""
    return f"{_clean(entry.provider)}: week {utilization}{ahead}, tokens {tokens}"


def account_line(row: AccountRow) -> str:
    """One account as a line: label, provider, week utilization and state."""
    week = f"{row.week_pct:.1f}%" if row.week_pct is not None else NOT_AVAILABLE
    return f"{_clean(row.label)} ({_clean(row.provider)}): week {week}, {_clean(row.state)}"


def _section(title: str, body: list[str]) -> list[str]:
    return [title, *body] if body else []


def progress_words(progress: Progress) -> str:
    """"3 of 7 done"; before the lane has a plan or a contract, its phase."""
    return f"{progress.done} of {progress.total} done" if progress.total else _clean(progress.phase)


def from_line(author: str, project: str) -> str:
    """"From claude in demo" for an outside agent's `outside:<name>` author and its project."""
    return f"From {author.partition(':')[2]} in {project}"


def _wait(wait: Wait) -> str:
    origin = f"{_clean(from_line(wait.author, wait.project))}  " if wait.source == "outside" else ""
    return (f"{_cells(wait.kind)}  {_clean(wait.project)}/{_clean(wait.task)}  {origin}{_clean(wait.title)}: "
            f"{_clean(wait.detail)}")


def _group(row: GroupRow, selected: tuple | None) -> str:
    noun = "lane" if row.lanes == 1 else "lanes"
    problem = f"  ! {_clean(row.problem)}" if row.problem else ""
    return f"{_mark(row.key, selected)} group {_clean(row.name)}  {row.lanes} {noun}  {_clean(row.next_command)}{problem}"


def _item(item: Item, selected: tuple | None) -> list[str]:
    models = " ".join(f"{_clean(role)}={_clean(model)}" for role, model in item.models.items())
    lines = [f"{_mark(item.key, selected)} {item.kind:<4} {_label(item)}  [{_clean(item.agent_state)}]  {_blank(item.pr)}",
             f"{INDENT}{_cells(item.branch, item.last_commit)}  stack {_blank(item.stack)}"]
    if item.progress is not None:
        lines.append(f"{INDENT}progress: {progress_words(item.progress)}")
    if models:
        lines.append(f"{INDENT}models: {models}")
    if item.pending:
        lines.append(f"{INDENT}pending: {', '.join(_clean(entry) for entry in item.pending)}")
    if item.usage is not None:
        lines.append(f"{INDENT}tokens: {item.usage.total:,}")
    if item.next_command:
        lines.append(f"{INDENT}next: {_clean(item.next_command)}")
    return lines


def _label(item: Item) -> str:
    if item.kind == "outside":
        return f"outside/{_clean(item.key[1])}"
    scope = item.repo.name if item.kind == "flow" else item.key[1]
    return f"{_clean(scope)}/{_clean(item.key[2])}"


def _event(event: dict) -> str:
    return f"{_cells(event.get('at', ''), event.get('source', ''), event.get('scope', ''))}: {_clean(event.get('text', ''))}"


def _mark(key: tuple, selected: tuple | None) -> str:
    return MARK_SELECTED if selected is not None and key == selected else MARK_NONE


def _cells(*values: object) -> str:
    return " ".join(cell for cell in (_clean(value) for value in values) if cell)


def _blank(value: object) -> str:
    return _clean(value) or BLANK


def _clean(value: object) -> str:
    """One line of printable text: controls removed, newlines and tabs and runs of spaces turned into one space."""
    return " ".join(strip_controls(str(value)).split())


def _fit(line: str, width: int) -> str:
    return line if len(line) <= width else line[: max(0, width - len(ELLIPSIS))] + ELLIPSIS
