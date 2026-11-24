//! The controls of Settings: a filled button, filled fields, a choice between a few words, a menu of
//! many, and the two things a row can say under itself (it is being applied; it failed, in place).
//! The parts of a section that are not one setting (a project's form, the flow that adds one) set
//! their rows here too, in the kit's look: a name at the left, its control at the right, a hairline
//! between two rows.

use crate::icons;
use crate::l10n::{say, tr, Key};
use crate::theme::kit::scale::{Typeset as _, CONTROL_LABEL, CONTROL_REGULAR, SPACE_M};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{failure_sentence, text};
use crate::ui::kit::{scroll_dropdown, ScrollMenuItem};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnimationExt as _, AnyElement, App, ClickEvent, Div, ElementId, Entity,
    InteractiveElement as _, IntoElement, Length, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{Icon, Sizable as _};
use sakura_core::error::BridgeError;

/// Something read from the service: on its way, here, or failed (and then asked again only by the
/// person).
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded<T> {
    Reading,
    Ready(T),
    Failed(BridgeError),
}

impl<T> Loaded<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loaded::Ready(value) => Some(value),
            Loaded::Reading | Loaded::Failed(_) => None,
        }
    }
}

/// What a row says of the change it sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// On its way; the number tells this sending from an earlier one of the same row.
    Applying(u64),
    Failed(BridgeError),
}

/// The kit's choice between a few words, set in the form's rows, and the press handler it takes.
#[allow(unused_imports)]
pub use crate::ui::kit::{segment_track as segments, OnPress, Segment};

/// The secondary button: a filled box a step above its surface, never an outline.
pub fn secondary(cx: &App) -> ButtonCustomVariant {
    let palette = Theme::palette(cx);
    ButtonCustomVariant::new(cx)
        .color(palette.fill)
        .foreground(palette.text)
        .hover(palette.fill_hover)
        .active(palette.fill_hover)
}

/// A filled button with one word, at the one control height. `id` names it in the window and keeps
/// its pressed and hovered state: a button built per row takes an id of its own.
pub fn button(
    id: impl Into<SharedString>,
    label: &'static str,
    on_press: OnPress,
    cx: &App,
) -> Button {
    morphing_button(id, label, None, on_press, cx)
}

/// A [`button`] whose words morph when they change: with a `state` (any word that names what the label says)
/// the label fades in again each time the state changes, over the shared fast duration, as a Text Morph does;
/// with none it is a plain button. Reduced motion shows the new words at once.
pub fn morphing_button(
    id: impl Into<SharedString>,
    label: &'static str,
    state: Option<&str>,
    on_press: OnPress,
    cx: &App,
) -> Button {
    let id: SharedString = id.into();
    let words = div()
        .text_style(CONTROL_LABEL)
        .whitespace_nowrap()
        .child(label);
    let words = match (state, cx.reduce_motion()) {
        (Some(state), false) => words
            .id(SharedString::from(format!("{id}-words")))
            .with_animation(
                SharedString::from(format!("{id}-morph-{state}")),
                gpui::Animation::new(theme::DURATION_FAST).with_easing(crate::ui::slide::ease_out),
                |words, progress| words.opacity(progress),
            )
            .into_any_element(),
        _ => words.into_any_element(),
    };
    let selector = id.clone();
    Button::new(id)
        .debug_selector(move || selector.to_string())
        .custom(secondary(cx))
        .bg(crate::theme::Theme::palette(cx).fill)
        .h(CONTROL_REGULAR)
        .px(SPACE_M)
        .rounded(theme::RADIUS_CONTROL)
        .child(words)
        .on_click(move |_: &ClickEvent, window, cx| on_press(window, cx))
}

/// A form inside a section (a project, the flow that adds one): its title, then its groups.
pub fn page(title: impl Into<SharedString>, groups: Vec<AnyElement>, palette: &Palette) -> Div {
    let heading = text(sizes::SETTING_TEXT, palette.text)
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(title.into());
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sizes::GROUP_GAP)
        .child(heading)
        .children(groups)
}

/// A group of a form: its name when it has one, in the kit's muted words, then its rows with a
/// hairline between two of them.
pub fn group(title: Option<&'static str>, rows: Vec<AnyElement>, palette: &Palette) -> AnyElement {
    let heading = title.map(|title| text(sizes::SETTING_TEXT, palette.text_tertiary).child(title));
    let hairline = palette.border;
    let rows = rows.into_iter().enumerate().map(|(nth, row)| {
        div()
            .when(nth > 0, |row| row.border_t_1().border_color(hairline))
            .child(row)
    });
    div()
        .flex()
        .flex_col()
        .gap(sizes::GROUP_TITLE_GAP)
        .children(heading)
        .child(div().flex().flex_col().children(rows))
        .into_any_element()
}

/// One row of a form: `name` at the left, `control` at the right, and under them what the row says
/// of its last change when it failed. While a change is on its way the row says so beside the
/// control, in room that is always there, so nothing moves.
pub fn row(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    control: impl IntoElement,
    sent: Option<&Sent>,
    palette: &Palette,
) -> AnyElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    let is_applying = matches!(sent, Some(Sent::Applying(_)));
    let applying = text(theme::TEXT_LABEL, palette.text_tertiary)
        .flex_1()
        .min_w_0()
        .flex()
        .justify_end()
        .whitespace_nowrap()
        .overflow_hidden()
        .children(is_applying.then(|| tr(Key("settings.applying"))));
    let line = div()
        .min_h(sizes::SETTING_ROW_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .child(
            text(sizes::SETTING_TEXT, palette.text)
                .flex_none()
                .child(name.into()),
        )
        .child(applying)
        .child(div().flex_none().child(control));
    let failure = match sent {
        Some(Sent::Failed(error)) => Some(failed(format!("{id}-failed"), error, palette)),
        _ => None,
    };
    div()
        .debug_selector(move || selector.to_string())
        .py(theme::GAP)
        .flex()
        .flex_col()
        .child(line)
        .children(failure)
        .into_any_element()
}

/// A failure said in place: Sakura's sentence, with what `box` wrote as its details.
pub fn failed(id: impl Into<SharedString>, error: &BridgeError, palette: &Palette) -> AnyElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    failure_sentence(
        ElementId::Name(id),
        say(&error.said),
        error.details.as_deref(),
    )
    .debug_selector(move || selector.to_string())
    .text_size(theme::TEXT_LABEL)
    .text_color(palette.danger)
    .into_any_element()
}

/// A quiet sentence: something is being read, a list is empty.
pub fn quiet(sentence: impl Into<SharedString>, palette: &Palette) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text_tertiary)
        .child(sentence.into())
        .into_any_element()
}

/// The state of a text field that shows `hint` while it is empty. No field is made without one, so a
/// person always sees what is to be typed; a field never takes the caret by itself.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub fn state(
    hint: Key,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<InputState>,
) -> InputState {
    state_with(tr(hint), window, cx)
}

/// The same with a hint that is data (a path the program found), not a word of the interface.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub fn state_with(
    hint: impl Into<SharedString>,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<InputState>,
) -> InputState {
    InputState::new(window, cx).placeholder(hint)
}

/// A field of a form: its `label` above it (a sentence that stays when the field has text), then the
/// filled field `width` wide. `id` names the label in the window as `<id>-label`.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub fn labeled_field(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    input: &Entity<InputState>,
    width: impl Into<Length>,
    palette: &Palette,
) -> AnyElement {
    let id: SharedString = id.into();
    let selector = format!("{id}-label");
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_SMALL)
        .child(field_label(label.into(), palette).debug_selector(move || selector.clone()))
        .child(field(input, width, palette).debug_selector(move || id.to_string()))
        .into_any_element()
}

pub fn field_label(label: SharedString, palette: &Palette) -> Div {
    text(theme::TEXT_LABEL, palette.text_secondary)
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(label)
}

/// The one primary button of a form, named for what it does: a filled button on the primary face.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub fn primary_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on_press: OnPress,
    cx: &App,
) -> AnyElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    let palette = Theme::palette(cx);
    let face = palette.primary;
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .flex()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .bg(face)
        .text_color(palette.primary_foreground)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .whitespace_nowrap()
        .hover(move |style| style.bg(face.opacity(theme::PRIMARY_HOVER_OPACITY)))
        .child(label.into())
        .on_click(move |_, window, cx| on_press(window, cx))
        .into_any_element()
}

/// A text field, filled and never outlined, `width` wide.
pub fn field(input: &Entity<InputState>, width: impl Into<Length>, palette: &Palette) -> Div {
    div()
        .w(width.into())
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false),
        )
}

/// Sizes the input viewport for the full line and the widget's vertical padding.
#[cfg(not(feature = "boxes"))]
pub fn styled_input(
    input: &Entity<InputState>,
    style: crate::theme::kit::scale::TextStyle,
) -> Input {
    use gpui_component::{Sizable as _, Size};
    Input::new(input)
        .with_size(Size::Medium)
        .h(style.line_height + Size::Medium.input_py() * 2.)
        .text_style(style)
}

/// One choice of a [`menu`]: its word, whether it is the one chosen, what picking it does.
#[derive(Clone)]
pub struct Choice {
    pub label: SharedString,
    pub is_on: bool,
    pub on_pick: OnPress,
}

/// A choice among many: a filled button showing the chosen word, opening a menu of all of them.
pub fn menu(
    id: impl Into<SharedString>,
    shown: SharedString,
    choices: Vec<Choice>,
    cx: &App,
) -> AnyElement {
    menu_with_width(id, shown, choices, sizes::CHOICE_MENU_WIDTH, cx)
}

pub fn menu_with_width(
    id: impl Into<SharedString>,
    shown: SharedString,
    choices: Vec<Choice>,
    width: gpui::Pixels,
    cx: &App,
) -> AnyElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    let palette = Theme::palette(cx);
    let chevron = Icon::new(icons::CHEVRON_OPEN)
        .with_size(theme::MARK_SIZE)
        .text_color(palette.text_secondary);
    let label = div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_SMALL)
        .child(div().flex_1().min_w_0().truncate().child(shown))
        .child(div().flex_none().child(chevron));
    let button = Button::new(id)
        .debug_selector(move || selector.to_string())
        .custom(secondary(cx))
        .bg(crate::theme::Theme::palette(cx).fill)
        .h(theme::CONTROL_HEIGHT)
        .w(width)
        .flex_none()
        .px(theme::CONTROL_PADDING_X)
        .child(label);
    scroll_dropdown(button, gpui::Anchor::TopRight, move |menu, _, _| {
        let items = choices.iter().map(|choice| {
            let on_pick = choice.on_pick.clone();
            ScrollMenuItem::new(choice.label.clone())
                .checked(choice.is_on)
                .on_click(move |_: &ClickEvent, window, cx| on_pick(window, cx))
        });
        let menu = menu.min_w(width).max_h(sizes::CHOICE_MENU_HEIGHT);
        items.fold(menu, |menu, item| menu.item(item))
    })
    .into_any_element()
}
