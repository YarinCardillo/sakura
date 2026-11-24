//! Holding Cmd shows the shortcuts and holding Option shows the technical names, and only after the key
//! has been held a moment, so a Cmd-click or a Cmd-K never flashes them.

use super::engine_rig_tests::{self, wait_until};
use super::held_keys::{Effect, HeldKeys};
use super::lane_tests::{bounds, open_answering};
use crate::theme::HINT_DELAY;
use gpui::{Modifiers, TestAppContext};
use sakura_core::tabs::Page;
use std::time::Duration;

fn command() -> Modifiers {
    Modifiers::command()
}

fn alt() -> Modifiers {
    Modifiers::alt()
}

#[test]
fn change_cmd_alone_starts_the_wait_and_shows_nothing_yet() {
    let mut held = HeldKeys::default();
    let effect = held.change(command());
    assert!(matches!(effect, Effect::WaitForHints(_)));
    assert!(!held.are_hints_shown());
}

#[test]
fn delay_over_while_cmd_is_still_held_shows_the_hints() {
    let mut held = HeldKeys::default();
    let Effect::WaitForHints(token) = held.change(command()) else {
        panic!("a wait");
    };
    assert!(held.delay_over(token));
    assert!(held.are_hints_shown());
}

#[test]
fn delay_over_after_cmd_was_released_shows_nothing() {
    let mut held = HeldKeys::default();
    let Effect::WaitForHints(token) = held.change(command()) else {
        panic!("a wait");
    };
    held.change(Modifiers::none());
    assert!(!held.delay_over(token));
    assert!(!held.are_hints_shown());
}

#[test]
fn change_releasing_cmd_hides_the_hints_at_once() {
    let mut held = HeldKeys::default();
    let Effect::WaitForHints(token) = held.change(command()) else {
        panic!("a wait");
    };
    held.delay_over(token);
    held.change(Modifiers::none());
    assert!(!held.are_hints_shown());
}

#[test]
fn change_cmd_with_another_modifier_is_a_shortcut_and_never_shows_hints() {
    let mut held = HeldKeys::default();
    let both = Modifiers {
        platform: true,
        shift: true,
        ..Modifiers::default()
    };
    assert_eq!(held.change(both), Effect::None);
    assert!(!held.are_hints_shown());
}

#[test]
fn change_option_alone_shows_the_technical_names_at_once_and_release_hides_them() {
    let mut held = HeldKeys::default();
    held.change(alt());
    assert!(held.is_identity_shown());
    held.change(Modifiers::none());
    assert!(!held.is_identity_shown());
}

fn two_tabs(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<super::engine_desk::Desk>,
    &mut gpui::VisualTestContext,
) {
    let (_rig, desk, cx) = open_answering(
        cx,
        engine_rig_tests::SNAPSHOT,
        r#"{"result": "done", "exit_code": 0, "output": ""}"#,
        r#"{"result": "done", "exit_code": 0, "output": ""}"#,
    );
    desk.update(cx, |desk, cx| desk.navigate(Page::Master, true, cx));
    wait_until(cx, "two tabs", |cx| bounds(cx, "tab-1").is_some());
    (desk, cx)
}

#[gpui::test]
fn holding_cmd_shows_the_tab_numbers_after_the_pause_and_releasing_hides_them(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = two_tabs(cx);
    assert!(bounds(cx, "tab-hint-1").is_none());
    cx.simulate_modifiers_change(command());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor().advance_clock(HINT_DELAY / 2);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, "tab-hint-1").is_none(), "not before the pause");
    cx.executor()
        .advance_clock(HINT_DELAY / 2 + Duration::from_millis(1));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, "tab-hint-1").is_some(), "the first tab's number");
    assert!(bounds(cx, "tab-hint-2").is_some(), "and the second's");
    cx.simulate_modifiers_change(Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, "tab-hint-1").is_none(), "gone at once");
}

#[gpui::test]
fn modifiers_update_the_desk_without_an_animation_frame(cx: &mut TestAppContext) {
    let (_, cx) = two_tabs(cx);
    cx.run_until_parked();
    cx.simulate_modifiers_change(command());
    cx.executor().advance_clock(
        crate::theme::MODIFIER_POLL_INTERVAL + HINT_DELAY + Duration::from_millis(1),
    );
    cx.run_until_parked();
    assert!(bounds(cx, "tab-hint-1").is_some());
    cx.simulate_modifiers_change(Modifiers::none());
    cx.executor()
        .advance_clock(crate::theme::MODIFIER_POLL_INTERVAL);
    cx.run_until_parked();
    assert!(bounds(cx, "tab-hint-1").is_none());
}

#[gpui::test]
fn holding_option_names_a_lane_tab_by_its_branch(cx: &mut TestAppContext) {
    let (desk, cx) = two_tabs(cx);
    let key = engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    cx.run_until_parked();
    let page = Page::Lane(key);
    let title = |is_identity: bool, cx: &mut gpui::VisualTestContext| {
        desk.read_with(cx, |desk, cx| {
            desk.bar.read(cx).crumb(&page, is_identity).title
        })
    };
    assert_eq!(title(false, cx).as_ref(), "demo");
    assert_eq!(title(true, cx).as_ref(), "lane/demo");
}
