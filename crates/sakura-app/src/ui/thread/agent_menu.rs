//! The menu of an agent's account, model and effort, as one component: the data (groups of entries, each
//! with what it says, whether it is in force and what picking it is), and the chip that opens it. The master's
//! composer and the Engine page's step rows both build their groups with `master_agent` and show them here, so
//! the same data reads the same in both.

use super::master::{MenuGroup, MenuPick};
use crate::theme::thread as tokens;
use crate::theme::{self, Theme};
use crate::ui::kit::{menu_chip_at_width, menu_chip_on_fill, scroll_dropdown, ScrollMenuItem};
use gpui::{
    div, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Styled as _, Window,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use std::rc::Rc;

/// What picking a menu entry does.
pub(crate) type OnPick = Rc<dyn Fn(&mut Window, &mut App)>;

/// One entry of a chip's menu: what it says, whether it is the one in force, and what picking it does.
pub(crate) struct MenuChoice {
    pub label: SharedString,
    pub is_on: bool,
    pub on_pick: OnPick,
}

/// A group of a chip's menu: its title and its entries.
pub(crate) struct ChipGroup {
    pub title: Option<SharedString>,
    pub choices: Vec<MenuChoice>,
}

/// The tallest a chip menu may be in a window of `window_height`: what the window leaves above the chip, so a
/// menu longer than that scrolls and every group stays reachable.
pub(crate) fn chip_menu_max_height(window_height: Pixels) -> Pixels {
    (window_height - tokens::CHIP_MENU_CLEARANCE).max(tokens::CHIP_MENU_LEAST_HEIGHT)
}

/// A chip with `label` that opens `groups` above it, each under its title and apart from the one before.
pub(crate) fn grouped_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    groups: Vec<ChipGroup>,
    cx: &App,
) -> impl IntoElement {
    grouped_chip_with_footer(id, label, groups, None, None, cx)
}

pub(crate) fn grouped_chip_with_footer(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    groups: Vec<ChipGroup>,
    footer: Option<MenuChoice>,
    width: Option<gpui::Pixels>,
    cx: &App,
) -> impl IntoElement {
    let id: SharedString = id.into();
    let selector = id.clone();
    let button = match width {
        Some(width) => menu_chip_at_width(gpui::ElementId::from(id), label, width, cx),
        None => menu_chip_on_fill(gpui::ElementId::from(id), label, cx),
    }
    .debug_selector(move || selector.to_string());
    scroll_dropdown(button, gpui::Anchor::BottomLeft, move |menu, window, _| {
        let mut menu = menu
            .min_w(tokens::CHIP_MENU_WIDTH)
            .max_h(chip_menu_max_height(window.viewport_size().height));
        for (at, group) in groups.iter().enumerate() {
            if at > 0 {
                menu = menu.separator();
            }
            if let Some(title) = &group.title {
                menu = menu.item(title_item(at, title.clone()));
            }
            for (nth, choice) in group.choices.iter().enumerate() {
                let on_pick = choice.on_pick.clone();
                let item = entry_item((at, nth), choice.label.clone(), choice.is_on);
                menu =
                    menu.item(item.on_click(move |_: &ClickEvent, window, cx| on_pick(window, cx)));
            }
        }
        if let Some(action) = &footer {
            let press = action.on_pick.clone();
            menu = menu.footer().item(
                ScrollMenuItem::new(action.label.clone())
                    .on_click(move |_, window, cx| press(window, cx)),
            );
        }
        menu
    })
}

/// A group's title: words over its entries, which nothing picks.
fn title_item(group: usize, title: SharedString) -> ScrollMenuItem {
    ScrollMenuItem::element(move |_, cx| {
        let palette = Theme::palette(cx);
        div()
            .debug_selector(move || format!("chip-title-{group}"))
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(title.clone())
            .into_any_element()
    })
    .disabled(true)
}

/// An entry of a group with its check drawn here, not by the menu: a menu that knows an entry is checked opens
/// scrolled to it, past the titles above it, and a menu of the chip opens at its top.
fn entry_item((group, nth): (usize, usize), label: SharedString, is_on: bool) -> ScrollMenuItem {
    ScrollMenuItem::element(move |_, _| {
        let mark = match is_on {
            true => Icon::new(IconName::Check).xsmall().into_any_element(),
            false => div().into_any_element(),
        };
        div()
            .debug_selector(move || format!("chip-entry-{group}-{nth}"))
            .flex()
            .items_center()
            .gap(theme::GAP_SMALL)
            .child(div().w(tokens::CHIP_CHECK_SLOT).flex_none().child(mark))
            .child(label.clone())
            .into_any_element()
    })
}

/// The groups of a menu as the chip shows them: each entry's pick is what `on_pick` makes of it.
pub(crate) fn chip_groups(
    groups: &[MenuGroup],
    on_pick: impl Fn(&MenuPick) -> OnPick,
) -> Vec<ChipGroup> {
    groups
        .iter()
        .map(|group| ChipGroup {
            title: group.title.clone(),
            choices: group
                .entries
                .iter()
                .map(|entry| MenuChoice {
                    label: entry.name.clone(),
                    is_on: entry.is_on,
                    on_pick: on_pick(&entry.pick),
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn a_chip_menu_in_a_1280_by_800_window_is_shorter_than_the_window_and_in_a_tiny_one_keeps_a_floor(
    ) {
        assert!(chip_menu_max_height(px(800.)) < px(800.) - px(100.));
        assert_eq!(
            chip_menu_max_height(px(100.)),
            tokens::CHIP_MENU_LEAST_HEIGHT
        );
    }
}
