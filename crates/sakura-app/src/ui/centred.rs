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
