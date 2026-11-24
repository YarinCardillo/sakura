//! An empty state: what the page would list is not there. A title, one sentence that says why and what
//! fills it, and the one action when there is one, centred in the room the page gives it.

use crate::theme::kit::scale::{Typeset as _, SPACE_S, SPACE_XXXL, TEXT_BODY, TEXT_HEADLINE};
use crate::theme::kit::EMPTY_STATE_MAX_WIDTH;
use crate::theme::Palette;
use gpui::{
    div, AnyElement, ElementId, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, Styled as _,
};

/// The empty state named `id` (the title is `{id}-title`, the sentence `{id}-sentence`): it fills the room
/// it is given and centres its parts in it.
pub fn empty_state(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    sentence: Option<SharedString>,
    action: Option<AnyElement>,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let (outer, title_selector, sentence_selector) =
        (id.clone(), format!("{id}-title"), format!("{id}-sentence"));
    div()
        .id(ElementId::from(id))
        .debug_selector(move || outer.to_string())
        .size_full()
        .p(SPACE_XXXL)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_S)
        .child(
            div()
                .debug_selector(move || title_selector.clone())
                .text_style(TEXT_HEADLINE)
                .text_color(palette.text)
                .child(title.into()),
        )
        .children(sentence.map(|sentence| {
            div()
                .debug_selector(move || sentence_selector.clone())
                .max_w(EMPTY_STATE_MAX_WIDTH)
                .text_style(TEXT_BODY)
                .text_color(palette.text_tertiary)
                .text_center()
                .child(sentence)
        }))
        .children(action)
}
