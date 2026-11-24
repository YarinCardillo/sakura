"""The seven verbs an outside agent calls: open, row, ask, close, inbox, list, show.

Both faces use these functions: `sakura outside <verb>` and the stdio MCP server. Each takes the agent, its
arguments as a dict and the time, and returns the answer document; none raises. A refusal is
`{"result": "refused", "reason": ...}` with nothing written; a store file that cannot be read or written is
`{"result": "error", "reason": ...}`.
"""

from __future__ import annotations

import errno
from datetime import datetime, timedelta
from typing import Callable, Protocol

from sakura import actors, engine_home, questions
from sakura.flow import config as flow_config
from sakura.flow import notify
from sakura.outside import inbox as lane_inbox
from sakura.outside import records, store
from sakura.outside.records import Agent, Refused, StoreError

PUSH_EVENT = "outside-question"
PUSH_INTERVAL = timedelta(minutes=10)
PUSH_TEXT_LIMIT = 300
MAX_WAIT_SECONDS = 300
MCP_SERVER = "sakura outside serve"
SANDBOX_ERRNOS = (errno.EPERM, errno.EACCES, errno.EROFS)
WRITE_VERBS = ("open", "row", "ask", "close", "inbox")
READ_VERBS = ("list", "show")
PUSH_NAMES = {notify.PUSHED: "sent", notify.DEFERRED: "deferred", notify.NO_TOPIC: "skipped"}


class Pusher(Protocol):
    """What `ask` needs to page the person: the engine's alert pusher or a fake."""

    def push(self, event: str, title: str, message: str) -> str: ...


Verb = Callable[..., dict]


def dispatch(name: str, agent: Agent, arguments: dict, now: datetime, pusher: Pusher | None = None,
             *, scoped: bool = False) -> dict:
    """Run the verb `name` and return its answer document; never raises."""
    verb = VERBS.get(name)
    if verb is None:
        return refused(f"unknown verb {name!r}; the verbs are {', '.join(VERBS)}")
    try:
        root = store.path()
        if scoped and name == "list":
            arguments = {**arguments, "agent": agent.name}
        if scoped and name == "show" and store.read(root, _text(arguments, "lane")).agent.name != agent.name:
            raise Refused("no lane for this outside agent")
        return verb(root, agent, arguments, now, pusher) if name == "ask" else verb(root, agent, arguments, now)
    except Refused as error:
        return refused(str(error))
    except (StoreError, engine_home.EngineHomeError) as error:
        return _store_failure(error)


def refused(reason: str) -> dict:
    """The answer of a refused call."""
    return {"result": "refused", "reason": reason}


def open_lane(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """Open a lane, or update the agent's own with new fields; the same fields answer `unchanged`."""
    lane_id = _text(arguments, "lane")
    links = [(_text(item, "label"), _text(item, "url")) for item in _list(arguments, "links")]
    change = store.open_lane(root, agent, lane_id, title=_text(arguments, "title"),
                             about=_text(arguments, "about", ""), links=links,
                             phase=_text(arguments, "phase", "draft"), now=now)
    return {"result": change.result, "lane": lane_id, "phase": change.lane.phase}


def put_row(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """Add an owner row, or change one; changed content makes the row wait for the person again."""
    lane_id, row_id = _text(arguments, "lane"), _text(arguments, "row")
    change = store.put_row(root, agent, lane_id, row_id, title=_text(arguments, "title"),
                           steps=_strings(arguments, "steps"), expected=_text(arguments, "expected", ""),
                           note=_text(arguments, "note", ""), now=now)
    digest = next(row.digest for row in change.lane.rows if row.id == row_id)
    return {"result": change.result, "lane": lane_id, "row": row_id, "digest": digest}


def ask(root, agent: Agent, arguments: dict, now: datetime, pusher: Pusher | None = None) -> dict:
    """Ask the person a question; a new one pages the person, at most once per lane per ten minutes."""
    lane_id, question_id = _text(arguments, "lane"), _text(arguments, "question")
    try:
        question = questions.make(question_id, _text(arguments, "text"), _strings(arguments, "choices"), now)
    except questions.QuestionRefused as error:
        raise Refused(str(error)) from error
    change = store.ask(root, agent, lane_id, question, now)
    answer = {"result": change.result, "lane": lane_id, "question": question_id, "digest": question.digest}
    answer["push"] = _push(root, agent, change, question, now, pusher) if change.is_new else "skipped"
    return answer


def close(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """Close the agent's lane as `done` or `dropped`."""
    lane_id = _text(arguments, "lane")
    by = actors.Actor(actors.OUTSIDE_SOURCE, agent.name)
    change = store.close(root, by, lane_id, outcome=_text(arguments, "outcome"), now=now)
    return {"result": change.result, "lane": lane_id, "outcome": change.lane.outcome}


def inbox(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """The person's answers to this lane not yet read; `after` acknowledges, `wait` blocks for an answer."""
    lane_id = _text(arguments, "lane")
    after, seconds = _count(arguments, "after"), _count(arguments, "wait") or 0
    if seconds > MAX_WAIT_SECONDS:
        raise Refused(f"wait is at most {MAX_WAIT_SECONDS} seconds")
    page = lane_inbox.wait(root, agent.name, lane_id, after, seconds)
    return {"result": "inbox", "lane": lane_id, "entries": [_entry(line) for line in page.entries],
            "cursor": page.cursor, "open_rows": page.open_rows, "open_questions": page.open_questions,
            "closed": page.is_closed}


def list_lanes(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """The lanes (the closed ones with `closed`), optionally of one agent, each with what it waits on."""
    named = arguments.get("agent")
    lanes = store.lanes(root, closed=bool(arguments.get("closed")))
    return {"result": "lanes", "lanes": [_summary(root, lane) for lane in lanes
                                         if named is None or lane.agent.name == named]}


def show(root, agent: Agent, arguments: dict, now: datetime) -> dict:
    """One lane's record and its whole log."""
    lane_id = _text(arguments, "lane")
    return {"result": "lane", "lane": records.lane_to_json(store.read(root, lane_id)),
            "log": store.log(root, lane_id)}


VERBS: dict[str, Verb] = {"open": open_lane, "row": put_row, "ask": ask, "close": close, "inbox": inbox,
                          "list": list_lanes, "show": show}


def _push(root, agent: Agent, change: store.Change, question: questions.Question, now: datetime,
          pusher: Pusher | None) -> str:
    if _pushed_recently(change.lane.pushed_at, now):
        return "skipped"
    title = f"From {agent.name} in {agent.where.name}: {change.lane.title}"
    try:
        sent = (pusher or notify.build_alert_pusher(flow_config.load_personal())).push(
            PUSH_EVENT, title, question.text[:PUSH_TEXT_LIMIT])
    except Exception:  # noqa: BLE001 (a failed push is reported in the answer and never fails the question)
        return "failed"
    name = PUSH_NAMES.get(sent, "failed")
    if name in ("sent", "deferred"):
        store.note_pushed(root, change.lane.id, now)
    return name


def _pushed_recently(pushed_at: str | None, now: datetime) -> bool:
    return pushed_at is not None and now - datetime.fromisoformat(pushed_at) < PUSH_INTERVAL


def _store_failure(error: Exception) -> dict:
    cause = error.__cause__
    if isinstance(cause, OSError) and cause.errno in SANDBOX_ERRNOS:
        return refused(f"this process cannot write the engine home ({error}); a sandboxed shell cannot, "
                       f"so use the MCP server `{MCP_SERVER}`, which your client starts outside the sandbox")
    return {"result": "error", "reason": str(error)}


def _summary(root, lane: records.Lane) -> dict:
    log = store.log(root, lane.id)
    return {"lane": lane.id, "title": lane.title, "agent": lane.agent.name, "where": str(lane.agent.where),
            "phase": lane.phase, "opened_at": lane.opened_at, "closed_at": lane.closed_at, "outcome": lane.outcome,
            "rows": len(lane.rows), "done_rows": lane_inbox.done_rows(lane, log),
            "open_rows": len(lane_inbox.open_rows(lane, log)),
            "open_questions": len(lane_inbox.open_questions(lane, log)), "unread": lane_inbox.unread(lane, log)}


def _entry(line: dict) -> dict:
    return {key: value for key, value in line.items() if key != "digest"}


def _text(arguments: dict, key: str, default: str | None = None) -> str:
    value = arguments.get(key, default)
    if value is None:
        raise Refused(f"{key} is required")
    if not isinstance(value, str):
        raise Refused(f"{key} must be text")
    return value


def _list(arguments: dict, key: str) -> list:
    value = arguments.get(key, [])
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        raise Refused(f"{key} must be a list of objects")
    return value


def _strings(arguments: dict, key: str) -> list[str]:
    value = arguments.get(key, [])
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise Refused(f"{key} must be a list of text")
    return value


def _count(arguments: dict, key: str) -> int | None:
    value = arguments.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise Refused(f"{key} must be a whole number of 0 or more")
    return value
