//! A tab dragged along the bar, as the strip before the redesign let it be: it follows the pointer, the
//! tabs it passes give way, the release lands it, and the trackpad taps once for each slot it crosses.
//! Driven like a person would, on GPUI's test platform.

use super::tests::{draw, lane, open, pages, shown};
use super::PageBar;
use crate::ui::tab_motion::haptics::Recorded;
use gpui::{
    point, px, AppContext as _, Bounds, Entity, Modifiers, MouseButton, Pixels, Point,
    TestAppContext, VisualTestContext,
};
use sakura_core::tabs::{Page, Tabs};
use std::rc::Rc;
use std::time::Duration;

/// The Inbox shown, then two lanes.
fn three() -> Tabs {
    let mut tabs = Tabs::new();
    tabs.open_new(lane("a"));
    tabs.open_new(lane("b"));
    tabs.activate_digit(1);
    tabs
}

/// Where the pill at `place` (0 is the first) is drawn.
pub(super) fn drawn(
    bar: &Entity<PageBar>,
    place: usize,
    cx: &mut VisualTestContext,
) -> Bounds<Pixels> {
    let is_open = cx.read_entity(bar, |bar, _| {
        bar.tabs().active().id() == bar.tabs().tabs()[place].id()
    });
    let selector: &'static str = match is_open {
        true => "tab-open",
        false => Box::leak(format!("tab-{}", place + 1).into_boxed_str()),
    };
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("no pill at place {place}"))
}

fn middle(bar: &Entity<PageBar>, place: usize, cx: &mut VisualTestContext) -> Point<Pixels> {
    drawn(bar, place, cx).center()
}

/// Presses the pill at `place` and moves the pointer `distance` px along the bar, the button held.
pub(super) fn pick_up(
    bar: &Entity<PageBar>,
    place: usize,
    distance: f32,
    cx: &mut VisualTestContext,
) {
    let start = middle(bar, place, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    hold_at(start, distance, cx);
}

fn hold_at(start: Point<Pixels>, distance: f32, cx: &mut VisualTestContext) {
    let spot = start + point(px(distance), px(0.));
    cx.simulate_mouse_move(spot, MouseButton::Left, Modifiers::none());
    draw(cx);
}

pub(super) fn let_go(bar: &Entity<PageBar>, cx: &mut VisualTestContext) {
    let anywhere = middle(bar, 0, cx);
    cx.simulate_mouse_up(anywhere, MouseButton::Left, Modifiers::none());
    draw(cx);
}

fn recorded(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Rc<Recorded> {
    let double = Rc::new(Recorded::default());
    let given = double.clone();
    bar.update(cx, |bar, _| bar.set_haptics(given));
    double
}

fn lefts(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Vec<f32> {
    let places = cx.read_entity(bar, |bar, _| bar.tabs().len());
    (0..places)
        .map(|place| f32::from(drawn(bar, place, cx).left()))
        .collect()
}

#[gpui::test]
fn a_drag_moves_the_tab_and_does_not_choose_it(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 1, 400., cx);
    assert_eq!(
        pages(&bar, cx),
        [Page::Inbox, lane("a"), lane("b")],
        "nothing lands before the release"
    );
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("b"), lane("a")]);
    assert_eq!(
        shown(&bar, cx),
        Page::Inbox,
        "a dragged tab stays what it was"
    );
}

#[gpui::test]
fn any_tab_can_land_first_and_the_first_can_leave(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 2, -900., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [lane("b"), Page::Inbox, lane("a")]);
    pick_up(&bar, 0, 900., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
}

#[gpui::test]
fn a_press_under_the_threshold_is_a_click(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 1, 3., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
    assert_eq!(shown(&bar, cx), lane("a"));
}

#[gpui::test]
fn a_dragged_tab_never_leaves_the_bar(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let first_left = drawn(&bar, 0, cx).left();
    pick_up(&bar, 2, -5000., cx);
    assert_eq!(
        drawn(&bar, 2, cx).left(),
        first_left,
        "it stops flush at the start"
    );
    let_go(&bar, cx);
}

#[gpui::test]
fn escape_puts_a_dragged_tab_back(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 1, 400., cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
    assert_eq!(
        shown(&bar, cx),
        Page::Inbox,
        "and the release chooses nothing"
    );
}

#[gpui::test]
fn losing_the_window_puts_a_dragged_tab_back(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 0, 200., cx);
    assert!(cx.read_entity(&bar, |bar, _| bar.is_dragging()));
    // Another app comes to the front with the button still down: the release never reaches this window.
    cx.deactivate_window();
    draw(cx);
    assert!(!cx.read_entity(&bar, |bar, _| bar.is_dragging()));
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
}

#[gpui::test]
fn a_dragged_tab_taps_once_for_each_slot_it_crosses_and_a_click_never(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let taps = recorded(&bar, cx);
    let second = middle(&bar, 1, cx);
    cx.simulate_click(second, Modifiers::none());
    draw(cx);
    assert!(taps.taps().is_empty(), "an ordinary click is silent");
    let boxes: Vec<_> = (0..3).map(|place| drawn(&bar, place, cx)).collect();
    let start = boxes[0].center();
    let to_cross = |slot: usize| f32::from(boxes[slot].center().x - boxes[0].right()) + 2.;
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    hold_at(start, 10., cx);
    assert!(taps.taps().is_empty(), "picked up, no slot crossed yet");
    hold_at(start, to_cross(1), cx);
    assert_eq!(taps.taps(), ["alignment"]);
    hold_at(start, to_cross(1) + 1., cx);
    assert_eq!(taps.taps().len(), 1, "moving inside a slot is silent");
    hold_at(start, to_cross(2), cx);
    assert_eq!(taps.taps().len(), 2);
    hold_at(start, 10., cx);
    assert_eq!(
        taps.taps().len(),
        3,
        "back across both in one move: one tap"
    );
    let_go(&bar, cx);
    assert_eq!(taps.taps().len(), 3, "the release is silent");
}

#[gpui::test]
fn a_tab_stopped_at_the_bar_s_end_taps_no_more(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let taps = recorded(&bar, cx);
    pick_up(&bar, 0, 5000., cx);
    let taken = taps.taps().len();
    hold_at(middle(&bar, 0, cx), 9000., cx);
    assert_eq!(
        taps.taps().len(),
        taken,
        "the tab has stopped: so has the trackpad"
    );
}

#[gpui::test]
fn the_tabs_a_drag_passes_step_aside_by_the_width_of_the_tab_in_the_hand(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    cx.update(|_, cx| cx.set_reduce_motion(false));
    let before = lefts(&bar, cx);
    let step = f32::from(drawn(&bar, 0, cx).size.width) + f32::from(crate::theme::TAB_GAP);
    pick_up(&bar, 0, 400., cx);
    settle(cx);
    let after = lefts(&bar, cx);
    assert!(
        (after[1] - (before[1] - step)).abs() < 1.,
        "{after:?} from {before:?}"
    );
    assert!(
        (after[2] - (before[2] - step)).abs() < 1.,
        "{after:?} from {before:?}"
    );
    let_go(&bar, cx);
}

/// Lets the springs run: frames drawn over a second of real time.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..70 {
        std::thread::sleep(Duration::from_millis(16));
        draw(cx);
    }
}

#[gpui::test]
fn with_motion_a_closed_tab_leaves_the_others_gliding_to_their_places(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    cx.update(|_, cx| cx.set_reduce_motion(false));
    let before = lefts(&bar, cx);
    let step = f32::from(drawn(&bar, 1, cx).size.width) + f32::from(crate::theme::TAB_GAP);
    let closed = cx.read_entity(&bar, |bar, _| bar.tabs().tabs()[1].id());
    bar.update(cx, |bar, cx| bar.close(closed, cx));
    draw(cx);
    let first_frame = lefts(&bar, cx);
    assert!(
        (first_frame[1] - before[2]).abs() < 1.,
        "it starts where it stood: {first_frame:?} from {before:?}"
    );
    settle(cx);
    let rested = lefts(&bar, cx);
    assert!((rested[1] - (before[2] - step)).abs() < 1., "{rested:?}");
}

#[gpui::test]
fn with_reduced_motion_a_dropped_tab_is_on_its_place_at_once(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    pick_up(&bar, 0, 100., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [lane("a"), Page::Inbox, lane("b")]);
    let places = lefts(&bar, cx);
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]),
        "{places:?}"
    );
    let gap = places[1] - places[0] - f32::from(drawn(&bar, 0, cx).size.width);
    assert!(
        (gap - f32::from(crate::theme::TAB_GAP)).abs() < 1.,
        "no tab is left displaced: {places:?}"
    );
}

#[gpui::test]
fn unpaired_drag_reorders_around_the_whole_pair(cx: &mut TestAppContext) {
    let mut tabs = three();
    let left = tabs.tabs()[0].id();
    let right = tabs.tabs()[1].id();
    tabs.create_split(left, right);
    let (cx, bar, _) = open(cx, tabs);
    pick_up(&bar, 2, -180., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [lane("b"), Page::Inbox, lane("a")]);
    pick_up(&bar, 1, 350., cx);
    let_go(&bar, cx);
    assert_eq!(pages(&bar, cx), [lane("b"), Page::Inbox, lane("a")]);
}

fn edge_band(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    let viewport = cx.update(|window, _| window.viewport_size());
    let top = crate::theme::page_bar::BAR_HEIGHT;
    let bounds = Bounds::new(
        point(px(0.), top),
        gpui::size(
            viewport.width,
            viewport.height - top - crate::theme::STATUS_BAR_HEIGHT,
        ),
    );
    bar.update(cx, |bar, _| bar.set_split_drop_bounds(bounds));
    bounds
}

fn drag_to_edge(
    bar: &Entity<PageBar>,
    place: usize,
    to: Point<Pixels>,
    cx: &mut VisualTestContext,
) {
    let start = middle(bar, place, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(point(start.x, to.y), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    draw(cx);
}

fn preview(bar: &Entity<PageBar>, cx: &mut VisualTestContext) -> Option<super::SplitSide> {
    cx.read_entity(bar, |bar, _| bar.split_drop_side())
}

#[gpui::test]
fn vertical_then_edge_drag_can_split_on_either_side(cx: &mut TestAppContext) {
    use super::SplitSide;
    for side in [SplitSide::Left, SplitSide::Right] {
        let (cx, bar, _) = open(cx, three());
        let bounds = edge_band(&bar, cx);
        let x = match side {
            SplitSide::Left => bounds.left() + px(5.),
            SplitSide::Right => bounds.right() - px(5.),
        };
        let end = point(x, bounds.center().y);
        drag_to_edge(&bar, 1, end, cx);
        assert_eq!(preview(&bar, cx), Some(side));
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        assert!(preview(&bar, cx).is_none());
        let expected = match side {
            SplitSide::Left => [lane("a"), Page::Inbox, lane("b")],
            SplitSide::Right => [Page::Inbox, lane("a"), lane("b")],
        };
        assert_eq!(pages(&bar, cx), expected);
        cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_some()));
    }
}

#[gpui::test]
fn drop_on_edge_commits_once_and_does_not_reorder(cx: &mut TestAppContext) {
    let (cx, bar, events) = open(cx, three());
    let bounds = edge_band(&bar, cx);
    let end = point(bounds.right() - px(2.), bounds.center().y);
    drag_to_edge(&bar, 1, end, cx);
    assert_eq!(events.get(), 1, "only the preview changed");
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    assert_eq!(events.get(), 1);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    let committed = events.get();
    assert_eq!(
        committed, 3,
        "the preview cleared and the model changed once"
    );
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    assert_eq!(events.get(), committed);
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
}

#[gpui::test]
fn edge_bands_exclude_titlebar_status_and_outside_window(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let bounds = edge_band(&bar, cx);
    for end in [
        point(px(2.), bounds.top() - px(2.)),
        point(px(2.), bounds.bottom() + px(2.)),
        point(bounds.left() - px(2.), bounds.center().y),
        point(bounds.right() + px(2.), bounds.center().y),
    ] {
        drag_to_edge(&bar, 1, end, cx);
        assert_eq!(preview(&bar, cx), None);
        cx.simulate_keystrokes("escape");
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    }
    cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_none()));
}

#[gpui::test]
fn near_edge_titlebar_reorder_stays_a_reorder(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let bounds = edge_band(&bar, cx);
    let end = point(bounds.left() + px(2.), middle(&bar, 2, cx).y);
    drag_to_edge(&bar, 2, end, cx);
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    assert_eq!(pages(&bar, cx), [lane("b"), Page::Inbox, lane("a")]);
}

#[gpui::test]
fn escape_blur_lost_button_or_modal_clears_preview_and_commit(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let end = point(px(2.), edge_band(&bar, cx).center().y);
    drag_to_edge(&bar, 1, end, cx);
    assert!(preview(&bar, cx).is_some());
    cx.simulate_keystrokes("escape");
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());

    drag_to_edge(&bar, 1, end, cx);
    cx.simulate_mouse_move(end, None, Modifiers::none());
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());

    drag_to_edge(&bar, 1, end, cx);
    cx.deactivate_window();
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.update(|window, _| window.activate_window());

    drag_to_edge(&bar, 1, end, cx);
    let guard = cx.update(|_, cx| crate::ui::dialog_guard::DialogToken::take(cx));
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    drop(guard);
    cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_none()));
}

#[gpui::test]
fn changed_target_between_preview_and_release_cancels(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let end = point(px(2.), edge_band(&bar, cx).center().y);
    drag_to_edge(&bar, 1, end, cx);
    assert!(preview(&bar, cx).is_some());
    cx.simulate_keystrokes("cmd-3");
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_none()));
}

#[gpui::test]
fn the_active_tab_splits_with_its_neighbor_on_either_edge(cx: &mut TestAppContext) {
    use super::SplitSide;
    for (place, side) in [(0, SplitSide::Right), (1, SplitSide::Left)] {
        let mut tabs = three();
        tabs.activate_digit(place + 1);
        let active = tabs.active().id();
        let peer = tabs.tabs()[usize::from(place == 0)].id();
        let (cx, bar, _) = open(cx, tabs);
        let bounds = edge_band(&bar, cx);
        let x = match side {
            SplitSide::Left => bounds.left() + px(5.),
            SplitSide::Right => bounds.right() - px(5.),
        };
        let end = point(x, bounds.center().y);
        drag_to_edge(&bar, place, end, cx);
        assert_eq!(preview(&bar, cx), Some(side));
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        assert!(preview(&bar, cx).is_none());
        cx.read_entity(&bar, |bar, _| {
            let split = bar.tabs().split().unwrap();
            match side {
                SplitSide::Left => assert_eq!((split.left, split.right), (active, peer)),
                SplitSide::Right => assert_eq!((split.left, split.right), (peer, active)),
            }
            assert_eq!(bar.tabs().active().id(), active);
        });
    }
}

#[gpui::test]
fn a_paired_member_cannot_create_another_split(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let end = point(px(2.), edge_band(&bar, cx).center().y);
    bar.update(cx, |bar, cx| {
        let left = bar.tabs().tabs()[0].id();
        let right = bar.tabs().tabs()[1].id();
        assert!(bar.create_split(left, right, cx));
    });
    draw(cx);
    drag_to_edge(&bar, 1, end, cx);
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    assert_eq!(pages(&bar, cx), [Page::Inbox, lane("a"), lane("b")]);
}

#[gpui::test]
fn divider_or_titlebar_drag_never_arms_tab_drop(cx: &mut TestAppContext) {
    let (cx, bar, _) = open(cx, three());
    let bounds = edge_band(&bar, cx);
    let to = point(bounds.right() - px(2.), bounds.center().y);
    cx.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());

    // The test platform cannot start a native window move.
    let titlebar = point(bounds.right() - px(2.), px(5.));
    cx.simulate_mouse_move(titlebar, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    assert_eq!(preview(&bar, cx), None);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.read_entity(&bar, |bar, _| assert!(bar.tabs().split().is_none()));
}
