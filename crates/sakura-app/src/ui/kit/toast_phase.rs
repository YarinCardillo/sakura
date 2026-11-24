//! Where a toast is in its coming and going: how much of it shows at a moment, from 0 to 1. It rises in
//! over the layout duration and sinks out over the shorter one (what closes is faster than what opens), with
//! the ease out of the panes' movement, from wherever it is, so a toast sent away while it is still rising
//! turns around without a jump.

use crate::theme;
use crate::ui::slide::ease_out;
use std::time::Instant;

/// A toast's phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Fully shown, with no movement.
    Shown,
    /// Coming in since a moment.
    Rising { since: Instant },
    /// Going out since a moment, from the share it showed then.
    Sinking { from: f32, since: Instant },
}

impl Phase {
    /// A toast that starts to come in at `now`.
    pub fn rising(now: Instant) -> Self {
        Self::Rising { since: now }
    }

    /// The same toast starting to go out at `now`, from the share it shows then.
    pub fn leaving(&self, now: Instant) -> Self {
        Self::Sinking {
            from: self.visibility(now),
            since: now,
        }
    }

    /// Whether the toast is going out.
    pub fn is_leaving(&self) -> bool {
        matches!(self, Self::Sinking { .. })
    }

    /// How much of the toast shows at `now`, from 0 to 1.
    pub fn visibility(&self, now: Instant) -> f32 {
        match *self {
            Self::Shown => 1.0,
            Self::Rising { since } => ease_out(progress(since, now, theme::DURATION_BASE)),
            Self::Sinking { from, since } => {
                from * (1.0 - ease_out(progress(since, now, theme::DURATION_FAST)))
            }
        }
    }

    /// Whether the toast is moving at `now`, so the window must draw again.
    pub fn is_moving(&self, now: Instant) -> bool {
        match *self {
            Self::Shown => false,
            Self::Rising { since } => progress(since, now, theme::DURATION_BASE) < 1.0,
            Self::Sinking { since, .. } => progress(since, now, theme::DURATION_FAST) < 1.0,
        }
    }

    /// Whether the toast has finished going out at `now`.
    pub fn is_over(&self, now: Instant) -> bool {
        self.is_leaving() && !self.is_moving(now)
    }
}

fn progress(since: Instant, now: Instant, duration: std::time::Duration) -> f32 {
    let elapsed = now.saturating_duration_since(since);
    (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.0)
}
