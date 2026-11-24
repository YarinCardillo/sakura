"""The latest events of the task ledgers and the lane event log, as one list."""

from __future__ import annotations

import io
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import TextIO

from sakura import file_cache
from sakura.cockpit import event_words
from sakura.flow import store
from sakura.lanes import events as lane_events
from sakura.outside import store as outside_store

EVENT_COUNT = 50
LEDGER_BULLET = "- "
LANE_TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"


def latest(ledgers: list[store.TaskStore], lane_log: Path | None = None, count: int = EVENT_COUNT, *,
           outside_root: Path | None = None, outside_histories: list[outside_store.History] | None = None) -> list[dict]:
    """Return the newest `count` events, oldest first, as `{source, scope, at, text, kind, words}` dicts.

    `kind` is what the event is (a ledger line's tag, a lane command, an outside lane's log kind such as `row`, `steer`
    or `mark`) and `words` its plain sentence where the engine has one (outside lanes), else None.

    A ledger line carries no time, so its lines sort by the ledger's last write and keep their order within
    the ledger; lane events and the log lines of open outside lanes (under `outside_root`) sort by their own
    time. Text is raw: the renderer strips controls.
    """
    ranked = [entry for ledger in ledgers for entry in _ledger_events(ledger, count)]
    ranked += _lane_events(count, lane_log)
    if outside_histories is None:
        outside_histories = outside_store.open_histories(outside_root)[0] if outside_root is not None else []
    ranked += _outside_events(outside_histories, count)
    ranked.sort(key=lambda entry: entry[0])
    return [event for _, event in ranked[-count:]]


def _ledger_events(ledger: store.TaskStore, count: int) -> list[tuple[tuple, dict]]:
    try:
        info = ledger.ledger.stat()
        contents = file_cache.small_bytes(ledger.ledger, info)
        stream = (ledger.ledger.open(encoding="utf-8", errors="replace") if contents is None
                  else io.TextIOWrapper(io.BytesIO(contents), encoding="utf-8", errors="replace"))
        with stream:
            lines = _ledger_lines(stream, count)
    except OSError:
        return []
    scope = f"{ledger.repo.name}/{ledger.task_id}"
    return [((info.st_mtime, number), {"source": "flow", "scope": scope, "at": "", "text": line[len(LEDGER_BULLET):],
                               "kind": event_words.ledger_kind(line[len(LEDGER_BULLET):]), "words": None})
            for number, line in enumerate(lines)]


def _ledger_lines(stream: TextIO, count: int) -> deque[str]:
    return deque((line.rstrip("\n") for line in stream if line.startswith(LEDGER_BULLET)), maxlen=count)


def _lane_events(count: int, lane_log: Path | None) -> list[tuple[tuple, dict]]:
    found = []
    for number, row in enumerate(lane_events.recent(count, lane_log)):
        stamp = _parse_time(str(row.get("time", "")))
        text = " ".join(str(row.get(key, "")) for key in ("kind", "outcome", "actor", "detail") if row.get(key))
        found.append(((stamp, number), {"source": "lane", "scope": f"{row.get('group', '')}/{row.get('name', '')}",
                                        "at": str(row.get("time", "")), "text": text,
                                        "kind": str(row.get("kind", "")), "words": None}))
    return found


def _outside_events(histories: list[outside_store.History], count: int) -> list[tuple[tuple, dict]]:
    found = []
    for lane, log in histories:
        for line in log[-count:]:
            stamp = _iso_time(str(line.get("at", "")))
            found.append(((stamp, line["seq"]), {"source": "outside", "scope": f"outside/{lane.id}",
                                                 "at": str(line.get("at", "")), "text": _outside_text(line),
                                                 "kind": str(line.get("kind", "")),
                                                 "words": event_words.outside_words(line),
                                                 **({"by": str(line["by"])} if line.get("by") else {})}))
    return found


def _outside_text(line: dict) -> str:
    detail = line.get("title") or line.get("question") or line.get("row") or line.get("outcome") or ""
    return " ".join(part for part in (str(line.get("kind", "")), str(detail), str(line.get("verdict", "")),
                                      f"by {line['by']}" if line.get("by") else "") if part)


def _iso_time(text: str) -> float:
    try:
        return datetime.fromisoformat(text).timestamp()
    except ValueError:
        return 0.0


def _parse_time(text: str) -> float:
    try:
        return datetime.strptime(text, LANE_TIME_FORMAT).replace(tzinfo=timezone.utc).timestamp()
    except ValueError:
        return 0.0
