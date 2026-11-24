"""The weekly report as text for `sakura usage weekly` (English) and as an owner report page (Italian).

Every percentage is printed with its basis (calibrated, scaled from another plan, or `no calibration`) and, when
the calibration has a spread, a band. Names come from paths and ids, so they are reduced to one line and cannot
break a table.
"""

from __future__ import annotations

from typing import NamedTuple

from sakura.accounts import labels as account_labels
from sakura.accounts.registry import Registry
from sakura.flow.render import strip_controls
from sakura.usage.archive.report import format_table

PAGE_PROJECT = "sakura"
TABLE_BAR = "|"


class Labels(NamedTuple):
    window: str
    total: str
    project: str
    role: str
    model: str
    headers: tuple[str, str, str, str]
    outside_title: str
    outside: str
    outside_unknown: str
    outside_none: str
    tasks_title: str
    tasks_note: str
    task_headers: tuple[str, str, str, str, str]
    cheaper: str
    cheaper_uncalibrated: str
    unknown: str
    notes_title: str
    notes: tuple[str, ...]


ENGLISH = Labels(
    window="account {account}, plan {plan}, {start} to {end} UTC",
    total="total: {tokens} weighted tokens, {pct}; basis: {basis}",
    project="by project", role="by role", model="by model class",
    headers=("name", "weighted tokens", "% of the week", "basis"),
    outside_title="outside Claude Code",
    outside=("{pct} of the week came from outside Claude Code ({share} of the {rise} the snapshots rose, "
             "band {low}-{high}%); basis: {basis}"),
    outside_unknown="not computable: the snapshots rose {rise}; basis: {basis}",
    outside_none="the snapshots show no weekly rise to attribute",
    tasks_title="three most expensive tasks",
    tasks_note=("cheaper route: the same weighted tokens priced one model class down; an upper bound of the saving, "
                "a cheaper model may need more tokens"),
    task_headers=("task", "weighted tokens", "% of the week", "cheaper route (upper bound)", "basis"),
    cheaper="{steps}: {now} to {then}, saves up to {saving} of the week; basis: {basis}",
    cheaper_uncalibrated="{steps}: no calibration for these classes; basis: {basis}",
    unknown="n/a",
    notes_title="notes",
    notes=("percentages are weighted tokens over the calibrated ratios of the plan; band: 20th to 80th percentile "
           "of the ratios, absent with one interval",
           "outside use covers the whole UTC days the week touches, so its first and last day may hold hours of a "
           "neighbouring week"),
)

ITALIAN = Labels(
    window="Account {account}, piano {plan}, dal {start} al {end} (UTC).",
    total="Totale: {tokens} token pesati, {pct} della settimana. Base: {basis}.",
    project="Per progetto", role="Per ruolo", model="Per classe di modello",
    headers=("Nome", "Token pesati", "% della settimana", "Base"),
    outside_title="Fuori da Claude Code",
    outside=("{pct} della settimana è arrivato da fuori Claude Code ({share} dei {rise} di salita degli snapshot, "
             "banda {low}-{high}%). Base: {basis}."),
    outside_unknown="Non calcolabile: gli snapshot sono saliti di {rise}. Base: {basis}.",
    outside_none="Gli snapshot non mostrano nessuna salita settimanale da attribuire.",
    tasks_title="I tre compiti più costosi",
    tasks_note=("Rotta più economica: gli stessi token pesati calcolati una classe di modello sotto. È un limite "
                "superiore del risparmio, perché un modello più economico può usare più token."),
    task_headers=("Compito", "Token pesati", "% della settimana", "Rotta più economica (limite superiore)", "Base"),
    cheaper="{steps}: da {now} a {then}, risparmio fino a {saving} della settimana. Base: {basis}",
    cheaper_uncalibrated="{steps}: nessuna calibrazione per queste classi. Base: {basis}",
    unknown="n.d.",
    notes_title="Come leggere i numeri",
    notes=("Le percentuali sono token pesati divisi per i rapporti calibrati del piano; la banda va dal 20° "
           "all'80° percentile dei rapporti e manca con un solo intervallo.",
           "L'uso da fuori copre i giorni UTC interi che la settimana tocca, quindi il primo e l'ultimo giorno "
           "possono contenere ore di una settimana vicina."),
)


def text(report, labels: Labels = ENGLISH, registry: Registry = account_labels.NO_ACCOUNTS) -> str:
    """The report as aligned text sections; the account is shown by its label from `registry`."""
    parts = [_summary(report, labels, registry)]
    for title, header, rows in _tables(report, labels):
        parts.append(f"{title}\n" + (format_table(header, rows) if rows else "none"))
    parts.append(f"{labels.outside_title}\n{_outside(report.outside, labels)}")
    parts.append(f"{labels.tasks_title} ({labels.tasks_note})\n" + _task_table(report, labels))
    return "\n\n".join(parts)


def page(report, week_number: int, registry: Registry = account_labels.NO_ACCOUNTS) -> str:
    """The owner report page: front matter, then the Italian sections.

    `report.account` is a safe file-name scalar and stays the machine-readable `account:` of the front matter; the
    summary line shows the label from `registry`.
    """
    labels = ITALIAN
    front = (f'---\ntitle: "Consumi settimana {week_number}"\nproject: {PAGE_PROJECT}\nkind: report\n'
             f"account: {report.account}\n---\n")
    body = [_summary(report, labels, registry)]
    for title, header, rows in _tables(report, labels):
        body.append(f"## {title}\n\n" + _markdown(header, rows))
    body.append(f"## {labels.outside_title}\n\n{_outside(report.outside, labels)}")
    body.append(f"## {labels.tasks_title}\n\n{labels.tasks_note}\n\n" + _markdown_tasks(report, labels))
    body.append(f"## {labels.notes_title}\n\n" + "\n".join(f"- {note}" for note in labels.notes))
    return front + "\n" + "\n\n".join(body) + "\n"


def _summary(report, labels: Labels, registry: Registry) -> str:
    account = _name(account_labels.display_name(report.account, registry))
    window = labels.window.format(account=account, plan=report.plan, start=_stamp(report.start),
                                  end=_stamp(report.end))
    total = report.total
    return window + "\n" + labels.total.format(tokens=f"{total.tokens:.0f}", pct=_pct(total, labels), basis=total.basis)


def _tables(report, labels: Labels) -> list[tuple[str, tuple[str, ...], list[tuple[str, ...]]]]:
    groups = ((labels.project, report.by_project), (labels.role, report.by_role), (labels.model, report.by_class))
    return [(title, labels.headers, [(_name(row.name), f"{row.figure.tokens:.0f}", _pct(row.figure, labels),
                                      row.figure.basis) for row in rows]) for title, rows in groups]


def _outside(outside, labels: Labels) -> str:
    if not outside.rise:
        return labels.outside_none
    if outside.pct is None:
        return labels.outside_unknown.format(rise=_points(outside.rise), basis=outside.basis)
    return labels.outside.format(pct=_points(outside.pct), share=f"{outside.share:.0%}", rise=_points(outside.rise),
                                 low=f"{outside.low:.1f}", high=f"{outside.high:.1f}", basis=outside.basis)


def _task_rows(report, labels: Labels) -> list[tuple[str, ...]]:
    return [(_name(task.name), f"{task.figure.tokens:.0f}", _pct(task.figure, labels), _cheaper(task, labels),
             task.figure.basis) for task in report.top_tasks]


def _task_table(report, labels: Labels) -> str:
    rows = _task_rows(report, labels)
    return format_table(labels.task_headers, rows) if rows else "none"


def _markdown_tasks(report, labels: Labels) -> str:
    rows = _task_rows(report, labels)
    return _markdown(labels.task_headers, rows) if rows else "Nessuno."


def _cheaper(task, labels: Labels) -> str:
    cheaper = task.cheaper
    if cheaper is None:
        return "-"
    steps = ", ".join(f"{old} > {new}" for old, new in cheaper.steps)
    if cheaper.saving_pct is None:
        return labels.cheaper_uncalibrated.format(steps=steps, basis=cheaper.basis)
    return labels.cheaper.format(
        steps=steps, now=_banded(cheaper.pct_now, cheaper.now_band), then=_banded(cheaper.pct_cheaper, cheaper.cheaper_band),
        saving=_banded(cheaper.saving_pct, cheaper.saving_band), basis=cheaper.basis)


def _pct(figure, labels: Labels) -> str:
    if figure.pct is None:
        return labels.unknown
    band = f" ({figure.low:.1f}-{figure.high:.1f})" if f"{figure.low:.1f}" != f"{figure.high:.1f}" else ""
    return f"{figure.pct:.1f}%{band}"


def _banded(value: float, band: tuple[float, float] | None) -> str:
    """`value` as a percentage, with its band when the edges differ at one decimal."""
    if band is None or f"{band[0]:.1f}" == f"{band[1]:.1f}":
        return _points(value)
    return f"{value:.1f}% ({band[0]:.1f}-{band[1]:.1f})"


def _points(value: float) -> str:
    return f"{value:.1f}%"


def _stamp(moment) -> str:
    return moment.strftime("%Y-%m-%d %H:%M")


def _name(value: str) -> str:
    return " ".join(strip_controls(value).split()).replace(TABLE_BAR, "/")


def _markdown(header: tuple[str, ...], rows: list[tuple[str, ...]]) -> str:
    lines = [_cells(header), _cells(("-",) * len(header))]
    return "\n".join(lines + [_cells(row) for row in rows]) if rows else "Nessuno."


def _cells(cells: tuple[str, ...]) -> str:
    return "| " + " | ".join(cells) + " |"
