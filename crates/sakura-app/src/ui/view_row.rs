//! A row of views: text tabs, the chosen one a filled pill, the others quiet. The right pane has a row of
//! them and the header carries the centre's as a switch, both drawn by this one control. Choosing a view changes a fill and a colour, never a weight or a size: nothing beside it
//! moves.

use crate::theme::{self, Palette};
use gpui::{
    div, prelude::FluentBuilder as _, AnyElement, Div, InteractiveElement as _, ParentElement,
    Pixels, Stateful, Styled,
};

/// One view of a row.
pub struct ViewTab {
    /// The name its element is found by, in any language.
    pub id: &'static str,
    pub selector: String,
    pub label: &'static str,
    pub is_current: bool,
    /// What the view counts beside its label, if anything.
    pub counted: Option<AnyElement>,
}

/// The row itself, one row high with a hairline under it; its first tab starts `inset` from its left
/// edge.
pub fn row(selector: &'static str, inset: Pixels, palette: &Palette) -> Div {
    div()
        .debug_selector(move || selector.into())
        .h(theme::PANE_VIEWS_HEIGHT)
        .flex_none()
        .pl(inset)
        .pr(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::TAB_GAP)
        .border_b_1()
        .border_color(palette.border)
}

/// One tab of a row: its label and what it counts. The chosen one is a filled pill in the full ink; the
/// weight is the same for all, so choosing one changes no width. The caller adds what a press does.
pub fn tab(view: ViewTab, palette: &Palette) -> Stateful<Div> {
    let (label, counted) = (view.label, view.counted);
    face(view.id, view.selector, view.is_current, palette)
        .flex_none()
        .px(theme::CONTROL_PADDING_X)
        .gap(theme::GAP_SMALL)
        .child(label)
        .children(counted)
}

/// A tab of a row that can be narrower than its tabs. Before such a row scrolls, the room at each side
/// of every label gives way, until the tab is `floor` wide (its words and the least room beside them):
/// chosen or not, a tab keeps its place.
pub fn tight_tab(view: ViewTab, floor: Pixels, palette: &Palette) -> Stateful<Div> {
    let side = || div().w(theme::CONTROL_PADDING_X);
    let words = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .child(view.label)
        .children(view.counted);
    face(view.id, view.selector, view.is_current, palette)
        .min_w(floor)
        .child(side())
        .child(words)
        .child(side())
}

/// What every tab is, whatever room it keeps at its sides.
fn face(id: &'static str, selector: String, is_current: bool, palette: &Palette) -> Stateful<Div> {
    let (fill, ink) = (palette.fill, palette.text);
    div()
        .id(id)
        .debug_selector(move || selector)
        .h(theme::STATUS_CONTROL_HEIGHT)
        .flex()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .whitespace_nowrap()
        .when(is_current, |tab| tab.bg(fill).text_color(ink))
        .when(!is_current, |tab| {
            tab.text_color(palette.text_secondary)
                .hover(move |style| style.text_color(ink))
        })
}
