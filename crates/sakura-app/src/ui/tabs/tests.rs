//! The strip driven like a person would, on GPUI's test platform: a simulated pointer and keys on a test
//! window holding the strip over an empty page.

use super::haptics::Recorded;
use super::keys;
use super::strip::{Strip, StripEvent};
use crate::theme::{Appearance, Theme};
use gpui::{
    div, point, px, AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton,
    ParentElement as _, Pixels, Point, Render, Styled as _, TestAppContext, VisualTestContext,
    Window,
};
use sakura_core::tabs::{Placement, TabId, Tabs};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The window's content: the strip framed over nothing.
struct Host {
    strip: Entity<Strip>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        div()
            .size_full()
            .child(super::framed(&self.strip, div(), &palette))
    }
}

/// The panel's rounded bottom-left corner is drawn again over whatever page is shown: a square of the
/// corner's size, on the panel's left edge and on its bottom edge, after the page in the panel's box.
#[gpui::test]
fn strip_the_panel_s_bottom_corner_lies_over_the_page_on_the_panel_s_edges(
    cx: &mut TestAppContext,
) {
    let (cx, _) = open(cx, &["m1"]);
    let corner = cx.debug_bounds("panel-corner").expect("the corner");
    let window = cx.update(|window, _| window.viewport_size());
    let radius = px(super::metrics::RADIUS);
    assert_eq!(
        corner.size,
        gpui::size(radius, radius),
        "the corner's own square"
    );
    assert_eq!(corner.left(), px(0.), "on the panel's left edge");
    assert_eq!(corner.bottom(), window.height, "on the panel's bottom edge");
}

/// The lavoro every test strip opens first: its tab is the one shown, flush on the panel's corner.
const FIRST: &str = "capo";

/// A strip with [`FIRST`] and one tab per name, in a test window, its first frame drawn. Motion is
/// reduced, so a tab is where its slot is as soon as the next frame is drawn.
fn open<'a>(
    cx: &'a mut TestAppContext,
    names: &[&str],
) -> (&'a mut VisualTestContext, Entity<Strip>) {
    let mut tabs = Tabs::new();
    for name in std::iter::once(&FIRST).chain(names) {
        tabs.open(name, Placement::Background);
    }
    open_on(cx, tabs)
}

/// A strip over `tabs` in a test window, its first frame drawn.
fn open_on(cx: &mut TestAppContext, tabs: Tabs) -> (&mut VisualTestContext, Entity<Strip>) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(true);
    });
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let strip = cx.new(|cx| Strip::new(tabs, cx));
        keys::install(&strip, cx);
        *kept.borrow_mut() = Some(strip.clone());
        let host = cx.new(|_| Host { strip });
        gpui_component::Root::new(host, window, cx)
    });
    let strip = slot.borrow_mut().take().expect("the strip was built");
    cx.update(|window, _| window.activate_window());
    // The window hears of its activation before any pointer is moved: the test platform puts the pointer
    // back on the window's corner when it does, and a tab there would take the hover from the one meant.
    cx.run_until_parked();
    draw(cx);
    (cx, strip)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The tab ids in strip order.
fn order(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> Vec<TabId> {
    cx.update(|_, cx| {
        strip
            .read(cx)
            .tabs()
            .tabs()
            .iter()
            .map(|tab| tab.id)
            .collect()
    })
}

fn active_place(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| strip.read(cx).tabs().active_index())
        .expect("a tab is shown")
}

/// The middle of the tab at `place` (0 is the first) as drawn.
fn middle(strip: &Entity<Strip>, place: usize, cx: &mut VisualTestContext) -> Point<Pixels> {
    cx.update(|_, cx| {
        let strip = strip.read(cx);
        let id = strip.tabs().tabs()[place].id;
        strip.tab_bounds(id).expect("the tab was drawn").center()
    })
}

fn corner(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> f32 {
    cx.update(|_, cx| strip.read(cx).corner)
}

/// Presses the tab at `place` and moves the pointer `distance` px along the strip, the button held.
fn pick_up(strip: &Entity<Strip>, place: usize, distance: f32, cx: &mut VisualTestContext) {
    let start = middle(strip, place, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    draw(cx);
    let held = start + point(px(distance), px(0.));
    cx.simulate_mouse_move(held, MouseButton::Left, Modifiers::none());
    draw(cx);
}

fn let_go(strip: &Entity<Strip>, cx: &mut VisualTestContext) {
    let anywhere = middle(strip, 0, cx);
    cx.simulate_mouse_up(anywhere, MouseButton::Left, Modifiers::none());
    draw(cx);
}

/// Counts the changes the strip tells the desk about.
fn changes(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    cx.update(|_, cx| {
        cx.subscribe(strip, move |_, _: &StripEvent, _| seen.set(seen.get() + 1))
            .detach();
    });
    count
}

#[gpui::test]
fn strip_a_click_on_a_tab_shows_it_and_tells_the_desk(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let told = changes(&strip, cx);
    let second = middle(&strip, 1, cx);
    cx.simulate_click(second, Modifiers::none());
    draw(cx);
    assert_eq!(active_place(&strip, cx), 1);
    assert_eq!(told.get(), 1);
}

#[gpui::test]
fn strip_a_drag_moves_the_tab_and_does_not_choose_it(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let before = order(&strip, cx);
    pick_up(&strip, 1, 400., cx);
    assert_eq!(
        order(&strip, cx),
        before,
        "nothing lands before the release"
    );
    let_go(&strip, cx);
    assert_eq!(order(&strip, cx), [before[0], before[2], before[1]]);
    assert_eq!(
        active_place(&strip, cx),
        0,
        "a dragged tab stays what it was"
    );
}

#[gpui::test]
fn strip_any_tab_can_land_first_and_the_first_can_leave(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let before = order(&strip, cx);
    pick_up(&strip, 2, -900., cx);
    let_go(&strip, cx);
    assert_eq!(order(&strip, cx), [before[2], before[0], before[1]]);
    pick_up(&strip, 0, 900., cx);
    let_go(&strip, cx);
    assert_eq!(order(&strip, cx), before);
}

#[gpui::test]
fn strip_a_press_under_the_threshold_is_a_click(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1"]);
    let before = order(&strip, cx);
    pick_up(&strip, 1, 3., cx);
    let_go(&strip, cx);
    assert_eq!(order(&strip, cx), before);
    assert_eq!(active_place(&strip, cx), 1);
}

#[gpui::test]
fn strip_the_corner_follows_the_active_tab_at_14_28_and_120_px(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    assert_eq!(corner(&strip, cx), 0.0, "flush on the corner: square");
    let start = middle(&strip, 0, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for (distance, progress) in [(14., 0.5), (28., 1.0), (120., 1.0), (14., 0.5)] {
        let held = start + point(px(distance), px(0.));
        cx.simulate_mouse_move(held, MouseButton::Left, Modifiers::none());
        draw(cx);
        assert_eq!(corner(&strip, cx), progress, "{distance} px from the edge");
    }
    let_go(&strip, cx);
    draw(cx);
    assert_eq!(corner(&strip, cx), 0.0, "back on the corner");
}

#[gpui::test]
fn strip_the_tab_that_lands_first_squares_the_corner_again(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    pick_up(&strip, 1, -900., cx);
    let_go(&strip, cx);
    draw(cx);
    assert_eq!(active_place(&strip, cx), 1, "the active tab was passed");
    assert_eq!(corner(&strip, cx), 1.0, "round: another tab stands first");
    pick_up(&strip, 1, -900., cx);
    let_go(&strip, cx);
    draw(cx);
    assert_eq!(active_place(&strip, cx), 0);
    assert_eq!(
        corner(&strip, cx),
        0.0,
        "the active tab landed first: square"
    );
}

#[gpui::test]
fn strip_a_dragged_tab_never_leaves_the_strip(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let first_left = cx.update(|_, cx| {
        let strip = strip.read(cx);
        strip
            .tab_bounds(strip.tabs().tabs()[0].id)
            .map(|tab| tab.left())
    });
    pick_up(&strip, 2, -5000., cx);
    let held_left = cx.update(|_, cx| {
        let strip = strip.read(cx);
        strip
            .tab_bounds(strip.tabs().tabs()[2].id)
            .map(|tab| tab.left())
    });
    assert_eq!(held_left, first_left, "it stops flush at the strip's start");
    let_go(&strip, cx);
}

#[gpui::test]
fn strip_escape_puts_a_dragged_tab_back(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let before = order(&strip, cx);
    pick_up(&strip, 1, 400., cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    let_go(&strip, cx);
    assert_eq!(order(&strip, cx), before);
    assert_eq!(
        active_place(&strip, cx),
        0,
        "and the release chooses nothing"
    );
}

#[gpui::test]
fn strip_the_middle_button_closes_a_tab_while_another_remains(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1"]);
    let second = middle(&strip, 1, cx);
    cx.simulate_mouse_down(second, MouseButton::Middle, Modifiers::none());
    cx.simulate_mouse_up(second, MouseButton::Middle, Modifiers::none());
    draw(cx);
    assert_eq!(order(&strip, cx).len(), 1);
    let only = middle(&strip, 0, cx);
    cx.simulate_mouse_down(only, MouseButton::Middle, Modifiers::none());
    cx.simulate_mouse_up(only, MouseButton::Middle, Modifiers::none());
    draw(cx);
    assert_eq!(order(&strip, cx).len(), 1, "the last tab stays");
}

#[gpui::test]
fn strip_cmd_digits_select_by_place_cmd_9_the_last_and_cmd_w_closes(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2", "m3"]);
    cx.simulate_keystrokes("cmd-2");
    assert_eq!(active_place(&strip, cx), 1);
    cx.simulate_keystrokes("cmd-9");
    assert_eq!(active_place(&strip, cx), 3);
    cx.simulate_keystrokes("cmd-1");
    assert_eq!(active_place(&strip, cx), 0);
    cx.simulate_keystrokes("cmd-8");
    assert_eq!(
        active_place(&strip, cx),
        0,
        "no eighth tab: nothing changes"
    );
    cx.simulate_keystrokes("cmd-w");
    assert_eq!(order(&strip, cx).len(), 3);
    assert_eq!(
        active_place(&strip, cx),
        0,
        "the right neighbour takes over"
    );
}

#[gpui::test]
fn strip_with_the_first_tab_away_from_the_edge_the_corner_is_round(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1"]);
    cx.update(|_, cx| {
        strip.update(cx, |strip, cx| {
            strip.set_leading_inset(px(72.));
            cx.notify();
        })
    });
    draw(cx);
    assert_eq!(corner(&strip, cx), 1.0);
}

#[gpui::test]
fn strip_with_motion_a_tab_switch_animates_nothing(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    assert_eq!(corner(&strip, cx), 0.0, "the first tab is flush");
    cx.simulate_keystrokes("cmd-3");
    draw(cx);
    assert_eq!(corner(&strip, cx), 1.0, "the corner is there at once");
    let is_moving = cx.update(|_, cx| strip.read(cx).is_moving());
    assert!(!is_moving, "no further frame is requested");
}

#[gpui::test]
fn strip_with_motion_a_closed_tab_leaves_a_glide(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    let first = order(&strip, cx)[0];
    cx.update(|_, cx| strip.update(cx, |strip, cx| strip.close(first, cx)));
    draw(cx);
    let gliding = cx.update(|_, cx| strip.read(cx).offsets.len());
    assert_eq!(gliding, 2, "both tabs left travel to their new slots");
}

/// Where every tab is drawn, in strip order.
fn all_bounds(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> Vec<gpui::Bounds<Pixels>> {
    cx.update(|_, cx| {
        let strip = strip.read(cx);
        let tabs = strip.tabs().tabs().iter();
        tabs.filter_map(|tab| strip.tab_bounds(tab.id)).collect()
    })
}

#[gpui::test]
fn strip_choosing_hovering_or_pressing_a_tab_moves_no_tab(cx: &mut TestAppContext) {
    // Ten long titles: more than the strip holds at full width, so every tab is narrowed.
    let long: Vec<String> = (0..10)
        .map(|n| format!("a-task-with-a-very-long-name-number-{n}"))
        .collect();
    let names: Vec<&str> = long.iter().map(String::as_str).collect();
    let (cx, strip) = open(cx, &names);
    let before = all_bounds(&strip, cx);
    assert_eq!(before.len(), 11);
    for digit in ["cmd-2", "cmd-5", "cmd-9", "cmd-1"] {
        cx.simulate_keystrokes(digit);
        draw(cx);
        assert_eq!(all_bounds(&strip, cx), before, "after {digit}");
    }
    let third = middle(&strip, 2, cx);
    cx.simulate_mouse_move(third, None, Modifiers::none());
    draw(cx);
    assert_eq!(all_bounds(&strip, cx), before, "under the pointer");
    cx.simulate_mouse_down(third, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(all_bounds(&strip, cx), before, "pressed");
    cx.simulate_mouse_up(third, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(active_place(&strip, cx), 2);
    assert_eq!(all_bounds(&strip, cx), before, "chosen with the pointer");
    assert!(
        cx.debug_bounds("tab-close-3").is_none(),
        "a narrowed tab gives its close button's room to its title, selected or not"
    );
}

#[gpui::test]
fn strip_a_tab_at_full_width_shows_its_close_button_when_selected(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1"]);
    assert!(cx.debug_bounds("tab-close-1").is_some(), "the active tab");
    let before = all_bounds(&strip, cx);
    cx.simulate_keystrokes("cmd-2");
    draw(cx);
    assert!(cx.debug_bounds("tab-close-2").is_some());
    assert_eq!(all_bounds(&strip, cx), before);
}

#[gpui::test]
fn strip_a_dragged_tab_taps_once_for_each_slot_it_crosses_and_a_click_never(
    cx: &mut TestAppContext,
) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let recorded = Rc::new(Recorded::default());
    let double = recorded.clone();
    cx.update(|_, cx| strip.update(cx, |strip, _| strip.set_haptics(double)));
    let second = middle(&strip, 1, cx);
    cx.simulate_click(second, Modifiers::none());
    draw(cx);
    assert!(recorded.taps().is_empty(), "an ordinary click is silent");
    let boxes = all_bounds(&strip, cx);
    let start = boxes[0].center();
    let to_cross = |slot: usize| f32::from(boxes[slot].center().x - boxes[0].right()) + 2.;
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    let hold_at = |distance: f32, cx: &mut VisualTestContext| {
        let spot = start + point(px(distance), px(0.));
        cx.simulate_mouse_move(spot, MouseButton::Left, Modifiers::none());
        draw(cx);
    };
    hold_at(10., cx);
    assert!(recorded.taps().is_empty(), "picked up, no slot crossed yet");
    hold_at(to_cross(1), cx);
    assert_eq!(recorded.taps(), ["alignment"]);
    hold_at(to_cross(1) + 1., cx);
    assert_eq!(recorded.taps().len(), 1, "moving inside a slot is silent");
    hold_at(to_cross(2), cx);
    assert_eq!(recorded.taps().len(), 2);
    hold_at(10., cx);
    assert_eq!(
        recorded.taps().len(),
        3,
        "back across both in one move: one tap"
    );
    let_go(&strip, cx);
    assert_eq!(recorded.taps().len(), 3, "the release is silent");
}

#[test]
fn the_strip_sets_no_cursor_so_the_pointer_stays_the_arrow() {
    let sources = [
        include_str!("tab.rs"),
        include_str!("element.rs"),
        include_str!("panel.rs"),
        include_str!("strip.rs"),
        include_str!("pointer.rs"),
    ];
    for source in sources {
        assert!(!source.contains("cursor_"), "a cursor style in the strip");
        assert!(
            !source.contains("CursorStyle"),
            "a cursor style in the strip"
        );
    }
}

/// Sixty tabs: more than the strip shows, so the last ones stand past its end.
fn overflowing(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Strip>) {
    let names: Vec<String> = (0..60).map(|n| format!("lavoro-{n}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    open(cx, &names)
}

/// The place the tab in the hand would land on, the right edge it is drawn with, and the middle of the
/// slot it would land on (window px).
fn landing(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> (usize, f32, f32) {
    cx.update(|_, cx| {
        let strip = strip.read(cx);
        let drag = strip.drag.expect("a tab is in the hand");
        let drawn = strip.tab_bounds(drag.id).expect("the tab was drawn");
        let slot = strip.slots[drag.to].1;
        let middle = f32::from(strip.origin.x) + slot.left + slot.width / 2.;
        (drag.to, f32::from(drawn.right()), middle)
    })
}

#[gpui::test]
fn strip_a_tab_stopped_at_the_strip_s_end_lands_where_it_is_drawn(cx: &mut TestAppContext) {
    let (cx, strip) = overflowing(cx);
    let recorded = Rc::new(Recorded::default());
    let double = recorded.clone();
    cx.update(|_, cx| strip.update(cx, |strip, _| strip.set_haptics(double)));
    pick_up(&strip, 0, 5000., cx);
    let (to, drawn_right, middle_of_target) = landing(&strip, cx);
    assert!(
        middle_of_target <= drawn_right,
        "place {to}: its middle at {middle_of_target} is past the drawn tab's edge at {drawn_right}"
    );
    let taps = recorded.taps().len();
    let farther = middle(&strip, 0, cx) + point(px(9000.), px(0.));
    cx.simulate_mouse_move(farther, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(
        landing(&strip, cx).0,
        to,
        "the tab has stopped: so has its target"
    );
    assert_eq!(recorded.taps().len(), taps, "and the trackpad stays silent");
}

#[gpui::test]
fn strip_losing_the_window_puts_a_dragged_tab_back(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let before = order(&strip, cx);
    pick_up(&strip, 0, 200., cx);
    assert!(cx.update(|_, cx| strip.read(cx).drag.is_some()));
    // Another app comes to the front with the button still down: the release never reaches this window.
    cx.deactivate_window();
    draw(cx);
    let (is_pressed, is_dragged) = cx.update(|_, cx| {
        let strip = strip.read(cx);
        (strip.press.is_some(), strip.drag.is_some())
    });
    assert!(!is_pressed && !is_dragged, "nothing stays in the hand");
    assert_eq!(
        order(&strip, cx),
        before,
        "and the tab is back where it was"
    );
}

/// Moves the pointer to the middle of the tab at `place` and draws.
fn rest_on(strip: &Entity<Strip>, place: usize, cx: &mut VisualTestContext) {
    let spot = middle(strip, place, cx);
    cx.simulate_mouse_move(spot, None, Modifiers::none());
    draw(cx);
}

/// Lets `time` pass and draws.
fn wait(time: std::time::Duration, cx: &mut VisualTestContext) {
    cx.executor().advance_clock(time);
    cx.run_until_parked();
    draw(cx);
}

/// Where the strip's tooltip is drawn, if one is.
fn tip(cx: &mut VisualTestContext) -> Option<gpui::Bounds<Pixels>> {
    cx.debug_bounds("tab-tip")
}

#[gpui::test]
fn strip_a_tooltip_shows_after_the_delay_and_moves_on_with_the_pointer_both_ways(
    cx: &mut TestAppContext,
) {
    use super::metrics::TOOLTIP_DELAY;
    let grace = std::time::Duration::from_millis(400);
    let (cx, strip) = open(cx, &["m1", "m2"]);
    rest_on(&strip, 1, cx);
    assert!(tip(cx).is_none(), "nothing before the delay");
    wait(TOOLTIP_DELAY, cx);
    assert!(
        tip(cx).is_some(),
        "the tooltip of the tab under the pointer"
    );
    // From an inactive tab onto the active one, which is painted later and so hears the pointer first.
    rest_on(&strip, 0, cx);
    wait(grace, cx);
    assert!(
        tip(cx).is_some(),
        "the active tab's tooltip stays after the grace period"
    );
    // And back, from the active tab onto an inactive one.
    rest_on(&strip, 2, cx);
    wait(grace, cx);
    assert!(tip(cx).is_some(), "the next tab's tooltip stays too");
    // Off the strip it goes.
    let below = middle(&strip, 1, cx) + point(px(0.), px(200.));
    cx.simulate_mouse_move(below, None, Modifiers::none());
    draw(cx);
    wait(grace, cx);
    assert!(tip(cx).is_none(), "no tab is under the pointer");
}

#[gpui::test]
fn strip_no_tooltip_shows_while_a_tab_is_pressed_or_dragged(cx: &mut TestAppContext) {
    use super::metrics::TOOLTIP_DELAY;
    let (cx, strip) = open(cx, &["m1", "m2"]);
    rest_on(&strip, 1, cx);
    pick_up(&strip, 1, 30., cx);
    wait(TOOLTIP_DELAY * 2, cx);
    assert!(tip(cx).is_none());
    let_go(&strip, cx);
}

/// Tells the strip the pointer came onto (or left) the tab at `place`, as that tab's own handler would.
fn hover(strip: &Entity<Strip>, place: usize, is_over: bool, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        strip.update(cx, |strip, cx| {
            let id = strip.tabs().tabs()[place].id;
            let bounds = strip.tab_bounds(id).expect("the tab was drawn");
            strip.hover_tab((id, is_over), ("m".into(), None), bounds, window, cx);
        });
    });
    draw(cx);
}

#[gpui::test]
fn strip_a_tab_left_after_the_next_was_entered_does_not_take_the_tooltip_away(
    cx: &mut TestAppContext,
) {
    use super::metrics::TOOLTIP_DELAY;
    let grace = std::time::Duration::from_millis(400);
    let (cx, strip) = open(cx, &["m1", "m2"]);
    hover(&strip, 1, true, cx);
    wait(TOOLTIP_DELAY, cx);
    assert!(tip(cx).is_some());
    // The pointer moves on, and the tab it came onto hears of it before the tab it left.
    hover(&strip, 0, true, cx);
    hover(&strip, 1, false, cx);
    wait(grace, cx);
    assert!(tip(cx).is_some(), "the entered tab keeps its tooltip");
    hover(&strip, 0, false, cx);
    wait(grace, cx);
    assert!(tip(cx).is_none(), "its own leave takes it away");
}

#[gpui::test]
fn strip_a_frame_that_draws_the_same_picture_builds_no_path(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let built = |cx: &mut VisualTestContext| cx.update(|_, cx| strip.read(cx).paths_built());
    let at_rest = built(cx);
    assert!(at_rest > 0, "the first frame built the shape");
    draw(cx);
    draw(cx);
    assert_eq!(
        built(cx),
        at_rest,
        "the same shape is submitted as prepared"
    );
    cx.simulate_keystrokes("cmd-2");
    draw(cx);
    let after_switch = built(cx);
    assert_eq!(
        after_switch,
        at_rest + 1,
        "another tab shown: the shape (a tab's mark is an icon of the set, cached by the window)"
    );
    draw(cx);
    assert_eq!(built(cx), after_switch);
}

#[gpui::test]
fn strip_with_no_tab_draws_the_panel_s_top_alone_and_has_nothing_to_press_or_close(
    cx: &mut TestAppContext,
) {
    let (cx, strip) = open_on(cx, Tabs::new());
    let told = changes(&strip, cx);
    assert!(order(&strip, cx).is_empty());
    assert_eq!(
        corner(&strip, cx),
        1.0,
        "both corners of the panel are round"
    );
    let built = cx.update(|_, cx| strip.read(cx).paths_built());
    assert_eq!(built, 1, "the panel's top, and no icon");
    for keys in ["cmd-1", "cmd-5", "cmd-9", "cmd-w"] {
        cx.simulate_keystrokes(keys);
        draw(cx);
    }
    // A drag that began in the page passes over the strip and ends there, then the pointer rests where
    // a tab would be. (A press on the bare strip itself moves the window, as on any title bar.)
    let in_the_page = point(px(60.), px(200.));
    let where_a_tab_would_be = point(px(60.), px(18.));
    cx.simulate_mouse_down(in_the_page, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(where_a_tab_would_be, MouseButton::Left, Modifiers::none());
    let farther = where_a_tab_would_be + point(px(200.), px(0.));
    cx.simulate_mouse_move(farther, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(farther, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(where_a_tab_would_be, None, Modifiers::none());
    draw(cx);
    wait(super::metrics::TOOLTIP_DELAY * 2, cx);
    assert!(tip(cx).is_none());
    assert!(order(&strip, cx).is_empty());
    assert_eq!(told.get(), 0, "nothing changed, the desk is told nothing");
    assert_eq!(cx.update(|_, cx| strip.read(cx).paths_built()), built);
}

#[gpui::test]
fn strip_the_first_lavoro_shown_gets_the_first_tab_and_the_last_one_gone_leaves_none(
    cx: &mut TestAppContext,
) {
    let (cx, strip) = open_on(cx, Tabs::new());
    let told = changes(&strip, cx);
    cx.update(|_, cx| strip.update(cx, |strip, cx| strip.show("m1", cx)));
    draw(cx);
    assert_eq!(order(&strip, cx).len(), 1);
    assert_eq!(active_place(&strip, cx), 0);
    assert_eq!(
        corner(&strip, cx),
        0.0,
        "the tab is flush: the corner is square"
    );
    assert_eq!(told.get(), 1);
    // The lavoro is removed: its tab goes although it is the last, and nothing is shown.
    cx.update(|_, cx| strip.update(cx, |strip, cx| strip.retain_tasks(|_| false, cx)));
    draw(cx);
    assert!(order(&strip, cx).is_empty());
    assert_eq!(corner(&strip, cx), 1.0);
    assert_eq!(told.get(), 2);
    let shown = cx.update(|_, cx| strip.read(cx).tabs().active_id());
    assert_eq!(shown, None);
}

/// How far the strip is scrolled, and how far it can be.
fn scrolled(strip: &Entity<Strip>, cx: &mut VisualTestContext) -> (f32, f32) {
    cx.update(|_, cx| (strip.read(cx).scroll, strip.read(cx).furthest))
}

/// Turns the wheel over the strip by `delta` px along it.
fn wheel(strip: &Entity<Strip>, delta: f32, cx: &mut VisualTestContext) {
    // A point of the strip that stays over the tabs however far they are scrolled.
    let over = cx.update(|_, cx| strip.read(cx).origin) + point(px(200.), px(18.));
    cx.simulate_event(gpui::ScrollEvent {
        position: over,
        delta: gpui::ScrollDelta::Pixels(point(px(delta), px(0.))),
        ..Default::default()
    });
    draw(cx);
}

#[gpui::test]
fn strip_a_wheel_over_tabs_that_do_not_fit_brings_the_others_into_sight(cx: &mut TestAppContext) {
    let (cx, strip) = overflowing(cx);
    let (at_rest, furthest) = scrolled(&strip, cx);
    assert_eq!(
        at_rest, 0.,
        "the tab shown is the first: nothing is scrolled"
    );
    assert!(furthest > 0., "sixty tabs do not fit");
    let shown = cx.update(|_, cx| strip.read(cx).tabs().active_id());
    wheel(&strip, -300., cx);
    assert_eq!(
        scrolled(&strip, cx).0,
        300.,
        "the tabs moved by what the wheel turned"
    );
    draw(cx);
    assert_eq!(
        scrolled(&strip, cx).0,
        300.,
        "and stay there: the tab shown is not pulled back"
    );
    assert_eq!(
        cx.update(|_, cx| strip.read(cx).tabs().active_id()),
        shown,
        "no tab was chosen"
    );
    wheel(&strip, -100_000., cx);
    assert_eq!(scrolled(&strip, cx).0, furthest, "never past the last tab");
    wheel(&strip, 100_000., cx);
    assert_eq!(scrolled(&strip, cx).0, 0., "nor before the first");
    cx.simulate_keystrokes("cmd-9");
    draw(cx);
    assert_eq!(
        scrolled(&strip, cx).0,
        furthest,
        "a tab chosen out of sight comes into sight"
    );
}

#[gpui::test]
fn strip_a_wheel_moves_nothing_when_every_tab_fits(cx: &mut TestAppContext) {
    let (cx, strip) = open(cx, &["m1", "m2"]);
    let before = all_bounds(&strip, cx);
    wheel(&strip, -300., cx);
    assert_eq!(scrolled(&strip, cx), (0., 0.));
    assert_eq!(all_bounds(&strip, cx), before);
}

#[gpui::test]
fn strip_the_edges_fade_only_where_tabs_are_cut(cx: &mut TestAppContext) {
    let (cx, strip) = overflowing(cx);
    let is_cut = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let strip = strip.read(cx);
            (strip.scroll > 0., strip.scroll < strip.furthest)
        })
    };
    assert_eq!(
        is_cut(cx),
        (false, true),
        "at the start only the end is cut"
    );
    wheel(&strip, -300., cx);
    assert_eq!(is_cut(cx), (true, true));
    wheel(&strip, -100_000., cx);
    assert_eq!(
        is_cut(cx),
        (true, false),
        "at the end only the start is cut"
    );
}

#[gpui::test]
fn strip_no_tab_touches_the_strip_s_top_and_every_tab_stands_on_the_panel(cx: &mut TestAppContext) {
    use super::metrics::{STRIP_HEIGHT, TAB_HEIGHT, TOP_INSET};
    let (cx, strip) = open(cx, &["m1", "m2", "m3"]);
    let top = cx.update(|_, cx| strip.read(cx).origin.y);
    for tab in all_bounds(&strip, cx) {
        assert!(tab.top() > top, "a tab starts at the window's top edge");
        assert_eq!(tab.top(), top + px(TOP_INSET));
        assert_eq!(tab.size.height, px(TAB_HEIGHT));
        assert_eq!(
            tab.bottom(),
            top + px(STRIP_HEIGHT),
            "it stands on the panel"
        );
    }
    // The traffic lights are centred on the line the tabs are centred on.
    let lights = crate::theme::TRAFFIC_LIGHTS_Y + crate::theme::TRAFFIC_LIGHT_SIZE / 2.;
    assert_eq!(lights, px(TOP_INSET + TAB_HEIGHT / 2.));
}
