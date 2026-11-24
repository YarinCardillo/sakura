//! What was typed and not sent stays: when the tab leaves its page and comes back, when the window is
//! built again after a quit (the preferences hold it), and not after it was sent.

use super::engine_desk::Desk;
use super::engine_rig_tests::{self, wait_until};
use super::lane_tests::{bounds, open_answering, open_answering_with};
use crate::prefs::Preferences;
use crate::theme::DRAFT_PAUSE;
use gpui::{Entity, Focusable as _, VisualTestContext};
use gpui_component::input::TextareaState;
use sakura_core::drafts::Drafts;
use sakura_core::tabs::Page;
use std::time::Duration;

const SENT: &str = r#"{"result": "done", "exit_code": 0, "output": "sent to measure-help\n"}"#;

fn lane_key() -> String {
    engine_rig_tests::lane_key("measure-help")
}

fn stored(cx: &mut VisualTestContext) -> Drafts {
    cx.update(|_, cx| Drafts::restore(Preferences::current(cx).drafts.as_ref()))
}

fn pause(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(DRAFT_PAUSE + Duration::from_millis(1));
    cx.run_until_parked();
}

fn lane_field(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Entity<TextareaState> {
    let key = lane_key();
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the composer", |cx| bounds(cx, "lane-send").is_some());
    desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(&key, cx).expect("the page");
        page.read(cx).composer_field(cx).expect("the field")
    })
}

fn type_in(field: &Entity<TextareaState>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    cx.simulate_input(text);
    cx.run_until_parked();
}

fn text_of(field: &Entity<TextareaState>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| field.read(cx).value().to_string())
}

#[gpui::test]
fn a_lane_draft_stays_when_the_tab_goes_to_another_page_and_comes_back(
    cx: &mut gpui::TestAppContext,
) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    let field = lane_field(&desk, cx);
    type_in(&field, "half a thought", cx);
    desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx));
    cx.run_until_parked();
    let field = lane_field(&desk, cx);
    assert_eq!(text_of(&field, cx), "half a thought");
}

#[gpui::test]
fn a_lane_draft_is_kept_in_the_preferences_after_a_pause_in_typing(cx: &mut gpui::TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    let field = lane_field(&desk, cx);
    type_in(&field, "half a thought", cx);
    assert!(stored(cx).is_empty(), "not while the person types");
    pause(cx);
    let kept = stored(cx);
    assert_eq!(kept.get(&Page::Lane(lane_key())), Some("half a thought"));
}

#[gpui::test]
fn leaving_the_page_keeps_the_draft_without_waiting_for_the_pause(cx: &mut gpui::TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    let field = lane_field(&desk, cx);
    type_in(&field, "half a thought", cx);
    desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx));
    cx.run_until_parked();
    let kept = stored(cx);
    assert_eq!(kept.get(&Page::Lane(lane_key())), Some("half a thought"));
}

#[gpui::test]
fn a_lane_draft_of_the_last_launch_is_in_the_box_when_the_page_is_built(
    cx: &mut gpui::TestAppContext,
) {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Lane(lane_key()), "left unsent");
    let preferences = Preferences {
        drafts: Some(drafts.to_json()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) =
        open_answering_with(cx, engine_rig_tests::SNAPSHOT, (SENT, SENT), preferences);
    let field = lane_field(&desk, cx);
    assert_eq!(text_of(&field, cx), "left unsent");
}

#[gpui::test]
fn a_master_draft_of_the_last_launch_is_in_its_composer(cx: &mut gpui::TestAppContext) {
    let mut drafts = Drafts::default();
    drafts.set(&Page::Master, "ask about the build");
    let preferences = Preferences {
        drafts: Some(drafts.to_json()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) =
        open_answering_with(cx, engine_rig_tests::SNAPSHOT, (SENT, SENT), preferences);
    let draft = desk.read_with(cx, |desk, cx| {
        desk.master_conversation(cx).read(cx).draft(cx)
    });
    assert_eq!(draft, "ask about the build");
}

#[gpui::test]
fn a_master_draft_is_kept_after_a_pause_in_typing(cx: &mut gpui::TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    desk.update(cx, |desk, cx| desk.navigate(Page::Master, false, cx));
    cx.run_until_parked();
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    let handle = cx.update(|_, cx| view.read(cx).composer_handle(cx));
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.simulate_input("what is running");
    pause(cx);
    assert_eq!(stored(cx).get(&Page::Master), Some("what is running"));
}

#[gpui::test]
fn a_lane_draft_is_forgotten_once_the_engine_took_the_message(cx: &mut gpui::TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    let field = lane_field(&desk, cx);
    type_in(&field, "Use argparse", cx);
    pause(cx);
    assert!(stored(cx).get(&Page::Lane(lane_key())).is_some());
    super::lane_tests::click(cx, "lane-send");
    wait_until(cx, "the box emptied", |cx| text_of(&field, cx).is_empty());
    pause(cx);
    assert_eq!(stored(cx).get(&Page::Lane(lane_key())), None);
}

#[gpui::test]
fn two_visible_masters_keep_separate_conversations_and_drafts(cx: &mut gpui::TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, engine_rig_tests::SNAPSHOT, SENT, SENT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let (left, right) = bar.update(cx, |bar, cx| {
        bar.navigate(Page::Master, cx);
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Master, cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    let first = desk.read_with(cx, |desk, cx| {
        desk.master_for_test(left, cx)
            .unwrap()
            .read(cx)
            .conversation()
    });
    let second = desk.read_with(cx, |desk, cx| {
        desk.master_for_test(right, cx)
            .unwrap()
            .read(cx)
            .conversation()
    });
    assert_ne!(first, second);
    cx.update(|window, cx| {
        first.update(cx, |view, cx| view.write("first", window, cx));
        second.update(cx, |view, cx| view.write("second", window, cx));
    });
    cx.run_until_parked();
    pause(cx);
    let drafts = stored(cx);
    bar.read_with(cx, |bar, _| {
        let first = bar.tabs().tab(left).unwrap().conversation();
        let second = bar.tabs().tab(right).unwrap().conversation();
        assert_eq!(
            drafts.get_for_owner(&Page::Master, None, first),
            Some("first")
        );
        assert_eq!(
            drafts.get_for_owner(&Page::Master, None, second),
            Some("second")
        );
    });
}
