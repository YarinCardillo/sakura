//! The controls of the composers can be seen in both appearances: every icon is at least 3:1 against the
//! box it stands on, the send button too when there is nothing to send (dimmed, never gone), and the words
//! of the chips are at least 4.5:1.

use super::thread::send_look;
use crate::theme::{Appearance, Palette};
use crate::theme_contrast::{contrast, GRAPHIC, TEXT};

fn both() -> [(Appearance, Palette); 2] {
    [Appearance::Dark, Appearance::Light].map(|appearance| (appearance, appearance.palette()))
}

#[test]
fn the_plus_and_the_microphone_can_be_seen_on_the_master_composer_and_the_lane_composer() {
    for (appearance, palette) in both() {
        for ground in [palette.fill, palette.raised] {
            let ratio = contrast(palette.text_secondary, ground);
            assert!(ratio >= GRAPHIC, "{appearance:?}: {ratio} on {ground:?}");
        }
    }
}

#[test]
fn the_send_button_can_be_seen_ready_and_not() {
    for (appearance, palette) in both() {
        for is_ready in [true, false] {
            let (face, mark) = send_look(&palette, is_ready);
            let ratio = contrast(mark, face);
            assert!(
                ratio >= GRAPHIC,
                "{appearance:?}, ready {is_ready}: {ratio}"
            );
            assert_ne!(face, palette.fill, "{appearance:?}: a face of its own");
        }
        let (ready, _) = send_look(&palette, true);
        for ground in [palette.fill, palette.raised] {
            assert!(contrast(ready, ground) >= GRAPHIC, "{appearance:?}");
        }
    }
}

#[test]
fn a_dimmed_send_button_is_not_the_ready_one() {
    for (appearance, palette) in both() {
        assert_ne!(
            send_look(&palette, true),
            send_look(&palette, false),
            "{appearance:?}"
        );
    }
}

#[test]
fn the_words_of_the_chips_are_text() {
    for (appearance, palette) in both() {
        let ratio = contrast(palette.text_secondary, palette.fill_hover);
        assert!(ratio >= TEXT, "{appearance:?}: {ratio}");
        let ratio = contrast(palette.text_secondary, palette.fill);
        assert!(ratio >= TEXT, "{appearance:?}: {ratio}");
    }
}
