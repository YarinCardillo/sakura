//! The row: the frame of every list, settings and inbox line. One kind sets the height, the room at each
//! side and the radius of its fill; the fill (under the pointer, selected) is the same in all of them and the
//! weight of the words never changes with it.

use crate::theme::kit::scale::{
    Typeset as _, CONTROL_LABEL, CONTROL_LARGE, CONTROL_REGULAR, RADIUS_CONTROL, ROW_PADDING,
    SETTINGS_ROW_PADDING, SPACE_M, SPACE_S, SPACE_XS, TEXT_BODY, TEXT_FOOTNOTE,
};
use crate::theme::kit::SETTINGS_ROW_MIN_HEIGHT;
use crate::theme::Palette;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window,
};
use std::rc::Rc;

/// What a row lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A line of a list (a lane, a file, a choice): the large control tall.
    #[cfg(any(test, feature = "snapshot", not(feature = "boxes")))]
    List,
    /// A setting: a name and its control, with room for a sentence under the name.
    Settings,
    /// An ask of the Inbox: two lines, so as tall as its content asks.
    Inbox,
}

/// The frame of a row named `id`: its height, its room at each side, a fill under the pointer and, when
/// `is_selected`, the selected fill. The caller adds the content.
pub fn row(
    kind: RowKind,
    id: impl Into<SharedString>,
    is_selected: bool,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let (hover, selected) = (palette.fill, palette.selected_row);
    let frame = div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .items_center()
        .gap(ROW_PADDING);
    match kind {
        #[cfg(any(test, feature = "snapshot", not(feature = "boxes")))]
        RowKind::List => frame
            .h(CONTROL_LARGE)
            .px(ROW_PADDING)
            .rounded(RADIUS_CONTROL),
        RowKind::Inbox => frame
            .min_h(CONTROL_LARGE)
            .px(ROW_PADDING)
            .py(SPACE_S)
            .rounded(RADIUS_CONTROL),
        RowKind::Settings => frame
            .min_h(SETTINGS_ROW_MIN_HEIGHT)
            .px(SETTINGS_ROW_PADDING)
            .py(SPACE_M),
    }
    .when(kind != RowKind::Settings, |frame| {
        frame
            .when(is_selected, |frame| frame.bg(selected))
            .hover(move |style| match is_selected {
                true => style,
                false => style.bg(hover),
            })
    })
}

/// Subtract the library's vertical padding from `CONTROL_LARGE` so popup rows keep the standard height.
pub fn command_row(
    lead: Option<AnyElement>,
    label: impl Into<SharedString>,
    key: Option<AnyElement>,
    frame: Pixels,
) -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .items_center()
        .gap(SPACE_S)
        .h(CONTROL_LARGE - frame * 2.)
        .text_style(CONTROL_LABEL)
        .children(lead)
        .child(div().min_w_0().truncate().child(label.into()))
        .child(div().flex_1())
        .children(key)
}

/// What a press of a setting's name does to its control.
pub type OnLabelPress = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a click on the name of a setting does to its control, as a click on a label does on the web.
#[derive(Clone)]
pub enum LabelTarget {
    /// The caret goes to the field.
    Focus(FocusHandle),
    /// The control is pressed: a switch asks for the other state.
    Press(OnLabelPress),
}

/// A setting: `name` (with `note` under it) at the start, `control` at the end.
#[cfg(any(test, feature = "snapshot"))]
pub fn settings_row(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    settings_row_with(id, None, name, note, control, None, palette)
}

/// A setting with an optional leading mark and feedback below its control.
pub fn settings_row_with(
    id: impl Into<SharedString>,
    mark: Option<AnyElement>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    below: Option<AnyElement>,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    settings_row_acting(id, mark, name, note, control, (below, None), palette)
}

/// A [`settings_row_with`] whose name acts on its control when it is clicked (`{id}-label` names it in the
/// window): `below` and the `target` of the click come as a pair.
pub fn settings_row_acting(
    id: impl Into<SharedString>,
    mark: Option<AnyElement>,
    name: impl Into<SharedString>,
    note: Option<SharedString>,
    control: impl IntoElement,
    (below, target): (Option<AnyElement>, Option<LabelTarget>),
    palette: &Palette,
) -> Stateful<gpui::Div> {
    let is_single_line = note.is_none() && below.is_none();
    let id: SharedString = id.into();
    let (name_selector, note_selector) = (format!("{id}-name"), format!("{id}-note"));
    let words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACE_XS)
        .child(
            div()
                .debug_selector(move || name_selector.clone())
                .text_style(TEXT_BODY)
                .text_color(palette.text)
                .child(name.into()),
        )
        .children(note.map(|note| {
            div()
                .debug_selector(move || note_selector.clone())
                .text_style(TEXT_FOOTNOTE)
                .text_color(palette.text_tertiary)
                .child(note)
        }));
    let label_selector = format!("{id}-label");
    let words = match target {
        Some(target) => words
            .id(SharedString::from(label_selector.clone()))
            .debug_selector(move || label_selector.clone())
            .on_click(move |_, window, cx| match &target {
                LabelTarget::Focus(handle) => window.focus(handle, cx),
                LabelTarget::Press(press) => press(window, cx),
            })
            .into_any_element(),
        None => words.into_any_element(),
    };
    let line = div()
        .w_full()
        .flex()
        .items_center()
        .gap(SPACE_M)
        .children(mark)
        .child(words)
        .child(div().flex_none().child(control));
    row(RowKind::Settings, id, false, palette)
        .when(is_single_line, |row| {
            row.min_h(CONTROL_REGULAR + SPACE_S * 2.).py(SPACE_S)
        })
        .flex_col()
        .items_start()
        .justify_center()
        .child(line)
        .children(below.map(|below| div().w_full().pt(SPACE_S).child(below)))
}

#[cfg(not(feature = "boxes"))]
pub fn press_row(
    row: Stateful<gpui::Div>,
    palette: &Palette,
    press: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let ring = super::button::keyboard_ring(palette);
    row.focusable()
        .tab_stop(true)
        .focus_visible(move |style| style.shadow(ring))
        .on_click(press)
}
