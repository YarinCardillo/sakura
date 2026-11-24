"""Project revision-bound scheduled approvals into Inbox."""

import json

from sakura.scheduled.store import Refused, Store

APPROVAL = "scheduled_approval"
LABEL = "Scheduled"


def pending_waits() -> list:
    """Read pending approvals with the definition presented in full."""
    from sakura.flow.waits import Wait, bound_key

    try:
        pending = Store().approvals()
    except Refused:
        return []
    return [Wait(json.dumps(["wait", "/", proposal.id, APPROVAL, bound_key(proposal.id, proposal.revision)],
                           separators=(",", ":")), APPROVAL, "/", LABEL, proposal.id,
                 proposal.definition["title"], proposal.preview, proposal.created_at, source="scheduled",
                 author=proposal.author) for proposal in pending]
