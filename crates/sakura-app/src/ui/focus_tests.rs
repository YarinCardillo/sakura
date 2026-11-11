//! The focus rule in a test window: an outline appears only for keyboard navigation. The frame's painted
//! quads are read back: after a click on a field, a button, a row or a segment none has a border in the ring
//! colour, typing changes no border of the field, and after Tab one does (so the check can see a ring).

use super::desk::Desk;
use super::interaction_tests::{centre, click, draw, motion_ended, open_with, wait_for};
use crate::theme::Theme;
use gpui::{px, Bounds, Entity, Hsla, Modifiers, Pixels, Quad, TestAppContext, VisualTestContext};

/// Whether two colours are the same hue, saturation and lightness, whatever their opacity.
fn is_same_tone(a: Hsla, b: Hsla) -> bool {
    let close = |x: f32, y: f32| (x - y).abs() < 0.01;
    close(a.h, b.h) && close(a.s, b.s) && close(a.l, b.l)
}

fn has_border(quad: &Quad) -> bool {
    let widths = &quad.border_widths;
    let any = [widths.top, widths.right, widths.bottom, widths.left];
    any.iter().any(|width| width.0 > 0.0) && quad.border_color.a > 0.0
}

/// The quads of the last frame drawn with a border in the ring colour.
pub(super) fn rings(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| {
        let ring = Theme::palette(cx).ring;
        let quads = window.painted_quads();
        let ringed = quads
            .iter()
            .filter(|quad| has_border(quad) && is_same_tone(quad.border_color, ring));
        ringed.count()
    })
}

/// The border colours of the quads drawn inside `area` (grown by a few pixels for a ring around it).
fn borders_in(cx: &mut VisualTestContext, area: Bounds<Pixels>) -> Vec<Hsla> {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let around = area.dilate(px(4.));
        let inside = |quad: &Quad| {
            let origin = quad.bounds.origin;
            let (x, y) = (px(origin.x.0 / scale), px(origin.y.0 / scale));
            around.contains(&gpui::point(x, y))
        };
        let quads = window.painted_quads();
        let bordered = quads.iter().filter(|quad| has_border(quad) && inside(quad));
        bordered.map(|quad| quad.border_color).collect()
    })
}

/// The desk on the page of a lavoro whose conversation is drawn.
fn open_on_the_page(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    click(cx, "row-sample-accent");
    wait_for(cx, "the conversation", |cx| {
        cx.debug_bounds("thread-composer").is_some()
    });
    (cx, desk)
}

#[gpui::test]
fn a_click_in_the_new_task_field_draws_no_ring_and_typing_changes_no_outline(
    cx: &mut TestAppContext,
) {
    let (cx, _) = open_on_the_page(cx);
    click(cx, "new-task");
    wait_for(cx, "the field", |cx| {
        cx.debug_bounds("new-task-field").is_some()
    });
    let field = cx.debug_bounds("new-task-field").expect("the field");
    let at_rest = borders_in(cx, field);
    assert_eq!(rings(cx), 0);
    click(cx, "new-task-field");
    cx.update(|window, cx| assert!(window.focused(cx).is_some(), "the click focused the field"));
    assert_eq!(rings(cx), 0, "a clicked field has no ring");
    assert_eq!(borders_in(cx, field), at_rest, "nor a different edge");
    cx.simulate_input("personal");
    draw(cx);
    assert_eq!(rings(cx), 0, "typing draws no ring");
    assert_eq!(borders_in(cx, field), at_rest, "and changes no edge");
}

#[gpui::test]
fn the_composer_waits_for_a_click_and_a_click_or_typing_in_it_draws_no_ring(
    cx: &mut TestAppContext,
) {
    let (cx, _) = open_on_the_page(cx);
    cx.update(|window, cx| assert!(window.focused(cx).is_none(), "nothing takes the focus"));
    let composer = cx.debug_bounds("thread-composer").expect("the composer");
    let at_rest = borders_in(cx, composer);
    click(cx, "thread-composer");
    cx.update(|window, cx| assert!(window.focused(cx).is_some(), "the click focused it"));
    assert_eq!(rings(cx), 0, "a clicked composer has no ring");
    cx.simulate_input("wave");
    draw(cx);
    assert_eq!(rings(cx), 0, "typing draws no ring");
    assert_eq!(borders_in(cx, composer), at_rest, "and changes no edge");
}

#[gpui::test]
fn a_click_on_a_button_a_row_or_a_segment_draws_no_ring(cx: &mut TestAppContext) {
    let (cx, _) = open_on_the_page(cx);
    let pressed = [
        "row-sample-folders",
        "toggle-changes",
        "toggle-changes",
        "changes-header-0",
        "pane-view-checks",
        "verify-test",
        "run-rules",
        "pane-view-changes",
        "task-menu",
    ];
    for selector in pressed {
        // A pane that slides (real time) is let come to rest first: the press lands on what is shown,
        // not on a control still clipped by the slide.
        motion_ended(cx);
        wait_for(cx, selector, |cx| cx.debug_bounds(selector).is_some());
        click(cx, selector);
        cx.run_until_parked();
        draw(cx);
        assert_eq!(rings(cx), 0, "after a click on {selector}");
    }
}

#[gpui::test]
fn a_key_pressed_after_a_click_on_a_button_draws_no_ring(cx: &mut TestAppContext) {
    let (cx, _) = open_on_the_page(cx);
    click(cx, "pane-view-checks");
    wait_for(cx, "Esegui", |cx| cx.debug_bounds("run-rules").is_some());
    click(cx, "run-rules");
    cx.simulate_keystrokes("x");
    draw(cx);
    assert_eq!(rings(cx), 0, "a key that moves no focus shows no ring");
}

#[gpui::test]
fn a_press_that_is_held_draws_no_ring_either(cx: &mut TestAppContext) {
    let (cx, _) = open_on_the_page(cx);
    let at = centre(cx, "toggle-changes");
    cx.simulate_mouse_down(at, gpui::MouseButton::Left, Modifiers::none());
    draw(cx);
    assert_eq!(rings(cx), 0);
    cx.simulate_mouse_up(at, gpui::MouseButton::Left, Modifiers::none());
}

#[gpui::test]
fn tab_draws_a_ring_so_keyboard_focus_is_visible(cx: &mut TestAppContext) {
    let (cx, _) = open_on_the_page(cx);
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| assert!(window.focused(cx).is_some(), "tab reached a control"));
    assert!(rings(cx) > 0, "the control Tab reached shows its ring");
}
