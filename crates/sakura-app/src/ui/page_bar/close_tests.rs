//! Tabs closed while the others glide into their places: the remaining tabs never overlap, in any frame,
//! however fast the closes come. Driven with real time on the springs, the way a person closes tabs.

use super::drag_tests::drawn;
use super::tests::{draw, lane, open};
use super::{LaneTab, PageBar};
use crate::ui::kit::StateGlyph;
use crate::ui::real_text;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use sakura_core::tabs::Tabs;
use std::collections::HashMap;
use std::time::Duration;

/// How many tabs the bar starts with: more than the bar has room for at their widest, so closing one makes
/// the others wider.
const TABS: usize = 16;
/// The least gap two neighbours may show between them; a hair under zero is rounding.
const OVERLAP_TOLERANCE: f32 = 0.5;
/// The tab shown at the start (from 0): in the middle, so that closing it hands the open pill to a neighbour.
const OPEN_PLACE: usize = 4;
/// How far behind the last tab the + may be while the tabs glide: the room of one closed tab.
const MOST_BEHIND: f32 = 120.;
/// Frames of real time after which a close has settled.
const SETTLED: usize = 120;
const FRAME: Duration = Duration::from_millis(8);

fn many() -> Tabs {
    let mut tabs = Tabs::new();
    for index in 1..TABS {
        tabs.open_new(lane(&format!("a-lane-with-a-long-name-{index}")));
    }
    tabs.activate_digit(OPEN_PLACE + 1);
    tabs
}

/// The edges (left, right) of every tab as drawn now, in order.
fn edges(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Vec<(f32, f32)> {
    let places = cx.read_entity(bar, |bar, _| bar.tabs().len());
    (0..places)
        .map(|place| {
            let bounds = drawn(bar, place, cx);
            (f32::from(bounds.left()), f32::from(bounds.right()))
        })
        .collect()
}

/// Fails when a tab is drawn over the one before it.
fn assert_no_overlap(edges: &[(f32, f32)], when: &str) {
    for (place, pair) in edges.windows(2).enumerate() {
        let gap = pair[1].0 - pair[0].1;
        assert!(
            gap >= -OVERLAP_TOLERANCE,
            "{when}: tab {} is drawn {:.1} px over tab {} ({edges:?})",
            place + 2,
            -gap,
            place + 1
        );
    }
}

/// Draws frames for `frames` steps of real time, checking each.
fn glide(bar: &Entity<PageBar>, frames: usize, when: &str, cx: &mut VisualTestContext) {
    for frame in 0..frames {
        std::thread::sleep(FRAME);
        draw(cx);
        assert_no_overlap(&edges(bar, cx), &format!("{when}, frame {frame}"));
    }
}

fn close_place(bar: &Entity<PageBar>, place: usize, cx: &mut VisualTestContext) {
    bar.update(cx, |bar, cx| {
        let id = bar.tabs().tabs()[place].id();
        bar.close(id, cx);
    });
}

/// Names for the lanes, long enough that the open pill is wider than the others.
fn name_the_lanes(bar: &Entity<PageBar>, cx: &mut VisualTestContext) {
    let lanes: HashMap<String, LaneTab> = (1..TABS)
        .map(|index| {
            let key = format!("a-lane-with-a-long-name-{index}");
            let lane = LaneTab {
                project: "project".into(),
                name: format!("A lane with a long name {index}").into(),
                identity: format!("lane/long-name-{index}").into(),
                glyph: StateGlyph::NeedsYou,
            };
            (key, lane)
        })
        .collect();
    bar.update(cx, |bar, cx| bar.set_lanes(lanes, cx));
    draw(cx);
}

fn moving(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<PageBar>) {
    let (cx, bar, _) = open(cx, many());
    name_the_lanes(&bar, cx);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    (cx, bar)
}

#[test]
fn closing_the_open_tab_in_the_middle_never_draws_a_tab_over_its_neighbour() {
    real_text::run(|cx| {
        let (cx, bar) = moving(cx);
        close_place(&bar, OPEN_PLACE, cx);
        glide(&bar, 90, "after closing the open tab", cx);
    });
}

#[test]
fn closing_a_tab_that_is_not_open_never_draws_a_tab_over_its_neighbour() {
    real_text::run(|cx| {
        let (cx, bar) = moving(cx);
        close_place(&bar, OPEN_PLACE + 3, cx);
        glide(&bar, 90, "after closing another tab", cx);
    });
}

#[test]
fn closing_several_tabs_in_a_row_never_overlaps_the_tabs_still_gliding() {
    real_text::run(|cx| {
        let (cx, bar) = moving(cx);
        for place in [5, OPEN_PLACE, 3, 1] {
            close_place(&bar, place, cx);
            glide(&bar, 3, "while closing in a row", cx);
        }
        glide(&bar, 120, "after the last close", cx);
    });
}

#[test]
fn closing_two_tabs_in_the_same_frame_never_overlaps() {
    real_text::run(|cx| {
        let (cx, bar) = moving(cx);
        close_place(&bar, OPEN_PLACE, cx);
        close_place(&bar, 2, cx);
        glide(&bar, 120, "after two closes at once", cx);
    });
}

#[test]
fn the_tabs_end_in_their_places_when_the_glide_is_over() {
    real_text::run(|cx| {
        let (cx, bar) = moving(cx);
        close_place(&bar, OPEN_PLACE, cx);
        glide(&bar, 200, "settling", cx);
        let settled = edges(&bar, cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        draw(cx);
        assert_eq!(
            settled,
            edges(&bar, cx),
            "the glide ends where the layout is"
        );
    });
}

/// Four tabs, the second one open, with real motion on.
fn four(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<PageBar>) {
    let mut tabs = Tabs::new();
    for index in 1..4 {
        tabs.open_new(lane(&format!("a-lane-{index}")));
    }
    tabs.activate_digit(2);
    let (cx, bar, _) = open(cx, tabs);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    draw(cx);
    (cx, bar)
}

/// What the bar shows after tab `place` was closed, `frames` frames of real time later: every remaining tab
/// whole, side by side, none over its neighbour, the closed one gone and the + right after the last.
fn assert_strip_is_whole(
    bar: &Entity<PageBar>,
    frames: usize,
    when: &str,
    is_settled: bool,
    cx: &mut VisualTestContext,
) {
    for _ in 0..frames {
        std::thread::sleep(FRAME);
        draw(cx);
    }
    let count = cx.read_entity(bar, |bar, _| bar.tabs().len());
    let row = cx.debug_bounds("page-tabs").expect("the row");
    let edges = edges(bar, cx);
    assert_no_overlap(&edges, when);
    for (place, (left, right)) in edges.iter().enumerate() {
        assert!(
            *left >= f32::from(row.left()) - OVERLAP_TOLERANCE,
            "{when}: tab {place} starts left of the row"
        );
        assert!(
            *right <= f32::from(row.right()) + OVERLAP_TOLERANCE,
            "{when}: tab {place} ends right of the row"
        );
        assert!(
            right - left > 40.,
            "{when}: tab {place} is {:.1} px wide, not whole",
            right - left
        );
    }
    let gone: &'static str = Box::leak(format!("tab-{}", count + 1).into_boxed_str());
    assert!(
        cx.debug_bounds(gone).is_none(),
        "{when}: {gone} is still drawn"
    );
    let last = edges.last().expect("a tab").1;
    let gap = f32::from(row.right()) - last;
    // While the tabs glide the row follows the last one a frame behind; settled it ends with it.
    let most = match is_settled {
        true => OVERLAP_TOLERANCE,
        // Mid-glide the + is at most one closed tab behind the last tab, however loaded the machine.
        false => MOST_BEHIND,
    };
    assert!(
        (0. ..=most).contains(&gap),
        "{when}: {gap:.1} px between the last tab and the row's end ({edges:?})"
    );
}

fn close_one_of_four_and_look(cx: &mut TestAppContext, place: usize) {
    let (cx, bar) = four(cx);
    close_place(&bar, place, cx);
    for frames in [1, 30, SETTLED] {
        let is_settled = frames == SETTLED;
        assert_strip_is_whole(
            &bar,
            frames,
            &format!("closing {place}, {frames} frames on"),
            is_settled,
            cx,
        );
    }
}

#[test]
fn closing_the_first_of_four_tabs_leaves_a_whole_strip_beside_the_plus() {
    real_text::run(|cx| close_one_of_four_and_look(cx, 0));
}

#[test]
fn closing_the_middle_of_four_tabs_leaves_a_whole_strip_beside_the_plus() {
    real_text::run(|cx| close_one_of_four_and_look(cx, 1));
}

#[test]
fn closing_the_last_of_four_tabs_leaves_a_whole_strip_beside_the_plus() {
    real_text::run(|cx| close_one_of_four_and_look(cx, 3));
}

#[gpui::test]
fn joined_group_keeps_both_close_hitboxes_through_swap_and_overflow(cx: &mut TestAppContext) {
    let mut tabs = many();
    let first = tabs.tabs()[0].id();
    let last = tabs.tabs().last().unwrap().id();
    tabs.activate(last);
    assert!(tabs.create_split(first, last));
    let (cx, bar, _) = open(cx, tabs);
    let selectors = |cx: &mut VisualTestContext| {
        [
            cx.debug_bounds("tab-close-15").unwrap(),
            cx.debug_bounds("tab-close-open").unwrap(),
        ]
    };
    let [first_close, last_close] = selectors(cx);
    assert!(first_close.right() <= last_close.left());
    bar.update(cx, |bar, cx| assert!(bar.swap_split(first, cx)));
    draw(cx);
    assert!(cx.debug_bounds("tab-close-16").is_some());
    assert!(cx.debug_bounds("tab-close-open").is_some());
}

#[gpui::test]
fn closing_either_hitbox_keeps_the_other_tab_and_its_history(cx: &mut TestAppContext) {
    let mut tabs = Tabs::new();
    tabs.navigate(lane("left"));
    let left = tabs.active().id();
    let right = tabs.open_new(lane("right"));
    tabs.navigate(lane("right history"));
    tabs.create_split(left, right);
    let (cx, bar, _) = open(cx, tabs);
    crate::ui::lane_tests::click(cx, "tab-close-1");
    cx.read_entity(&bar, |bar, _| {
        assert_eq!(bar.tabs().visible(), (right, None));
        assert!(bar.tabs().active().can_go_back());
    });
}
