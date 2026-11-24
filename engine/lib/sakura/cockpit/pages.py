"""The cockpit's pages as HTML strings. `text` is the only way a value reaches a page.

Nothing here composes HTML from repository data, agent output or request fields any other way: every value is
stripped of terminal controls and then escaped. There is no script, no inline style and no external resource.
"""

from __future__ import annotations

import html
import json
import re
import shlex
from datetime import UTC, datetime
from pathlib import Path
from uuid import uuid4

from sakura.cockpit import (
    INPUT_PREFIX,
    account_actions,
    actions,
    host_actions,
    proposal_actions,
    remote,
    track_actions,
    wait_actions,
)
from sakura.cockpit.account_actions import AccountTarget
from sakura.cockpit.actions import Action, ActionResult, Invocation, Target
from sakura.cockpit.gate import Identity
from sakura.cockpit.items import GroupRow, Item
from sakura.cockpit.snapshot import Snapshot
from sakura.cockpit.text import (
    DEFERRED_EXPLANATION,
    account_line,
    from_line,
    progress_words,
    provider_line,
)
from sakura.cockpit.wait_actions import WaitTarget
from sakura.flow import checks, owner_marks, waits
from sakura.flow.render import strip_controls
from sakura.tracks.domain import ANSWER_LIMIT

STYLESHEET = Path(__file__).with_name("style.css")
REFRESH_SECONDS = 10
EVENTS_SHOWN = 20
TEXT_ROWS = 3
INBOX_SECTIONS = ((wait_actions.NEEDS_YOU, "Needs you"), (wait_actions.TRIAGE, "Triage"))
STATE_GLYPHS = {wait_actions.OPEN: "○", "done": "✓"}
SNOOZE_PRESETS = (("1h", "1 hour"), ("tomorrow", "Tomorrow"), ("nextweek", "Next week"))
DIGEST_KEY = "G"
VERDICTS = (("ok", "Ok"), ("ko", "Ko"))
REPLY_IN_WORDS = "Reply in words"
HTTPS = "https://"
OUTSIDE_SOURCE = "outside"
STEP_LINE = re.compile(r"\d+\. (.*)")


def text(value: object) -> str:
    """Return `value` as text safe inside HTML content and quoted attributes."""
    value = strip_controls(str(value))
    return html.escape(remote.text(value) if remote.VIEW_IDS.get() else value, quote=True)


# Pages

def index(snapshot: Snapshot, identity: Identity, csrf: str) -> str:
    """The overview: header, one section per lane group, one card per item, the latest events."""
    lanes = _lanes_by_group(snapshot)
    body = [_header(snapshot, identity),
            _inbox(snapshot, csrf),
            *_group_section(snapshot, csrf, lanes),
            *_flow_section([item for item in snapshot.items if item.kind == "flow"], csrf),
            *_outside_section([item for item in snapshot.items if item.kind == "outside"], csrf),
            *([] if remote.VIEW_IDS.get() else _events_section(snapshot.events))]
    return _document("sakura cockpit", body, refresh=True)


def confirm(action: Action, target: Target, invocation: Invocation, inputs: dict, csrf: str, proof: str) -> str:
    """Ask before a confirmed action: the exact command and a form that carries the one-time proof."""
    hidden = _hidden(csrf=csrf, key=action.key, target=_target_field(target), proof=proof,
                     **{INPUT_PREFIX + name: value for name, value in inputs.items()})
    body = [f"<main><h1>Confirm: {text(action.label)}</h1>",
            f"<p>on {text(_label(target))}</p>", _command_block(invocation, target),
            f'<form method="post" action="/action">{hidden}<button type="submit">Confirm</button></form>',
            '<p><a href="/">Cancel</a></p></main>']
    return _document("confirm", body)


def result(done: ActionResult) -> str:
    """What a run did: the exact command, its exit code and its output."""
    state = "done" if done.exit_code == 0 else "failed"
    body = [f"<main><h1>{state}: exit code {text(done.exit_code)}</h1>", _command_block(done.invocation),
            f"<pre>{text(done.output)}</pre>", *([f"<p>audit {text(done.audit)}</p>"] if done.audit else []),
            '<p><a href="/">Back to the cockpit</a></p></main>']
    return _document("result", body)


def compose(action: Action, target: Target, csrf: str) -> str:
    """The page for an answer in words: the lane or wait it goes to, one text field, and no refresh.

    The index refreshes itself, which would wipe what is typed, so it carries no text field of its own.
    """
    hidden = _hidden(csrf=csrf, key=action.key, target=_target_field(target))
    field = _field("text", True)
    body = [f"<main><h1>{text(action.label)}</h1>", *_compose_subject(target),
            f'<form method="post" action="/action">{hidden}{field}<button type="submit">Send</button></form>',
            '<p><a href="/">Cancel</a></p></main>']
    return _document("compose", body)


def wait_view(snapshot: Snapshot, wait: waits.Wait, csrf: str) -> str:
    """The selected Inbox item with its properties and actions."""
    target = snapshot.wait_target(wait)
    kind = wait_actions.inbox_kind(wait).capitalize()
    links = "".join(f"<p>{_link(label, url)}</p>" for label, url in wait.links)
    body = [f"<main><h1>{_state(wait_actions.OPEN)} {text(wait.title)}</h1>",
            _wait_detail(wait), links, _properties(wait, kind),
            _wait_form(target, csrf), _attention_forms(target, csrf),
            '<p><a href="/">Back to Inbox</a></p></main>']
    return _document(wait.title, body)


def refused(reason: str) -> str:
    """A request the action layer or the gate of the confirmation turned down."""
    body = [f"<main><h1>refused</h1><p>{text(reason)}</p>", '<p><a href="/">Back to the cockpit</a></p></main>']
    return _document("refused", body)


# Skeleton

def _document(title: str, body: list[str], *, refresh: bool = False) -> str:
    reload = f'<meta http-equiv="refresh" content="{REFRESH_SECONDS}">' if refresh else ""
    head = ('<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">'
            f'{reload}<title>{text(title)}</title><link rel="stylesheet" href="/style.css">')
    return f'<!doctype html><html lang="en"><head>{head}</head><body>{"".join(body)}</body></html>\n'


def _header(snapshot: Snapshot, identity: Identity) -> str:
    load = snapshot.load
    lines = [(f"<h1>sakura cockpit</h1><p>mode: {text(snapshot.mode)}, slots {text(load['slots_in_use'])}/"
              f"{text(load['slots_total'])}, queue {text(len(load['queue']))}</p>")]
    lines += [f"<p>{text(provider_line(entry))}</p>" for entry in snapshot.usage.providers]
    lines += [f"<p>{text(account_line(row))}</p>" for row in snapshot.accounts]
    lines += [f'<p class="problem">{text(problem)}</p>' for problem in snapshot.problems]
    if snapshot.deferred:
        lines.append(_deferred(snapshot.deferred))
    lines.append(f"<p><small>signed in as {text(identity.login)} on {text(identity.device)}</small></p>")
    return f"<header>{''.join(lines)}</header>"


def _deferred(entries: list[dict]) -> str:
    rows = "".join(f"<li>{text(_join(e.get('at'), e.get('task'), e.get('event'), e.get('message')))}</li>"
                   for e in entries)
    return f"<section><h2>{text(len(entries))} {text(DEFERRED_EXPLANATION)}</h2><ul>{rows}</ul></section>"


def _inbox(snapshot: Snapshot, csrf: str) -> str:
    waiting = snapshot.visible_waits
    groups = "".join(_inbox_section(section, title, waiting, snapshot.usage.now, csrf)
                     for section, title in INBOX_SECTIONS)
    snoozed = _inbox_section(None, "Snoozed", [wait for wait in snapshot.waits if wait.is_snoozed],
                             snapshot.usage.now, csrf)
    empty = _inbox_empty(snapshot) if not snapshot.waits else ""
    digest = _form(actions.ACTIONS[DIGEST_KEY], host_actions.DIGEST, csrf)
    return f"<section><h2>Inbox</h2>{empty}{groups}{snoozed}{_done(snapshot.decided)}{digest}</section>"


def _inbox_section(section: str | None, title: str, waiting: list[waits.Wait], now: datetime, csrf: str) -> str:
    groups: dict[str, tuple[str, list[waits.Wait]]] = {}
    for wait in waiting:
        if section is None or wait_actions.inbox_section(wait) == section:
            group = wait_actions.inbox_group(wait)
            groups.setdefault(group["id"], (group["label"], []))[1].append(wait)
    if not groups:
        return ""
    rendered = []
    for label, rows in groups.values():
        contents = "".join(_wait_row(wait, label, now, csrf) for wait in rows)
        rendered.append(f"<div><h4>{text(label)}</h4>{contents}</div>")
    return f"<div><h3>{text(title)}</h3>{''.join(rendered)}</div>"


def _wait_row(wait: waits.Wait, label: str, now: datetime, csrf: str) -> str:
    kind = wait_actions.inbox_kind(wait).capitalize()
    target = wait_actions.target_of(wait)
    hidden = _hidden(csrf=csrf, key="read", target=_target_field(target))
    unread = '<span class="unread-dot" role="img" aria-label="Unread"></span>' if not wait.is_read else ""
    row_class = "inbox-row inbox-row-read" if wait.is_read else "inbox-row"
    unsnooze = _form(actions.ACTIONS["unsnooze"], target, csrf) if wait.is_snoozed else ""
    return (f'<article class="{row_class}"><form method="post" action="/action">{hidden}'
            f'<button type="submit">{_state(wait_actions.OPEN)} {unread}{text(wait.title)}</button></form>'
            f'<small>{text(kind)}, {text(label)}</small>{_relative_time(wait.since, now)}{unsnooze}</article>')


def _relative_time(since: str | None, now: datetime) -> str:
    if not since:
        return ""
    try:
        moment = datetime.fromisoformat(since)
    except ValueError:
        return ""
    minutes = max(0, int((now - moment.replace(tzinfo=moment.tzinfo or UTC)).total_seconds()) // 60)
    if minutes >= 1440:
        label = f"{minutes // 1440}d"
    elif minutes >= 60:
        label = f"{minutes // 60}h"
    else:
        label = f"{minutes}m" if minutes else "Now"
    return f'<time datetime="{text(since)}">{text(label)}</time>'


def _state(resolution: str) -> str:
    return f'<span data-resolution="{text(resolution)}" aria-label="{text(resolution)}">{STATE_GLYPHS[resolution]}</span>'


def _properties(wait: waits.Wait, kind: str) -> str:
    props = [("Kind", kind), ("Lane" if wait.lane_title else "Source", wait_actions.inbox_group(wait)["label"]),
             ("Project", wait.project if wait_actions.inbox_section(wait) != wait_actions.TRIAGE else ""),
             ("Since", wait.since)]
    rows = "".join(f"<dt>{text(label)}</dt><dd>{text(value)}</dd>" for label, value in props if value)
    origin = f'<p class="from">{text(from_line(wait.author, wait.project))}</p>' if wait.source == OUTSIDE_SOURCE else ""
    return f"<h2>Properties</h2><dl>{rows}</dl>{origin}"


def _inbox_empty(snapshot: Snapshot) -> str:
    action = "Start a lane" if snapshot.projects else "Add a project"
    return (f"<p>Questions, approvals, reviews, blocked lanes and proposals appear here when they need you.</p>"
            f"<p>{action} in Sakura on this Mac.</p>")


def _done(entries: list[dict]) -> str:
    rows = []
    for entry in entries:
        place = "GitHub" if entry.get("place") == "github" else str(entry.get("place") or "").capitalize()
        lane = f', Lane {text(entry["lane"])}' if entry.get("lane") else ""
        rows.append(f'<li>{_state("done")} {text(entry.get("title") or "")}'
                    f'<br><small>{text(entry.get("actor") or "")}, {text(place)}{lane}</small></li>')
    return f"<div><h3>Done</h3><ul>{''.join(rows)}</ul></div>" if rows else ""


def _wait_detail(wait: waits.Wait) -> str:
    if not wait.detail or (remote.VIEW_IDS.get() and wait.kind == "scheduled_approval"):
        return ""
    if wait.source == OUTSIDE_SOURCE and wait.kind == waits.OWNER_ROW:
        return _row_detail(wait.detail)
    return f'<p class="detail">{text(wait.detail)}</p>'


def _row_detail(detail: str) -> str:
    """An outside owner row's text (`outside/waits.py`): numbered steps as a list, then Expected and Note lines."""
    lines = detail.split("\n")
    steps = [match[1] for match in map(STEP_LINE.fullmatch, lines) if match]
    others = [line for line in lines if not STEP_LINE.fullmatch(line)]
    listing = f"<ol>{''.join(f'<li>{text(step)}</li>' for step in steps)}</ol>" if steps else ""
    return listing + "".join(f"<p>{text(line)}</p>" for line in others)


def _wait_form(target: WaitTarget, csrf: str) -> str:
    if wait_actions.inbox_kind(target.wait) == waits.PROPOSAL:
        return _start_proposal_form(target, csrf) + _form(actions.ACTIONS["dismiss"], target, csrf)
    if target.wait_kind in (waits.REVIEW, waits.BLOCKED):
        return "".join(_form(action, target, csrf) for action in actions.applicable(target)
                       if action.key in (*wait_actions.BLOCKED_ACTIONS, "markreviewed"))
    if target.wait_kind == waits.OWNER_ROW:
        mark = "" if remote.VIEW_IDS.get() else _mark_form(target, csrf)
        return mark + (_restore_form(target, csrf) if target.wait.head_moved else "")
    if target.wait_kind == waits.TRACK_DECISION:
        return _track_answer_form(target, csrf)
    if target.wait_kind == waits.QUESTION:
        return _question_forms(target, csrf)
    if target.wait_kind == waits.APPROVAL:
        return "<p>Approve in Sakura on this Mac.</p>" if remote.VIEW_IDS.get() else _approve_form(target, csrf)
    if all(wait_actions.allows(target, key) for key in ("y", "N")):
        return _proposal_forms(target, csrf)
    return ""


def _attention_forms(target: WaitTarget, csrf: str) -> str:
    key = "unread" if target.wait.is_read else "read"
    mark = _form(actions.ACTIONS[key], target, csrf)
    if target.wait.is_snoozed:
        return mark + _form(actions.ACTIONS["unsnooze"], target, csrf)
    hidden = _hidden(csrf=csrf, key="snooze", target=_target_field(target))
    options = "".join(f'<option value="{text(value)}">{text(label)}</option>' for value, label in SNOOZE_PRESETS)
    return (f'{mark}<form method="post" action="/action">{hidden}'
            f'<label>Snooze until <select name="{INPUT_PREFIX}until">{options}</select></label>'
            '<button type="submit">Snooze</button></form>')


def _start_proposal_form(target: WaitTarget, csrf: str) -> str:
    if not target.projects:
        return "<p>Add a project in Sakura on this Mac to start a lane.</p>"
    defaults = proposal_actions.defaults(target.wait, target.projects)
    hidden = _hidden(csrf=csrf, key="start", target=_target_field(target), **{INPUT_PREFIX + "op": defaults["op"]})
    projects = ['<option value="">Choose a project</option>']
    ids = remote.VIEW_IDS.get()
    for project in target.projects:
        selected = " selected" if str(project) == defaults["project"] else ""
        reference = ids.register(("project", str(project))) if ids else str(project)
        projects.append(f'<option value="{text(reference)}"{selected}>{text(project.name)}</option>')
    fields = "".join(_field(name, False, defaults[name] or "") for name in (("title",) if ids else ("title", "account")))
    return (f'<form method="post" action="/action">{hidden}'
            f'<label>Project <select name="{INPUT_PREFIX}project" required>{"".join(projects)}</select></label>'
            f'{fields}<button type="submit">Start a lane</button></form>')


def _restore_form(target: WaitTarget, csrf: str) -> str:
    hidden = _hidden(csrf=csrf, key=wait_actions.RESTORE_KEY, target=_target_field(target))
    return (f'<form method="post" action="/action">{hidden}<p>The workspace is not on the engine\'s head, so a mark '
            f'would not count.</p><button type="submit">{text(actions.ACTIONS[wait_actions.RESTORE_KEY].label)}'
            "</button></form>")


def _mark_form(target: WaitTarget, csrf: str) -> str:
    hidden = _hidden(csrf=csrf, key="k", target=_target_field(target))
    buttons = "".join(f'<button type="submit" name="{INPUT_PREFIX}verdict" value="{value}">{text(label)}</button>'
                      for value, label in VERDICTS)
    note = "" if target.is_outside else _field("note", False)  # an outside row is one press
    return f'<form method="post" action="/action">{hidden}{note}{buttons}</form>'


def _track_answer_form(target: WaitTarget, csrf: str) -> str:
    hidden = _hidden(csrf=csrf, key=track_actions.SETTLE, target=_target_field(target),
                     **{INPUT_PREFIX + "op": uuid4().hex})
    answer = _field("answer", True, textarea_limit=ANSWER_LIMIT)
    return (f'<form method="post" action="/action?{track_actions.PHONE_FORM_QUERY}">{hidden}{answer}'
            '<button type="submit">Answer decision</button></form>')


def _question_forms(target: WaitTarget, csrf: str) -> str:
    hidden = _hidden(csrf=csrf, key="r", target=_target_field(target))
    buttons = "".join(f'<button type="submit" name="{INPUT_PREFIX}choice" value="{text(choice)}">{text(choice)}</button>'
                      for choice in target.wait.choices)
    choices = f'<form method="post" action="/action">{hidden}{buttons}</form>' if buttons else ""
    words = f'<button type="submit">{text(REPLY_IN_WORDS)}</button>'
    return f'{choices}<form method="post" action="/action">{hidden}{words}</form>'


def _approve_form(target: WaitTarget, csrf: str) -> str:
    hidden = _hidden(csrf=csrf, key="a", target=_target_field(target))
    remember = (f'<label><input type="checkbox" name="{INPUT_PREFIX}remember" value="yes"> '
                "remember these commands for later contracts</label>") if target.subject == checks.APPROVAL_GATE else ""
    return (f'<form method="post" action="/action">{hidden}{remember}'
            f'<button type="submit">{text(actions.ACTIONS["a"].label)}</button></form>')


def _proposal_forms(target: WaitTarget, csrf: str) -> str:
    return "".join(f'<form method="post" action="/action">{_hidden(csrf=csrf, key=key, target=_target_field(target))}'
                   f'<button type="submit">{text(actions.ACTIONS[key].label)}</button></form>' for key in ("y", "N")
                   if not remote.VIEW_IDS.get() or not actions.is_local_only(key, target.key))


def _group_section(snapshot: Snapshot, csrf: str, lanes: dict[str, list[Item]]) -> list[str]:
    if not snapshot.groups and not lanes:
        return []
    known = {row.name for row in snapshot.groups}
    sections = [_group(row, csrf, lanes.get(row.name, [])) for row in snapshot.groups]
    sections += [_group(GroupRow(name, Path("."), "", len(found), "", ""), csrf, found, forms=False)
                 for name, found in lanes.items() if name not in known]
    return sections


def _group(row: GroupRow, csrf: str, lanes: list[Item], *, forms: bool = True) -> str:
    problem = f'<p class="problem">{text(row.problem)}</p>' if row.problem else ""
    heading = f"<h2>{text(row.name)} <small>{text(row.lanes)} lanes</small></h2>"
    controls = "".join(_forms(row, csrf)) if forms else ""
    return f"<section>{heading}{problem}{controls}{''.join(_card(lane, csrf) for lane in lanes)}</section>"


def _flow_section(items: list[Item], csrf: str) -> list[str]:
    if not items:
        return []
    return [f"<section><h2>Flow tasks</h2>{''.join(_card(item, csrf) for item in items)}</section>"]


def _outside_section(items: list[Item], csrf: str) -> list[str]:
    if not items:
        return []
    return [f"<section><h2>Outside lanes</h2>{''.join(_outside_card(item, csrf) for item in items)}</section>"]


def _outside_card(item: Item, csrf: str) -> str:
    facts = [("progress", progress_words(item.progress) if item.progress else ""),
             ("repository", item.repo), ("unread", f"{item.unread} not read yet" if item.unread else "")]
    rows = "".join(f"<dt>{text(name)}</dt><dd>{text(value)}</dd>" for name, value in facts if str(value))
    heading = f'<h3>{text(item.title)} <span class="state">{text(item.agent_state)}</span></h3>'
    origin = f'<p class="from">{text(from_line(item.author, item.repo.name))}</p>'
    links = "".join(f"<p>{_link(label, url)}</p>" for label, url in item.links)
    return f"<article>{heading}{origin}<dl>{rows}</dl>{links}{''.join(_forms(item, csrf))}</article>"


def _link(label: str, url: str) -> str:
    """An https URL as a link that sends no referrer; anything else (the engine never stores one) as plain text."""
    if url.startswith(HTTPS):
        return f'<a href="{text(url)}" rel="noreferrer">{text(label)}</a>'
    return f"{text(label)} {text(url)}"


def _events_section(events: list[dict]) -> list[str]:
    if not events:
        return []
    rows = "".join(f"<li>{text(_join(e.get('at'), e.get('source'), e.get('scope')))}: {text(e.get('text', ''))}</li>"
                   for e in events[-EVENTS_SHOWN:])
    return [f"<section><h2>Events</h2><ul>{rows}</ul></section>"]


# Cards and forms

def _card(item: Item, csrf: str) -> str:
    facts = [("progress", progress_words(item.progress) if item.progress else ""),
             ("repository", item.repo), ("worktree", item.worktree or ""), ("branch", item.branch),
             ("last commit", item.last_commit), ("stack", item.stack), ("pull request", item.pr),
             ("models", _join(*(f"{role}={model}" for role, model in item.models.items()))),
             ("pending", ", ".join(item.pending)), ("tokens", f"{item.usage.total:,}" if item.usage else ""),
             ("next", item.next_command)]
    rows = "".join(f"<dt>{text(name)}</dt><dd>{text(value)}</dd>" for name, value in facts if str(value))
    heading = f'<h3>{text(_label(item))} <span class="state">{text(item.agent_state)}</span></h3>'
    return f"<article>{heading}<dl>{rows}</dl>{''.join(_forms(item, csrf))}</article>"


def _forms(target: Target, csrf: str) -> list[str]:
    return [_form(action, target, csrf) for action in actions.applicable(target)]


def _form(action: Action, target: Target, csrf: str) -> str:
    if remote.VIEW_IDS.get() and actions.is_local_only(action.key, target.key):
        return ""
    needed = [name for name in action.needs(target) if name != "text"]  # a message has a page of its own (compose)
    fields = [_field(name, True) for name in needed]
    fields += [_field(name, False, textarea_limit=owner_marks.MAX_NOTE if action.key == "R" and name == "note" else 0)
               for name in action.optional]
    hidden = _hidden(csrf=csrf, key=action.key, target=_target_field(target))
    return (f'<form method="post" action="/action">{hidden}{"".join(fields)}'
            f'<button type="submit">{text(action.label)}</button></form>')


def _field(name: str, is_required: bool, value: str = "", *, textarea_limit: int = 0) -> str:
    required = " required" if is_required else ""
    label = text(name if is_required else f"{name} (optional)")
    if name == "text" or textarea_limit:
        limit = textarea_limit or actions.MAX_TEXT
        control = (f'<textarea name="{INPUT_PREFIX}{text(name)}" rows="{TEXT_ROWS}" '
                   f'maxlength="{limit}"{required}></textarea>')
    else:
        control = (f'<input type="text" name="{INPUT_PREFIX}{text(name)}" maxlength="{actions.MAX_LINE}" '
                   f'value="{text(value)}" autocomplete="off" autocapitalize="off"{required}>')
    return f"<label>{label} {control}</label>"


def _hidden(**fields: str) -> str:
    values = ((text(name), html.escape(strip_controls(str(value)), quote=True)) for name, value in fields.items())
    return "".join(f'<input type="hidden" name="{name}" value="{value}">' for name, value in values)


def _target_field(target: Target) -> str:
    ids = remote.VIEW_IDS.get()
    return ids.register(target.key) if ids else json.dumps(list(target.key))


def _command_block(invocation: Invocation, target: Target | None = None) -> str:
    if remote.VIEW_IDS.get():
        return ""
    if isinstance(target, AccountTarget):  # no install path, no working directory
        return f"<p>command:</p><pre><code>{text(account_actions.shown_command(invocation.argv))}</code></pre>"
    return (f"<p>command, run in {text(invocation.cwd)}:</p>"
            f"<pre><code>{text(shlex.join(invocation.argv))}</code></pre>")


# Small helpers

def _lanes_by_group(snapshot: Snapshot) -> dict[str, list[Item]]:
    grouped: dict[str, list[Item]] = {}
    for item in snapshot.items:
        if item.kind == "lane":
            grouped.setdefault(item.key[1], []).append(item)
    return grouped


def _label(target: Target) -> str:
    if isinstance(target, GroupRow):
        return f"group {target.name}"
    if target.kind == "outside":
        return f"outside/{target.key[1]}"
    if target.kind == host_actions.KIND_HOST:
        return target.name
    scope = target.repo.name if target.kind in ("flow", wait_actions.KIND_WAIT) else target.key[1]
    return f"{scope}/{target.key[2]}"


def _compose_subject(target: Target) -> list[str]:
    if isinstance(target, WaitTarget):
        return [f"<p><strong>{text(target.wait.title)}</strong></p>",
                f'<p class="from">{text(from_line(target.wait.author, target.wait.project))}</p>'
                if target.wait.source == OUTSIDE_SOURCE else ""]
    title = target.title if target.kind == "outside" else _label(target)
    return [f"<p>to {text(title)}</p>"]


def _join(*values: object) -> str:
    return " ".join(str(value) for value in values if value)
