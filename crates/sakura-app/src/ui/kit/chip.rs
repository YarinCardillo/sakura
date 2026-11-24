//! A chip: a small filled pill with an optional icon, for a choice that opens a menu or a sheet (the
//! project a request is about, the preset of a lane).

use crate::theme::kit::scale::{
    Typeset as _, CHIP_HEIGHT, CHIP_ICON, CHIP_PADDING_END, CHIP_PADDING_START, CHIP_RADIUS,
    CONTROL_LABEL, ICON_TEXT_GAP,
};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::Animation;
use gpui::{
    div, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// A filled chip with `label` and, before it, `icon`. `id` names it in the window and keeps its pressed
/// and hovered state.
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Button {
    shaped(id, label, (icon, None), None, on_click, cx)
}

/// What a chip is before it has a name to say (the engine has not answered yet): a static, quiet block of the chip's
/// own size with no words, that cannot be pressed. `selector` names it in the window; `fill` is the chip's fill.
pub fn chip_placeholder(selector: &'static str, fill: gpui::Hsla) -> gpui::Div {
    div()
        .debug_selector(|| selector.into())
        .flex_none()
        .h(CHIP_HEIGHT)
        .w(crate::theme::kit::CHIP_PLACEHOLDER_WIDTH)
        .rounded(CHIP_RADIUS)
        .bg(fill)
}

/// `chip` as it takes the place of its placeholder: it comes in over the shortest duration of the motion, once,
/// and with reduced motion it is there at once.
pub fn arriving(id: impl Into<SharedString>, chip: impl IntoElement, cx: &App) -> AnyElement {
    let id: SharedString = id.into();
    let held = div().id(id.clone()).child(chip);
    match cx.reduce_motion() {
        true => held.into_any_element(),
        false => held
            .with_animation(
                id,
                Animation::new(crate::theme::DURATION_FAST).with_easing(crate::ui::slide::ease_out),
                |held, progress| held.opacity(progress),
            )
            .into_any_element(),
    }
}

/// A chip that opens a menu: `label`, then a chevron after it, and `icon` before the label when there is one.
pub fn menu_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    cx: &App,
) -> Button {
    let marks = (icon, Some(IconName::ChevronDown));
    shaped(id, label, marks, None, |_, _, _| {}, cx)
}

pub fn menu_chip_at_width(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    width: gpui::Pixels,
    cx: &App,
) -> Button {
    shaped(
        id,
        label,
        (None, Some(IconName::ChevronDown)),
        Some(width),
        |_, _, _| {},
        cx,
    )
}

/// The same chip on a surface that already has the quiet fill (a composer): one step above it, so it is
/// still a surface and not bare words.
pub fn menu_chip_on_fill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Button {
    let palette = Theme::palette(cx);
    let look = ButtonCustomVariant::new(cx)
        .color(palette.fill_hover)
        .foreground(palette.text_secondary)
        .hover(palette.hover)
        .active(palette.hover)
        .shadow(false);
    menu_chip(id, label, None, cx).custom(look)
}

fn shaped(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    (before, after): (Option<IconName>, Option<IconName>),
    width: Option<gpui::Pixels>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Button {
    let palette = Theme::palette(cx);
    let look = ButtonCustomVariant::new(cx)
        .color(palette.fill)
        .foreground(palette.text_secondary)
        .hover(palette.fill_hover)
        .active(palette.fill_hover)
        .shadow(false);
    let mark = |name, selector: &'static str| {
        let icon = Icon::new(name).with_size(CHIP_ICON);
        div()
            .debug_selector(|| selector.into())
            .flex_none()
            .child(icon)
    };
    let content = div()
        .when(width.is_some(), |content| content.w_full())
        .min_w_0()
        .flex()
        .items_center()
        .gap(ICON_TEXT_GAP)
        .text_style(CONTROL_LABEL)
        .whitespace_nowrap()
        .children(before.map(|name| mark(name, "chip-before")))
        .child(
            div()
                .debug_selector(|| "chip-label".into())
                .when(width.is_some(), |label| label.flex_1())
                .min_w_0()
                .truncate()
                .child(label.into()),
        )
        .children(after.map(|name| mark(name, "chip-after")));
    Button::new(id)
        .when_some(width, |button, width| button.w(width))
        .custom(look)
        // The kit draws a custom button's fill at a fifth of its strength: the chip states its own fill.
        .bg(palette.fill)
        // The kit's button never shrinks; a label too long for its room is cut, not spilled.
        .flex_shrink(1.)
        .min_w_0()
        .h(CHIP_HEIGHT)
        .pl(CHIP_PADDING_START)
        .pr(CHIP_PADDING_END)
        .rounded(CHIP_RADIUS)
        .child(content)
        .on_click(on_click)
}
