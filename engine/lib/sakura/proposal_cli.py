"""`sakura proposal accept|reject <id> <digest>`: the person's decision on an agent's proposal for a knowledge folder.

Only the person runs these. An engine role is refused by its environment and by the cage itself (it cannot write the
engine's trust root), the same two checks every operator command makes. The folders come from the settings of the
project the proposal was made for, checked again now, so an accept never writes into a folder that has become unsafe.
"""

from __future__ import annotations

import argparse
import os
from datetime import datetime, timezone
from pathlib import Path

from sakura import actors, engine_home, knowledge, proposals
from sakura.cockpit import decided
from sakura.flow import brakes, cage, config, exits, knowledge_folders, waits

SANDBOX_REFUSAL = "operator commands cannot run inside an agent cage: the engine home is not writable here"
USAGE_FAILURES = (proposals.ProposalRefused, knowledge.KnowledgeRefused, config.FlowConfigError, actors.ActorError,
                  engine_home.EngineHomeError)


def main(argv: list[str]) -> int:
    """Run `sakura proposal <accept|reject> [--actor A] -- ID`; 2 for a refusal (printed), 0 when decided."""
    parser = argparse.ArgumentParser(prog="sakura proposal")
    parser.add_argument("verb", choices=("accept", "reject"))
    parser.add_argument("--actor")
    parser.add_argument("proposal")
    parser.add_argument("digest")
    args = parser.parse_args(argv)
    try:
        _require_operator()
        by = actors.format(actors.resolve(args.actor))
        asked = _ask_of(proposals.waiting(engine_home.root(), args.proposal, args.digest))
        found = _decide(args.verb, args.proposal, args.digest, by)
        decided.record_now(asked, args.verb, by)
    except USAGE_FAILURES as refusal:
        print(refusal)
        return exits.USAGE
    print(f"{args.verb}ed proposal {found.id}: {found.path} in the {found.folder} knowledge folder")
    return exits.OK


def _decide(verb: str, proposal_id: str, digest: str, by: str) -> proposals.Proposal:
    root, now = engine_home.root(), datetime.now(timezone.utc)
    if verb == "reject":
        return proposals.reject(root, proposal_id, digest, by, now)
    values = config.load(Path(proposals.waiting(root, proposal_id, digest).repo)).values
    return proposals.accept(root, proposal_id, digest, knowledge_folders.of(values, cage.account_home()), by, now)


def _ask_of(pending: proposals.Proposal) -> dict:
    """The ask the person is deciding, as the decided-asks log keeps it: the engine's own record while it is readable."""
    repo, name = Path(pending.repo), waits.bound_key(pending.id, pending.digest)
    return decided.find_wait(repo, [], waits.PROPOSAL, pending.id) or decided.fallback_wait(repo, pending.task, waits.PROPOSAL, name)


def _require_operator() -> None:
    if os.environ.get(brakes.ROLE_ENV):
        raise proposals.ProposalRefused(brakes.ENGINE_ROLE_REFUSAL)
    if engine_home.sandboxed():
        raise proposals.ProposalRefused(SANDBOX_REFUSAL)
