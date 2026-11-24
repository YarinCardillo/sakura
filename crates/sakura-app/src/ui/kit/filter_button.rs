//! The Filter button of a page's header: a list-filter mark, its word and a chevron, quiet until the pointer is
//! on it, opening the menu its page gives it.

use super::{scroll_dropdown, ScrollMenu};
use crate::icons;
use crate::theme::{self, Palette};
use crate::ui::chrome::quiet_variant;
use gpui::InteractiveElement as _;
use gpui::{
    div, AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// The button named `id` (which is also its selector) saying `words`; with a `menu` it opens it, with none it
/// is the same button that opens nothing.
pub fn filter_button<M>(
    id: &'static str,
    words: impl Into<SharedString>,
    menu: Option<M>,
    palette: &Palette,
    cx: &App,
) -> AnyElement
where
    M: Fn(ScrollMenu, &mut Window, &mut App) -> ScrollMenu + 'static,
{
    let button = Button::new(id)
        .debug_selector(move || id.into())
        .custom(quiet_variant(palette, cx))
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(label(words));
    match menu {
        Some(build_menu) => {
            scroll_dropdown(button, gpui::Anchor::TopLeft, build_menu).into_any_element()
        }
        None => button.into_any_element(),
    }
}

fn label(words: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .whitespace_nowrap()
        .child(Icon::new(IconName::ListFilter).with_size(theme::HAND_MARK_SIZE))
        .child(words.into())
        .child(Icon::new(icons::CHEVRON_OPEN).with_size(theme::HAND_MARK_SIZE))
}

pub fn checked_item(
    label: impl Into<SharedString>,
    is_checked: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_component::menu::PopupMenuItem {
    gpui_component::menu::PopupMenuItem::new(label)
        .checked(is_checked)
        .on_click(on_click)
}
