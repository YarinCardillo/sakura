//! The mark's motion and the About window, driven in test windows on GPUI's test platform, whose
//! clock moves only when a test moves it: the reveal plays once and then asks for nothing, the
//! connecting sign moves only between its start and its stop, reduced motion keeps both still.

use super::about::{self, AboutView};
use super::{BrandMark, Ink, MarkSize, Phase, Rest};
use crate::l10n::{self, tr, Language};
use crate::theme::{Appearance, Theme};
use crate::theme_brand::{REPOSITORY, REVEAL_STEP, REVEAL_STEPS};
use gpui::{AppContext as _, Entity, Keystroke, Modifiers, TestAppContext, VisualTestContext};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Longer than anything of the brand lasts: what happens after it is what happens at rest.
const LONG_AFTER: Duration = Duration::from_secs(600);

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Moves the clock by `time`, lets what was due run and draws the frame.
fn pass(cx: &mut VisualTestContext, time: Duration) {
    cx.executor().advance_clock(time);
    cx.run_until_parked();
    draw(cx);
}

fn phase(cx: &mut VisualTestContext, mark: &Entity<BrandMark>) -> Phase {
    mark.read_with(cx, |mark, cx| {
        mark.phase(cx.background_executor().now(), cx.reduce_motion())
    })
}

fn is_ticking(cx: &mut VisualTestContext, mark: &Entity<BrandMark>) -> bool {
    mark.read_with(cx, |mark, _| mark.is_ticking())
}

/// Counts every time `mark` asks to be drawn again.
fn count_requests(cx: &mut VisualTestContext, mark: &Entity<BrandMark>) -> Rc<Cell<usize>> {
    let requests = Rc::new(Cell::new(0));
    let counted = requests.clone();
    cx.update(|_, cx| {
        cx.observe(mark, move |_, _| counted.set(counted.get() + 1))
            .detach();
    });
    requests
}

/// What the mark asks of the window over ten minutes at rest: the times it asks to be drawn again,
/// and the frames asked of the window.
fn asked_at_rest(cx: &mut VisualTestContext, mark: &Entity<BrandMark>) -> (usize, usize) {
    // Frames requested while the mark still moved are delivered first.
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    draw(cx);
    let requests = count_requests(cx, mark);
    let drawn = mark.read_with(cx, |mark, _| mark.renders());
    cx.executor().advance_clock(LONG_AFTER);
    cx.run_until_parked();
    let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert_eq!(mark.read_with(cx, |mark, _| mark.renders()), drawn);
    (requests.get(), frames)
}

#[gpui::test]
fn the_reveal_plays_once_from_the_first_drawing_and_then_asks_for_no_frame(
    cx: &mut TestAppContext,
) {
    let (mark, cx) =
        cx.add_window_view(|_, _| BrandMark::revealing(MarkSize::MEDIUM, Ink::Fuchsia));
    draw(cx);
    assert_eq!(phase(cx, &mark), Phase::On(1));
    assert!(is_ticking(cx, &mark));
    let while_moving = count_requests(cx, &mark);
    for steps in 2..REVEAL_STEPS {
        pass(cx, REVEAL_STEP);
        assert_eq!(phase(cx, &mark), Phase::On(steps));
    }
    pass(cx, REVEAL_STEP);
    assert_eq!(phase(cx, &mark), Phase::Whole, "the last step is the mark");
    assert!(!is_ticking(cx, &mark), "the timer ended with the reveal");
    let steps_drawn = usize::from(REVEAL_STEPS) - 1;
    assert_eq!(while_moving.get(), steps_drawn, "one drawing a step");
    assert_eq!(asked_at_rest(cx, &mark), (0, 0), "nothing is asked at rest");
    assert_eq!(phase(cx, &mark), Phase::Whole, "it does not play again");
}

#[gpui::test]
fn a_mark_made_after_the_reveal_was_played_is_whole_from_its_first_drawing(
    cx: &mut TestAppContext,
) {
    let first = || BrandMark::revealing(MarkSize::MEDIUM, Ink::Fuchsia);
    let (mark, window) = cx.add_window_view(|_, _| first());
    draw(window);
    assert_eq!(phase(window, &mark), Phase::On(1));
    // The window is closed and opened again in the same launch.
    let (again, window) = cx.add_window_view(|_, _| first());
    draw(window);
    assert_eq!(phase(window, &again), Phase::Whole);
    assert!(!is_ticking(window, &again));
}

#[gpui::test]
fn with_reduced_motion_the_mark_appears_whole_and_nothing_ever_ticks(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (mark, cx) =
        cx.add_window_view(|_, _| BrandMark::revealing(MarkSize::MEDIUM, Ink::Fuchsia));
    draw(cx);
    assert_eq!(phase(cx, &mark), Phase::Whole);
    assert!(!is_ticking(cx, &mark));
    assert_eq!(asked_at_rest(cx, &mark), (0, 0));
}

#[gpui::test]
fn the_connecting_sign_moves_only_between_its_start_and_its_stop(cx: &mut TestAppContext) {
    let (sign, cx) = cx.add_window_view(|_, _| BrandMark::connecting(Rest::Nothing));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Nothing);
    assert_eq!(asked_at_rest(cx, &sign), (0, 0), "it waits for its start");

    sign.update(cx, |sign, cx| sign.start(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::On(1));
    assert!(is_ticking(cx, &sign));
    let sweep = REVEAL_STEP * u32::from(REVEAL_STEPS);
    pass(cx, sweep - REVEAL_STEP);
    assert_eq!(phase(cx, &sign), Phase::On(REVEAL_STEPS), "whole");
    pass(cx, REVEAL_STEP);
    assert_eq!(
        phase(cx, &sign),
        Phase::Off(1),
        "off again, in the same order"
    );
    pass(cx, sweep);
    assert_eq!(phase(cx, &sign), Phase::On(1), "and on again");
    // A second start while it loops changes nothing.
    pass(cx, REVEAL_STEP);
    sign.update(cx, |sign, cx| sign.start(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::On(2));

    sign.update(cx, |sign, cx| sign.stop(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Nothing);
    assert!(!is_ticking(cx, &sign));
    assert_eq!(asked_at_rest(cx, &sign), (0, 0), "nothing is asked at rest");
}

#[gpui::test]
fn a_connecting_sign_that_rests_whole_is_the_whole_mark_before_and_after(cx: &mut TestAppContext) {
    let (sign, cx) = cx.add_window_view(|_, _| BrandMark::connecting(Rest::Whole));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Whole);
    sign.update(cx, |sign, cx| sign.start(cx));
    pass(cx, REVEAL_STEP);
    assert_eq!(phase(cx, &sign), Phase::On(2));
    sign.update(cx, |sign, cx| sign.stop(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Whole);
}

#[gpui::test]
fn with_reduced_motion_the_connecting_sign_is_the_whole_mark_still_while_it_waits(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (sign, cx) = cx.add_window_view(|_, _| BrandMark::connecting(Rest::Nothing));
    sign.update(cx, |sign, cx| sign.start(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Whole);
    assert!(!is_ticking(cx, &sign));
    assert_eq!(asked_at_rest(cx, &sign), (0, 0));
    sign.update(cx, |sign, cx| sign.stop(cx));
    draw(cx);
    assert_eq!(phase(cx, &sign), Phase::Nothing);
}

/// The kit and the theme, as the app sets them before any window.
fn prepare(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
        super::install(cx);
    });
}

#[gpui::test]
fn the_about_action_opens_one_window_and_a_second_press_brings_it_forward(cx: &mut TestAppContext) {
    prepare(cx);
    cx.update(|cx| cx.dispatch_action(&about::About));
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1);
    cx.update(|cx| cx.dispatch_action(&about::About));
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1, "one About window");
    let window = cx.windows()[0];
    let mut about_cx = VisualTestContext::from_window(window, cx);
    about_cx.update(|window, _| window.activate_window());
    draw(&mut about_cx);
    about_cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 0, "Escape closes it");
    cx.update(about::open);
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 1, "it opens again after it was closed");
    // The system's close key: Cmd+W on the Mac, Ctrl+W elsewhere.
    let window = cx.windows()[0];
    let close = Keystroke {
        modifiers: cx.update(|cx| Modifiers::secondary_key(cx)),
        key: "w".into(),
        key_char: None,
    };
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .expect("the About window");
    cx.dispatch_keystroke(window, close);
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 0, "the close key closes it");
}

#[gpui::test]
fn the_about_column_says_the_version_built_and_its_link_opens_the_repository(
    cx: &mut TestAppContext,
) {
    l10n::set_for_test(Language::English);
    prepare(cx);
    let (_, cx) = cx.add_window_view(|_, cx| AboutView::new(cx));
    draw(cx);
    assert!(cx.debug_bounds("about-version").is_some());
    let link = cx.debug_bounds("about-repository").expect("the link");
    cx.simulate_click(link.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(cx.opened_url().as_deref(), Some(REPOSITORY));
    assert_eq!(tr(about::TITLE), "About Sakura");
    l10n::set_for_test(Language::Italian);
    assert_eq!(tr(about::TITLE), "Informazioni su Sakura");
}
