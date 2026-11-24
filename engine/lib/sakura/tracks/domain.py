"""Track records and mutation refusals shared by engine adapters."""

from dataclasses import dataclass
from enum import StrEnum
from typing import Literal

NAME_LIMIT = 80
GOAL_LIMIT = 500
SUMMARY_LIMIT = 4000
DECISION_LIMIT = 500
ANSWER_LIMIT = 2000
TITLE_LIMIT = 120
OPERATION_LIMIT = 256
CONVERSATION_PART_LIMIT = 256

TrackStatus = Literal["open", "paused", "closed"]


class RefusalCode(StrEnum):
    """Stable reason codes for expected Track action failures."""

    INVALID_INPUT = "invalid_input"
    FORBIDDEN = "forbidden"
    OPERATION_CONFLICT = "operation_conflict"
    NOT_FOUND = "not_found"
    NAME_IN_USE = "name_in_use"
    CLOSED = "closed"
    OPEN_LANES = "open_lanes"
    DECISION_SETTLED = "decision_settled"
    LANE_UNAVAILABLE = "lane_unavailable"


class Refusal(ValueError):
    """A rejected Track action with a safe message and optional display labels."""

    def __init__(self, code: RefusalCode, message: str, labels: tuple[str, ...] = ()) -> None:
        super().__init__(message)
        self.code = code
        self.labels = labels


class LaneState(StrEnum):
    """Lifecycle facts from the saved flow task."""

    LIVE = "live"
    FINISHED = "finished"
    GONE = "gone"
    UNREADABLE = "unreadable"


@dataclass(frozen=True)
class ProjectLink:
    """A project's canonical identity and its saved display label."""

    id: str
    folder: str
    name: str


@dataclass(frozen=True)
class LaneLink:
    """Membership and the fallback title for a flow task."""

    track_id: str
    project_id: str
    task: str
    title: str
    start_op: str | None = None


@dataclass(frozen=True)
class ConversationKey:
    """The identity of a persisted Master conversation across tab lifetimes."""

    provider: str
    account: str
    session: str


@dataclass(frozen=True)
class ConversationLink:
    """A person's explicit link to a started Master conversation."""

    track_id: str
    key: ConversationKey
    title: str


@dataclass(frozen=True)
class Decision:
    """A numbered question and its first settlement, if any."""

    track_id: str
    number: int
    text: str
    state: Literal["open", "settled"]
    answer: str | None
    created_at: str
    settled_at: str | None


@dataclass(frozen=True)
class Track:
    """A Track and, on collection reads, its related records."""

    id: str
    name: str
    goal: str
    status: TrackStatus
    summary: str
    created_at: str
    updated_at: str
    projects: tuple[ProjectLink, ...] = ()
    lanes: tuple[LaneLink, ...] = ()
    conversations: tuple[ConversationLink, ...] = ()
    decisions: tuple[Decision, ...] = ()


@dataclass(frozen=True)
class AuditEntry:
    """One attributed change committed alongside the record and replay result."""

    seq: int
    track_id: str
    at: str
    actor: str
    actor_class: str
    actor_session: str | None
    place: str | None
    operation: str
    op_id: str
    before: dict
    after: dict
