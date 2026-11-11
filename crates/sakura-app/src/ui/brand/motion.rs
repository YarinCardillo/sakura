//! The mark's one motion, in its two uses. The motion is the brand sheet's: the cells switch on in the
//! order of the Bayer matrix the mark is dithered with, sixteen steps of 30 ms.
//!
//! - The reveal plays once, the first time the mark is drawn, and then the mark rests whole. Once is
//!   once a launch: a mark made later (a window opened again) is whole from its first drawing.
//! - The sign of an agent connecting is the small mark whose cells switch on in that order and off again
//!   in the same order, for as long as its calling component says something is waited for.
//!
//! A mark that moves is drawn again by a timer of its own, one tick a step, and only while it moves.
//! At rest there is no timer and no frame is requested: the mark costs what a still picture costs.
//! When the system asks for reduced motion nothing moves: the reveal is the whole mark at once, and the
//! connecting sign is the whole mark, still, while something is waited for.

use super::cells::Phase;
use super::mark::{mark, Ink, MarkSize};
use crate::theme_brand::{REVEAL_STEP, REVEAL_STEPS};
use gpui::{App, AsyncApp, Context, Global, IntoElement, Render, Task, WeakEntity, Window};
use std::time::{Duration, Instant};

/// What a connecting sign shows while nothing is waited for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rest {
    /// The whole mark, still.
    #[cfg(test)]
    Whole,
    /// Nothing: the sign keeps its room and shows no cell.
    Nothing,
}

impl Rest {
    fn phase(self) -> Phase {
        match self {
            #[cfg(test)]
            Rest::Whole => Phase::Whole,
            Rest::Nothing => Phase::Nothing,
        }
    }
}

/// Where the mark is in its motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    /// Still.
    Rest,
    /// The reveal starts with the first drawing.
    RevealPending,
    Revealing {
        since: Instant,
    },
    Connecting {
        since: Instant,
    },
}

impl Motion {
    fn is_moving(self) -> bool {
        matches!(self, Motion::Revealing { .. } | Motion::Connecting { .. })
    }
}

/// The mark as a view that can move.
pub struct BrandMark {
    size: MarkSize,
    ink: Ink,
    rest: Phase,
    motion: Motion,
    /// Draws the mark again at every step, while it moves.
    ticker: Option<Task<()>>,
    #[cfg(test)]
    renders: usize,
}

impl BrandMark {
    /// The mark at `size` in `ink` that reveals itself once, the first time it is drawn, and rests whole.
    pub fn revealing(size: MarkSize, ink: Ink) -> Self {
        Self::new(size, ink, Phase::Whole, Motion::RevealPending)
    }

    /// The sign of an agent connecting: the small mark, showing `rest` until [`Self::start`].
    pub fn connecting(rest: Rest) -> Self {
        Self::new(MarkSize::SMALL, Ink::Fuchsia, rest.phase(), Motion::Rest)
    }

    fn new(size: MarkSize, ink: Ink, rest: Phase, motion: Motion) -> Self {
        Self {
            size,
            ink,
            rest,
            motion,
            ticker: None,
            #[cfg(test)]
            renders: 0,
        }
    }

    /// Something is waited for: the cells switch on and off in a loop until [`Self::stop`]. A sign
    /// that already loops goes on from where it is.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.motion, Motion::Connecting { .. }) {
            let since = cx.background_executor().now();
            self.motion = Motion::Connecting { since };
            // A timer that ended with a reveal is not the loop's: the next drawing starts a new one.
            self.ticker = None;
            cx.notify();
        }
    }

    /// Nothing is waited for any more: the sign rests at once.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.motion != Motion::Rest {
            self.motion = Motion::Rest;
            self.ticker = None;
            cx.notify();
        }
    }

    /// Whether the mark's timer runs.
    #[cfg(test)]
    pub fn is_ticking(&self) -> bool {
        self.ticker.is_some()
    }

    /// How many times the mark was drawn.
    #[cfg(test)]
    pub fn renders(&self) -> usize {
        self.renders
    }

    /// What the mark shows now.
    #[cfg(test)]
    pub fn phase(&self, now: Instant, is_motion_reduced: bool) -> Phase {
        phase_at(self.motion, self.rest, now, is_motion_reduced)
    }

    /// Takes the motion to where it is at `now`: a reveal starts with its first drawing, unless the
    /// app has played it already, and ends after its last step; with reduced motion it is over
    /// before it starts.
    fn advance(&mut self, now: Instant, is_motion_reduced: bool, cx: &mut App) {
        self.motion = match self.motion {
            Motion::RevealPending if is_motion_reduced || was_played(cx) => Motion::Rest,
            Motion::RevealPending => Motion::Revealing { since: now },
            Motion::Revealing { since } if is_motion_reduced || is_revealed(now - since) => {
                Motion::Rest
            }
            motion => motion,
        };
    }

    /// Runs the timer while the mark moves, and only then.
    fn keep_ticking(&mut self, is_needed: bool, cx: &mut Context<Self>) {
        match (is_needed, self.ticker.is_some()) {
            (true, false) => self.ticker = Some(cx.spawn(tick)),
            (false, true) => self.ticker = None,
            _ => {}
        }
    }
}

impl Render for BrandMark {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let now = cx.background_executor().now();
        let is_motion_reduced = cx.reduce_motion();
        self.advance(now, is_motion_reduced, cx);
        self.keep_ticking(self.motion.is_moving() && !is_motion_reduced, cx);
        let phase = phase_at(self.motion, self.rest, now, is_motion_reduced);
        mark(self.size, self.ink, phase)
    }
}

/// Set once the app has played the reveal.
struct Played;

impl Global for Played {}

/// Whether the reveal was played in this launch already; the first to ask is the one that plays it.
fn was_played(cx: &mut App) -> bool {
    let was_played = cx.has_global::<Played>();
    cx.set_global(Played);
    was_played
}

/// Draws the mark again once a step. It ends with the reveal; the loop of a connecting sign is ended
/// by its calling component, who drops the timer.
async fn tick(view: WeakEntity<BrandMark>, cx: &mut AsyncApp) {
    loop {
        cx.background_executor().timer(REVEAL_STEP).await;
        let now = cx.background_executor().now();
        let moves = view.update(cx, |mark, cx| {
            mark.advance(now, cx.reduce_motion(), cx);
            cx.notify();
            mark.motion.is_moving()
        });
        if !matches!(moves, Ok(true)) {
            return;
        }
    }
}

/// How long the cells take to switch all on, or all off.
fn sweep() -> Duration {
    REVEAL_STEP * u32::from(REVEAL_STEPS)
}

/// The steps that have switched `elapsed` after a sweep began: the first at once, one more every
/// [`REVEAL_STEP`].
fn steps_after(elapsed: Duration) -> u8 {
    let passed = elapsed.as_millis() / REVEAL_STEP.as_millis();
    let steps = passed.saturating_add(1).min(u128::from(REVEAL_STEPS));
    steps as u8
}

/// Whether a reveal that began `elapsed` ago has lit its last step.
fn is_revealed(elapsed: Duration) -> bool {
    steps_after(elapsed) == REVEAL_STEPS
}

/// What a mark in `motion` shows at `now`.
fn phase_at(motion: Motion, rest: Phase, now: Instant, is_motion_reduced: bool) -> Phase {
    match motion {
        Motion::Rest => rest,
        Motion::RevealPending => Phase::Nothing,
        Motion::Revealing { .. } | Motion::Connecting { .. } if is_motion_reduced => Phase::Whole,
        Motion::Revealing { since } => Phase::On(steps_after(now - since)),
        Motion::Connecting { since } => connecting_phase(now - since),
    }
}

/// The connecting loop `elapsed` after it began: one sweep on, one sweep off, again.
fn connecting_phase(elapsed: Duration) -> Phase {
    let sweep = sweep().as_millis();
    let into = elapsed.as_millis() % (sweep * 2);
    let since_sweep = Duration::from_millis((into % sweep) as u64);
    if into < sweep {
        Phase::On(steps_after(since_sweep))
    } else {
        Phase::Off(steps_after(since_sweep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn the_reveal_takes_sixteen_steps_of_30_ms_and_is_whole_before_480_ms() {
        assert_eq!(sweep(), ms(480));
        assert_eq!(steps_after(ms(0)), 1);
        assert_eq!(steps_after(ms(29)), 1);
        assert_eq!(steps_after(ms(30)), 2);
        assert_eq!(steps_after(ms(449)), 15);
        assert_eq!(steps_after(ms(450)), 16);
        assert_eq!(steps_after(ms(60_000)), 16);
        assert!(!is_revealed(ms(449)) && is_revealed(ms(450)));
    }

    #[test]
    fn the_connecting_loop_switches_on_then_off_in_the_same_order_and_begins_again() {
        assert_eq!(connecting_phase(ms(0)), Phase::On(1));
        assert_eq!(connecting_phase(ms(450)), Phase::On(16));
        assert_eq!(connecting_phase(ms(480)), Phase::Off(1));
        assert_eq!(connecting_phase(ms(930)), Phase::Off(16));
        assert_eq!(connecting_phase(ms(960)), Phase::On(1));
        assert_eq!(connecting_phase(ms(960 * 7 + 95)), Phase::On(4));
    }

    #[test]
    fn with_reduced_motion_a_mark_that_would_move_is_whole() {
        let since = Instant::now();
        let later = since + ms(90);
        for motion in [Motion::Revealing { since }, Motion::Connecting { since }] {
            assert_eq!(phase_at(motion, Phase::Nothing, later, true), Phase::Whole);
            assert_ne!(phase_at(motion, Phase::Nothing, later, false), Phase::Whole);
        }
        assert_eq!(
            phase_at(Motion::Rest, Phase::Nothing, later, true),
            Phase::Nothing
        );
    }
}
