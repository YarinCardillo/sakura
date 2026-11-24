//! The rows of a section: a group is a raised box with a hairline between its rows, a row has its name
//! (and the sentence under it) at the left and its control at the right.

use crate::l10n::{tr, Key};
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Palette};
use crate::ui::kit;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _,
};

/// A box of `rows`, each already a [`row`], with a hairline between two of them.
pub(in crate::ui) fn group(rows: Vec<AnyElement>, palette: &Palette) -> AnyElement {
    let hairline = palette.border;
    let rows = rows.into_iter().enumerate().map(|(nth, row)| {
        div()
            .when(nth > 0, |row| row.border_t_1().border_color(hairline))
            .child(row)
    });
    div()
        .flex()
        .flex_col()
        .rounded(sizes::GROUP_RADIUS)
        .bg(palette.grouped)
        .children(rows)
        .into_any_element()
}

/// One row: `name`, with `note` under it when there is one, at the left; `control` at the right; and
/// `below` (what a press told) under both. `id` names the row in the window.
pub(in crate::ui) fn row(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    below: Option<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    row_with_mark(id, None, name, note, control, below, palette)
}

/// A [`row`] with a bare `mark` before its name, when there is one: the kit's settings row.
pub(super) fn row_with_mark(
    id: impl Into<SharedString>,
    mark: Option<AnyElement>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    below: Option<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    kit::settings_row_with(id, mark, name, note, control, below, palette).into_any_element()
}

/// A [`row`] whose name acts on its control when it is clicked (a switch is pressed, a field takes the
/// caret).
pub(super) fn row_acting(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    (below, target): (Option<AnyElement>, kit::LabelTarget),
    palette: &Palette,
) -> AnyElement {
    let acting = (below, Some(target));
    kit::settings_row_acting(id, None, name, note, control, acting, palette).into_any_element()
}

/// A sentence of the section, quiet, between its groups.
pub(super) fn sentence(words: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    div()
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(words.into())
        .into_any_element()
}

/// A filled button with a word that comes from data (an action's label, a connector's name): a press
/// calls `on_press`. `id` names it in the window.
pub(crate) fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_press: super::controls::OnPress,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let palette = crate::theme::Theme::palette(cx);
    kit::button(id, label, kit::ButtonKind::Card, &palette)
        .on_click(move |_, window, cx| on_press(window, cx))
}

/// A control with "Use default" before it while its value is not the default; the control alone while it
/// is. `id` names the reset in the window as `<id>-default`.
pub(super) fn with_default(
    id: &str,
    is_changed: bool,
    on_press: super::controls::OnPress,
    control: impl IntoElement,
    palette: &Palette,
) -> AnyElement {
    let reset = is_changed.then(|| {
        let selector = format!("{id}-default");
        let (idle, hover) = (palette.text_tertiary, palette.text);
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .flex_none()
            .whitespace_nowrap()
            .text_size(theme::TEXT_LABEL)
            .text_color(idle)
            .hover(move |style| style.text_color(hover))
            .child(tr(Key("settings_modal.use_default")))
            .on_click(move |_, window, cx| on_press(window, cx))
    });
    div()
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .children(reset)
        .child(control)
        .into_any_element()
}
