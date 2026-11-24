"""What a reading view of the morning digest needs: its text, its news and its asks, each ask with its state.

The digest is an outside lane of the agent `digest` (`digest/publish.py`). Its news are kept in the run's own file
(`<engine home>/digest/<date>.json`), its checks and decisions are the lane's rows and questions. Everything here is the
digest's text, written by a model from what the sources returned: data, shown as text, never run. A news entry has no
read state of its own; an ask is `open` until the person answers it, then `decided` with their answer.
"""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

from sakura import beneath, engine_home, questions
from sakura.cockpit import proposal_actions
from sakura.digest import config, publish, schema
from sakura.flow import waits
from sakura.outside import records, store
from sakura.outside import waits as outside_waits
from sakura.outside.records import Lane

OPEN, DECIDED = "open", "decided"
KIND_DIGEST = "digest"
DIGEST_TITLE = "Morning digest"


def of_lane(lane: Lane, log: list[dict]) -> dict | None:
    """The reading view of the lane when it is a day's digest (its agent is `digest`, its id `digest-<date>`), else None."""
    if (lane.agent.name != publish.AGENT_NAME or not schema.LANE_ID.fullmatch(lane.id)
            or lane.agent.where != config.digest_folder()):
        return None
    date = lane.id.removeprefix("digest-")
    return {"date": date, "text": lane.about, "news": _news(date), "asks": _asks(lane, log)}


def date_of_wait(wait: waits.Wait) -> str | None:
    """Return the date only when the exact wait revision comes from the host's published digest."""
    if (wait.source != outside_waits.SOURCE or wait.author != outside_waits.author_of_agent(publish.AGENT_NAME)
            or not schema.LANE_ID.fullmatch(wait.task)):
        return None
    date = wait.task.removeprefix("digest-")
    try:
        if Path(wait.repo) != config.digest_folder():
            return None
        digest = _published(date)
        if digest.lane == wait.task and _matches_published(wait, digest):
            return date
    except (OSError, ValueError, engine_home.EngineHomeError, schema.SchemaError, records.Refused, records.StoreError):
        pass
    return None


def _matches_published(wait: waits.Wait, digest: schema.Digest) -> bool:
    name, _, revision = json.loads(wait.id)[4].partition("@")
    if wait.kind == waits.QUESTION:
        return any(decision.id == name and questions.make(
            name, publish.decision_text(decision), decision.choices, datetime.fromisoformat(digest.generated_at)
        ).digest == revision for decision in digest.output.decisions)
    if wait.kind != waits.OWNER_ROW:
        return False
    lane = store.read(store.path(), wait.task)
    row = next((row for row in lane.rows if row.id == name and row.digest == revision), None)
    if row is None:
        return False
    return any(check.id == name and records.make_row(
        name, **publish.row_fields(check), expected="", now_text=row.since, revision=row.revision
    ).digest == revision for check in digest.output.checks)


def _published(date: str) -> schema.Digest:
    return schema.digest_from_json(json.loads(beneath.read_bytes(config.digest_folder(), f"{date}.json")))


def _news(date: str) -> list[dict]:
    """The news of the day's file with their sources and links; none when the file is gone or does not read."""
    try:
        digest = _published(date)
    except (OSError, ValueError, schema.SchemaError):
        return []
    return [{"title": item.title, "text": item.text, "source": item.source, "links": _links(item.links)}
            for item in digest.output.news]


def _links(links) -> list[dict]:
    return [{"label": link.label, "url": link.url} for link in links]


def _asks(lane: Lane, log: list[dict]) -> list[dict]:
    open_ids = {_name(wait): wait.id for wait in outside_waits.of_lane(lane, log)}
    answers = {(line["kind"], line.get("row") or line.get("question"), line["digest"]): line for line in log
               if line.get("kind") in ("mark", "reply")}
    rows = [_ask(waits.OWNER_ROW, row.id, row.digest, row.title, _row_text(row), row.note, (), open_ids,
                 answers.get(("mark", row.id, row.digest)), "verdict") for row in lane.rows]
    questions = [_ask(waits.QUESTION, item.id, item.digest, _first_line(item.text), item.text, "", item.choices, open_ids,
                      answers.get(("reply", item.id, item.digest)), "choice") for item in lane.questions]
    return rows + questions


def _ask(kind: str, name: str, digest: str, title: str, text: str, note: str, choices: tuple[str, ...],
         open_ids: dict[str, str], answer: dict | None, answer_field: str) -> dict:
    body, links = outside_waits.split_links(f"{text}\n{note}" if note else text)
    asked = {"kind": kind, "title": title, "text": body, "links": [{"label": label, "url": url} for label, url in links],
             "choices": list(choices), "state": OPEN, "id": open_ids.get((kind, name, digest))}
    if answer is None:
        resolution = proposal_actions.outcome(asked["id"]).get("answer") if asked["id"] else None
        return {**asked, "state": DECIDED, "id": None, "answer": resolution} if resolution else asked
    given = answer.get(answer_field) or answer.get("text") or ""
    return {**asked, "state": DECIDED, "id": None, "answer": given}


def _name(wait: waits.Wait) -> tuple[str, str, str]:
    name, _, digest = json.loads(wait.id)[4].partition("@")
    return wait.kind, name, digest


def _row_text(row) -> str:
    return " ".join(row.steps)


def _first_line(text: str) -> str:
    return " ".join(text.split("\n", 1)[0].split())
