//! What the conversation shows when it has no rows yet, and the buttons of the conversation. Start-up is a
//! quiet line where the first row will be, never a spinner in the middle of the pane.

use super::{column, rows};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::text;
use gpui::{
    div, AnyElement, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled,
};

/// How a button is painted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// The one action a block leads to: the action colour, filled.
    Primary,
    /// Every other action: a quiet fill, never an outline.
    Secondary,
    /// The same on a surface that already has the quiet fill (the card): one step above it, so the
    /// button is still a surface and not bare words.
    OnFill,
    /// The choice that should be reached for last: no fill at rest, the quiet ink.
    Quiet,
}

/// The space above the composer with one waiting line at its top, under `sign` when there is one.
pub(super) fn waiting(
    sign: Option<AnyElement>,
    words: &'static str,
    palette: &Palette,
) -> AnyElement {
    let lines = div()
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .children(sign)
        .child(rows::waiting_label(words, palette));
    top(lines.into_any_element())
}

/// The space above the composer with one sentence, and under it a hint when there is something more to
/// say than the composer already does.
pub(super) fn quiet(
    sentence: &'static str,
    hint: Option<&'static str>,
    palette: &Palette,
) -> AnyElement {
    let hint = hint.map(|hint| text(theme::TEXT_SMALL, palette.text_tertiary).child(hint));
    let lines = div()
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .child(text(theme::TEXT_BODY, palette.text_secondary).child(sentence))
        .children(hint);
    top(lines.into_any_element())
}

fn top(content: AnyElement) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .child(column().pt(tokens::TRANSCRIPT_PADDING_Y).child(content))
        .into_any_element()
}

/// A button of the conversation, and of the pages that lead to one; the caller adds what a click does.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    face: Face,
    palette: &Palette,
) -> Stateful<Div> {
    let (fill, hover, ink) = match face {
        Face::Primary => (palette.primary, palette.primary, palette.primary_foreground),
        Face::Secondary => (palette.fill, palette.fill_hover, palette.text),
        Face::OnFill => (palette.fill_hover, palette.fill_hover, palette.text),
        Face::Quiet => (
            gpui::transparent_black(),
            palette.fill_hover,
            palette.text_secondary,
        ),
    };
    div()
        .id(id)
        .h(tokens::BUTTON_HEIGHT)
        .px(tokens::BUTTON_PADDING_X)
        .flex()
        .flex_none()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .bg(fill)
        .text_color(ink)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .active(|style| style.opacity(tokens::PRESSED_OPACITY))
        .child(label.into())
}
