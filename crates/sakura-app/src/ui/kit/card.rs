//! A card: a raised surface with a hairline ring, the same room inside on all four sides, so text never
//! touches an edge and never sits closer to one side than another.

use crate::theme::kit::scale::{CARD_GAP, CARD_PADDING, RADIUS_SURFACE};
use crate::theme::{self, Palette};
use gpui::{div, ElementId, InteractiveElement as _, SharedString, Stateful, Styled as _};

/// An empty card named `id` in the window (its selector too); the caller adds its blocks, which stand
/// [`CARD_GAP`] apart. The ring takes one point of the padding, so the room from the visible edge is
/// [`CARD_PADDING`].
pub fn card(id: impl Into<SharedString>, palette: &Palette) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    div()
        .id(ElementId::from(id))
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_col()
        .gap(CARD_GAP)
        .p(CARD_PADDING - theme::PANEL_RING_WIDTH)
        .rounded(RADIUS_SURFACE)
        .bg(palette.raised)
        .border(theme::PANEL_RING_WIDTH)
        .border_color(palette.panel_ring)
}
