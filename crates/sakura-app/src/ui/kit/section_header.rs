//! A section header: the small grey name over a group of rows, with an optional control at its end. It
//! starts where the text of the rows under it starts.

use crate::theme::kit::scale::{
    Typeset as _, CONTROL_REGULAR, ROW_PADDING, TEXT_FOOTNOTE, WEIGHT_MEDIUM,
};
use crate::theme::Palette;
use gpui::{
    div, AnyElement, ElementId, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, Styled as _,
};

/// The header named `id` (its title is `{id}-title`) saying `title`, with `trailing` at the end.
pub fn section_header(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let (outer, title_selector) = (id.clone(), format!("{id}-title"));
    div()
        .id(ElementId::from(id))
        .debug_selector(move || outer.to_string())
        .h(CONTROL_REGULAR)
        .px(ROW_PADDING)
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .debug_selector(move || title_selector.clone())
                .text_style(TEXT_FOOTNOTE)
                .font_weight(WEIGHT_MEDIUM)
                .text_color(palette.text_tertiary)
                .child(title.into()),
        )
        .children(trailing)
}
