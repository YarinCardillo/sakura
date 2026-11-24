"""Failures of lane commands, split by what the operator can do about them."""

from __future__ import annotations


class LaneError(RuntimeError):
    """Raised when a lane command cannot run or a command it started fails."""


class LaneConflict(LaneError):
    """Raised when a window or directory the lane needs is already taken by something sakura did not create."""


class LaneBusy(LaneError):
    """Raised by `lane done` when the lane holds work that closing would lose; nothing was changed."""

    def __init__(self, reasons: list[str]):
        super().__init__("; ".join(reasons))
        self.reasons = reasons


class InputRefused(LaneError):
    """Raised by `lane say` and `lane model` when the lane's agent cannot take input now; nothing was sent."""


class InputInvalid(ValueError):
    """Raised by `lane say` and `lane model` when the text or model name is unusable; nothing was sent."""
