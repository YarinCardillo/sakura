//! What every section of Settings is made of: a page with its title, groups of rows (a name at the
//! left, its control at the right), filled fields, a choice between a few words, a menu of many, and
//! the two things a row can say under itself (it is being applied; it failed, in place).

use crate::icons;
use crate::l10n::{say, tr, Key};
use crate::theme::settings as sizes;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{failure_sentence, text};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, ClickEvent, Div, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use sakura_core::error::BridgeError;
use std::rc::Rc;

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

/// A press handler of a control.
pub type OnPress = Rc<dyn Fn(&mut Window, &mut App)>;

/// The secondary button: a filled box a step above its surface, never an outline.
pub fn secondary(cx: &App) -> ButtonCustomVariant {
    let palette = Theme::palette(cx);
    ButtonCustomVariant::new(cx)
        .color(palette.fill)
        .foreground(palette.text)
        .hover(palette.fill_hover)
        .active(palette.fill_hover)
}

/// A filled button with one word, at the one control height.
pub fn button(id: &'static str, label: &'static str, on_press: OnPress, cx: &App) -> Button {
    Button::new(id)
        .debug_selector(move || id.into())
        .custom(secondary(cx))
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(
            div()
                .text_size(theme::TEXT_SMALL)
                .font_weight(theme::WEIGHT_MEDIUM)
                .whitespace_nowrap()
                .child(label),
        )
        .on_click(move |_: &ClickEvent, window, cx| on_press(window, cx))
}

/// One section: its title, then its groups, in one column.
pub fn page(title: impl Into<SharedString>, groups: Vec<AnyElement>, palette: &Palette) -> Div {
    let heading = text(sizes::PAGE_TITLE, palette.text)
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(title.into());
    div()
        .w_full()
        .max_w(sizes::PAGE_MAX_WIDTH)
        .flex()
        .flex_col()
        .gap(sizes::GROUP_GAP)
        .child(heading)
        .children(groups)
}

/// A group: its name when it has one, then its rows in one well.
pub fn group(title: Option<&'static str>, rows: Vec<AnyElement>, palette: &Palette) -> AnyElement {
    named_group(title.map(SharedString::from), rows, palette)
}

/// [`group`], under a name that is not one of the app's own words (a menu's, as the menu bar has it).
pub fn named_group(
    title: Option<SharedString>,
    rows: Vec<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    let heading = title.map(|title| text(theme::TEXT_LABEL, palette.text_tertiary).child(title));
    let well = div()
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_SURFACE)
        .bg(palette.inset)
        .children(rows);
    div()
        .flex()
        .flex_col()
        .gap(sizes::GROUP_TITLE_GAP)
        .children(heading)
        .child(well)
        .into_any_element()
}

/// One row: `name` at the left, `control` at the right, and under them what the row says of its
/// last change when it failed. While a change is on its way the row says so beside the control, in
/// room that is always there, so nothing moves.
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
            text(theme::TEXT_SMALL, palette.text)
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
        .px(sizes::SETTING_ROW_PADDING_X)
        .flex()
        .flex_col()
        .child(line)
        .children(failure.map(|failure| div().pb(theme::GAP).child(failure)))
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

/// A text field, filled and never outlined, `width` wide.
pub fn field(input: &Entity<InputState>, width: Pixels, palette: &Palette) -> Div {
    div()
        .w(width)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.fill)
        .child(
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false),
        )
}

/// One word of a [`segments`] choice: what names it in the window, its word, whether it is the one
/// chosen, and what a press does.
pub struct Segment {
    pub id: &'static str,
    pub label: SharedString,
    pub is_on: bool,
    pub on_press: OnPress,
}

/// A choice between a few words, all on screen: the chosen one is filled. Choosing changes no size.
pub fn segments(options: Vec<Segment>, palette: &Palette) -> AnyElement {
    let (selected, ink, quiet) = (
        palette.segment_selected,
        palette.text,
        palette.text_secondary,
    );
    let parts = options.into_iter().map(|option| {
        let (id, on_press) = (option.id, option.on_press);
        div()
            .id(id)
            .debug_selector(move || id.into())
            .h_full()
            .px(theme::CONTROL_PADDING_X)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .text_size(theme::TEXT_SMALL)
            .font_weight(theme::WEIGHT_MEDIUM)
            .whitespace_nowrap()
            .text_color(if option.is_on { ink } else { quiet })
            .when(option.is_on, |part| part.bg(selected))
            .on_click(move |_, window, cx| on_press(window, cx))
            .child(option.label)
    });
    div()
        .h(theme::CONTROL_HEIGHT)
        .p(theme::TAB_GAP)
        .flex()
        .gap(theme::TAB_GAP)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.segment_track)
        .children(parts)
        .into_any_element()
}

/// One choice of a [`menu`]: its word, whether it is the one chosen, what picking it does.
#[derive(Clone)]
pub struct Choice {
    pub label: SharedString,
    pub is_on: bool,
    pub on_pick: OnPress,
}

/// A choice among many: a filled button showing the chosen word, opening a menu of all of them.
pub fn menu(id: &'static str, shown: SharedString, choices: Vec<Choice>, cx: &App) -> AnyElement {
    let palette = Theme::palette(cx);
    let chevron = Icon::new(icons::CHEVRON_OPEN)
        .with_size(theme::MARK_SIZE)
        .text_color(palette.text_secondary);
    let label = div()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_SMALL)
        .whitespace_nowrap()
        .child(shown)
        .child(chevron);
    Button::new(id)
        .debug_selector(move || id.into())
        .custom(secondary(cx))
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(label)
        .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
            let items = choices.iter().map(|choice| {
                let on_pick = choice.on_pick.clone();
                PopupMenuItem::new(choice.label.clone())
                    .checked(choice.is_on)
                    .on_click(move |_: &ClickEvent, window, cx| on_pick(window, cx))
            });
            let menu = menu
                .min_w(sizes::CHOICE_MENU_WIDTH)
                .max_h(sizes::CHOICE_MENU_HEIGHT)
                .scrollable(true);
            items.fold(menu, |menu, item| menu.item(item))
        })
        .into_any_element()
}
