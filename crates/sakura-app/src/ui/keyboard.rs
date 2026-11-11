//! Tab before anything has focus. The window opens with nothing focused (no autofocus), and GPUI then
//! dispatches keys at the window's root, outside the kit Root's own Tab binding, so Tab would do nothing.
//! These bindings have no context: they apply only where no deeper binding does, and move focus to the first
//! or last control, which then shows its ring because the input was the keyboard.

use gpui::{actions, App, KeyBinding};

actions!(sakura_keyboard, [FocusFirst, FocusLast]);

/// Binds Tab and Shift-Tab for a window where nothing has focus yet.
pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusFirst, None),
        KeyBinding::new("shift-tab", FocusLast, None),
    ]);
    cx.on_action(|_: &FocusFirst, cx| move_focus(true, cx));
    cx.on_action(|_: &FocusLast, cx| move_focus(false, cx));
}

/// The window is being updated while its key event dispatches, so the move runs right after.
fn move_focus(is_forward: bool, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let moved = window.update(cx, |_, window, cx| {
            if is_forward {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
        });
        if let Err(error) = moved {
            eprintln!("sakura: cannot move focus: {error}");
        }
    });
}
