//! A tag: a chip that is not pressed. A filled pill with an optional dot and a label, the same height, padding
//! and label as a chip, so a tag and a chip side by side are one family.

use crate::theme::kit::scale::{
    Typeset as _, CHIP_HEIGHT, CHIP_PADDING_END, CHIP_PADDING_START, CHIP_RADIUS, CONTROL_LABEL,
    DOT_SIZE, ICON_TEXT_GAP,
};
use crate::theme::Palette;
use gpui::{
    div, Div, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Stateful,
    Styled as _,
};

/// A filled tag saying `label`, with a dot of `dot` before it when there is one. `id` names it in the window
/// (the label is `{id}-label`, the dot `{id}-dot`).
pub fn tag(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    dot: Option<Hsla>,
    palette: &Palette,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let (outer, words, mark) = (id.clone(), format!("{id}-label"), format!("{id}-dot"));
    let dot = dot.map(|colour| {
        div()
            .debug_selector(move || mark.clone())
            .size(DOT_SIZE)
            .flex_none()
            .rounded_full()
            .bg(colour)
    });
    div()
        .id(id)
        .debug_selector(move || outer.to_string())
        .h(CHIP_HEIGHT)
        .pl(CHIP_PADDING_START)
        .pr(CHIP_PADDING_END)
        .flex()
        .min_w_0()
        .items_center()
        .gap(ICON_TEXT_GAP)
        .rounded(CHIP_RADIUS)
        .bg(palette.fill)
        .text_style(CONTROL_LABEL)
        .text_color(palette.text_secondary)
        .whitespace_nowrap()
        .children(dot)
        .child(
            div()
                .debug_selector(move || words.clone())
                .min_w_0()
                .truncate()
                .child(label.into()),
        )
}
