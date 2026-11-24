//! What Enter means in a field that sends. While an input method holds marked text (a dead key, the accent
//! pop-up, a candidate list) Enter belongs to it and the app does nothing with it. Inside an unclosed code
//! fence Enter breaks the line and only Cmd+Enter sends; anywhere else Enter sends.

use gpui::{App, Entity, EntityInputHandler, Window};
use gpui_component::input::{Enter, TextareaState};

/// The marks that open and close a fenced code block, at the start of a line.
const FENCE: &str = "```";

/// Whether the caret stands inside a fenced code block: an odd number of fence lines come before it.
pub fn is_fence_open(text_before_caret: &str) -> bool {
    let fences = text_before_caret
        .lines()
        .filter(|line| line.trim_start().starts_with(FENCE))
        .count();
    fences % 2 == 1
}

/// Whether an input method holds marked text in `field`; its handlers then leave Enter alone.
pub fn is_composing<T: EntityInputHandler>(
    field: &Entity<T>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    field.update(cx, |field, cx| {
        field.marked_text_range(window, cx).is_some()
    })
}

/// The listener of a composer's box for the field's Enter action, set to run before the field's own, so
/// that the field never hears of an Enter that is not for it: while an input method holds marked text the
/// Enter is the input method's and is swallowed; a plain Enter inside an unclosed code fence puts a line
/// break at the caret instead of sending. Any other Enter goes on to the field.
pub fn guard_enter(
    field: Entity<TextareaState>,
) -> impl Fn(&Enter, &mut Window, &mut App) + 'static {
    move |action, window, cx| {
        if is_composing(&field, window, cx) {
            cx.stop_propagation();
            return;
        }
        let is_plain = !action.shift && !action.secondary;
        if is_plain && is_fence_open_at_caret(&field, cx) {
            field.update(cx, |field, cx| field.insert("\n", window, cx));
            cx.stop_propagation();
        }
    }
}

fn is_fence_open_at_caret(field: &Entity<TextareaState>, cx: &App) -> bool {
    let state = field.read(cx);
    let before_caret = state
        .value()
        .get(..state.cursor())
        .unwrap_or_default()
        .to_string();
    is_fence_open(&before_caret)
}
