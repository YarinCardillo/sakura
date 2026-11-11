//! The tabs in the window strip, driven in a test window on the fixture: a row opens in the tab on screen
//! or, with Cmd, in one of its own behind it; the keys choose and close tabs; the tabs come back at the
//! next launch; a lavoro that is gone loses its tab; and the first tab starts on the panel's left edge.

use super::desk::Desk;
use super::grid_tests::bounds;
use super::interaction_tests::{centre, click, draw, open_remembering, open_with, wait_for};
use crate::data::source::{Recorder, Source};
use crate::prefs::Preferences;
use crate::theme;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::tabs::{Placement, Tabs};
use std::sync::Arc;

const VERIFIED: &str = "sample-accent";
const WORKING: &str = "sample-folders";
const TAKEN: &str = "headings-26";

/// The lavori of the tabs, in strip order, and the one on screen.
fn tabs(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> (Vec<String>, Option<String>) {
    desk.read_with(cx, |desk, cx| {
        let tabs = desk.strip().read(cx).tabs();
        let open = tabs.tabs().iter().map(|tab| tab.task.clone()).collect();
        (open, desk.shown(cx))
    })
}

fn names(open: &[&str]) -> Vec<String> {
    open.iter().map(|name| (*name).to_string()).collect()
}

/// The fixture's window with its rows listed.
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-headings-26").is_some()
    });
    (cx, desk)
}

/// A press on a row with Cmd held.
fn cmd_click(cx: &mut VisualTestContext, row: &'static str) {
    let at = centre(cx, row);
    cx.simulate_click(at, Modifiers::command());
    draw(cx);
}

/// The fixture's window as it comes back with `open` in its tabs and `shown` on screen.
fn reopened<'a>(
    cx: &'a mut TestAppContext,
    open: &[&str],
    shown: &str,
) -> (&'a mut VisualTestContext, Entity<Desk>) {
    let mut tabs = Tabs::new();
    for task in open {
        tabs.open(task, Placement::Background);
    }
    tabs.show(shown);
    let preferences = Preferences {
        tabs: Some(tabs.to_json()),
        last_task: Some(shown.to_string()),
        ..Default::default()
    };
    let start = |runtime| Source::for_test(runtime, Arc::new(Recorder::default()));
    open_remembering(cx, start, None, preferences)
}

#[gpui::test]
fn a_row_opens_in_the_tab_on_screen_and_with_cmd_in_a_tab_of_its_own_behind(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    click(cx, "row-sample-accent");
    assert_eq!(tabs(cx, &desk), (names(&[VERIFIED]), Some(VERIFIED.into())));
    click(cx, "row-headings-26");
    assert_eq!(
        tabs(cx, &desk),
        (names(&[TAKEN]), Some(TAKEN.into())),
        "the tab on screen shows the other lavoro: no second tab"
    );
    cmd_click(cx, "row-sample-folders");
    assert_eq!(
        tabs(cx, &desk),
        (names(&[TAKEN, WORKING]), Some(TAKEN.into())),
        "a tab of its own, behind the one on screen"
    );
    click(cx, "row-sample-folders");
    assert_eq!(
        tabs(cx, &desk),
        (names(&[TAKEN, WORKING]), Some(WORKING.into())),
        "a lavoro that has a tab is shown in it"
    );
    assert!(
        cx.debug_bounds("page-header").is_some(),
        "its page is drawn"
    );
}

#[gpui::test]
fn the_keys_choose_a_tab_and_close_the_one_on_screen_while_another_remains(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    click(cx, "row-headings-26");
    cmd_click(cx, "row-sample-folders");
    cx.simulate_keystrokes("cmd-2");
    draw(cx);
    assert_eq!(tabs(cx, &desk).1.as_deref(), Some(WORKING));
    cx.simulate_keystrokes("cmd-1");
    draw(cx);
    assert_eq!(tabs(cx, &desk).1.as_deref(), Some(TAKEN));
    cx.simulate_keystrokes("cmd-w");
    draw(cx);
    assert_eq!(tabs(cx, &desk), (names(&[WORKING]), Some(WORKING.into())));
    cx.simulate_keystrokes("cmd-w");
    draw(cx);
    assert_eq!(tabs(cx, &desk).0, names(&[WORKING]), "the last tab stays");
}

#[gpui::test]
fn the_tabs_and_the_one_on_screen_come_back_at_the_next_launch(cx: &mut TestAppContext) {
    let (cx, desk) = reopened(cx, &[TAKEN, WORKING, VERIFIED], WORKING);
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("page-header").is_some()
    });
    assert_eq!(
        tabs(cx, &desk),
        (names(&[TAKEN, WORKING, VERIFIED]), Some(WORKING.into()))
    );
    cx.simulate_keystrokes("cmd-3");
    draw(cx);
    let kept = cx.update(|_, cx| Preferences::current(cx));
    let kept_tabs = kept.tabs.as_ref().and_then(Tabs::from_json);
    let shown = kept_tabs.as_ref().and_then(|tabs| tabs.active());
    assert_eq!(
        shown.map(|tab| tab.task.as_str()),
        Some(VERIFIED),
        "a change of tab is kept for the next launch"
    );
    assert_eq!(kept.last_task.as_deref(), Some(VERIFIED));
}

#[gpui::test]
fn the_tab_of_a_lavoro_that_is_gone_closes_once_every_lavoro_was_read(cx: &mut TestAppContext) {
    let (cx, desk) = reopened(cx, &[VERIFIED, "non-esiste-piu"], "non-esiste-piu");
    wait_for(cx, "the tab to close", |cx| {
        tabs(cx, &desk).0 == names(&[VERIFIED])
    });
    assert_eq!(
        tabs(cx, &desk).1.as_deref(),
        Some(VERIFIED),
        "the tab beside it is the one on screen"
    );
}

#[gpui::test]
fn with_every_tab_gone_the_window_opens_on_a_lavoro_as_it_does_at_launch(cx: &mut TestAppContext) {
    let (cx, desk) = reopened(cx, &["non-esiste-piu"], "non-esiste-piu");
    wait_for(cx, "a lavoro to open", |cx| {
        tabs(cx, &desk)
            .1
            .is_some_and(|shown| shown != "non-esiste-piu")
    });
    assert_eq!(tabs(cx, &desk).0.len(), 1);
}

/// Where the first tab is drawn.
fn first_tab(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> gpui::Bounds<gpui::Pixels> {
    desk.read_with(cx, |desk, cx| {
        let strip = desk.strip().read(cx);
        let first = strip.tabs().tabs().first().expect("a tab");
        strip.tab_bounds(first.id).expect("the tab was drawn")
    })
}

#[gpui::test]
fn the_first_tab_starts_on_the_panel_s_left_edge_and_after_the_lights_without_the_sidebar(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    click(cx, "row-sample-accent");
    let tab = first_tab(cx, &desk);
    let header = bounds(cx, "page-header");
    assert_eq!(
        tab.left(),
        header.left() - theme::PANEL_RING_WIDTH,
        "on the panel's left edge"
    );
    assert_eq!(tab.left(), theme::SIDEBAR_WIDTH);
    assert_eq!(
        tab.top(),
        gpui::px(theme::tabs::TOP_INSET),
        "in the window strip, under the room it leaves at the top"
    );
    assert_eq!(
        tab.bottom(),
        theme::WINDOW_STRIP_HEIGHT,
        "standing on the panel"
    );
    assert_eq!(
        header.top(),
        theme::WINDOW_STRIP_HEIGHT + theme::PANEL_RING_WIDTH,
        "the page starts under the strip, inside the panel's ring"
    );
    cx.simulate_keystrokes("cmd-b");
    draw(cx);
    let tab = first_tab(cx, &desk);
    assert_eq!(
        tab.left(),
        theme::TRAFFIC_LIGHTS_ROOM,
        "the sidebar is hidden: the tab starts after the traffic lights"
    );
    let header = bounds(cx, "page-header");
    assert_eq!(header.left(), theme::PANEL_INSET + theme::PANEL_RING_WIDTH);
}

#[gpui::test]
fn what_a_lavoro_had_open_is_there_when_its_tab_comes_back(cx: &mut TestAppContext) {
    let (cx, desk) = open(cx);
    click(cx, "row-sample-accent");
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    let closed = desk.read_with(cx, |desk, cx| desk.pane_rows(VERIFIED, cx));
    click(cx, "changes-header-0");
    let open_rows = desk.read_with(cx, |desk, cx| desk.pane_rows(VERIFIED, cx));
    assert!(open_rows > closed, "a file was opened in place");
    cmd_click(cx, "row-headings-26");
    cx.simulate_keystrokes("cmd-2");
    draw(cx);
    assert_eq!(tabs(cx, &desk).1.as_deref(), Some(TAKEN));
    cx.simulate_keystrokes("cmd-1");
    draw(cx);
    assert_eq!(
        desk.read_with(cx, |desk, cx| desk.pane_rows(VERIFIED, cx)),
        open_rows,
        "the file is still open"
    );
}
