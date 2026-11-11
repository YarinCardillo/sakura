//! The panes that hide, driven in a test window: the keys, the buttons, what the preferences keep, where
//! the header starts when the sidebar is gone, and the movement over the layout duration.

use super::desk::Desk;
use super::interaction_tests::{click, draw, open_with, wait_for};
use crate::prefs::Preferences;
use crate::theme;
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::time::Instant;

const ROW: &str = "row-sample-accent";

/// The desk on the fixture, its page drawn, with panes that move at once (reduced motion).
fn open_still(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_with(cx, None);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    (cx, desk)
}

fn key(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    draw(cx);
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

in_every_language! {
fn cmd_b_hides_the_sidebar_and_brings_it_back_and_the_preferences_keep_it(cx: &mut TestAppContext) {
    let (cx, _) = open_still(cx);
    assert!(is_on_screen(cx, ROW) && is_on_screen(cx, "toggle-sidebar"));
    key(cx, "cmd-b");
    assert!(!is_on_screen(cx, ROW), "the sidebar is gone");
    assert!(is_on_screen(cx, "toggle-sidebar"), "its toggle stays");
    assert!(cx.update(|_, cx| Preferences::current(cx).is_sidebar_hidden));
    key(cx, "cmd-b");
    assert!(is_on_screen(cx, ROW), "and back");
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_sidebar_hidden));
}
}

in_every_language! {
fn the_sidebar_toggle_sits_at_the_far_left_of_the_status_bar_and_stays_there(cx: &mut TestAppContext) {
    let (cx, _) = open_still(cx);
    let window = cx.update(|window, _| window.viewport_size());
    let shown = cx.debug_bounds("toggle-sidebar").expect("the toggle");
    let icon = cx.debug_bounds("toggle-sidebar-icon").expect("its icon");
    assert_eq!(icon.left(), theme::SIDEBAR_INSET, "on the sidebar's inset");
    assert!(
        shown.top() >= window.height - theme::STATUS_BAR_HEIGHT,
        "in the status bar, not in the window strip"
    );
    click(cx, "toggle-sidebar");
    assert!(!is_on_screen(cx, ROW), "the button hides the sidebar");
    let hidden = cx.debug_bounds("toggle-sidebar").expect("still there");
    assert_eq!(hidden, shown, "and stays where it was");
    click(cx, "toggle-sidebar");
    assert!(is_on_screen(cx, ROW), "and brings it back");
}
}

in_every_language! {
fn the_changes_toggle_sits_at_the_far_right_of_the_status_bar_on_every_view(cx: &mut TestAppContext) {
    let (cx, _) = open_still(cx);
    let window = cx.update(|window, _| window.viewport_size());
    for view in ["pane-view-checks", "pane-view-changes"] {
        click(cx, view);
        let toggle = cx.debug_bounds("toggle-changes").expect("the toggle");
        assert!(
            toggle.top() >= window.height - theme::STATUS_BAR_HEIGHT,
            "in the status bar, not in the window strip"
        );
        let from_edge = window.width - toggle.right();
        assert!(
            from_edge < theme::CONTENT_INSET,
            "its fill reaches past the content inset, where its icon ends"
        );
    }
    assert!(!is_on_screen(cx, "hide-sidebar") && !is_on_screen(cx, "show-sidebar"));
}
}

in_every_language! {
fn cmd_alt_b_hides_the_changes_pane_and_its_button_brings_it_back(cx: &mut TestAppContext) {
    let (cx, _) = open_still(cx);
    key(cx, "alt-cmd-b");
    assert!(!is_on_screen(cx, "changes-header-0"), "the pane is gone");
    assert!(cx.update(|_, cx| Preferences::current(cx).is_changes_hidden));
    click(cx, "toggle-changes");
    assert!(is_on_screen(cx, "changes-header-0"), "and back");
    assert!(!cx.update(|_, cx| Preferences::current(cx).is_changes_hidden));
}
}

in_every_language! {
fn a_pane_moves_for_the_layout_duration_and_then_is_gone(cx: &mut TestAppContext) {
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| is_on_screen(cx, ROW));
    let started = Instant::now();
    cx.update(|_, cx| desk.update(cx, |desk, cx| desk.toggle_sidebar(cx)));
    assert!(
        desk.read_with(cx, |desk, _| desk.is_sliding(started)),
        "it has started to go"
    );
    wait_for(cx, "the sidebar to go", |cx| !is_on_screen(cx, ROW));
    assert!(
        started.elapsed() >= theme::DURATION_BASE,
        "not before the duration"
    );
    assert!(!desk.read_with(cx, |desk, _| desk.is_sliding(Instant::now())));
}
}

in_every_language! {
fn with_reduced_motion_a_pane_is_gone_at_once(cx: &mut TestAppContext) {
    let (cx, desk) = open_still(cx);
    cx.update(|_, cx| desk.update(cx, |desk, cx| desk.toggle_sidebar(cx)));
    assert!(!desk.read_with(cx, |desk, _| desk.is_sliding(Instant::now())));
    draw(cx);
    assert!(!is_on_screen(cx, ROW));
}
}
