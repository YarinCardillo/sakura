"""The quota service object: status and advice for this Mac, shared by the CLI and the HTTP server."""

from __future__ import annotations

import threading
from collections.abc import Callable
from dataclasses import asdict
from datetime import datetime
from pathlib import Path

from sakura.accounts.registry import Registry
from sakura.quota import advise, readings, status, targets
from sakura.quota.config import QuotaConfig
from sakura.usage.archive import quota, settings, store


class Service:
    """Builds the status document and answers advice requests; processes and the clock are injected.

    Calls are serialized: two probes of one account at once would only spend the quota twice.
    """

    def __init__(self, usage: settings.Settings, registry: Registry, config: QuotaConfig, run: quota.Runner,
                 clock: Callable[[], datetime], home: Path) -> None:
        self._usage, self._registry, self._config = usage, registry, config
        self._run, self._clock, self._home = run, clock, home
        self._lock = threading.Lock()

    @property
    def config(self) -> QuotaConfig:
        return self._config

    def status(self, refresh: bool = False) -> dict:
        """The status document; sources are asked only for accounts whose snapshot is stale (all with `refresh`)."""
        with self._lock:
            return self._status(refresh)

    def advise(self, request: advise.Request) -> dict:
        """The advice for `request` as the contract's JSON object."""
        document = self.status()
        answer = advise.advise(request, document, advise.ceilings(self._registry), self._config)
        return asdict(answer)

    def _status(self, refresh: bool) -> dict:
        found = targets.discover(self._registry, self._home)
        conn = store.open_db(self._usage.db)
        try:
            collector = readings.Collector(conn, self._run, self._clock, self._usage.claude_bin)
            return status.build(found, collector, conn, self._clock(), refresh=refresh)
        finally:
            conn.close()
