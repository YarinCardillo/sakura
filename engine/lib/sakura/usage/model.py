"""Provider-neutral usage figures."""

from dataclasses import dataclass
from datetime import datetime


@dataclass(frozen=True)
class Usage:
    uncached_input: int
    cache_write: int
    cache_read: int
    output: int
    cost_usd: float | None

    @property
    def total(self) -> int:
        return self.uncached_input + self.cache_write + self.cache_read + self.output


@dataclass(frozen=True)
class UsageEvent:
    at: datetime
    usage: Usage


@dataclass(frozen=True)
class Sample:
    at: datetime
    provider: str
    window: str
    utilization: float
    resets_at: datetime
