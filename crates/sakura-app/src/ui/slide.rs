//! A pane that hides and comes back: how much of it shows at a moment, from 0 (hidden) to 1 (shown). A toggle
//! moves it over the layout duration with an ease out, starting from wherever it is, so a second toggle in
//! the middle turns it around without a jump. With reduced motion it is at its target at once.

use crate::theme;
use std::time::Instant;

/// The showing of one pane: where it was when it last started to move, where it goes, and since when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slide {
    from: f32,
    to: f32,
    started_at: Instant,
}

impl Slide {
    /// A pane at rest, shown or hidden.
    pub fn at_rest(is_shown: bool, now: Instant) -> Self {
        let share = if is_shown { 1.0 } else { 0.0 };
        Self {
            from: share,
            to: share,
            started_at: now,
        }
    }

    /// Whether the pane is shown, or on its way to being shown.
    pub fn is_shown(&self) -> bool {
        self.to > 0.5
    }

    /// The same pane toggled at `now`: it turns towards the other state from where it is; with reduced
    /// motion it is there at once.
    pub fn toggled(&self, now: Instant, is_motion_reduced: bool) -> Self {
        if is_motion_reduced {
            return Self::at_rest(!self.is_shown(), now);
        }
        Self {
            from: self.share(now),
            to: if self.is_shown() { 0.0 } else { 1.0 },
            started_at: now,
        }
    }

    /// How much of the pane shows at `now`, from 0 to 1.
    pub fn share(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started_at);
        let progress = elapsed.as_secs_f32() / theme::DURATION_BASE.as_secs_f32();
        if progress >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * ease_out(progress)
    }

    /// Whether the pane has stopped moving at `now`.
    pub fn is_at_rest(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) >= theme::DURATION_BASE
            || (self.from - self.to).abs() < f32::EPSILON
    }
}

/// Fast at the start, slow at the end: `progress` from 0 to 1 gives the share of the way covered.
fn ease_out(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(theme::EASE_OUT_POWER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn after(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_pane_at_rest_shows_all_or_nothing() {
        let now = Instant::now();
        assert_eq!(Slide::at_rest(true, now).share(after(now, 500)), 1.0);
        assert_eq!(Slide::at_rest(false, now).share(now), 0.0);
        assert!(Slide::at_rest(true, now).is_at_rest(now));
    }

    #[test]
    fn a_toggle_takes_the_layout_duration_and_ends_exactly_hidden() {
        let now = Instant::now();
        let hiding = Slide::at_rest(true, now).toggled(now, false);
        assert!(!hiding.is_shown());
        assert_eq!(hiding.share(now), 1.0, "it starts where it was");
        assert!(!hiding.is_at_rest(after(now, 199)));
        assert!(hiding.is_at_rest(after(now, 200)));
        assert_eq!(hiding.share(after(now, 200)), 0.0);
        assert_eq!(hiding.share(after(now, 5000)), 0.0);
    }

    #[test]
    fn the_movement_eases_out_most_of_the_way_in_the_first_half() {
        let now = Instant::now();
        let showing = Slide::at_rest(false, now).toggled(now, false);
        let halfway = showing.share(after(now, 100));
        assert!(halfway > 0.8 && halfway < 1.0, "{halfway}");
        let early = showing.share(after(now, 20));
        let late = showing.share(after(now, 180)) - showing.share(after(now, 160));
        assert!(early > late, "fast at the start, slow at the end");
    }

    #[test]
    fn a_toggle_in_the_middle_turns_around_from_where_the_pane_is() {
        let now = Instant::now();
        let hiding = Slide::at_rest(true, now).toggled(now, false);
        let middle = after(now, 60);
        let before = hiding.share(middle);
        let back = hiding.toggled(middle, false);
        assert!(back.is_shown());
        assert_eq!(back.share(middle), before, "no jump");
        assert_eq!(back.share(after(middle, 200)), 1.0);
    }

    #[test]
    fn with_reduced_motion_a_toggle_is_there_at_once() {
        let now = Instant::now();
        let hidden = Slide::at_rest(true, now).toggled(now, true);
        assert_eq!(hidden.share(now), 0.0);
        assert!(hidden.is_at_rest(now));
    }
}
