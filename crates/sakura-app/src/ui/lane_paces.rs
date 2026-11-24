//! The paces a lane may be given, in one place: the new lane's sheet, the lane's Details and the Engine page's
//! table of running lanes offer the same five, slowest first, and say a lane's pace in the same words.

use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
};

/// The paces of a lane, slowest first, each with its word.
pub(in crate::ui) const LANE_PACES: [(&str, Key); 5] = [
    ("slowest", Key("pace.slowest")),
    ("slow", Key("pace.slow")),
    ("normal", Key("pace.normal")),
    ("fast", Key("pace.fast")),
    ("fastest", Key("pace.fastest")),
];

/// The localized label for a pace.
pub(in crate::ui) fn pace_words(level: &str) -> SharedString {
    match LANE_PACES.iter().find(|(known, _)| *known == level) {
        Some((_, word)) => tr(*word).into(),
        None => tr(Key("pace.unknown")).into(),
    }
}

/// The note under a lane's pace while the engine holds it for the lane's next step, the same in Details and in
/// the Engine page's table: `selector` names it in the window.
pub(in crate::ui) fn pending_note(selector: String, palette: &Palette) -> AnyElement {
    crate::ui::chrome::text(theme::TEXT_LABEL, palette.text_tertiary)
        .debug_selector(move || selector)
        .child(tr(Key("lane_side.applies_next")))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::pace_words;

    #[test]
    fn known_paces_have_shared_labels_and_unknown_paces_have_a_neutral_label() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(pace_words("slowest"), "Slowest");
        assert_eq!(pace_words("normal"), "Normal");
        assert_eq!(pace_words("fastest"), "Fastest");
        assert_eq!(pace_words("careful"), "Unknown pace");
    }
}
