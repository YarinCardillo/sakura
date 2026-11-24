//! Trackpad feedback, as Linear gives it on a Mac: a light tap when something dragged snaps onto a place.
//! The strip calls [`Haptics::alignment`] each time a dragged tab crosses into another slot, never on an
//! ordinary click. The system's performer stands behind the trait; tests record the taps instead and never
//! reach the trackpad of whoever runs them.

use std::rc::Rc;

/// The feedback the interface can ask of the trackpad.
pub trait Haptics {
    /// Something dragged came to rest on a guide or a slot.
    fn alignment(&self);
}

/// No feedback at all: what the tests get, and any machine that is not a Mac.
#[cfg(any(test, not(target_os = "macos")))]
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

#[cfg(any(test, not(target_os = "macos")))]
impl Haptics for Silent {
    fn alignment(&self) {}
}

/// The system's feedback: the trackpad on a Mac, nothing elsewhere and nothing in the tests, which must not
/// tap the trackpad of whoever runs them.
pub fn system() -> Rc<dyn Haptics> {
    #[cfg(all(target_os = "macos", not(test)))]
    return Rc::new(trackpad::Trackpad);
    #[cfg(not(all(target_os = "macos", not(test))))]
    Rc::new(Silent)
}

#[cfg(all(target_os = "macos", not(test)))]
mod trackpad {
    use super::Haptics;
    use objc2_app_kit::{
        NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
        NSHapticFeedbackPerformer,
    };

    pub struct Trackpad;

    impl Haptics for Trackpad {
        fn alignment(&self) {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::Alignment,
                NSHapticFeedbackPerformanceTime::Now,
            );
        }
    }
}

/// A double that records what was asked, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Recorded {
    taps: std::cell::RefCell<Vec<&'static str>>,
}

#[cfg(test)]
impl Recorded {
    /// What was asked so far, in order.
    pub fn taps(&self) -> Vec<&'static str> {
        self.taps.borrow().clone()
    }
}

#[cfg(test)]
impl Haptics for Recorded {
    fn alignment(&self) {
        self.taps.borrow_mut().push("alignment");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_double_keeps_the_taps_in_order() {
        let recorded = Recorded::default();
        recorded.alignment();
        recorded.alignment();
        assert_eq!(recorded.taps(), ["alignment", "alignment"]);
    }

    #[test]
    fn in_tests_the_system_feedback_is_silent() {
        // Nothing to observe but that asking does not reach the trackpad code: this must return.
        let haptics = system();
        haptics.alignment();
    }
}
