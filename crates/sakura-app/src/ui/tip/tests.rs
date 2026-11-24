//! The tooltip controller driven like a person would, on GPUI's test platform with its clock: the cold
//! delay, the warm switch, one at a time, a press hiding it, and where it stands.

#![cfg(not(feature = "boxes"))]

use super::{
    bind, tip, Placement, Side, TipLayer, Tips, TipsHandle, Words, COLD_DELAY, GAP, WARM_WINDOW,
};
use crate::theme::{Appearance, Palette, Theme};
use gpui::{
    div, point, px, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window,
};
use std::time::Duration;

/// Three controls along the foot of the window, one in the top bar.
struct Host {
    layer: Entity<TipLayer>,
}

fn target(id: &'static str, left: f32, top: f32, cx: &gpui::App) -> gpui::AnyElement {
    let body = div()
        .debug_selector(move || id.into())
        .size(px(28.))
        .id(id)
        .into_any_element();
    let placed = div().absolute().left(px(left)).top(px(top)).child(tip(
        body,
        id,
        Words::with_keys(id, "cmd-1"),
        cx,
    ));
    placed.into_any_element()
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let right = f32::from(viewport.width - px(28.));
        let bottom = f32::from(viewport.height - px(28.));
        div()
            .size_full()
            .relative()
            .child(target("one", 10., 700., cx))
            .child(target("two", 50., 700., cx))
            .child(target("three", 90., 700., cx))
            .child(target("top", 600., 4., cx))
            .child(target("right edge", right, bottom, cx))
            .child(self.layer.clone())
    }
}

#[gpui::test]
fn a_tooltip_above_the_rightmost_status_control_stays_inside_the_window(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    hover(cx, "right edge");
    wait(cx, COLD_DELAY);
    let viewport = cx.update(|window, _| window.viewport_size());
    let card = cx.debug_bounds("tip-card").unwrap();
    let target = cx.debug_bounds("right edge").unwrap();
    assert!(card.left() >= px(0.) && card.right() <= viewport.width);
    assert!(card.top() >= px(0.) && card.bottom() <= target.top());
}

fn open(cx: &mut TestAppContext) -> (Entity<Tips>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(false);
    });
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let tips = cx.update(|cx| {
        let tips = cx.new(|_| Tips::new());
        cx.set_global(TipsHandle(tips.clone()));
        tips
    });
    let layer = tips.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let layer = cx.new(|cx| TipLayer::new(layer, cx));
        let host = cx.new(|_| Host { layer });
        gpui_component::Root::new(host, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    draw(cx);
    (tips, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn centre(cx: &mut VisualTestContext, id: &'static str) -> gpui::Point<gpui::Pixels> {
    cx.debug_bounds(id).expect("a target").center()
}

fn hover(cx: &mut VisualTestContext, id: &'static str) {
    let at = centre(cx, id);
    cx.simulate_mouse_move(at, None, Modifiers::none());
    draw(cx);
}

fn leave(cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(point(px(400.), px(400.)), None, Modifiers::none());
    draw(cx);
}

/// A press at `at` that comes with no move before it, as a click driven by a script does: the control the
/// pointer last rested on never hears that the pointer left it.
fn press_without_moving(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    cx.simulate_event(gpui::MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
}

fn wait(cx: &mut VisualTestContext, time: Duration) {
    cx.executor().advance_clock(time);
    draw(cx);
}

fn shown(tips: &Entity<Tips>, cx: &mut VisualTestContext) -> Option<String> {
    tips.read_with(cx, |tips, _| tips.shown().map(|s| s.key.to_string()))
}

fn cards(cx: &mut VisualTestContext) -> usize {
    usize::from(cx.debug_bounds("tip-card").is_some())
}

#[gpui::test]
fn the_first_tooltip_comes_after_four_hundred_milliseconds_of_hover_and_not_before(
    cx: &mut TestAppContext,
) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY - Duration::from_millis(1));
    assert_eq!(shown(&tips, cx), None, "399 ms is not enough");
    wait(cx, Duration::from_millis(1));
    assert_eq!(shown(&tips, cx).as_deref(), Some("one"));
    assert!(cx.debug_bounds("tip-card").is_some(), "and it is drawn");
    assert!(
        tips.read_with(cx, |tips, _| tips.shown().is_some_and(|s| s.is_animated)),
        "the first after a rest fades in"
    );
}

#[gpui::test]
fn leaving_before_the_delay_shows_nothing(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, Duration::from_millis(300));
    leave(cx);
    wait(cx, Duration::from_secs(2));
    assert_eq!(shown(&tips, cx), None);
}

#[gpui::test]
fn the_next_tooltip_comes_at_once_and_without_a_fade_while_the_system_is_warm(
    cx: &mut TestAppContext,
) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    assert_eq!(shown(&tips, cx).as_deref(), Some("one"));
    hover(cx, "two");
    assert_eq!(shown(&tips, cx).as_deref(), Some("two"), "at once");
    assert!(
        tips.read_with(cx, |tips, _| tips.shown().is_some_and(|s| !s.is_animated)),
        "with no enter animation"
    );
    assert_eq!(cards(cx), 1);
    leave(cx);
    assert_eq!(shown(&tips, cx), None, "it goes with the pointer");
    wait(cx, WARM_WINDOW - Duration::from_millis(100));
    hover(cx, "three");
    assert_eq!(shown(&tips, cx).as_deref(), Some("three"), "still warm");
    leave(cx);
    wait(cx, WARM_WINDOW + Duration::from_millis(1));
    hover(cx, "one");
    assert_eq!(shown(&tips, cx), None, "cold again after the warm window");
    wait(cx, COLD_DELAY);
    assert_eq!(shown(&tips, cx).as_deref(), Some("one"));
}

#[gpui::test]
fn only_one_tooltip_is_up_at_a_time(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    for id in ["two", "three", "one", "top"] {
        hover(cx, id);
        assert_eq!(cards(cx), 1, "{id}");
        assert_eq!(shown(&tips, cx).as_deref(), Some(id));
    }
}

#[gpui::test]
fn a_press_hides_the_tooltip_and_it_comes_back_only_after_the_pointer_left_and_returned(
    cx: &mut TestAppContext,
) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    let at = centre(cx, "one");
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(shown(&tips, cx), None, "gone with the press");
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    wait(cx, Duration::from_secs(2));
    hover(cx, "one");
    wait(cx, Duration::from_secs(2));
    assert_eq!(shown(&tips, cx), None, "not while the pointer stays");
    leave(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    assert_eq!(
        shown(&tips, cx).as_deref(),
        Some("one"),
        "after it returned"
    );
}

#[gpui::test]
fn a_press_anywhere_in_the_window_hides_the_tooltip_that_is_up(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    assert_eq!(shown(&tips, cx).as_deref(), Some("one"));
    // A control that is no tooltip's target (the tab bar's plus) was pressed: the tooltip does not stay drawn
    // over the page.
    press_without_moving(cx, point(px(400.), px(400.)));
    draw(cx);
    assert_eq!(shown(&tips, cx), None, "gone with a press anywhere");
    assert_eq!(cards(cx), 0, "and no longer drawn");
}

#[gpui::test]
fn a_press_anywhere_asked_of_the_controller_hides_what_is_up_or_waiting(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    tips.update(cx, |tips, cx| tips.press_anywhere(cx));
    assert_eq!(shown(&tips, cx), None);
    let again = centre(cx, "one");
    cx.simulate_mouse_move(again, None, Modifiers::none());
    wait(cx, Duration::from_secs(2));
    assert_eq!(
        shown(&tips, cx),
        None,
        "it stays quiet until the pointer has left and returned"
    );
}

#[gpui::test]
fn a_control_carried_under_the_pointer_earns_no_tooltip_until_the_button_is_up(
    cx: &mut TestAppContext,
) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    let at = centre(cx, "one");
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    draw(cx);
    // Held: the pointer leaves the control and meets it again, as it does when the control is dragged.
    leave(cx);
    hover(cx, "two");
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    assert_eq!(
        shown(&tips, cx),
        None,
        "no tooltip while the button is down"
    );
    let up_at = centre(cx, "one");
    cx.simulate_mouse_up(up_at, MouseButton::Left, Modifiers::none());
    draw(cx);
    leave(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    assert_eq!(
        shown(&tips, cx).as_deref(),
        Some("one"),
        "after the button went up"
    );
}

#[gpui::test]
fn a_press_while_a_tooltip_waits_cancels_it(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    press_without_moving(cx, point(px(400.), px(400.)));
    wait(cx, COLD_DELAY);
    assert_eq!(shown(&tips, cx), None, "it never came");
}

#[gpui::test]
fn a_tooltip_stands_above_a_control_at_the_foot_and_below_one_at_the_top_on_its_left_edge(
    cx: &mut TestAppContext,
) {
    let (tips, cx) = open(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    for id in ["one", "two", "three"] {
        hover(cx, id);
        wait(cx, COLD_DELAY);
        let control = cx.debug_bounds(id).expect("the control");
        let card = cx.debug_bounds("tip-card").expect("the card");
        assert_eq!(card.left(), control.left(), "{id}: left edges align");
        let gap = control.top() - card.bottom();
        assert!(
            (gap - GAP).abs() < px(1.),
            "{id}: above, the same gap: {gap:?}"
        );
        let placement = tips.read_with(cx, |tips, _| tips.shown().map(|s| s.placement));
        assert_eq!(placement.map(|p| p.side), Some(Side::Above));
        assert_eq!(placement, Some(Placement::of(control, viewport)));
    }
    hover(cx, "top");
    wait(cx, Duration::from_millis(1));
    let control = cx.debug_bounds("top").expect("the control");
    let card = cx.debug_bounds("tip-card").expect("the card");
    let gap = card.top() - control.bottom();
    assert!(
        (gap - GAP).abs() < px(1.),
        "below a control at the top: {gap:?}"
    );
}

#[gpui::test]
fn switching_between_neighbours_moves_nothing_but_the_words(cx: &mut TestAppContext) {
    let (tips, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    let first = cx.debug_bounds("tip-card").expect("the card");
    hover(cx, "two");
    let second = cx.debug_bounds("tip-card").expect("the card");
    assert_eq!(
        first.bottom(),
        second.bottom(),
        "the same height above the row"
    );
    assert_eq!(
        first.size, second.size,
        "the same size for the same words' length"
    );
    assert_eq!(
        second.left() - first.left(),
        px(40.),
        "one control to the right"
    );
    let _ = tips;
}

#[test]
fn a_key_is_written_as_the_mac_s_menus_write_it() {
    assert_eq!(
        Words::with_keys("Tab", "cmd-1").shortcut.as_deref(),
        Some("⌘1")
    );
    assert_eq!(
        Words::with_keys("Hide", "alt-cmd-b").shortcut.as_deref(),
        Some("⌥⌘B")
    );
    assert_eq!(Words::plain("Close").shortcut, None);
}

#[gpui::test]
fn a_control_with_no_controller_is_left_as_it_is(cx: &mut TestAppContext) {
    let built = cx.update(|cx| {
        let element = div().id("lonely");
        bind(element, "lonely", Words::plain("x"), cx)
    });
    drop(built);
}

#[gpui::test]
fn a_tooltip_wears_the_window_s_raised_surface_and_a_thin_border_not_inverted_colours(
    cx: &mut TestAppContext,
) {
    let (_, cx) = open(cx);
    hover(cx, "one");
    wait(cx, COLD_DELAY);
    wait(cx, crate::theme::TIP_FADE);
    let palette = Palette::dark();
    let surface = format!("{:?}", gpui::Background::from(palette.overlay_surface));
    let inverted = format!("{:?}", gpui::Background::from(palette.text));
    cx.update(|window, cx| {
        window.set_scale_factor(1.);
        window.draw(cx).clear(cx);
    });
    let card = cx.debug_bounds("tip-card").expect("a card").scale(1.);
    let quads: Vec<(String, String)> = cx.update(|window, _| {
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds == card)
            .map(|quad| {
                (
                    format!("{:?}", quad.background),
                    format!("{:?}", quad.border_color),
                )
            })
            .collect()
    });
    assert!(
        quads.iter().any(|(background, _)| *background == surface),
        "the card is on the raised surface: {quads:?}"
    );
    assert!(
        quads.iter().all(|(background, _)| *background != inverted),
        "the card is not drawn in the text colour"
    );
    let border = format!("{:?}", palette.overlay_edge);
    assert!(
        quads.iter().any(|(_, colour)| *colour == border),
        "the card has the theme's border: {quads:?}"
    );
}

#[test]
fn reaudit_sidebar_label_placement_requires_truncation_and_clears_the_next_row() {
    let label = gpui::Bounds::new(point(px(40.), px(365.)), gpui::size(px(160.), px(26.)));
    let place = |width| super::truncated_sidebar_placement(label, px(width));
    assert_eq!(place(159.), None);
    assert_eq!(place(160.), None);
    let shown = place(161.).expect("the label is truncated");
    assert!(shown.point.y <= label.top() - GAP);
    assert!(shown.point.x < crate::theme::SIDEBAR_WIDTH);
}

#[test]
fn reaudit3_path_tooltip_is_anchored_outside_the_sheet() {
    let field = gpui::Bounds::new(point(px(620.), px(460.)), gpui::size(px(220.), px(28.)));
    let sheet_top = px(116.);
    let placement = super::path_placement(field, sheet_top);
    assert_eq!(placement.point, point(field.right(), sheet_top - GAP));
}
