//! The tabs' tooltips: the tab's title and its key after the cold delay, the next tab's at once; and the
//! keys themselves: Cmd+1 to Cmd+8 show the tab at that place, Cmd+9 the last.

use super::drag_tests::{drawn, let_go, pick_up};
use super::tests::{draw, lane, open, shown};
use crate::ui::tip::{Tips, TipsHandle, COLD_DELAY};
use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::tabs::Tabs;

fn many(count: usize) -> Tabs {
    let mut tabs = Tabs::new();
    for index in 1..count {
        tabs.open_new(lane(&format!("lane-{index}")));
    }
    tabs.activate_digit(1);
    tabs
}

fn with_tips(cx: &mut TestAppContext) -> Entity<Tips> {
    cx.update(|cx| {
        let tips = cx.new(|_| Tips::new());
        cx.set_global(TipsHandle(tips.clone()));
        tips
    })
}

fn words(tips: &Entity<Tips>, cx: &mut VisualTestContext) -> Option<(String, Option<String>)> {
    tips.read_with(cx, |tips, _| {
        tips.shown().map(|shown| {
            let words = &shown.words;
            (
                words.label.to_string(),
                words.shortcut.as_ref().map(ToString::to_string),
            )
        })
    })
}

fn hover(bar: &Entity<super::PageBar>, place: usize, cx: &mut VisualTestContext) {
    let at = drawn(bar, place, cx).center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    draw(cx);
}

#[gpui::test]
fn a_tab_says_its_title_and_its_key_after_the_cold_delay_and_the_next_one_at_once(
    cx: &mut TestAppContext,
) {
    let tips = with_tips(cx);
    let (cx, bar, _) = open(cx, many(12));
    hover(&bar, 1, cx);
    assert_eq!(words(&tips, cx), None, "not at once");
    cx.executor().advance_clock(COLD_DELAY);
    draw(cx);
    let said = words(&tips, cx).expect("the tooltip");
    assert_eq!(said.1.as_deref(), Some("⌘2"), "the second tab");
    hover(&bar, 2, cx);
    let said = words(&tips, cx).expect("at once");
    assert_eq!(said.1.as_deref(), Some("⌘3"));
}

#[gpui::test]
fn a_drag_closes_the_tooltip_and_the_moved_tab_then_says_its_own_words(cx: &mut TestAppContext) {
    let tips = with_tips(cx);
    let (cx, bar, _) = open(cx, many(12));
    hover(&bar, 1, cx);
    cx.executor().advance_clock(COLD_DELAY);
    draw(cx);
    assert_eq!(
        words(&tips, cx).expect("the tooltip").1.as_deref(),
        Some("⌘2")
    );
    // The tab is picked up and carried one place along: its tooltip does not stay where it stood.
    let one_place = f32::from(drawn(&bar, 2, cx).left() - drawn(&bar, 1, cx).left());
    pick_up(&bar, 1, one_place + 1., cx);
    assert_eq!(words(&tips, cx), None, "closed when the drag starts");
    let_go(&bar, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    draw(cx);
    // The pointer comes to rest on the first tab: a tooltip there is that tab's own, never the carried one's.
    let at_rest = words(&tips, cx).and_then(|said| said.1);
    assert_ne!(
        at_rest.as_deref(),
        Some("⌘2"),
        "not the words of the tab that left"
    );
    hover(&bar, 2, cx);
    cx.executor().advance_clock(COLD_DELAY);
    draw(cx);
    let said = words(&tips, cx).expect("the moved tab's tooltip");
    assert_eq!(said.1.as_deref(), Some("⌘3"), "its own place now");
}

#[gpui::test]
fn the_ninth_key_belongs_to_the_last_tab_and_the_tabs_between_have_none(cx: &mut TestAppContext) {
    let tips = with_tips(cx);
    let (cx, bar, _) = open(cx, many(12));
    for (place, keys) in [(8, None), (11, Some("⌘9"))] {
        hover(&bar, place, cx);
        cx.executor().advance_clock(COLD_DELAY);
        draw(cx);
        let said = words(&tips, cx).expect("the tooltip");
        assert_eq!(said.1.as_deref(), keys, "the tab at place {place}");
    }
}

#[gpui::test]
fn cmd_1_to_cmd_8_show_the_tab_at_that_place_and_cmd_9_the_last(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, many(12));
    for digit in 1..=8usize {
        cx.simulate_keystrokes(&format!("cmd-{digit}"));
        let place = cx.read_entity(&bar, |bar, _| {
            let active = bar.tabs().active().id();
            bar.tabs().index_of(active)
        });
        assert_eq!(place, Some(digit - 1), "Cmd+{digit}");
    }
    cx.simulate_keystrokes("cmd-9");
    let place = cx.read_entity(&bar, |bar, _| {
        let active = bar.tabs().active().id();
        bar.tabs().index_of(active)
    });
    assert_eq!(place, Some(11), "Cmd+9 is the last");
    assert_eq!(shown(&bar, cx), lane("lane-11"));
}
