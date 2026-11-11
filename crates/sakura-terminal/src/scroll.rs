//! Wheel and trackpad deltas as whole lines, without losing the remainder: a trackpad swipe and the momentum
//! after it scroll exactly as far as the pixels say, however small each event is.

/// Lines one wheel notch scrolls (Alacritty's and Ghostty's multiplier for discrete wheels).
pub(crate) const LINES_PER_NOTCH: f32 = 3.0;

/// A scroll delta as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ScrollAmount {
    /// Precise pixels from a trackpad or a smooth mouse; positive scrolls back into history.
    Pixels(f32),
    /// Wheel notches; positive scrolls back into history.
    Notches(f32),
}

/// Where in a gesture an event falls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GesturePhase {
    /// Fingers touched the trackpad: start from zero.
    Started,
    /// Movement, including the momentum macOS sends after the fingers lift.
    Moved,
    /// Fingers lifted or the gesture was cancelled.
    Ended,
}

/// Accumulates scroll deltas into whole lines, carrying the remainder between events.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollAccumulator {
    remainder: f32,
}

impl ScrollAccumulator {
    /// Lines to scroll for this event; positive goes back into history.
    pub fn lines(&mut self, amount: ScrollAmount, phase: GesturePhase, line_height: f32) -> i32 {
        if phase == GesturePhase::Started {
            self.remainder = 0.0;
        }
        if line_height <= 0.0 || phase == GesturePhase::Ended {
            return 0;
        }
        let pixels = match amount {
            ScrollAmount::Pixels(pixels) => pixels,
            ScrollAmount::Notches(notches) => notches * LINES_PER_NOTCH * line_height,
        };
        let total = self.remainder + pixels;
        let lines = (total / line_height).trunc();
        self.remainder = total - lines * line_height;
        lines as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_small_pixel_deltas_add_up_exactly() {
        let mut scroll = ScrollAccumulator::default();
        let mut total = 0;
        scroll.lines(ScrollAmount::Pixels(0.0), GesturePhase::Started, 17.0);
        for _ in 0..34 {
            total += scroll.lines(ScrollAmount::Pixels(3.0), GesturePhase::Moved, 17.0);
        }
        // 34 events of 3 px are 102 px: 6 lines of 17 px, nothing lost and nothing invented.
        assert_eq!(total, 6);
    }

    #[test]
    fn scroll_momentum_after_lift_keeps_scrolling() {
        let mut scroll = ScrollAccumulator::default();
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(10.0), GesturePhase::Moved, 20.0),
            0
        );
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(0.0), GesturePhase::Ended, 20.0),
            0
        );
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(30.0), GesturePhase::Moved, 20.0),
            2
        );
    }

    #[test]
    fn scroll_direction_change_cancels_the_remainder_symmetrically() {
        let mut scroll = ScrollAccumulator::default();
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(15.0), GesturePhase::Moved, 20.0),
            0
        );
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(-15.0), GesturePhase::Moved, 20.0),
            0
        );
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(-20.0), GesturePhase::Moved, 20.0),
            -1
        );
    }

    #[test]
    fn scroll_new_gesture_starts_from_zero_and_wheel_notches_scroll_three_lines() {
        let mut scroll = ScrollAccumulator::default();
        scroll.lines(ScrollAmount::Pixels(19.0), GesturePhase::Moved, 20.0);
        assert_eq!(
            scroll.lines(ScrollAmount::Pixels(0.0), GesturePhase::Started, 20.0),
            0
        );
        assert_eq!(
            scroll.lines(ScrollAmount::Notches(-1.0), GesturePhase::Moved, 20.0),
            -3
        );
    }
}
