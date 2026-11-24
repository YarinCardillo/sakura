//! The cursor over the bar: the arrow in every state, and for the whole window while a tab is pressed or in
//! the hand. The tests read what the bar requested at its last paint; no element of the bar asks for
//! another cursor, which the last test holds the sources to.

use super::cursor::LAST_REQUEST;
use super::drag_tests::drawn;
use super::tests::{draw, lane, open};
use gpui::{point, px, CursorStyle, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use sakura_core::tabs::Tabs;

const ARROW_OVER_THE_BAR: Option<(CursorStyle, bool)> = Some((CursorStyle::Arrow, false));
const ARROW_OVER_THE_WINDOW: Option<(CursorStyle, bool)> = Some((CursorStyle::Arrow, true));

fn three() -> Tabs {
    let mut tabs = Tabs::new();
    tabs.open_new(lane("a"));
    tabs.open_new(lane("b"));
    tabs.activate_digit(1);
    tabs
}

fn requested(cx: &mut VisualTestContext) -> Option<(CursorStyle, bool)> {
    LAST_REQUEST.with(|last| last.set(None));
    draw(cx);
    LAST_REQUEST.with(std::cell::Cell::get)
}

#[gpui::test]
fn the_arrow_is_asked_for_over_the_bar_at_rest_and_under_the_pointer(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    assert_eq!(requested(cx), ARROW_OVER_THE_BAR);
    for place in 0..3 {
        let at = drawn(&bar, place, cx).center();
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert_eq!(requested(cx), ARROW_OVER_THE_BAR, "over the tab {place}");
    }
    for selector in ["bar-forward", "bar-back"] {
        let at = cx.debug_bounds(selector).expect("a button").center();
        cx.simulate_mouse_move(at, None, Modifiers::none());
        assert_eq!(requested(cx), ARROW_OVER_THE_BAR, "over {selector}");
    }
}

#[gpui::test]
fn the_arrow_is_asked_for_the_whole_window_while_a_tab_is_held_across_two_others_and_after_the_drop(
    cx: &mut TestAppContext,
) {
    let (cx, bar, _) = open(cx, three());
    let start = drawn(&bar, 0, cx).center();
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    assert_eq!(requested(cx), ARROW_OVER_THE_WINDOW, "pressed");
    for step in 1..=8 {
        let spot = start + point(px(step as f32 * 40.), px(0.));
        cx.simulate_mouse_move(spot, MouseButton::Left, Modifiers::none());
        assert_eq!(
            requested(cx),
            ARROW_OVER_THE_WINDOW,
            "held, step {step}, with a tab sliding under the pointer"
        );
    }
    cx.simulate_mouse_up(
        start + point(px(320.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert_eq!(requested(cx), ARROW_OVER_THE_BAR, "after the drop");
}

#[test]
fn no_element_of_the_bar_asks_for_a_cursor_of_its_own() {
    let sources = [
        ("pill.rs", include_str!("pill.rs")),
        ("buttons.rs", include_str!("buttons.rs")),
        ("render.rs", include_str!("render.rs")),
        ("header.rs", include_str!("header.rs")),
        ("glide.rs", include_str!("glide.rs")),
        ("drag.rs", include_str!("drag.rs")),
    ];
    for (name, source) in sources {
        for line in source.lines() {
            let asks = line.contains(".cursor_") || line.contains("set_cursor_style");
            assert!(!asks, "{name} asks for a cursor: {line}");
        }
    }
}
