//! One tab as a pill with the page's icon and name: the shown tab filled, the others plain. A pill under
//! the pointer shows its close button while more than one tab is open.

use super::{LaneTab, PageBar, TabMenu};
use crate::icons;
use crate::l10n::{tr, Key};
use crate::theme::{self, page_bar as bar, Palette};
use crate::ui::chrome::{keep_click, tabular};
use crate::ui::held_keys::HeldKeys;
use crate::ui::kit::StateGlyph;
use crate::ui::tip;
use gpui::{
    div, AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::tabs::{Page, SplitTabs, Tab, TabId};

/// What a page is called, and for a lane the project it belongs to: the name on its tab, and the breadcrumb
/// of its header.
pub(in crate::ui) struct Crumb {
    pub(in crate::ui) project: Option<SharedString>,
    pub(in crate::ui) title: SharedString,
}

impl Crumb {
    /// What the page's header row says: the project a lane belongs to, since the lane's own name is said by
    /// its tab and by the page's title; a page that is no lane says its own name.
    pub(super) fn in_header(&self) -> SharedString {
        self.project.clone().unwrap_or_else(|| self.title.clone())
    }
}

impl PageBar {
    /// The pill of `tab`, which stands at `place` (from 1).
    pub(super) fn pill(
        &self,
        place: usize,
        tab: &Tab,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = tab.id() == self.tabs.active().id();
        let held = HeldKeys::current(cx);
        let crumb = self.crumb(tab.page(), held.is_identity_shown());
        let group: SharedString = format!("pill-{}", tab.id().as_u64()).into();
        let id = tab.id();
        let pill = div()
            .id(("tab", id.as_u64()))
            .group(group.clone())
            .h(bar::PILL_HEIGHT)
            .flex()
            .items_center()
            .gap(bar::PILL_GAP)
            .px(bar::PILL_PADDING_X)
            .rounded(bar::PILL_RADIUS)
            .text_size(theme::TEXT_BODY)
            .min_w_0();
        let is_joined = self
            .tabs
            .split()
            .is_some_and(|pair| pair.other(id).is_some());
        let pill = if is_joined {
            pill.flex_none().w(bar::PILL_MAX_WIDTH)
        } else if is_open {
            pill.flex_none().max_w(bar::OPEN_PILL_MAX_WIDTH)
        } else {
            pill.flex_shrink(1.)
                .min_w(bar::PILL_MIN_WIDTH)
                .max_w(bar::PILL_MAX_WIDTH)
        };
        let pill = if is_open {
            pill.debug_selector(|| "tab-open".into())
                .bg(palette.selected_row)
                .text_color(palette.text)
        } else {
            let hover = (palette.hover, palette.text_secondary);
            pill.debug_selector(move || format!("tab-{place}"))
                .text_color(palette.text_tertiary)
                .hover(move |style| style.bg(hover.0).text_color(hover.1))
        };
        let pill = pill
            .child(self.icon(place, tab.page(), palette))
            .child(div().min_w_0().truncate().child(crumb.title.clone()));
        let words = tab_words(&crumb, place, self.tabs.len());
        let pill = tip::bind(pill, format!("tab-{id:?}"), words, cx);
        let close = (self.tabs.len() > 1).then(|| match held.are_hints_shown() {
            true => hint_slot(place, self.tabs.len(), palette).into_any_element(),
            false => close_button(place, is_open, &group, id, palette, cx).into_any_element(),
        });
        // A press picks the tab up or, released where it went down, chooses it ([`super::drag`]). It is
        // added after `keep_click`, which stops the press reaching the window's own drag.
        let weak = cx.weak_entity();
        keep_click(pill.children(close))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |bar, event: &MouseDownEvent, _, cx| {
                    bar.press_tab(id, event.position, cx);
                }),
            )
            .context_menu(move |menu, window, cx| tab_context_menu(menu, id, &weak, window, cx))
            .into_any_element()
    }

    /// What `page` is called, and for a lane the project it belongs to. A lane the engine does not list
    /// is called by its page.
    pub(in crate::ui) fn crumb(&self, page: &Page, is_identity: bool) -> Crumb {
        let lane = match page {
            Page::Lane(key) => self.lanes.get(key),
            _ => None,
        };
        match (page, lane) {
            (_, Some(lane)) => Crumb {
                project: Some(lane.project.clone()).filter(|project| !project.is_empty()),
                title: match is_identity {
                    true => lane.identity.clone(),
                    false => lane.name.clone(),
                },
            },
            (Page::Project(folder), None) => Crumb {
                project: None,
                title: SharedString::from(std::path::Path::new(folder).file_name().map_or_else(
                    || folder.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )),
            },
            (Page::Track(id), None) => Crumb {
                project: None,
                title: self
                    .track_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| tr(Key("track.one")).into()),
            },
            (page, None) => Crumb {
                project: None,
                title: tr(page_key(page)).into(),
            },
        }
    }

    /// The icon of `page` on its pill; a lane has the mark of its state.
    fn icon(&self, place: usize, page: &Page, palette: &Palette) -> AnyElement {
        let size = bar::PILL_ICON_SIZE;
        let name = match page {
            Page::Inbox => icons::WAITING_PAGE,
            Page::Master => icons::MASTER_PAGE,
            Page::Lanes => icons::LANES_PAGE,
            Page::Scheduled => IconName::Calendar,
            Page::Engine => icons::ENGINE_PAGE,
            Page::Project(_) => icons::PROJECT_PAGE,
            Page::Track(_) => IconName::Layers,
            Page::Lane(key) => {
                let glyph = self
                    .lanes
                    .get(key)
                    .map_or(StateGlyph::NotStarted, |lane: &LaneTab| lane.glyph);
                return div()
                    .debug_selector(move || format!("tab-icon-{place}"))
                    .flex_none()
                    .child(glyph.paint(size, palette))
                    .into_any_element();
            }
        };
        div()
            .debug_selector(move || format!("tab-icon-{place}"))
            .flex_none()
            .child(Icon::new(name).with_size(size))
            .into_any_element()
    }
}

fn tab_context_menu(
    menu: PopupMenu,
    id: TabId,
    bar: &WeakEntity<PageBar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let add = tr(Key("tabs.add_to_split"));
    match bar
        .read_with(cx, |bar, _| bar.tab_menu(id))
        .unwrap_or(TabMenu::Disabled)
    {
        TabMenu::Disabled => menu.item(PopupMenuItem::new(add).disabled(true)),
        TabMenu::Add { active, peer } => {
            menu.item(add_split_item(add.into(), bar.clone(), active, peer))
        }
        TabMenu::Peers { active, peers } => {
            let titled = bar
                .read_with(cx, |bar, _| {
                    peers
                        .iter()
                        .filter_map(|id| {
                            let tab = bar.tabs().tab(*id)?;
                            Some((*id, bar.crumb(tab.page(), false).title))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let bar = bar.clone();
            menu.submenu(add, window, cx, move |menu, _, _| {
                titled.iter().fold(menu, |menu, (id, title)| {
                    menu.item(add_split_item(title.clone(), bar.clone(), active, *id))
                })
            })
        }
        TabMenu::Pair(pair) => menu
            .item(pair_item(
                Key("tabs.swap_sides"),
                bar.clone(),
                id,
                pair,
                PageBar::swap_split,
            ))
            .item(pair_item(
                Key("tabs.close_side"),
                bar.clone(),
                id,
                pair,
                PageBar::close,
            )),
    }
}

fn add_split_item(
    label: SharedString,
    bar: WeakEntity<PageBar>,
    active: TabId,
    peer: TabId,
) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, cx| {
        if !crate::ui::dialog_guard::is_open(cx) {
            drop(bar.update(cx, |bar, cx| bar.create_menu_split(active, peer, cx)));
        }
    })
}

fn pair_item(
    label: Key,
    bar: WeakEntity<PageBar>,
    id: TabId,
    pair: SplitTabs,
    action: fn(&mut PageBar, TabId, &mut Context<PageBar>) -> bool,
) -> PopupMenuItem {
    PopupMenuItem::new(tr(label)).on_click(move |_, _, cx| {
        if !crate::ui::dialog_guard::is_open(cx) {
            drop(bar.update(cx, |bar, cx| {
                if bar
                    .tabs()
                    .split()
                    .is_some_and(|current| (current.left, current.right) == (pair.left, pair.right))
                {
                    action(bar, id, cx);
                }
            }));
        }
    })
}

/// What the tooltip of the tab at `place` (from 1) of `count` says: its title and the key that shows it,
/// Cmd+1 to Cmd+8 for the first eight and Cmd+9 for the last.
fn tab_words(crumb: &Crumb, place: usize, count: usize) -> tip::Words {
    const KEYS: [&str; 9] = [
        "cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6", "cmd-7", "cmd-8", "cmd-9",
    ];
    let digit = tab_digit(place, count);
    let title = crumb.title.clone();
    match digit {
        Some(digit) => tip::Words::with_keys(title, KEYS[digit - 1]),
        None => tip::Words::plain(title),
    }
}

/// The digit of the Cmd key that shows the tab at `place` (from 1) of `count`: 1 to 8 for the first eight and
/// 9 for the last; none for a tab in the middle of a long strip.
fn tab_digit(place: usize, count: usize) -> Option<usize> {
    match place {
        1..=8 => Some(place),
        _ if place == count => Some(9),
        _ => None,
    }
}

/// What stands in the close button's place while Cmd is held: the digit of the key that shows the tab, in
/// a box of the same size so that nothing around it moves.
fn hint_slot(place: usize, count: usize, palette: &Palette) -> gpui::Div {
    let digit = tab_digit(place, count).map(|digit| digit.to_string());
    div()
        .debug_selector(move || format!("tab-hint-{place}"))
        .size(bar::CLOSE_SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(theme::TEXT_LABEL)
        .font_features(tabular())
        .text_color(palette.text_tertiary)
        .children(digit)
}

/// The locale key of the name of `page`.
fn page_key(page: &Page) -> Key {
    match page {
        Page::Inbox => Key("page.inbox"),
        Page::Master => Key("page.master"),
        Page::Lanes => Key("page.lanes"),
        Page::Scheduled => Key("page.scheduled"),
        Page::Engine => Key("page.engine"),
        Page::Lane(_) => Key("page.lane"),
        Page::Project(_) => Key("page.project"),
        Page::Track(_) => Key("track.one"),
    }
}

/// The close button of the pill at `place`: invisible until the pill is under the pointer.
fn close_button(
    place: usize,
    is_open: bool,
    group: &SharedString,
    id: sakura_core::tabs::TabId,
    palette: &Palette,
    cx: &mut Context<PageBar>,
) -> Stateful<gpui::Div> {
    let hover = (palette.hover, palette.text);
    let selector = match is_open {
        true => "tab-close-open".to_string(),
        false => format!("tab-close-{place}"),
    };
    let key = ElementId::from(("tab-close", id.as_u64()));
    let button = div()
        .id(key)
        .debug_selector(move || selector)
        .size(bar::CLOSE_SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(bar::CLOSE_RADIUS)
        .opacity(0.)
        .group_hover(group.clone(), |style| style.opacity(1.))
        .text_color(palette.text_tertiary)
        .hover(move |style| style.bg(hover.0).text_color(hover.1))
        .on_click(cx.listener(move |bar, _, _, cx| {
            bar.close(id, cx);
        }))
        .child(Icon::new(IconName::Close).with_size(bar::PILL_ICON_SIZE));
    keep_click(button)
}
