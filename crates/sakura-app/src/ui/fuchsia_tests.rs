//! Lane state marks use neutral colours. The Inbox uses the full accent, and the retino uses a muted tint.

use super::kit::StateGlyph;
use crate::theme::{Appearance, Palette};

const STATES: [StateGlyph; 5] = [
    StateGlyph::NeedsYou,
    StateGlyph::Running,
    StateGlyph::Finished,
    StateGlyph::Stopped,
    StateGlyph::NotStarted,
];

fn both() -> [(Appearance, Palette); 2] {
    [Appearance::Dark, Appearance::Light].map(|appearance| (appearance, appearance.palette()))
}

#[test]
fn no_state_mark_is_drawn_in_the_accent() {
    for (appearance, palette) in both() {
        for state in STATES {
            assert_ne!(
                state.colour(&palette),
                palette.attention,
                "{appearance:?} {state:?}"
            );
        }
    }
}

#[test]
fn a_lane_that_needs_the_person_is_still_told_apart_by_its_filled_disc_and_a_stronger_tone() {
    for (appearance, palette) in both() {
        assert_eq!(
            StateGlyph::NeedsYou.colour(&palette),
            palette.text_secondary,
            "{appearance:?}"
        );
    }
}
