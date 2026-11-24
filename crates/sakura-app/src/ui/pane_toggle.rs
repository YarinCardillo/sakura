//! The icon button of the window's chrome (the two of the status bar that hide a pane or bring it back,
//! the sidebar's "New task"): the kit's icon button, quiet at rest, with a tooltip that says what a
//! press would do and its key. A press dispatches the same action as the key and the menu item.

use super::chrome::{keep_click, quiet_variant};
use super::tip;
use crate::theme::{self, Theme};
use gpui::{
    div, Action, AnyElement, App, InteractiveElement as _, ParentElement as _, Pixels,
    SharedString, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};

/// The quiet icon button `id` of the chrome: `icon` on the grid, `side` high and wide, no tooltip or handler yet.
fn quiet_icon(
    (id, icon_id): (SharedString, &'static str),
    (icon, side): (impl Into<Icon>, Pixels),
    cx: &App,
) -> Button {
    let mark = div()
        .debug_selector(move || icon_id.into())
        .size(theme::ICON_SIZE)
        .flex_none()
        .child(Icon::new(icon).with_size(theme::ICON_SIZE));
    Button::new(id.clone())
        .debug_selector(move || id.to_string())
        .custom(quiet_variant(&Theme::palette(cx), cx))
        .w(side)
        .h(side)
        .p(Pixels::ZERO)
        .child(mark)
}

/// The button `id` with `icon` that runs `on_press` instead of an action: a control of a row, such as a
/// project's "New lane"; `label` is what a press would do, said in a tooltip. Like the toggles it stands
/// in a header that moves the window, and a press on it does not.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub fn press_button(
    (id, icon_id): (SharedString, &'static str),
    (icon, side): (impl Into<Icon>, Pixels),
    label: &'static str,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let button = quiet_icon((id.clone(), icon_id), (icon, side), cx)
        .on_click(move |_, window, cx| on_press(window, cx));
    let held = keep_click(div().id((id.clone(), 0usize)).flex_none().child(button));
    tip::tip(held, format!("tip-{id}"), tip::Words::plain(label), cx)
}

/// The button `id` with `icon`; `label` is what a press would do, `action` what it dispatches. The button
/// is `side` high and wide (one control, or the status bar's own); its icon, one icon in size, is what
/// sits on the grid (`icon_id` names it).
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub fn button(
    (id, icon_id): (&'static str, &'static str),
    (icon, side): (impl Into<Icon>, Pixels),
    label: &'static str,
    action: &dyn Action,
    cx: &App,
) -> AnyElement {
    let pressed = action.boxed_clone();
    let button = quiet_icon((id.into(), icon_id), (icon, side), cx)
        .on_click(move |_, window, cx| window.dispatch_action(pressed.boxed_clone(), cx));
    // Headers move the window; a press on the toggle must not.
    let held = keep_click(div().id((id, 0usize)).flex_none().child(button));
    tip::tip(
        held,
        format!("tip-{id}"),
        tip::words_for(label, action, cx),
        cx,
    )
}
