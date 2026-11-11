//! The springs a tab moves on: nothing bounces:
//! a tab stepping aside for the one in the hand rides the press-down spring, quick; a tab let go, and the
//! tabs closing a gap, come to rest on the press-up spring, soft. A value keeps its speed when its target
//! changes, so a turn in the middle has no jerk.

use super::metrics::{
    SPRING_LONGEST_FRAME as LONGEST_FRAME, SPRING_REST_DISTANCE as REST_DISTANCE,
    SPRING_REST_SPEED as REST_SPEED, SPRING_STEP as STEP,
};
use std::time::Duration;

/// A damped spring: stiffness, damping and mass, as Motion names them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

/// A control pressed: quick to go down.
pub const PRESS_DOWN: Spring = Spring {
    stiffness: 700.,
    damping: 53.,
    mass: 1.,
};
/// A control released: soft to come back.
pub const PRESS_UP: Spring = Spring {
    stiffness: 260.,
    damping: 33.,
    mass: 1.,
};

/// A value on a spring: where it is, how fast it moves and where it goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprung {
    pub value: f32,
    velocity: f32,
    target: f32,
}

impl Sprung {
    /// A value at rest on `value`.
    pub fn at_rest(value: f32) -> Self {
        Self {
            value,
            velocity: 0.,
            target: value,
        }
    }

    /// Where the value goes.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Sends the value to `target` from where it is, at the speed it has.
    pub fn send_to(&mut self, target: f32) {
        self.target = target;
    }

    /// Puts the value on `target` at once (reduced motion).
    pub fn jump_to(&mut self, target: f32) {
        *self = Self::at_rest(target);
    }

    /// Moves the value by `shift` without touching its target: it is drawn where it was although its slot
    /// moved, and travels the difference.
    pub fn displace(&mut self, shift: f32) {
        self.value += shift;
    }

    /// Whether the value has arrived.
    pub fn is_at_rest(&self) -> bool {
        (self.value - self.target).abs() < REST_DISTANCE && self.velocity.abs() < REST_SPEED
    }

    /// Lets `elapsed` pass on `spring`.
    pub fn advance(&mut self, spring: Spring, elapsed: Duration) {
        let mut left = elapsed.min(LONGEST_FRAME).as_secs_f32();
        while left > 0. && !self.is_at_rest() {
            let step = left.min(STEP);
            let pull = -spring.stiffness * (self.value - self.target);
            let drag = -spring.damping * self.velocity;
            self.velocity += (pull + drag) / spring.mass * step;
            self.value += self.velocity * step;
            left -= step;
        }
        if self.is_at_rest() {
            self.jump_to(self.target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(8);

    /// The lowest point and the frames taken by a value let go 100 px from its place.
    fn settle(spring: Spring) -> (f32, usize) {
        let mut value = Sprung::at_rest(100.);
        value.send_to(0.);
        let (mut lowest, mut frames) = (f32::MAX, 0);
        while !value.is_at_rest() && frames < 1000 {
            value.advance(spring, FRAME);
            lowest = lowest.min(value.value);
            frames += 1;
        }
        (lowest, frames)
    }

    #[test]
    fn no_spring_overshoots_more_than_two_percent() {
        for spring in [PRESS_DOWN, PRESS_UP] {
            let (lowest, _) = settle(spring);
            assert!(lowest >= -2.0, "{spring:?} went to {lowest}");
        }
    }

    #[test]
    fn press_down_arrives_sooner_than_press_up_and_both_within_a_second() {
        let (_, down) = settle(PRESS_DOWN);
        let (_, up) = settle(PRESS_UP);
        assert!(down < up, "{down} frames against {up}");
        assert!(up * 8 < 1000, "{up} frames");
        assert!(down * 8 > 100, "a spring, not a jump: {down} frames");
    }

    #[test]
    fn a_value_arrives_exactly_on_its_target() {
        let mut value = Sprung::at_rest(40.);
        value.send_to(0.);
        for _ in 0..400 {
            value.advance(PRESS_UP, FRAME);
        }
        assert_eq!(value.value, 0.);
        assert!(value.is_at_rest());
    }

    #[test]
    fn a_new_target_in_the_middle_keeps_the_speed() {
        let mut value = Sprung::at_rest(0.);
        value.send_to(100.);
        value.advance(PRESS_DOWN, Duration::from_millis(40));
        let before = value.value;
        value.send_to(0.);
        value.advance(PRESS_DOWN, Duration::from_millis(2));
        assert!(value.value > before, "still carried forward for a moment");
    }

    #[test]
    fn a_displaced_value_travels_back_to_its_target() {
        let mut value = Sprung::at_rest(0.);
        value.displace(-124.);
        assert_eq!(value.value, -124.);
        assert_eq!(value.target(), 0.);
        assert!(!value.is_at_rest());
    }

    #[test]
    fn a_stalled_frame_does_not_fling_the_value() {
        let mut value = Sprung::at_rest(100.);
        value.send_to(0.);
        value.advance(PRESS_UP, Duration::from_secs(30));
        assert!(value.value > 0., "one long frame counts as 64 ms");
    }
}
