//! The one rule for what stands alone in the panel: it is centred in it. A page or a pane that has only
//! a sentence to show, the blocked and the first page, and the sheet that asks for a new task all take
//! their place from here, so their centres are one.

use gpui::{div, Div, IntoElement, ParentElement as _, Pixels, Styled as _};

/// `content` centred in the room it is given: a page or a pane that has only this to show.
pub fn centred(content: impl IntoElement) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
}

/// How far the panel's centre is from the window's, across, for a panel that starts `panel_left` from
/// the window's left edge and ends on its right edge: what moves an overlay the kit centres on the
/// window to the panel's centre.
pub fn panel_offset(panel_left: Pixels) -> Pixels {
    panel_left / 2.
}

/// Where the top of an overlay with a list stands: the overlay at its tallest (`full_height`) is in the
/// middle of the window's height, and a shorter list keeps that top, so nothing moves while the person
/// types. A window too low for it keeps `margin` above.
pub fn overlay_top(window_height: Pixels, full_height: Pixels, margin: Pixels) -> Pixels {
    ((window_height - full_height) / 2.).max(margin)
}
