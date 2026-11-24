//! The bar's icon buttons: the arrows and the new-tab button. Quiet at rest, a tooltip that says what a
//! press does and its key, and a press that dispatches the same action as the key and the menu item.

use crate::theme::{self, Theme};
use crate::ui::chrome::{keep_click, quiet_variant};
use crate::ui::tip;
use gpui::{
    div, Action, AnyElement, App, InteractiveElement as _, ParentElement as _, Styled as _,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Disableable as _, Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// The icon button `id` with `icon`; `label` says what a press does, `action` is what it dispatches. A
/// disabled button draws dimmed and does nothing.
pub(super) fn icon_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    action: &dyn Action,
    is_disabled: bool,
    cx: &App,
) -> AnyElement {
    let pressed = action.boxed_clone();
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(quiet_variant(&Theme::palette(cx), cx))
        .w(theme::CONTROL_HEIGHT)
        .h(theme::CONTROL_HEIGHT)
        .p(gpui::Pixels::ZERO)
        .rounded(theme::RADIUS_CONTROL)
        .disabled(is_disabled)
        .child(Icon::new(icon).with_size(theme::ICON_SIZE))
        .on_click(move |_, window, cx| window.dispatch_action(pressed.boxed_clone(), cx));
    // The bar moves the window; a press on a button must not.
    let held = keep_click(div().id((id, 0usize)).flex_none().child(button));
    let words = tip::words_for(label, action, cx);
    tip::tip(held, format!("tip-{id}"), words, cx)
}
