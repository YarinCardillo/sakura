"""Send text, or a /model command, to the agent pane of a manual lane; the only tmux input sakura sends."""

from __future__ import annotations

import fcntl
import os
import re
import time
from collections.abc import Callable
from pathlib import Path

from sakura.flow.render import strip_controls
from sakura.flow.tmux import TmuxClient, paste_refusal
from sakura.lanes import lane, status, transcripts
from sakura.lanes.config import LaneGroup
from sakura.lanes.errors import InputInvalid, InputRefused
from sakura.lanes.identity import LaneIdentity
from sakura.lanes.transcripts import Sources

MAX_TEXT = 8000
PASTE_SETTLE_SECONDS = 0.5
MODEL_CONFIRM_SECONDS = 10
MODEL_POLL_SECONDS = 0.5
CONFIRMED = "confirmed"
NOT_CONFIRMED = "sent, not confirmed"
MESSAGE_LOG = "~/.local/state/tmux-agents/messages.log"

MODEL_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:\[\]-]{0,79}")
_WAITING = "waiting"


def say(identity: LaneIdentity, group: LaneGroup, text: str, client: TmuxClient, *,
        sleep: Callable[[float], None] = time.sleep, messages_log: Path | None = None) -> None:
    """Paste `text` into the lane's agent pane and press Enter.

    Raises InputInvalid for text that is empty, over MAX_TEXT or holds control characters, and InputRefused
    unless the lane has exactly one agent pane and it is waiting. Nothing is sent in either case.
    """
    _check_text(text)
    pane = _waiting_agent_pane(identity, group, client)
    _deliver(identity, pane, text, client, sleep, messages_log)


def model(identity: LaneIdentity, group: LaneGroup, name: str, client: TmuxClient, *,
          sources: Sources | None = None, sleep: Callable[[float], None] = time.sleep,
          messages_log: Path | None = None) -> str:
    """Send `/model <name>` to a waiting Claude lane; return `confirmed` or `sent, not confirmed`.

    Confirmed means the lane's transcript shows the change within MODEL_CONFIRM_SECONDS. Raises InputInvalid
    for an unusable name and InputRefused for a lane that is not a waiting Claude lane.
    """
    sources = sources or Sources()
    _check_model_name(name)
    pane = _waiting_agent_pane(identity, group, client)
    record = transcripts.read_record(client, pane, sources)
    if not transcripts.is_current(record, identity.worktree):
        raise InputRefused(f"cannot tell which agent runs in lane {identity.window_marker}: no current pane record")
    if record.get("tool") != transcripts.CLAUDE_TOOL:
        raise InputRefused(f"lane model supports Claude lanes only; {identity.window_marker} runs "
                           f"{strip_controls(str(record.get('tool')))}")
    transcript = transcripts.locate(record, identity.worktree, sources)
    since = time.time()
    _deliver(identity, pane, f"/model {name}", client, sleep, messages_log)
    return CONFIRMED if transcript and _confirmed(transcript, name, since, sleep) else NOT_CONFIRMED


def _check_text(text: str) -> None:
    if not text:
        raise InputInvalid("text is empty")
    reason = paste_refusal(text)
    if reason:
        raise InputInvalid(reason)
    if len(text) > MAX_TEXT:
        raise InputInvalid(f"text is {len(text)} characters; the limit is {MAX_TEXT}")


def _check_model_name(name: str) -> None:
    reason = paste_refusal(name) if name else "the name is empty"
    if reason:
        raise InputInvalid(f"invalid model name: {reason}")
    if not MODEL_NAME.fullmatch(name):
        raise InputInvalid("invalid model name: use letters, digits and . _ : [ ] -, at most 80 characters")


def _waiting_agent_pane(identity: LaneIdentity, group: LaneGroup, client: TmuxClient) -> str:
    window = next((w for w in lane.marked_windows(client, group)
                   if w.options.get(lane.LANE_MARKER_KEY) == identity.window_marker), None)
    if window is None:
        raise InputRefused(f"no open lane {identity.window_marker}")
    panes = lane.agent_panes(window)
    if len(panes) != 1:
        raise InputRefused(f"lane {identity.window_marker} has {len(panes)} agent panes; expected exactly one")
    state = status.agent_state(client.pane_title(panes[0].id))
    if state != _WAITING:
        raise InputRefused(f"the agent of lane {identity.window_marker} is {state}, not {_WAITING}")
    return panes[0].id


def _deliver(identity: LaneIdentity, pane: str, text: str, client: TmuxClient,
             sleep: Callable[[float], None], messages_log: Path | None) -> None:
    client.paste(pane, text)
    sleep(PASTE_SETTLE_SECONDS)
    client.send_enter(pane)
    _log_message(identity, pane, text, messages_log)


def _confirmed(transcript: Path, name: str, since: float, sleep: Callable[[float], None]) -> bool:
    for _ in range(int(MODEL_CONFIRM_SECONDS / MODEL_POLL_SECONDS)):
        if transcripts.model_change_seen(transcript, name, since):
            return True
        sleep(MODEL_POLL_SECONDS)
    return transcripts.model_change_seen(transcript, name, since)


def _log_message(identity: LaneIdentity, pane: str, text: str, messages_log: Path | None) -> None:
    """Append the message in the format of `ask-codex`, which shares this log."""
    sender = strip_controls(os.environ.get("TMUX_PANE", "-"))
    stamp = time.strftime("%F %T")
    entry = f"=== {stamp} {sender} -> lane {identity.window_marker} ({pane})\n{strip_controls(text)}\n\n"
    target = Path(messages_log or MESSAGE_LOG).expanduser()
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a", encoding="utf-8") as log:
        fcntl.flock(log, fcntl.LOCK_EX)
        try:
            log.write(entry)
        finally:
            fcntl.flock(log, fcntl.LOCK_UN)
