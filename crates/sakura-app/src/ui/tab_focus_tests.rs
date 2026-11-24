use super::engine_desk::Desk;
use super::engine_desk_tests::{make_split, open, pane_bounds};
use super::engine_rig_tests::SNAPSHOT;
use super::lane_tests::bounds;
use crate::prefs::Preferences;
use gpui::{point, px, Entity, FocusHandle, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::tabs::{Page, TabId, Tabs};

fn active(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> TabId {
    desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().active().id())
}

fn pane_handle(desk: &Entity<Desk>, id: TabId, cx: &mut VisualTestContext) -> FocusHandle {
    desk.read_with(cx, |desk, _| desk.pane_focus_for_test(id).unwrap())
}

fn frame(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn focus_other_pane_moves_container_focus_both_ways_without_a_caret(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Master, Page::Master, 5000, cx);
    frame(cx);
    let right_composer = desk.read_with(cx, |desk, cx| {
        desk.master_for_test(right, cx)
            .unwrap()
            .read(cx)
            .conversation()
            .read(cx)
            .composer_handle(cx)
    });
    cx.update(|window, cx| window.focus(&right_composer, cx));
    frame(cx);
    for expected in [left, right] {
        cx.simulate_keystrokes("alt-cmd-j");
        frame(cx);
        let handle = pane_handle(&desk, expected, cx);
        assert_eq!(active(&desk, cx), expected);
        assert!(cx.update(|window, _| handle.is_focused(window)));
        assert!(!cx.update(|window, _| right_composer.is_focused(window)));
    }
}

#[gpui::test]
fn mouse_activation_and_scroll_have_distinct_focus_behaviour(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Master, 5000, cx);
    frame(cx);
    let composer = desk.read_with(cx, |desk, cx| {
        desk.master_for_test(right, cx)
            .unwrap()
            .read(cx)
            .conversation()
            .read(cx)
            .composer_handle(cx)
    });
    cx.update(|window, cx| window.focus(&composer, cx));
    frame(cx);
    let at = pane_bounds(left, cx).center();
    cx.simulate_event(gpui::ScrollEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-80.))),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    assert_eq!(active(&desk, cx), right);
    let header = bounds(cx, &format!("tab-header-{}", left.as_u64())).unwrap();
    cx.simulate_click(
        point(header.left() + px(3.), header.center().y),
        Modifiers::none(),
    );
    frame(cx);
    assert_eq!(active(&desk, cx), left);
    assert!(cx.update(|window, cx| window.focused(cx).is_none()));
}

#[gpui::test]
fn tabbing_into_each_pane_updates_active_tab(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    cx.update(|_, cx| super::keyboard::install(cx));
    let (left, right) = make_split(&desk, Page::Inbox, Page::Lanes, 5000, cx);
    frame(cx);
    for side in [left, right] {
        for _ in 0..50 {
            cx.simulate_keystrokes("tab");
            frame(cx);
            if active(&desk, cx) == side {
                break;
            }
        }
        assert_eq!(active(&desk, cx), side);
    }
}

#[gpui::test]
fn back_forward_and_close_address_only_the_active_side(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Engine, 5000, cx);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| {
        bar.navigate_from(left, Page::Lanes, cx);
        bar.navigate_from(right, Page::Master, cx);
    });
    frame(cx);
    cx.simulate_keystrokes("alt-cmd-j cmd-[");
    frame(cx);
    bar.read_with(cx, |bar, _| {
        assert_eq!(bar.tabs().active().id(), left);
        assert_eq!(bar.tabs().active().page(), &Page::Inbox);
        assert_eq!(bar.tabs().tab(right).unwrap().page(), &Page::Master);
    });
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.tabs().active().page().clone()),
        Page::Lanes
    );
    cx.simulate_keystrokes("cmd-w");
    bar.read_with(cx, |bar, _| {
        assert_eq!(bar.tabs().visible(), (right, None));
        assert!(bar.tabs().active().can_go_back());
    });
    assert!(!cx.update(|window, cx| desk.update(cx, |desk, cx| desk.focus_other_pane(window, cx))));
}

#[gpui::test]
fn restore_split_does_not_autofocus_either_pane(cx: &mut TestAppContext) {
    let mut tabs = Tabs::new();
    tabs.navigate(Page::Engine);
    let left = tabs.active().id();
    let right = tabs.open_new(Page::Lanes);
    tabs.create_split(left, right);
    let preferences = Preferences {
        tabs: Some(tabs.to_json()),
        ..Preferences::default()
    };
    let done = r#"{"result":"done","exit_code":0}"#;
    let (_rig, desk, cx) =
        super::lane_tests::open_answering_with(cx, SNAPSHOT, (done, done), preferences);
    assert!(cx.update(|window, cx| window.focused(cx).is_none()));
    cx.simulate_keystrokes("alt-cmd-j");
    frame(cx);
    assert_eq!(active(&desk, cx), left);
}

#[gpui::test]
fn swap_keeps_focus_with_the_same_tab_id(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Lanes, 5500, cx);
    frame(cx);
    cx.simulate_keystrokes("alt-cmd-j");
    frame(cx);
    let left_handle = pane_handle(&desk, left, cx);
    assert!(cx.update(|window, _| left_handle.is_focused(window)));
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| assert!(bar.swap_split(right, cx)));
    frame(cx);
    assert_eq!(active(&desk, cx), left);
    assert!(cx.update(|window, _| left_handle.is_focused(window)));
}
