"""Transfer legacy digest sources to Morning summary without moving the read boundary twice."""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import asdict, dataclass

from sakura.connectors import registry
from sakura.digest import config, run, source_words, state
from sakura.flow import gates

MARKER = "morning-summary-handoff.json"
MIGRATION_ID = "morning-summary-v1"
CUSTOM_READS = "Reads items from the configured program."


@dataclass(frozen=True)
class Binding:
    """The legacy source identity, its connection and its independent read progress."""

    source_id: str
    connector_id: str
    label: str
    reads: str
    enabled: bool
    timeout: int
    checkpoint: state.SourceState

    def source(self) -> config.Source:
        """Rebuild the executable source using the same digest intake."""
        return config.Source(self.source_id, connector=self.connector_id, label=self.label, reads=self.reads,
                             enabled=self.enabled, timeout_seconds=self.timeout)


@dataclass(frozen=True)
class Handoff:
    """Saved Morning summary template and source state accepted by the scheduler."""

    id: str
    at: str
    language: str
    model: str
    account: str
    lookback_hours: int
    done_date: str
    last_run_at: str
    bindings: tuple[Binding, ...]


def stage() -> Handoff:
    """Import inline sources and capture state, leaving the legacy digest active."""
    return _handoff(None)


def cutover(adopt: Callable[[Handoff], None]) -> Handoff:
    """Stage the scheduler task durably and transfer ownership of the next read.

    The scheduler must upsert by handoff ID and gate claims on ready(). A crash before the
    marker leaves the legacy digest active and a retry captures its latest state.
    """
    return _handoff(adopt)


def _handoff(adopt: Callable[[Handoff], None] | None) -> Handoff:
    with run._lock() as is_free:
        if not is_free:
            raise registry.ConnectorError("digest_busy")
        if saved := _saved():
            return saved
        from sakura.scheduled import migrate, store

        try:
            if migrate.owns_digest():
                raise registry.ConnectorError("migration_unavailable")
        except store.Refused as error:
            raise registry.ConnectorError("migration_unavailable") from error
        settings = config.load_or_default()
        memory = state.load()
        fallback = [(source, source_words.label_of(source), _reads(source)) for source in settings.sources]
        mapping = registry.import_legacy(fallback)
        bindings = tuple(Binding(source.id, mapping[source.id], label, reads, source.enabled, source.timeout_seconds,
                                 memory.of(source.id)) for source, label, reads in fallback)
        found = Handoff(MIGRATION_ID, settings.at, settings.language, settings.model, settings.account,
                        settings.lookback_hours, memory.done_date, memory.last_run_at, bindings)
        if adopt is not None:
            adopt(found)
            gates.write_atomically(config.digest_folder() / MARKER, json.dumps(asdict(found)).encode())
        return found


def _reads(source: config.Source) -> str:
    return source_words.reads_of(source) or (CUSTOM_READS if source.run else "")


def ready() -> bool:
    """Whether the durable ownership marker allows only Morning summary to claim."""
    return _saved() is not None


def _saved() -> Handoff | None:
    try:
        document = json.loads((config.digest_folder() / MARKER).read_text())
        if document.get("id") != MIGRATION_ID:
            raise ValueError
        rows = tuple(Binding(**{**entry, "checkpoint": state.SourceState(**entry["checkpoint"])})
                     for entry in document["bindings"])
        return Handoff(**{**document, "bindings": rows})
    except FileNotFoundError:
        return None
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise registry.ConnectorError("migration_unavailable") from error
