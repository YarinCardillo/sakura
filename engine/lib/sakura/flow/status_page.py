"""The live status page of a flow: one engine-written Italian page per task in the notes app, refreshed on every state write."""

from __future__ import annotations

import os
import re
import sys
import threading
import urllib.parse
from datetime import datetime
from pathlib import Path

from sakura.flow import owner_report, plan
from sakura.flow.config import FlowConfig
from sakura.flow.render import strip_controls
from sakura.flow.store import TaskStore

STATUS_HISTORY_ITEMS = 8
PAGE_MODE = 0o644
FAILURE_EVENT = "status_page_failed"
APPROVE_COMMAND = re.compile(r"sakura flow approve \S+ \S+")
MARKDOWN_SPECIAL = re.compile(r"([\\`*_\[\]<>#|~!])")
STAMP_KEY = "updated"

GROUP_RUNNING, GROUP_WAITING, GROUP_BLOCKED, GROUP_TODO, GROUP_DONE = "running", "waiting", "blocked", "todo", "done"
GROUP_LABELS = {
    GROUP_RUNNING: "In corso",
    GROUP_WAITING: "In attesa dell'owner",
    GROUP_BLOCKED: "Bloccato",
    GROUP_TODO: "Da fare",
    GROUP_DONE: "Fatto",
}
STARTED_TASK_STATUSES = ("building", "review", "approved")
DONE_TASK_STATUS = "merged"
PHASE_LABELS = {
    "intake": "Raccolta della richiesta", "contract": "Contratto", "plan": "Piano", "build": "Costruzione",
    "evaluate": "Verifica", "pr": "Pull request", "report": "Rapporto", "cleanup": "Pulizia",
}
STATUS_LABELS = {
    "started": "avviato", "running": "in corso", "awaiting_approval": "in attesa del tuo ok", "blocked": "bloccato",
    "failed": "fallito", "stopped": "fermato", "report_ready": "pronto per la revisione", "cleaned": "concluso",
    "done": "fatto", "skipped": "saltato",
}
DECISION_LABELS = {"grant": "concesso", "deny": "negato"}
KIND_LABELS = {"test_change": "modifica ai test", "timeout": "tempo in piu", "allowlist": "eccezione alla lista"}
UNKNOWN_LABEL = "sconosciuto"
FLOW_STATUS_GROUPS = {"blocked": GROUP_BLOCKED, "failed": GROUP_BLOCKED, "stopped": GROUP_BLOCKED,
                      "awaiting_approval": GROUP_WAITING}

_refresh_lock = threading.Lock()


def path(values: dict, repo: str, task_id: str) -> Path:
    """Where the status page of a task lives: `<status_dir>/<task_id>.md`, with `~`, `{repo}` and `{owner_dir}` expanded."""
    owner_dir = owner_report.path(values, repo, task_id).parent
    template = values["report"]["status_dir"].replace("{owner_dir}", str(owner_dir)).replace("{repo}", repo)
    return Path(os.path.expanduser(template)) / f"{task_id}.md"


def render(state: dict, plan_tasks: list[plan.BuildTask], rulings: list[dict], owner_report_link: str | None,
           now: datetime, *, project: str) -> str:
    """Return the whole page. Pure: the same inputs give the same text."""
    task_id, links = state["task"], _links(state)
    front = owner_report.front_matter(f"Stato: {task_id}", f"Stato del flusso per {state.get('ref', task_id)}", project,
                                      "status", task_id, (STAMP_KEY, now.isoformat(timespec="seconds")), links)
    sections = [_board(state, plan_tasks), _updates(state, now), _phase(state), *_cost(state),
                _tasks(state, plan_tasks), _decisions(rulings)]
    if owner_report_link:
        sections.append(f"## Rapporto\n\n[Rapporto per il proprietario]({urllib.parse.quote(owner_report_link)})")
    return front + "\n" + "\n\n".join(sections) + "\n"


def refresh(store: TaskStore, cfg: FlowConfig, now: datetime | None = None) -> None:
    """Write the page of `store`'s task when its content changed; a failure is ledgered once and never raised.

    The page is a convenience for the owner, so no problem here may stop or block a flow.
    """
    try:
        with _refresh_lock:
            _write_if_changed(store, cfg, now or datetime.now().astimezone())
    except Exception as error:  # noqa: BLE001 - a broken page must never fail the flow
        _log_failure(store, error)


def _write_if_changed(store: TaskStore, cfg: FlowConfig, now: datetime) -> None:
    values, repo = cfg.values, store.repo.name
    state = store.last_written() or store.read_state()
    target = path(values, repo, store.task_id)
    document = render(state, _plan_tasks(store), store.ruling_entries(), _report_link(values, repo, store.task_id, target),
                      now, project=owner_report.project(values, repo))
    if _without_stamp(document) != _without_stamp(_read_text(target)):
        owner_report.write(target, document)


def _plan_tasks(store: TaskStore) -> list[plan.BuildTask]:
    try:
        return plan.load(store, None)
    except plan.PlanError:
        return []


def _report_link(values: dict, repo: str, task_id: str, target: Path) -> str | None:
    report = owner_report.path(values, repo, task_id)
    return os.path.relpath(report, target.parent) if report.is_file() else None


def _read_text(target: Path) -> str:
    try:
        return target.read_text()
    except OSError:
        return ""


def _without_stamp(document: str) -> str:
    return "\n".join(line for line in document.split("\n") if not line.startswith(f"{STAMP_KEY}: "))


def _log_failure(store: TaskStore, error: Exception) -> None:
    line = f"{FAILURE_EVENT}: {_one_line(f'{type(error).__name__}: {error}')}"
    try:
        if line not in store.ledger.read_text():
            store.append_ledger("status", line)
    except OSError as ledger_error:
        print(f"{line} (and the ledger is unwritable: {_one_line(str(ledger_error))})", file=sys.stderr)


# -- sections ---------------------------------------------------------------


def _board(state: dict, plan_tasks: list[plan.BuildTask]) -> str:
    groups: dict[str, list[plan.BuildTask]] = {group: [] for group in GROUP_LABELS}
    for task in plan_tasks:
        groups[_group_of(state, task)].append(task)
    lines = ["## Bacheca"]
    if not plan_tasks:
        lines += ["", "Nessun task nel piano per ora."]
    for group, label in GROUP_LABELS.items():
        lines += ["", f"### {label} ({len(groups[group])})"]
        lines += [f"- **{_text(task.id)}** {_text(task.title)}" for task in groups[group]]
    return "\n".join(lines)


def _group_of(state: dict, task: plan.BuildTask) -> str:
    status = (state.get("build") or {}).get(task.id, {}).get("status")
    if status == DONE_TASK_STATUS:
        return GROUP_DONE
    if status in STARTED_TASK_STATUSES:
        return FLOW_STATUS_GROUPS.get(state.get("status"), GROUP_RUNNING)
    return GROUP_TODO


def _updates(state: dict, now: datetime) -> str:
    entries = list(reversed((state.get("history") or [])[-STATUS_HISTORY_ITEMS:]))
    lines = ["## Ultimo aggiornamento", ""]
    lines += [_history_line(entry, now) for entry in entries] or ["Ancora nessun aggiornamento."]
    return "\n".join(lines)


def _history_line(entry: dict, now: datetime) -> str:
    detail = f": {_text(entry.get('detail', ''))}" if entry.get("detail") else ""
    return (f"- {_local_time(entry.get('at', ''), now)} {_phase_label(entry.get('phase'))}, "
            f"{STATUS_LABELS.get(entry.get('status'), UNKNOWN_LABEL)}{detail}")


def _local_time(stamp: str, now: datetime) -> str:
    try:
        return datetime.fromisoformat(stamp).astimezone(now.tzinfo).strftime("%Y-%m-%d %H:%M")
    except (TypeError, ValueError):
        return "?"


def _phase(state: dict) -> str:
    status = state.get("status")
    lines = ["## Fase", "", f"- Fase: {_phase_label(state.get('phase'))}",
             f"- Stato: {STATUS_LABELS.get(status, UNKNOWN_LABEL)}"]
    reason = state.get("reason") or ""
    if status == "awaiting_approval":
        lines.append("- Cosa devi fare: serve il tuo ok per andare avanti." + _approve_hint(reason))
    elif reason:
        lines.append(f"- Motivo: {_text(reason)}")
    return "\n".join(lines)


def _cost(state: dict) -> list[str]:
    """The pre-flight cost section, once the plan phase has stored one."""
    saved = (state.get("budget") or {}).get("preflight")
    if not saved:
        return []
    lines = ["## Costo previsto", "", f"- Stima prima di partire: {_text(saved.get('text', ''))}"]
    if not saved.get("fits", True):
        lines.append(f"- Non rientra nella riserva: {_text(saved.get('reason', ''))}")
    return ["\n".join(lines)]


def _approve_hint(reason: str) -> str:
    found = APPROVE_COMMAND.search(reason)
    return f" Comando: {_code(found.group(0))}" if found else ""


def _tasks(state: dict, plan_tasks: list[plan.BuildTask]) -> str:
    lines = ["## Task", ""]
    for task in plan_tasks:
        label = GROUP_LABELS[_group_of(state, task)]
        lines.append(f"- **{_text(task.id)}** {_text(task.title)} ({label.lower()}, {len(task.files)} file)")
        lines.append(f"  {_text(task.owner_line) or _text(task.title)}")
    return "\n".join(lines if plan_tasks else [*lines, "Nessun task nel piano per ora."])


def _decisions(rulings: list[dict]) -> str:
    lines = ["## Decisioni", ""]
    for ruling in rulings:
        decision = DECISION_LABELS.get(ruling.get("decision"), UNKNOWN_LABEL)
        kind = KIND_LABELS.get(ruling.get("kind"), UNKNOWN_LABEL)
        lines.append(f"- {decision}, {kind} su {_code(str(ruling.get('path', '')))}: {_text(str(ruling.get('why', '')))} "
                     f"(se e sbagliato: {_text(str(ruling.get('cost_if_wrong', '')))})")
    return "\n".join(lines if rulings else [*lines, "Nessuna decisione presa."])


# -- text safety ------------------------------------------------------------


def _links(state: dict) -> list[str]:
    url = (state.get("pr") or {}).get("url")
    return [url] if url else []


def _phase_label(phase: object) -> str:
    return PHASE_LABELS.get(phase, UNKNOWN_LABEL)


def _one_line(value: str) -> str:
    return " ".join(strip_controls(str(value)).split())


def _text(value: str) -> str:
    """One line of plain text with every Markdown and HTML special character escaped."""
    return MARKDOWN_SPECIAL.sub(r"\\\1", _one_line(value))


def _code(value: str) -> str:
    """A code span; backticks inside the value cannot close it."""
    return f"`{_one_line(value).replace('`', chr(39))}`"
