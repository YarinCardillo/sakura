//! The cursor over the page bar is the arrow, always: over a tab, its close button, the + and the empty
//! strip, hovering, pressing, dragging a tab while another slides under the pointer, and after the drop.
//! Nothing in the bar asks for another cursor (a kit button asks for the hand, which would show whenever a
//! tab passes under the pointer), and this element, painted after all of them, asks for the arrow over the
//! whole bar; while a tab is pressed or in the hand it asks for it for the whole window.

use super::PageBar;
use gpui::{canvas, App, CursorStyle, Entity, HitboxBehavior, IntoElement, Styled as _};

// What the arrow's request was at the bar's last paint, for the tests: the style and whether it was for the
// whole window.
#[cfg(test)]
thread_local! {
    pub(super) static LAST_REQUEST: std::cell::Cell<Option<(CursorStyle, bool)>> =
        const { std::cell::Cell::new(None) };
}

/// The arrow over the whole row of the bar `bar`, painted last so that it comes after every request of the
/// bar's controls.
pub(super) fn arrow_over(bar: Entity<PageBar>) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, cx: &mut App| {
            let is_held = bar.read(cx).is_pressing();
            window.set_cursor_style(CursorStyle::Arrow, &hitbox);
            if is_held {
                window.set_window_cursor_style(CursorStyle::Arrow);
            }
            #[cfg(test)]
            LAST_REQUEST.with(|last| last.set(Some((CursorStyle::Arrow, is_held))));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
