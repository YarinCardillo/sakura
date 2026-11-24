//! A whole number with a button at each side, kept between `min` and `max` when they are given. A press on a
//! button that would leave the range does nothing.

use super::OnPress;
use crate::theme::kit::scale::{Typeset as _, CONTROL_REGULAR, RADIUS_CONTROL, TEXT_BODY};
use crate::theme::kit::STEPPER_NUMBER_WIDTH;
use crate::theme::Palette;
use crate::ui::chrome::tabular;
use gpui::{
    div, AnyElement, App, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use std::rc::Rc;

/// What a press asks for: the number it would give.
pub type OnNumber = Rc<dyn Fn(i64, &mut Window, &mut App)>;

/// Shows the confirmed number while repeated presses build on the latest request.
pub fn stepper(
    id: &str,
    (confirmed, requested): (i64, i64),
    min: Option<i64>,
    max: Option<i64>,
    on_change: OnNumber,
    palette: &Palette,
) -> gpui::Div {
    let (less, more) = (on_change.clone(), on_change);
    let can_less = min.is_none_or(|min| requested > min);
    let can_more = max.is_none_or(|max| requested < max);
    let number_id = format!("{id}-value-{confirmed}");
    let number = div()
        .debug_selector(move || number_id)
        .min_w(STEPPER_NUMBER_WIDTH)
        .flex()
        .justify_center()
        .text_style(TEXT_BODY)
        .font_features(tabular())
        .child(confirmed.to_string());
    let selector = id.to_string();
    div()
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .justify_between()
        .rounded(RADIUS_CONTROL)
        .bg(palette.fill)
        .child(step_button(
            format!("{id}-less"),
            "\u{2212}",
            can_less,
            Rc::new(move |window, cx| less(requested - 1, window, cx)),
            palette,
        ))
        .child(number)
        .child(step_button(
            format!("{id}-more"),
            "+",
            can_more,
            Rc::new(move |window, cx| more(requested + 1, window, cx)),
            palette,
        ))
}

fn step_button(
    id: String,
    sign: &'static str,
    is_enabled: bool,
    press: OnPress,
    palette: &Palette,
) -> AnyElement {
    let kind = super::ButtonKind::Quiet;
    let button = if is_enabled {
        super::button(id, sign, kind, palette).on_click(move |_, window, cx| press(window, cx))
    } else {
        super::button_disabled(id, sign, kind, palette)
    };
    button
        .size(CONTROL_REGULAR)
        .px_0()
        .justify_center()
        .into_any_element()
}
