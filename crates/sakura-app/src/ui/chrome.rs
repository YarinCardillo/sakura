//! Window chrome without a native title bar: regions that move the window, and the typographic helpers every
//! view shares (tabular figures, monospace for shas and branch names).

use crate::l10n::{tr, Key};
use crate::theme;
use gpui::{
    div, Div, ElementId, Font, FontFeatures, FontWeight, Hsla, InteractiveElement as _,
    MouseButton, ParentElement as _, Pixels, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, TextRun, Window,
};
use gpui_component::button::Button;
use gpui_component::tooltip::Tooltip;
use std::sync::Arc;

/// A region that drags the window, and zooms it on a double click like a title bar. Controls inside it stop
/// the mouse-down from reaching it with [`keep_click`].
pub fn drag_region(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count >= 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
}

/// Keeps a click on a control inside a [`drag_region`] from moving the window.
pub fn keep_click(element: Stateful<Div>) -> Stateful<Div> {
    element.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A block of interface text at `size` in `colour`.
pub fn text(size: Pixels, colour: Hsla) -> Div {
    div().text_size(size).text_color(colour)
}

/// `shown` in the room of the widest of `words` (every word the control can show): a press that changes
/// the word changes no size, so nothing beside the control moves.
pub fn steady_label(shown: &'static str, words: &[&'static str]) -> Div {
    let room = words
        .iter()
        .filter(|word| **word != shown)
        .map(|word| div().h(Pixels::ZERO).overflow_hidden().child(*word));
    div()
        .flex()
        .flex_col()
        .items_center()
        .whitespace_nowrap()
        .child(shown)
        .children(room)
}

/// A kit button at the one control height, its word in the small text at the weight of the view tabs,
/// keeping room for the widest of `words`.
pub fn control(button: Button, shown: &'static str, words: &[&'static str]) -> Button {
    let label = steady_label(shown, words)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM);
    button
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(label)
}

/// What a failure's details are titled, wherever they are shown.
pub const DETAILS: Key = Key("common.details");

/// The sentence of a failure, with what `box` itself wrote under the pointer as its details: the
/// interface says the failure in Italian, and the line of `box` is there only for who asks for it.
pub fn failure_sentence(
    id: impl Into<ElementId>,
    sentence: impl Into<SharedString>,
    details: Option<&str>,
) -> Stateful<Div> {
    let said = div().id(id).child(sentence.into());
    let Some(details) = details else {
        return said;
    };
    let details = SharedString::from(format!("{}: {details}", tr(DETAILS)));
    said.tooltip(move |window, cx| Tooltip::new(details.clone()).build(window, cx))
}

/// How wide `label` is in the interface font at `size` and `weight`.
pub fn text_width(window: &Window, label: &str, size: Pixels, weight: FontWeight) -> f32 {
    let font = Font {
        family: theme::FONT_UI.into(),
        weight,
        ..Font::default()
    };
    let run = TextRun {
        len: label.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let text = SharedString::from(label.to_string());
    let line = window.text_system().shape_line(text, size, &[run], None);
    f32::from(line.width)
}

/// Tabular figures, for every number in the interface.
pub fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// Text in the monospaced family (shas and branch names only), with Geist Mono's plain zero.
pub fn mono() -> Div {
    div()
        .font_family(theme::FONT_MONO)
        .font_features(mono_features())
}

/// Geist Mono's stylistic set 9 replaces its slashed zero with a plain one (no slash, no dot). The terminal
/// and diff views use the same features.
pub fn mono_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![(theme::MONO_PLAIN_ZERO.into(), 1)]))
}
