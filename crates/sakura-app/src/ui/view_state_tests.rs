//! Where the person had scrolled stays when the tab leaves the page and comes back.

use super::engine_rig_tests::{self, wait_until};
use super::lane_tests::{bounds, open_answering};
use gpui::{
    point, px, size, Modifiers, Pixels, ScrollDelta, ScrollEvent, ScrollSource, TestAppContext,
    TouchPhase, VisualTestContext,
};
use sakura_core::tabs::Page;

const SENT: &str = r#"{"result": "done", "exit_code": 0, "output": "sent\n"}"#;

fn wheel(cx: &mut VisualTestContext, over: &str, by: f32) {
    let at = bounds(cx, over).expect("the place to scroll").center();
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-by))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn offset(desk: &gpui::Entity<super::engine_desk::Desk>, cx: &mut VisualTestContext) -> Pixels {
    let key = engine_rig_tests::lane_key("measure-help");
    desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(&key, cx).expect("the page");
        page.read(cx).scroll_offset().y
    })
}

#[gpui::test]
fn an_outside_lane_page_keeps_its_scroll_when_the_tab_leaves_and_comes_back(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    cx.simulate_resize(size(px(900.), px(240.)));
    let key = engine_rig_tests::lane_key("measure-help");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the page", |cx| bounds(cx, "lane-page").is_some());
    wheel(cx, "lane-page", 60.);
    let scrolled = offset(&desk, cx);
    assert!(scrolled < px(0.), "the page scrolled: {scrolled:?}");
    desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the page again", |cx| bounds(cx, "lane-page").is_some());
    assert_eq!(offset(&desk, cx), scrolled);
}
