//! A field with its label: the label is always on screen, above the control, and a hint may stand under
//! it. A control is never shown on its own.

use crate::theme::kit::scale::{Typeset as _, FIELD_GAP, TEXT_FOOTNOTE, WEIGHT_MEDIUM};
use crate::theme::Palette;
use gpui::{
    div, Div, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _,
};

#[cfg(any(test, feature = "snapshot"))]
/// `control` (an input, a menu button, a stepper) under its visible `label`, and `hint` under it when there is
/// one. `id` names the field in the window (`{id}-label`, `{id}-hint`).
pub fn field(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    control: impl IntoElement,
    hint: Option<SharedString>,
    palette: &Palette,
) -> Div {
    let id: SharedString = id.into();
    let (label_selector, hint_selector) = (format!("{id}-label"), format!("{id}-hint"));
    div()
        .flex()
        .flex_col()
        .gap(FIELD_GAP)
        .child(
            div()
                .debug_selector(move || label_selector.clone())
                .text_style(TEXT_FOOTNOTE)
                .font_weight(WEIGHT_MEDIUM)
                .text_color(palette.text_secondary)
                .child(label.into()),
        )
        .child(control)
        .children(hint.map(|hint| {
            div()
                .debug_selector(move || hint_selector.clone())
                .text_style(TEXT_FOOTNOTE)
                .text_color(palette.text_tertiary)
                .child(hint)
        }))
}
