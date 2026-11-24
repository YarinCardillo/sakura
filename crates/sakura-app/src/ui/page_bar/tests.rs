//! The page bar driven like a person would, on GPUI's test platform: a simulated pointer and keys on a test
//! window holding the bar over an empty page.

use super::{keys, LaneTab, PageBar, PageBarEvent};
use crate::theme::{Appearance, Theme};
use crate::ui::kit::StateGlyph;
use gpui::{
    div, px, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window,
};
use sakura_core::tabs::{Page, Tabs};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// The window's content: the bar over the header row of the page shown, with a marked control in it.
struct Host {
    bar: Entity<PageBar>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let controls = div()
            .debug_selector(|| "page-controls".into())
            .w(px(40.))
            .h(px(20.))
            .into_any_element();
        let bar = self.bar.read(cx);
        let header = bar.header_row(bar.tabs().active(), controls, &palette);
        div().size_full().child(self.bar.clone()).child(header)
    }
}

pub(super) fn lane(name: &str) -> Page {
    Page::Lane(name.to_string())
}

/// A bar over `tabs` in a test window, its first frame drawn, with the keys installed.
pub(super) fn open(
    cx: &mut TestAppContext,
    tabs: Tabs,
) -> (&mut VisualTestContext, Entity<PageBar>, Rc<Cell<usize>>) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(true);
    });
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let changes = Rc::new(Cell::new(0));
    let counted = changes.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let bar = cx.new(|cx| PageBar::new(tabs, cx));
        keys::install(&bar, cx);
        cx.subscribe(&bar, move |_, _, _: &PageBarEvent, _| {
            counted.set(counted.get() + 1)
        })
        .detach();
        *kept.borrow_mut() = Some(bar.clone());
        let host = cx.new(|_| Host { bar });
        gpui_component::Root::new(host, window, cx)
    });
    let bar = slot.borrow_mut().take().expect("the bar was built");
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    draw(cx);
    (cx, bar, changes)
}

pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing named {selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    draw(cx);
}

pub(super) fn pages(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Vec<Page> {
    cx.read_entity(bar, |bar, _| {
        bar.tabs().tabs().iter().map(|t| t.page().clone()).collect()
    })
}

pub(super) fn shown(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Page {
    cx.read_entity(bar, |bar, _| bar.tabs().active().page().clone())
}

fn with_lane(name: &str) -> Tabs {
    let mut tabs = Tabs::new();
    tabs.open_new(lane(name));
    tabs.activate_digit(1);
    tabs
}

#[gpui::test]
fn two_tabs_render_one_filled_pill_and_one_plain_pill_each_with_its_icon(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, with_lane("retries"));
    assert!(cx.debug_bounds("tab-open").is_some(), "the open pill");
    assert!(
        cx.debug_bounds("tab-1").is_none(),
        "the open tab is the filled pill"
    );
    assert!(cx.debug_bounds("tab-2").is_some(), "the other tab");
    assert!(cx.debug_bounds("tab-icon-1").is_some());
    assert!(cx.debug_bounds("tab-icon-2").is_some());
    bar.update(cx, |bar, cx| {
        bar.activate_digit(2, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("tab-1").is_some(), "the Inbox is plain now");
}

#[gpui::test]
fn clicking_a_compact_pill_activates_its_tab(cx: &mut TestAppContext) {
    let (cx, bar, changes) = open(cx, with_lane("retries"));
    click(cx, "tab-2");
    assert_eq!(shown(&bar, cx), lane("retries"));
    assert_eq!(changes.get(), 1, "the desk is told once");
    assert!(
        cx.debug_bounds("tab-1").is_some(),
        "the Inbox is compact now"
    );
}

#[gpui::test]
fn the_close_button_closes_a_tab_and_the_last_tab_has_none(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, with_lane("retries"));
    assert!(cx.debug_bounds("tab-close-2").is_some());
    click(cx, "tab-close-2");
    assert_eq!(pages(&bar, cx), [Page::Inbox]);
    assert!(
        cx.debug_bounds("tab-close-1").is_none(),
        "no button on the last tab"
    );
    assert!(cx.debug_bounds("tab-close-open").is_none());
    let closed = bar.update(cx, |bar, cx| bar.close_active(cx));
    assert!(!closed, "the last tab stays");
}

#[gpui::test]
fn back_is_disabled_on_a_fresh_tab_and_works_after_a_navigation(cx: &mut TestAppContext) {
    let (cx, bar, changes) = open(cx, Tabs::new());
    click(cx, "bar-back");
    assert_eq!(changes.get(), 0, "no history: nothing happens");
    bar.update(cx, |bar, cx| bar.navigate(Page::Master, cx));
    draw(cx);
    assert_eq!(shown(&bar, cx), Page::Master);
    click(cx, "bar-back");
    assert_eq!(shown(&bar, cx), Page::Inbox);
    click(cx, "bar-forward");
    assert_eq!(shown(&bar, cx), Page::Master);
}

#[gpui::test]
fn the_plus_and_cmd_t_each_open_a_new_tab_on_the_master_and_ask_for_an_empty_conversation(
    cx: &mut TestAppContext,
) {
    let (cx, bar, _) = open(cx, Tabs::new());
    let asked = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = asked.clone();
    cx.update(|_, cx| {
        cx.subscribe(&bar, move |_, event: &PageBarEvent, _| {
            if matches!(event, PageBarEvent::NewConversation) {
                seen.set(seen.get() + 1);
            }
        })
        .detach()
    });
    click(cx, "bar-new-tab");
    assert_eq!(pages(&bar, cx), [Page::Inbox, Page::Master]);
    assert_eq!(
        cx.read_entity(&bar, |bar, _| bar.tabs().active().id()),
        cx.read_entity(&bar, |bar, _| bar.tabs().tabs()[1].id())
    );
    cx.simulate_keystrokes("cmd-t");
    assert_eq!(pages(&bar, cx), [Page::Inbox, Page::Master, Page::Master]);
    assert_eq!(asked.get(), 2, "each asks for a conversation of its own");
}

#[gpui::test]
fn the_keys_open_close_go_back_and_select_tabs(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, Tabs::new());
    bar.update(cx, |bar, cx| {
        bar.open_new(Page::Master, cx);
    });
    assert_eq!(pages(&bar, cx).len(), 2);
    cx.simulate_keystrokes("cmd-1");
    assert_eq!(
        cx.read_entity(&bar, |bar, _| bar.tabs().active().id()),
        cx.read_entity(&bar, |bar, _| bar.tabs().tabs()[0].id())
    );
    bar.update(cx, |bar, cx| bar.navigate(Page::Engine, cx));
    cx.simulate_keystrokes("cmd-[");
    assert_eq!(shown(&bar, cx), Page::Inbox);
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(shown(&bar, cx), Page::Engine);
    cx.simulate_keystrokes("cmd-9");
    cx.simulate_keystrokes("cmd-w");
    assert_eq!(pages(&bar, cx), [Page::Engine]);
}

fn thirty_tabs(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<PageBar>) {
    let mut tabs = Tabs::new();
    for number in 1..30 {
        tabs.open_new(lane(&format!("lane-{number}")));
    }
    let (cx, bar, _) = open(cx, tabs);
    (cx, bar)
}

fn assert_open_pill_in_view(cx: &mut VisualTestContext) {
    let row = cx.debug_bounds("page-tabs").expect("the row of tabs");
    let open = cx.debug_bounds("tab-open").expect("the open pill");
    assert!(
        open.left() >= row.left() - px(0.5),
        "{open:?} inside {row:?}"
    );
    assert!(
        open.right() <= row.right() + px(0.5),
        "{open:?} inside {row:?}"
    );
}

#[gpui::test]
fn thirty_tabs_keep_the_open_pill_fully_visible(cx: &mut TestAppContext) {
    let (cx, bar) = thirty_tabs(cx);
    assert_eq!(pages(&bar, cx).len(), 30);
    for digit in [1, 5, 9] {
        bar.update(cx, |bar, cx| bar.activate_digit(digit, cx));
        draw(cx);
        draw(cx);
        assert_open_pill_in_view(cx);
    }
}

#[gpui::test]
fn compact_pills_shrink_to_their_minimum_and_no_further(cx: &mut TestAppContext) {
    let (cx, _bar) = thirty_tabs(cx);
    let pill = cx.debug_bounds("tab-1").expect("a compact pill");
    assert!(pill.size.width >= crate::theme::page_bar::PILL_MIN_WIDTH - px(0.5));
}

#[gpui::test]
fn the_header_names_the_project_of_a_lane_and_puts_the_page_s_controls_at_its_right_end(
    cx: &mut TestAppContext,
) {
    let (cx, bar, _) = open(cx, with_lane("retries"));
    bar.update(cx, |bar, cx| {
        let lane = LaneTab {
            project: "shop".into(),
            name: "Upload retries".into(),
            identity: "lane/upload-retries".into(),
            glyph: StateGlyph::Running,
        };
        bar.set_lanes(HashMap::from([("retries".to_string(), lane)]), cx);
        bar.activate_digit(2, cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("crumb-name").is_some());
    assert!(
        cx.debug_bounds("crumb-project").is_none(),
        "the lane's name is said once in the page, so the header says only the parent"
    );
    let said = bar.read_with(cx, |bar, _| bar.crumb(&lane("retries"), false).in_header());
    assert_eq!(said.as_ref(), "shop");
    let controls = cx.debug_bounds("page-controls").expect("the controls");
    let header = cx.debug_bounds("page-header").expect("the header");
    assert_eq!(header.size.height, crate::theme::page_bar::HEADER_HEIGHT);
    assert!(controls.left() > header.center().x, "at the right end");
    assert!(controls.right() <= header.right());
}

#[gpui::test]
fn the_header_of_a_plain_page_has_only_its_name(cx: &mut TestAppContext) {
    let (cx, _bar, _) = open(cx, Tabs::new());
    assert!(cx.debug_bounds("crumb-name").is_some());
    assert!(cx.debug_bounds("crumb-project").is_none());
}

#[gpui::test]
fn every_pill_carries_the_icon_of_its_page(cx: &mut TestAppContext) {
    let mut tabs = Tabs::new();
    for page in [Page::Master, Page::Lanes, Page::Engine, lane("gone")] {
        tabs.open_new(page);
    }
    tabs.activate_digit(1);
    let (cx, _bar, _) = open(cx, tabs);
    for selector in [
        "tab-icon-1",
        "tab-icon-2",
        "tab-icon-3",
        "tab-icon-4",
        "tab-icon-5",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
}

#[gpui::test]
fn background_menu_pairs_with_active_and_active_menu_lists_peers(cx: &mut TestAppContext) {
    use super::TabMenu;
    let mut tabs = with_lane("peer");
    let last = tabs.open_new(Page::Engine);
    tabs.activate_digit(1);
    let active = tabs.active().id();
    let peer = tabs.tabs()[1].id();
    let (cx, bar, _) = open(cx, tabs);
    cx.read_entity(&bar, |bar, _| {
        assert_eq!(
            bar.tab_menu(active),
            TabMenu::Peers {
                active,
                peers: vec![peer, last]
            }
        );
        assert_eq!(bar.tab_menu(peer), TabMenu::Add { active, peer });
    });
    bar.update(cx, |bar, cx| {
        assert!(bar.create_menu_split(active, peer, cx))
    });
    cx.read_entity(&bar, |bar, _| {
        assert_eq!(bar.tabs().visible(), (active, Some(peer)));
        assert_eq!(bar.tab_menu(last), TabMenu::Disabled);
    });
}

#[gpui::test]
fn right_click_does_not_start_drag_or_activate(cx: &mut TestAppContext) {
    let mut tabs = with_lane("peer");
    tabs.activate_digit(1);
    let (cx, bar, _) = open(cx, tabs);
    let position = cx.debug_bounds("tab-2").unwrap().center();
    cx.simulate_mouse_down(position, gpui::MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(position, gpui::MouseButton::Right, Modifiers::none());
    draw(cx);
    assert_eq!(shown(&bar, cx), Page::Inbox);
    cx.read_entity(&bar, |bar, _| assert!(!bar.is_pressing()));
    cx.simulate_keystrokes("down enter");
    draw(cx);
    cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_some()));
}

#[gpui::test]
fn stale_context_menu_selection_is_a_noop(cx: &mut TestAppContext) {
    let mut tabs = with_lane("peer");
    let active = tabs.tabs()[0].id();
    let peer = tabs.tabs()[1].id();
    tabs.activate(active);
    let (cx, bar, _) = open(cx, tabs);
    bar.update(cx, |bar, cx| {
        let different = bar.open_new(Page::Engine, cx);
        assert!(!bar.create_menu_split(active, peer, cx));
        assert!(bar.activate(active, cx));
        assert!(bar.close(peer, cx));
        assert!(!bar.create_menu_split(active, peer, cx));
        assert!(bar.create_menu_split(active, different, cx));
    });
}

#[gpui::test]
fn active_side_change_does_not_move_pills_or_new_tab_button(cx: &mut TestAppContext) {
    let mut tabs = with_lane("an extra long peer name that cannot fit");
    let left = tabs.tabs()[0].id();
    let right = tabs.tabs()[1].id();
    assert!(tabs.create_split(left, right));
    let (cx, bar, _) = open(cx, tabs);
    let before = [
        super::drag_tests::drawn(&bar, 0, cx),
        super::drag_tests::drawn(&bar, 1, cx),
        cx.debug_bounds("bar-new-tab").unwrap(),
    ];
    bar.update(cx, |bar, cx| assert!(bar.activate(right, cx)));
    draw(cx);
    let after = [
        super::drag_tests::drawn(&bar, 0, cx),
        super::drag_tests::drawn(&bar, 1, cx),
        cx.debug_bounds("bar-new-tab").unwrap(),
    ];
    assert_eq!(before, after);
}
