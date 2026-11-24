"""Verify that an engine-attested replacement preserves authority and read progress."""

from __future__ import annotations

from dataclasses import asdict, dataclass, replace

from sakura.connectors import contract, kinds
from sakura.digest import state


@dataclass(frozen=True)
class Coverage:
    """Trusted adapter facts about the implementation and its effective authority."""

    principal: str
    resources: tuple[str, ...]
    item_types: tuple[str, ...]
    selection: str
    permissions: tuple[str, ...]
    operations: tuple[str, ...]
    events: tuple[str, ...]
    cursor_semantics: str
    deduplication: str


@dataclass(frozen=True)
class Evidence:
    """Fresh adapter coverage and a per-binding checkpoint translation."""

    original: Coverage
    candidate: Coverage
    current_checkpoints: dict[str, state.SourceState]
    translated: dict[str, state.SourceState]


def validate(original, candidate, ports: kinds.Ports) -> Evidence:
    """Require verified equal coverage and a complete cursor translation."""
    if (original.id == candidate.id or original.revoked or candidate.revoked or original.kind != kinds.PROGRAM
            or candidate.kind not in (kinds.GITHUB, kinds.LINEAR, kinds.NOTION) or not candidate.selectors):
        raise contract.Refusal("replacement_incompatible")
    if ports.prove_replacement is None:
        raise contract.Refusal("replacement_unproven")
    proof = ports.prove_replacement(original, candidate)
    if not isinstance(proof, Evidence):
        raise contract.Refusal("replacement_unproven")
    if not _matches(proof) or not _positions_match(proof):
        raise contract.Refusal("replacement_incompatible")
    access = kinds.verify_scope(candidate, kinds.KEYCHAIN_TIMEOUT_SECONDS, ports)
    if (proof.candidate.principal != access.principal or proof.candidate.resources != (access.resource,)
            or set(proof.candidate.operations) != set(kinds.operation_keys(candidate))
            or not set(proof.candidate.operations) - {kinds.READ} <= set(access.operations)):
        raise contract.Refusal("replacement_incompatible")
    if not kinds.check_instance(original, ports).is_connected:
        raise contract.Refusal("approval_required")
    return proof


def _matches(proof: Evidence) -> bool:
    before, after = proof.original, proof.candidate
    required = (before.principal, before.resources, before.item_types, before.selection, before.permissions,
                before.cursor_semantics, before.deduplication, after.cursor_semantics)
    return all(required) and replace(before, cursor_semantics=after.cursor_semantics) == after


def _positions_match(proof: Evidence) -> bool:
    if proof.current_checkpoints.keys() != proof.translated.keys():
        return False
    for source_id, before in proof.current_checkpoints.items():
        after = proof.translated[source_id]
        if before.at != after.at or (before.cursor and not after.cursor) or len(after.cursor) > state.MAX_CURSOR:
            return False
    return True


def swapped(original, candidate, evidence: Evidence):
    """Keep the original ID and attach the checkpoint translation to the new revision."""
    translated = {source_id: {"before": asdict(previous), "after": asdict(evidence.translated[source_id])}
                  for source_id, previous in evidence.current_checkpoints.items()}
    return replace(candidate, id=original.id, label=original.label, revision=original.revision + 1,
                   last_used_at=original.last_used_at, legacy_source_id=original.legacy_source_id,
                   legacy_fingerprint=original.legacy_fingerprint, revoked_at_revision=original.revoked_at_revision,
                   replacements=translated)


def checkpoint(connector, source_id: str, current: state.SourceState) -> state.SourceState:
    """Use the translated cursor only at the old position, so a progressed read never backs up."""
    translation = connector.replacements.get(source_id)
    if translation and asdict(current) == translation["before"]:
        return state.SourceState(**translation["after"])
    return current
