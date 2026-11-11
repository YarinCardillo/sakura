//! The icon button of the window's chrome (the two of the status bar that hide a pane or bring it back,
//! the sidebar's "New task"): the kit's icon button, quiet at rest, with a tooltip that says what a
//! press would do and its key. A press dispatches the same action as the key and the menu item.

use super::chrome::keep_click;
use super::task_menu::quiet_variant;
use crate::theme::{self, Theme};
use gpui::{
    div, Action, AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Styled as _,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};

/// The button `id` with `icon`; `label` is what a press would do, `action` what it dispatches. The button
/// is `side` high and wide (one control, or the status bar's own); its icon, one icon in size, is what
/// sits on the grid (`icon_id` names it).
pub fn button(
    (id, icon_id): (&'static str, &'static str),
    (icon, side): (impl Into<Icon>, Pixels),
    label: &'static str,
    action: &dyn Action,
    cx: &App,
) -> AnyElement {
    let pressed = action.boxed_clone();
    let mark = div()
        .debug_selector(move || icon_id.into())
        .size(theme::ICON_SIZE)
        .flex_none()
        .child(Icon::new(icon).with_size(theme::ICON_SIZE));
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(quiet_variant(&Theme::palette(cx), cx))
        .w(side)
        .h(side)
        .p(Pixels::ZERO)
        .child(mark)
        .tooltip_with_action(label, action, None)
        .on_click(move |_, window, cx| window.dispatch_action(pressed.boxed_clone(), cx));
    // Headers move the window; a press on the toggle must not.
    keep_click(div().id((id, 0usize)).flex_none().child(button)).into_any_element()
}
