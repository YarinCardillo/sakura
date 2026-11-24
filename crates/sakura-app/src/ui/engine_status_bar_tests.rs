//! The bottom row of the engine's window: quiet icon buttons and no words.

use super::engine_desk::Desk;
use super::engine_rig_tests::SNAPSHOT;
use crate::menus::ShowSettings;
use crate::theme;
use crate::updates::{self, Release, UpdateState};
use gpui::{px, Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::tabs::Page;
use std::cell::Cell;
use std::rc::Rc;

fn open(cx: &mut TestAppContext) -> (Entity<Desk>, &mut VisualTestContext) {
    let (_rig, desk, cx) = super::engine_desk_tests::open(cx, SNAPSHOT);
    (desk, cx)
}

#[gpui::test]
fn the_bottom_strip_draws_focus_only_for_keyboard_navigation(cx: &mut TestAppContext) {
    struct Strip;
    impl gpui::Render for Strip {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            super::engine_status_bar::view(true, None, cx)
        }
    }
    super::engine_rig_tests::prepare(cx);
    cx.update(super::keyboard::install);
    let (_, cx) = cx.add_window_view(|window, cx| {
        use gpui::AppContext as _;
        let strip = cx.new(|_| Strip);
        gpui_component::Root::new(strip, window, cx)
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx);
    });
    for selector in ["toggle-sidebar", "status-settings", "status-tour"] {
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(strip_ring(cx, selector), "{selector} reached by Tab");
        let at = cx.debug_bounds(selector).expect(selector).center();
        cx.simulate_click(at, Modifiers::none());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!strip_ring(cx, selector), "{selector} after a click");
    }
}

fn strip_ring(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    let center = cx.debug_bounds(selector).expect(selector).center();
    cx.update(|window, _| {
        let center = center.scale(window.scale_factor());
        window.painted_quads().iter().any(|quad| {
            quad.border_color.a > 0.
                && quad.border_widths.top.0 > 0.
                && (quad.bounds.center().x.0 - center.x.0).abs() < 1.
                && (quad.bounds.center().y.0 - center.y.0).abs() < 1.
        })
    })
}

#[gpui::test]
fn the_bar_is_thirty_two_points_high_along_the_foot_with_the_sidebar_toggle_and_settings_first(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open(cx);
    let bar = cx.debug_bounds("status-bar").expect("the bar");
    assert_eq!(bar.size.height, px(32.));
    let window = cx.update(|window, _| window.viewport_size());
    assert_eq!(bar.bottom(), window.height, "at the window's foot");
    assert_eq!(bar.size.width, window.width, "across the whole window");
    let toggle = cx.debug_bounds("toggle-sidebar").expect("the toggle");
    let settings = cx.debug_bounds("status-settings").expect("Settings");
    for button in [toggle, settings] {
        assert_eq!(button.size, gpui::size(px(28.), px(28.)));
    }
    assert_eq!(
        toggle.left(),
        theme::engine_bar::BAR_PADDING_X,
        "the bar's padding from the window's edge"
    );
    assert_eq!(
        settings.left() - toggle.right(),
        theme::engine_bar::BAR_GAP,
        "the bar's gap apart"
    );
    let icon = cx.debug_bounds("toggle-sidebar-icon").expect("the icon");
    assert_eq!(icon.size, gpui::size(px(16.), px(16.)));
}

#[gpui::test]
fn the_bar_says_nothing_of_the_engine_in_words(cx: &mut TestAppContext) {
    let (_desk, cx) = open(cx);
    for gone in ["status-engine", "status-pace", "status-lanes"] {
        assert!(cx.debug_bounds(gone).is_none(), "{gone}");
    }
}

#[gpui::test]
fn tour_stays_in_the_right_half_with_the_sidebar_open_or_closed(cx: &mut TestAppContext) {
    let (_desk, cx) = open(cx);
    for _ in 0..2 {
        let bar = cx.debug_bounds("status-bar").unwrap();
        let tour = cx.debug_bounds("status-tour").unwrap();
        assert!(tour.left() > bar.center().x, "{tour:?}");
        assert_eq!(tour.right(), bar.right() - theme::engine_bar::BAR_PADDING_X);
        let toggle = cx.debug_bounds("toggle-sidebar").unwrap();
        cx.simulate_click(toggle.center(), Modifiers::none());
        cx.run_until_parked();
    }
}

#[gpui::test]
fn the_lane_column_toggle_stands_at_the_right_end_of_a_page_with_a_column_and_is_not_drawn_elsewhere(
    cx: &mut TestAppContext,
) {
    let (desk, cx) = open(cx);
    assert!(
        cx.debug_bounds("toggle-column").is_none(),
        "the Inbox has no column to toggle"
    );
    open_a_lane(&desk, cx);
    let window = cx.update(|window, _| window.viewport_size());
    let column = cx.debug_bounds("toggle-column").expect("the toggle");
    assert_eq!(column.size, gpui::size(px(28.), px(28.)));
    assert!(
        window.width - column.right() == theme::engine_bar::BAR_PADDING_X,
        "the bar's padding from the right edge"
    );
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("toggle-column").is_none());
}

/// Shows the lane `demo`, which has a column.
fn open_a_lane(desk: &Entity<Desk>, cx: &mut VisualTestContext) {
    let key = super::engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn the_update_button_shows_only_when_a_release_is_ready_and_opens_its_page(
    cx: &mut TestAppContext,
) {
    let (desk, cx) = open(cx);
    assert!(cx.debug_bounds("status-update").is_none(), "none ready");
    let release = Release {
        version: "0.4.1".into(),
        url: "https://example.com/releases/0.4.1".into(),
    };
    cx.update(|_, cx| updates::install_for_test(UpdateState::Available(release), cx));
    desk.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let update = cx.debug_bounds("status-update").expect("the button");
    assert_eq!(update.size, gpui::size(px(28.), px(28.)));
    open_a_lane(&desk, cx);
    let update = cx.debug_bounds("status-update").expect("the button");
    let column = cx.debug_bounds("toggle-column").expect("the toggle");
    let tour = cx.debug_bounds("status-tour").expect("the tour");
    assert_eq!(update.left() - tour.right(), theme::engine_bar::BAR_GAP);
    assert!(update.right() <= column.left(), "before the column toggle");
    cx.simulate_click(update.center(), Modifiers::none());
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/releases/0.4.1")
    );
}

#[gpui::test]
fn an_update_the_platform_updater_found_shows_the_button_and_a_press_asks_to_install_it(
    cx: &mut TestAppContext,
) {
    let (desk, cx) = open(cx);
    let found = Release {
        version: "0.4.1".into(),
        url: String::new(),
    };
    cx.update(|_, cx| updates::install_for_test(UpdateState::Available(found), cx));
    desk.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let asked = Rc::new(Cell::new(0));
    let counted = asked.clone();
    cx.update(|_, cx| {
        cx.on_action(move |_: &crate::menus::InstallUpdate, _| counted.set(counted.get() + 1));
    });
    let update = cx.debug_bounds("status-update").expect("the button");
    cx.simulate_click(update.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(asked.get(), 1);
    assert_eq!(cx.opened_url(), None, "there is no page to open");
}

#[gpui::test]
fn the_settings_button_asks_for_settings(cx: &mut TestAppContext) {
    let (_desk, cx) = open(cx);
    let asked = Rc::new(Cell::new(0));
    let counted = asked.clone();
    cx.update(|_, cx| {
        cx.on_action(move |_: &ShowSettings, _| counted.set(counted.get() + 1));
    });
    let at = cx
        .debug_bounds("status-settings")
        .expect("Settings")
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(asked.get(), 1);
}

#[gpui::test]
fn the_sidebar_toggle_hides_the_sidebar_and_keeps_its_place(cx: &mut TestAppContext) {
    let (desk, cx) = open(cx);
    let before = cx.debug_bounds("toggle-sidebar").expect("the toggle");
    let at = before.center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert!(!desk.read_with(cx, |desk, _| desk.panes_shown().sidebar));
    assert_eq!(
        cx.debug_bounds("toggle-sidebar").map(|b| b.origin),
        Some(before.origin)
    );
}

#[gpui::test]
fn the_title_bar_runs_over_the_sidebar_and_the_panel_and_the_panel_ends_just_above_the_bottom_row(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open(cx);
    let window = cx.update(|window, _| window.viewport_size());
    let title = cx.debug_bounds("page-bar").expect("the title bar");
    assert_eq!((title.top(), title.size.height), (px(0.), px(46.)));
    assert_eq!(title.size.width, window.width, "over the sidebar too");
    let panel = cx.debug_bounds("page-panel").expect("the panel");
    let status = cx.debug_bounds("status-bar").expect("the status bar");
    assert_eq!(panel.top(), title.bottom(), "starts under the title bar");
    assert_eq!(
        panel.bottom(),
        status.top(),
        "ends just above the bottom row"
    );
}

#[gpui::test]
fn the_arrows_end_where_the_sidebar_s_rows_end_and_the_first_tab_starts_at_the_panel(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open(cx);
    let sidebar = crate::theme::SIDEBAR_WIDTH;
    let forward = cx.debug_bounds("bar-forward").expect("the forward arrow");
    assert_eq!(
        forward.right(),
        sidebar - px(8.),
        "not past the sidebar's row padding"
    );
    let panel = cx.debug_bounds("page-panel").expect("the panel");
    let tab = cx.debug_bounds("tab-open").expect("the open tab");
    assert_eq!(panel.left(), sidebar);
    assert_eq!(
        tab.left(),
        panel.left(),
        "the first tab starts at the panel's edge"
    );
}

#[gpui::test]
fn with_the_sidebar_hidden_the_arrows_group_shrinks_and_the_tabs_follow(cx: &mut TestAppContext) {
    let (desk, cx) = open(cx);
    let at = cx
        .debug_bounds("toggle-sidebar")
        .expect("the toggle")
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert!(!desk.read_with(cx, |desk, _| desk.panes_shown().sidebar));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let tab = cx.debug_bounds("tab-open").expect("the open tab");
    let forward = cx.debug_bounds("bar-forward").expect("the forward arrow");
    assert!(
        tab.left() < crate::theme::SIDEBAR_WIDTH,
        "the tabs follow the group"
    );
    assert!(
        forward.left() >= crate::theme::TRAFFIC_LIGHTS_ROOM,
        "after the lights' room"
    );
    assert!(
        tab.left() >= forward.right(),
        "the tabs come after the arrows"
    );
}
