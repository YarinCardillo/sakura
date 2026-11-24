//! The shared kit pieces on GPUI's test platform: what a pick, a press and the clock do, and what the
//! marks draw.

use super::*;
use crate::theme::kit::{
    TOAST_ERROR as ERROR, TOAST_WITHOUT_UNDO as WITHOUT_UNDO, TOAST_WITH_UNDO as WITH_UNDO,
};
use crate::theme::segments::TRACK_PADDING;
use crate::theme::{Appearance, Palette, Theme};
use gpui::{
    div, px, AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, VisualTestContext, Window,
};
use gpui_component::Root;
use gpui_kit_assets::IconName;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

type Build = Box<dyn Fn(&mut Window, &mut Context<Host>) -> gpui::AnyElement>;

/// What a test window draws: whatever `build` makes of the app's state.
pub(super) struct Host {
    build: Build,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

/// A window under the kit's root that draws what `build` says.
pub(super) fn open(
    cx: &mut TestAppContext,
    build: impl Fn(&mut Window, &mut Context<Host>) -> gpui::AnyElement + 'static,
) -> &mut VisualTestContext {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(true);
    });
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let host = cx.new(|_| Host {
            build: Box::new(build),
        });
        Root::new(host, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx
}

pub(super) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing named {selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

#[gpui::test]
fn dropping_the_undo_of_the_toast_on_screen_removes_only_its_button(cx: &mut TestAppContext) {
    let count = Rc::new(Cell::new(0));
    let cx = toast_panel(cx);
    let first = cx.update(|window, cx| {
        let undo: toast::Undo = Box::new({
            let count = count.clone();
            move |_, _| count.set(count.get() + 1)
        });
        Toast::show(window, cx, "Approved", Some(undo))
    });
    wait(cx, Duration::from_millis(200));
    cx.update(|window, cx| Toast::drop_undo(window, cx, first + 100));
    wait(cx, Duration::from_millis(100));
    assert!(
        cx.debug_bounds("toast-undo").is_some(),
        "another toast's id changes nothing"
    );
    cx.update(|window, cx| Toast::drop_undo(window, cx, first));
    wait(cx, Duration::from_millis(100));
    assert!(cx.debug_bounds("toast-undo").is_none(), "no Undo left");
    assert!(cx.debug_bounds("toast-text").is_some(), "the words stay");
    assert_eq!(count.get(), 0);
}

#[gpui::test]
fn the_pointer_on_a_toast_is_told(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), true);
    assert!(!cx.update(|window, cx| Toast::is_hovered(window, cx)));
    let at = cx.debug_bounds("toast").expect("the toast").center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    wait(cx, Duration::from_millis(100));
    assert!(cx.update(|window, cx| Toast::is_hovered(window, cx)));
}

/// Lets `time` pass in the steps the toast's clock takes, so every timer that falls in it runs.
fn wait(cx: &mut VisualTestContext, time: Duration) {
    let step = Duration::from_millis(50);
    let mut passed = Duration::ZERO;
    while passed < time {
        cx.executor().advance_clock(step);
        cx.run_until_parked();
        passed += step;
    }
}

/// How many toasts the window draws: none, or the one.
fn toasts(cx: &mut VisualTestContext) -> usize {
    usize::from(cx.debug_bounds("toast").is_some())
}

// Segments.

struct Picker {
    selected: usize,
    picked: Rc<RefCell<Vec<usize>>>,
}

fn picker_window(cx: &mut TestAppContext) -> (Rc<RefCell<Vec<usize>>>, &mut VisualTestContext) {
    let picked = Rc::new(RefCell::new(Vec::new()));
    let state: Rc<RefCell<Option<gpui::Entity<Picker>>>> = Rc::new(RefCell::new(None));
    let (kept, log) = (state.clone(), picked.clone());
    let cx = open(cx, move |_, cx| {
        let picker = kept
            .borrow_mut()
            .get_or_insert_with(|| {
                cx.new(|_| Picker {
                    selected: 0,
                    picked: log.clone(),
                })
            })
            .clone();
        let selected = picker.read(cx).selected;
        let options: Vec<(SharedString, bool)> = ["Details", "Changes", "Files"]
            .iter()
            .enumerate()
            .map(|(index, label)| ((*label).into(), index == selected))
            .collect();
        let host = cx.entity();
        segments("views", &options, move |index, _, cx| {
            picker.update(cx, |picker, _| {
                picker.selected = index;
                picker.picked.borrow_mut().push(index);
            });
            host.update(cx, |_, cx| cx.notify());
        })
        .into_any_element()
    });
    (picked, cx)
}

fn selected_fills(cx: &mut VisualTestContext) -> Vec<Pixels> {
    let fill: gpui::Background = Palette::dark().segment_selected.into();
    cx.update(|window, cx| {
        window.set_scale_factor(1.);
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == fill)
            .map(|quad| px(quad.bounds.origin.x.0))
            .collect()
    })
}

#[gpui::test]
fn segments_a_pick_calls_back_with_its_index_and_fills_that_segment(cx: &mut TestAppContext) {
    let (picked, cx) = picker_window(cx);
    let second = cx.debug_bounds("views-1").expect("the second segment");
    assert_eq!(
        selected_fills(cx).len(),
        1,
        "one segment is filled to begin with"
    );
    click(cx, "views-1");
    cx.run_until_parked();
    assert_eq!(*picked.borrow(), vec![1]);
    assert_eq!(selected_fills(cx), vec![second.origin.x]);
}

/// A three-word choice inside a 300 px column with a 12 px inset, in the form `full` says; the second
/// word is the chosen one until a press moves it.
fn form_window(cx: &mut TestAppContext, is_full: bool) -> &mut VisualTestContext {
    let chosen = Rc::new(Cell::new(1usize));
    open(cx, move |_, cx| {
        let options: Vec<(SharedString, bool)> = ["Slow", "Normal", "Fast"]
            .iter()
            .enumerate()
            .map(|(index, label)| ((*label).into(), index == chosen.get()))
            .collect();
        let (host, chosen) = (cx.entity(), chosen.clone());
        let track = segments("form", &options, move |index, _, cx| {
            chosen.set(index);
            host.update(cx, |_, cx| cx.notify());
        });
        let track = if is_full { track.full_width() } else { track };
        div().w(px(300.)).p(px(12.)).child(track).into_any_element()
    })
}

fn part_bounds(cx: &mut VisualTestContext) -> Vec<gpui::Bounds<Pixels>> {
    ["form-0", "form-1", "form-2"]
        .iter()
        .map(|id| cx.debug_bounds(id).expect("a part of the choice"))
        .collect()
}

#[gpui::test]
fn segments_full_width_parts_are_equal_and_fill_the_track_inside_its_padding(
    cx: &mut TestAppContext,
) {
    let cx = form_window(cx, true);
    let track = cx.debug_bounds("form").expect("the track");
    let parts = part_bounds(cx);
    let tolerance = px(1.);
    assert!(
        (track.size.width - px(276.)).abs() < tolerance,
        "the track spans its column"
    );
    for part in &parts {
        assert!(
            (part.size.width - parts[0].size.width).abs() < tolerance,
            "equal widths"
        );
        assert_eq!(part.size.height, parts[0].size.height, "equal heights");
    }
    let spanned = parts[2].right() - parts[0].left();
    assert!(
        (spanned - (track.size.width - TRACK_PADDING * 2.)).abs() < tolerance,
        "parts span the track minus its padding"
    );
    assert!(
        (parts[0].left() - track.left() - TRACK_PADDING).abs() < tolerance,
        "an even inset at the left"
    );
    assert!(
        (track.right() - parts[2].right() - TRACK_PADDING).abs() < tolerance,
        "an even inset at the right"
    );
}

#[gpui::test]
fn segments_full_width_choosing_moves_nothing_and_fills_exactly_the_part(cx: &mut TestAppContext) {
    let cx = form_window(cx, true);
    let before = part_bounds(cx);
    let track = cx.debug_bounds("form").expect("the track");
    click(cx, "form-2");
    cx.run_until_parked();
    assert_eq!(part_bounds(cx), before, "no part moves or resizes");
    assert_eq!(
        cx.debug_bounds("form"),
        Some(track),
        "the track keeps its size"
    );
    let fill: gpui::Background = Palette::dark().segment_selected.into();
    let filled: Vec<_> = cx.update(|window, cx| {
        window.set_scale_factor(1.);
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == fill)
            .map(|quad| {
                let b = quad.bounds;
                [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
            })
            .collect()
    });
    let part = before[2];
    let expected = [
        part.origin.x,
        part.origin.y,
        part.size.width,
        part.size.height,
    ]
    .map(f32::from);
    assert_eq!(filled.len(), 1, "one fill");
    // The painter snaps a quad to whole pixels, so a part that starts on a half pixel may move by one.
    for (drawn, wanted) in filled[0].iter().zip(expected) {
        assert!(
            (drawn - wanted).abs() <= 1.,
            "the fill covers its part and no more"
        );
    }
}

#[gpui::test]
fn segments_inline_hugs_its_parts_and_keeps_its_size_when_the_choice_changes(
    cx: &mut TestAppContext,
) {
    let cx = form_window(cx, false);
    let track = cx.debug_bounds("form").expect("the track");
    let before = part_bounds(cx);
    let spanned = before[2].right() - before[0].left();
    assert!(
        (track.size.width - (spanned + TRACK_PADDING * 2.)).abs() < px(1.),
        "the track is its parts and a padding step each side, not its column"
    );
    assert!(
        track.size.width < px(276.),
        "it does not stretch to the column"
    );
    assert_eq!(before[0].size.height, px(22.), "a part is 22 px high");
    click(cx, "form-0");
    cx.run_until_parked();
    assert_eq!(part_bounds(cx), before, "no part moves or resizes");
}

// Chip.

#[gpui::test]
fn chip_is_the_small_control_high_and_calls_back_when_pressed(cx: &mut TestAppContext) {
    let pressed = Rc::new(Cell::new(0));
    let counted = pressed.clone();
    let cx = open(cx, move |_, cx| {
        let counted = counted.clone();
        div()
            .child(
                chip(
                    "preset",
                    "Standard",
                    Some(IconName::FolderPlus),
                    move |_, _, _| counted.set(counted.get() + 1),
                    cx,
                )
                .debug_selector(|| "chip".to_string()),
            )
            .into_any_element()
    });
    assert_eq!(
        cx.debug_bounds("chip").expect("the chip").size.height,
        crate::theme::kit::scale::CHIP_HEIGHT
    );
    click(cx, "chip");
    assert_eq!(pressed.get(), 1);
}

// Toast.

/// A window with a toast layer in a positioned panel, and no toast yet.
fn toast_panel(cx: &mut TestAppContext) -> &mut VisualTestContext {
    open(cx, |window, cx| {
        div()
            .relative()
            .size_full()
            .child(Toast::layer(window, cx, gpui::Pixels::ZERO))
            .into_any_element()
    })
}

fn error_window(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let cx = toast_panel(cx);
    cx.update(|window, cx| Toast::show_error(window, cx, "The act could not be sent"));
    wait(cx, Duration::from_millis(500));
    cx
}

fn toast_window(
    cx: &mut TestAppContext,
    undo_count: Rc<Cell<u32>>,
    with_undo: bool,
) -> &mut VisualTestContext {
    let cx = toast_panel(cx);
    cx.update(|window, cx| {
        let undo: Option<toast::Undo> = with_undo.then(|| {
            Box::new(move |_: &mut Window, _: &mut gpui::App| undo_count.set(undo_count.get() + 1))
                as toast::Undo
        });
        Toast::show(window, cx, "Approved", undo);
    });
    wait(cx, Duration::from_millis(500));
    cx
}

#[gpui::test]
fn toast_detail_is_a_second_line_under_the_sentence(cx: &mut TestAppContext) {
    let cx = toast_panel(cx);
    cx.update(|window, cx| {
        Toast::show_with_detail(
            window,
            cx,
            "Approving the spec of demo",
            Some("From the engine".into()),
            None,
        );
    });
    wait(cx, Duration::from_millis(500));
    let text = cx.debug_bounds("toast-text").expect("the sentence");
    let detail = cx.debug_bounds("toast-detail").expect("the second line");
    assert!(
        detail.origin.y >= text.origin.y + text.size.height,
        "under the sentence"
    );
    assert!(cx.debug_bounds("toast-undo").is_none());
}

#[gpui::test]
fn toast_without_a_detail_draws_no_second_line(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), true);
    assert!(cx.debug_bounds("toast-detail").is_none());
}

#[gpui::test]
fn toast_undo_is_called_once_and_never_after_the_toast_closed(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_window(cx, undone.clone(), true);
    let button = cx.debug_bounds("toast-undo").expect("the Undo button");
    cx.simulate_click(button.center(), Modifiers::none());
    cx.simulate_click(button.center(), Modifiers::none());
    wait(cx, Duration::from_secs(1));
    assert_eq!(undone.get(), 1);
    assert_eq!(toasts(cx), 0, "pressing Undo closes the toast");
    assert!(cx.debug_bounds("toast-undo").is_none());
    assert_eq!(undone.get(), 1);
}

#[gpui::test]
fn toast_with_undo_stays_6_seconds_and_closes_then(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), true);
    wait(cx, WITH_UNDO - Duration::from_secs(1));
    assert_eq!(toasts(cx), 1, "still there before its time is up");
    wait(cx, Duration::from_secs(2));
    assert_eq!(toasts(cx), 0);
}

#[gpui::test]
fn toast_without_undo_has_no_button_stays_4_seconds_and_closes_then(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), false);
    assert!(cx.debug_bounds("toast-undo").is_none());
    wait(cx, WITHOUT_UNDO - Duration::from_secs(1));
    assert_eq!(toasts(cx), 1);
    wait(cx, Duration::from_secs(2));
    assert_eq!(toasts(cx), 0);
}

#[gpui::test]
fn toast_waits_while_the_pointer_is_on_it_and_goes_when_it_leaves(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), false);
    let text = cx.debug_bounds("toast-text").expect("the toast's text");
    cx.simulate_mouse_move(text.center(), None, Modifiers::none());
    wait(cx, WITH_UNDO + WITHOUT_UNDO);
    assert_eq!(toasts(cx), 1, "the pointer on it holds it");
    cx.simulate_mouse_move(gpui::point(px(1.), px(1.)), None, Modifiers::none());
    wait(cx, WITHOUT_UNDO + Duration::from_secs(1));
    assert_eq!(toasts(cx), 0);
}

#[gpui::test]
fn toast_goes_at_escape_without_calling_undo(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_window(cx, undone.clone(), true);
    cx.simulate_keystrokes("escape");
    wait(cx, Duration::from_secs(1));
    assert_eq!(toasts(cx), 0);
    assert_eq!(undone.get(), 0);
}

/// Presses every point of the band where a toast sits, but the Undo button, with the pointer moved
/// there first so a control that shows on hover is shown.
fn press_everywhere_but_undo(cx: &mut VisualTestContext) {
    let size = cx.update(|window, _| window.viewport_size());
    let (centre, height, step) = (f32::from(size.width) / 2., f32::from(size.height), 8.);
    let undo = cx.debug_bounds("toast-undo");
    let mut y = height - 140.;
    while y < height {
        let mut x = centre - 300.;
        while x < centre + 300. {
            let point = gpui::point(px(x), px(y));
            x += step;
            if undo.is_some_and(|bounds| bounds.contains(&point)) {
                continue;
            }
            cx.simulate_mouse_move(point, None, Modifiers::none());
            cx.run_until_parked();
            cx.simulate_click(point, Modifiers::none());
        }
        y += step;
    }
    wait(cx, Duration::from_millis(500));
}

#[gpui::test]
fn toast_has_no_close_control_whatever_is_pressed_on_it(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), false);
    press_everywhere_but_undo(cx);
    assert_eq!(
        toasts(cx),
        1,
        "nothing on the toast closes it but time, Escape and a swipe"
    );
}

#[gpui::test]
fn toast_with_undo_has_no_close_control_beside_undo(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_window(cx, undone.clone(), true);
    press_everywhere_but_undo(cx);
    assert_eq!(toasts(cx), 1);
    assert_eq!(undone.get(), 0, "nothing but the button undoes");
}

#[gpui::test]
fn error_toast_has_no_button_no_close_control_and_stays_8_seconds(cx: &mut TestAppContext) {
    let cx = error_window(cx);
    assert!(cx.debug_bounds("toast-undo").is_none());
    press_everywhere_but_undo(cx);
    wait(
        cx,
        ERROR - Duration::from_secs(1) - Duration::from_millis(500),
    );
    assert_eq!(toasts(cx), 1, "still there before its time is up");
    wait(cx, Duration::from_secs(2));
    assert_eq!(toasts(cx), 0);
}

/// Drags across the toast from its middle by `across`, in the steps a hand takes.
fn drag_across_the_toast(cx: &mut VisualTestContext, across: f32) {
    const STEPS: u8 = 6;
    let middle = cx.debug_bounds("toast").expect("the toast").center();
    cx.simulate_mouse_down(middle, gpui::MouseButton::Left, Modifiers::none());
    let mut at = middle;
    for _ in 0..STEPS {
        at = gpui::point(at.x + px(across / f32::from(STEPS)), at.y);
        cx.simulate_mouse_move(at, Some(gpui::MouseButton::Left), Modifiers::none());
    }
    cx.simulate_mouse_up(at, gpui::MouseButton::Left, Modifiers::none());
    wait(cx, Duration::from_millis(100));
}

#[gpui::test]
fn toast_goes_when_it_is_swiped_across(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_window(cx, undone.clone(), true);
    drag_across_the_toast(cx, 120.);
    assert_eq!(toasts(cx), 0);
    assert_eq!(undone.get(), 0, "a swipe sends it away without undoing");
}

#[gpui::test]
fn toast_stays_when_the_drag_across_it_is_short(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), false);
    drag_across_the_toast(cx, 10.);
    assert_eq!(toasts(cx), 1);
}

#[gpui::test]
fn toast_goes_when_it_is_scrolled_sideways(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), false);
    let middle = cx.debug_bounds("toast").expect("the toast").center();
    cx.simulate_event(gpui::ScrollEvent {
        position: middle,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(-120.), px(0.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    wait(cx, Duration::from_millis(100));
    assert_eq!(toasts(cx), 0);
}

#[gpui::test]
fn toast_replaces_the_one_on_screen_and_the_old_undo_is_gone(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_window(cx, undone.clone(), true);
    cx.update(|window, cx| Toast::show(window, cx, "Declined", None));
    wait(cx, Duration::from_millis(500));
    assert_eq!(toasts(cx), 1, "one at a time");
    assert!(cx.debug_bounds("toast-undo").is_none());
    wait(cx, WITHOUT_UNDO - Duration::from_secs(1));
    assert_eq!(toasts(cx), 1, "the new toast keeps its own time");
    wait(cx, Duration::from_secs(2));
    assert_eq!(toasts(cx), 0);
    assert_eq!(undone.get(), 0);
}

#[gpui::test]
fn toast_undo_may_show_a_toast_of_its_own(cx: &mut TestAppContext) {
    let cx = toast_panel(cx);
    cx.update(|window, cx| {
        let undo: toast::Undo = Box::new(|window, cx| {
            Toast::show(window, cx, "Undone", None);
        });
        Toast::show(window, cx, "Approved", Some(undo));
    });
    wait(cx, Duration::from_millis(500));
    click(cx, "toast-undo");
    wait(cx, Duration::from_millis(500));
    assert_eq!(toasts(cx), 1, "the toast the undo showed");
    assert!(cx.debug_bounds("toast-undo").is_none());
}

#[gpui::test]
fn toast_sits_at_the_bottom_centre_of_its_panel(cx: &mut TestAppContext) {
    let cx = toast_window(cx, Rc::new(Cell::new(0)), true);
    let size = cx.update(|window, _| window.viewport_size());
    let sheet = cx.debug_bounds("toast").expect("the toast");
    let off_centre = (sheet.center().x - size.width / 2.).abs();
    assert!(
        off_centre < px(1.),
        "centred across the panel, {off_centre:?} off"
    );
    let from_bottom = size.height - sheet.bottom();
    assert!(
        (from_bottom - crate::theme::kit::TOAST_BOTTOM).abs() < px(1.),
        "{from_bottom:?} above the bottom edge"
    );
}

#[cfg(not(feature = "boxes"))]
#[gpui::test]
fn polish_x_dialog_toast_leaves_the_page_at_full_height(cx: &mut TestAppContext) {
    use crate::ui::{engine_desk_tests, engine_rig_tests};

    let (_rig, desk, cx) = engine_desk_tests::open(cx, engine_rig_tests::SNAPSHOT);
    let id = desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().id().as_u64()
    });
    let pane: &'static str = Box::leak(format!("page-pane-{id}").into_boxed_str());
    cx.update(|window, cx| Toast::show(window, cx, "Saved", Some(Box::new(|_, _| {}))));
    wait(cx, Duration::from_millis(250));
    let panel = cx.debug_bounds("page-panel").unwrap();
    let page = cx.debug_bounds(pane).unwrap();
    let toast = cx.debug_bounds("toast").unwrap();
    assert!(panel.bottom() - page.bottom() <= crate::theme::PANEL_RING_WIDTH);
    assert!(toast.bottom() < page.bottom());
}

#[cfg(not(feature = "boxes"))]
#[gpui::test]
fn desk_toasts_clear_the_panel_edges_and_share_a_raised_bordered_surface(cx: &mut TestAppContext) {
    use crate::ui::{engine_desk_tests, engine_rig_tests};
    let (_rig, _desk, cx) = engine_desk_tests::open(cx, engine_rig_tests::SNAPSHOT);
    let panel = cx.debug_bounds("page-panel").unwrap();
    for detail in [None, Some("The change applies to new lanes.".into())] {
        cx.update(|window, cx| Toast::show_with_detail(window, cx, "Saved", detail, None));
        wait(cx, Duration::from_millis(250));
        let toast = cx.debug_bounds("toast").unwrap();
        assert!(panel.bottom() - toast.bottom() >= px(13.));
        assert!(toast.right() < panel.left());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let palette = crate::theme::Theme::palette(cx);
            let at = toast.center().scale(window.scale_factor());
            let quads: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.bounds.contains(&at))
                .collect();
            let face = quads
                .iter()
                .find(|quad| quad.background == palette.overlay_surface.into())
                .expect("raised toast surface");
            assert!(quads.iter().any(|quad| {
                quad.bounds == face.bounds
                    && quad.border_color == palette.overlay_edge
                    && quad.border_widths.bottom == px(1.).scale(window.scale_factor())
            }));
        });
    }
}

// The toast's coming and going.

fn after(since: std::time::Instant, millis: u64) -> std::time::Instant {
    since + Duration::from_millis(millis)
}

#[test]
fn toast_rises_over_the_layout_duration_without_passing_its_place() {
    let now = std::time::Instant::now();
    let rising = toast_phase::Phase::rising(now);
    assert_eq!(rising.visibility(now), 0.0);
    let samples: Vec<f32> = (0..=20)
        .map(|step| rising.visibility(after(now, step * 10)))
        .collect();
    assert!(
        samples.windows(2).all(|pair| pair[0] <= pair[1]),
        "it never goes back"
    );
    assert!(
        samples.iter().all(|share| *share <= 1.0),
        "no bounce past its place"
    );
    assert!(rising.is_moving(after(now, 199)));
    assert_eq!(rising.visibility(after(now, 200)), 1.0);
    assert!(!rising.is_moving(after(now, 200)));
}

#[test]
fn toast_sunk_while_still_rising_leaves_from_where_it_is() {
    let now = std::time::Instant::now();
    let rising = toast_phase::Phase::rising(now);
    let middle = after(now, 40);
    let sinking = rising.leaving(middle);
    assert!(sinking.is_leaving());
    assert_eq!(
        sinking.visibility(middle),
        rising.visibility(middle),
        "no jump"
    );
    assert!(!sinking.is_over(after(middle, 119)));
    assert!(sinking.is_over(after(middle, 120)));
    assert_eq!(sinking.visibility(after(middle, 120)), 0.0);
}

// State glyphs.

/// What a glyph painted at 28 px draws, quad by quad: size, corners, border and fill, so two marks are
/// the same only when they draw the same shapes.
fn drawn(glyph: StateGlyph, cx: &mut TestAppContext) -> Vec<String> {
    let palette = Palette::dark();
    let cx = open(cx, move |_, _| glyph.paint(px(28.), &palette));
    cx.update(|window, cx| {
        window.set_scale_factor(1.);
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds.size.width.0 <= 28.)
            .map(|quad| {
                format!(
                    "{:?} {:?} {:?} {:?} {:?}",
                    quad.bounds.size,
                    quad.corner_radii,
                    quad.border_widths,
                    quad.border_style,
                    quad.background
                )
            })
            .collect()
    })
}

#[gpui::test]
fn glyph_every_state_paints_its_own_shape(cx: &mut TestAppContext) {
    let states = [
        (StateGlyph::NeedsYou, 1),
        (StateGlyph::Running, 2),
        (StateGlyph::Finished, 1),
        (StateGlyph::Stopped, 2),
        (StateGlyph::NotStarted, 1),
    ];
    let mut seen: Vec<Vec<String>> = Vec::new();
    for (state, quads) in states {
        let shapes = drawn(state, cx);
        assert_eq!(shapes.len(), quads, "{state:?}");
        assert!(
            !seen.contains(&shapes),
            "{state:?} draws what another state draws"
        );
        seen.push(shapes);
    }
}

#[gpui::test]
fn glyph_no_state_is_drawn_in_the_accent_which_an_unanswered_ask_alone_wears(
    cx: &mut TestAppContext,
) {
    for palette in [Palette::dark(), Palette::light()] {
        let accent: gpui::Background = palette.attention.into();
        let accent = format!("{accent:?}");
        for state in [
            StateGlyph::NeedsYou,
            StateGlyph::Running,
            StateGlyph::Finished,
            StateGlyph::Stopped,
            StateGlyph::NotStarted,
        ] {
            let cx = open(cx, move |_, _| state.paint(px(28.), &palette));
            let shapes: Vec<String> = cx.update(|window, cx| {
                window.set_scale_factor(1.);
                window.draw(cx).clear(cx);
                window
                    .painted_quads()
                    .iter()
                    .map(|quad| format!("{:?}{:?}", quad.background, quad.border_color))
                    .collect()
            });
            assert!(
                shapes.iter().all(|shape| !shape.contains(&accent)),
                "{state:?} is drawn in the accent"
            );
        }
    }
}

// Progress line.

fn fill_widths(done: u32, total: u32, cx: &mut TestAppContext) -> Vec<f32> {
    let cx = open(cx, move |_, _| {
        progress_line(done, total, px(40.)).into_any_element()
    });
    let fill: gpui::Background = Palette::dark().text_tertiary.into();
    cx.update(|window, cx| {
        window.set_scale_factor(1.);
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == fill)
            .map(|quad| quad.bounds.size.width.0)
            .collect()
    })
}

#[gpui::test]
fn progress_line_fills_the_share_of_steps_done(cx: &mut TestAppContext) {
    assert_eq!(fill_widths(3, 6, cx), vec![20.]);
    assert_eq!(
        fill_widths(9, 6, cx),
        vec![40.],
        "past the total it is full"
    );
}

#[gpui::test]
fn progress_line_with_no_steps_is_empty(cx: &mut TestAppContext) {
    assert!(fill_widths(0, 0, cx).iter().all(|width| *width == 0.));
}

#[gpui::test]
fn toast_surface_draws_its_words_inside_a_sheet_of_its_own(cx: &mut TestAppContext) {
    let cx = open(cx, |_, cx| {
        let palette = Theme::palette(cx);
        div()
            .size_full()
            .child(toast_surface(
                "specimen",
                "Approved".into(),
                None,
                true,
                &palette,
            ))
            .into_any_element()
    });
    let sheet = cx.debug_bounds("specimen").expect("the sheet");
    let words = cx.debug_bounds("toast-text").expect("the words");
    let undo = cx.debug_bounds("toast-undo").expect("the Undo label");
    assert!(sheet.contains(&words.origin) && sheet.contains(&undo.center()));
    assert!(sheet.size.width > words.size.width + undo.size.width);
}

// Path label.

const PATH: &str = "src/app/features/catalogs/components/visible.ts";

fn shown_path(room: f32, cx: &mut TestAppContext) -> &mut VisualTestContext {
    let cx = open(cx, move |_, cx| {
        let palette = Theme::palette(cx);
        div()
            .debug_selector(|| "room".into())
            .w(px(room))
            .child(path_label(
                "path",
                PATH,
                (palette.text, palette.text_tertiary),
            ))
            .into_any_element()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

#[test]
fn split_path_keeps_the_first_folder_the_middle_and_the_name_apart() {
    let parts = split_path_for_test(PATH);
    assert_eq!(
        parts,
        ("src/", "app/features/catalogs/components/", "visible.ts")
    );
    assert_eq!(split_path_for_test("src/a.rs"), ("src/", "", "a.rs"));
    assert_eq!(split_path_for_test("a.rs"), ("", "", "a.rs"));
    assert_eq!(split_path_for_test("a/b/c/"), ("a/", "b/c/", ""));
}

fn split_path_for_test(path: &str) -> (&str, &str, &str) {
    let parts = super::path_label::split_path(path);
    (parts.root, parts.middle, parts.name)
}

#[gpui::test]
fn path_label_in_a_narrow_row_cuts_the_middle_and_keeps_the_root_and_the_name(
    cx: &mut TestAppContext,
) {
    let wide = shown_path(1000., cx);
    let (natural, name) = (
        wide.debug_bounds("path-middle")
            .expect("the middle")
            .size
            .width,
        wide.debug_bounds("path-name").expect("the name").size.width,
    );
    let narrow = shown_path(220., cx);
    let room = narrow.debug_bounds("room").expect("the room");
    let middle = narrow.debug_bounds("path-middle").expect("the middle");
    let shown = narrow.debug_bounds("path-name").expect("the name");
    assert!(middle.size.width < natural, "the middle gave way");
    assert_eq!(shown.size.width, name, "the name is whole");
    assert!(shown.right() <= room.right(), "and inside the row");
    assert!(narrow.debug_bounds("path-root").is_some(), "the root stays");
}

// Closing never waits for the opening.

/// A toast with Undo, drawn once and no more: it is still coming in.
fn toast_coming_in(cx: &mut TestAppContext, undone: Rc<Cell<u32>>) -> &mut VisualTestContext {
    let cx = toast_panel(cx);
    cx.update(|window, cx| {
        cx.set_reduce_motion(false);
        let undo: toast::Undo =
            Box::new(move |_: &mut Window, _: &mut gpui::App| undone.set(undone.get() + 1));
        Toast::show(window, cx, "Approved", Some(undo));
        window.draw(cx).clear(cx);
    });
    cx
}

#[gpui::test]
fn toast_undo_works_in_the_frame_the_toast_comes_in(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_coming_in(cx, undone.clone());
    let button = cx.debug_bounds("toast-undo").expect("the Undo button");
    cx.simulate_click(button.center(), Modifiers::none());
    assert_eq!(undone.get(), 1, "no wait for the entrance");
}

#[gpui::test]
fn toast_leaves_at_escape_in_the_frame_it_comes_in(cx: &mut TestAppContext) {
    let undone = Rc::new(Cell::new(0));
    let cx = toast_coming_in(cx, undone.clone());
    cx.simulate_keystrokes("escape");
    // The toast sinks on the real clock, which the test's own clock does not move.
    std::thread::sleep(Duration::from_millis(600));
    wait(cx, Duration::from_secs(1));
    assert_eq!(toasts(cx), 0);
    assert_eq!(undone.get(), 0);
}

// Toggle: a tab stop for the keyboard, a ring only for it.

fn ring_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| {
        let ring = Theme::palette(cx).ring;
        let close = |x: f32, y: f32| (x - y).abs() < 0.01;
        let is_ring = |quad: &gpui::Quad| {
            let width = quad.border_widths;
            let has_border = [width.top, width.right, width.bottom, width.left]
                .iter()
                .any(|side| side.0 > 0.0);
            let tone = quad.border_color;
            has_border
                && tone.a > 0.0
                && close(tone.h, ring.h)
                && close(tone.s, ring.s)
                && close(tone.l, ring.l)
        };
        window
            .painted_quads()
            .iter()
            .filter(|quad| is_ring(quad))
            .count()
    })
}

fn toggle_window(cx: &mut TestAppContext, asked: Rc<RefCell<Vec<bool>>>) -> &mut VisualTestContext {
    cx.update(crate::ui::keyboard::install);
    let cx = open(cx, move |_, _| {
        let asked = asked.clone();
        div()
            .size_full()
            .child(toggle("switch", false, move |wanted, _, _| {
                asked.borrow_mut().push(wanted)
            }))
            .into_any_element()
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx)
    });
    cx
}

/// Tab, as the keyboard presses it, with the move of focus it asks for done and drawn.
fn press_tab(cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn toggle_is_reached_by_tab_and_space_asks_for_the_other_state(cx: &mut TestAppContext) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let cx = toggle_window(cx, asked.clone());
    press_tab(cx);
    cx.simulate_keystrokes("space");
    assert_eq!(*asked.borrow(), [true], "Space on the focused switch");
}

#[gpui::test]
fn toggle_shows_its_ring_after_tab_and_never_after_a_click(cx: &mut TestAppContext) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let cx = toggle_window(cx, asked.clone());
    let centre = cx.debug_bounds("switch").expect("the switch").center();
    cx.simulate_click(centre, Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(asked.borrow().len(), 1, "the click asked once");
    assert_eq!(ring_count(cx), 0, "no ring after a click");
    press_tab(cx);
    assert!(ring_count(cx) > 0, "a ring after Tab");
}

#[gpui::test]
fn button_is_reached_by_tab_and_return_presses_it(cx: &mut TestAppContext) {
    let pressed = Rc::new(Cell::new(0));
    let counted = pressed.clone();
    cx.update(crate::ui::keyboard::install);
    let cx = open(cx, move |_, cx| {
        let counted = counted.clone();
        let palette = Theme::palette(cx);
        div()
            .size_full()
            .child(
                button("go", "Go", ButtonKind::Secondary, &palette)
                    .on_click(move |_, _, _| counted.set(counted.get() + 1)),
            )
            .into_any_element()
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx)
    });
    let centre = cx.debug_bounds("go").expect("the button").center();
    cx.simulate_click(centre, Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(pressed.get(), 1);
    press_tab(cx);
    cx.update(|window, cx| assert!(window.focused(cx).is_some(), "Tab reached the button"));
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").expect("a key"),
    });
    assert_eq!(pressed.get(), 2, "Return presses the focused button");
}

#[gpui::test]
fn an_undo_toast_stays_centred_when_the_pointer_is_near_the_bottom(cx: &mut TestAppContext) {
    let cx = toast_panel(cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    cx.simulate_mouse_move(
        gpui::point(viewport.width / 2., viewport.height - px(12.)),
        None,
        Modifiers::none(),
    );
    cx.update(|window, cx| Toast::show(window, cx, "Approved", Some(Box::new(|_, _| {}))));
    wait(cx, Duration::from_millis(500));
    let toast = cx.debug_bounds("toast").unwrap();
    assert!(
        (toast.center().x - viewport.width / 2.).abs() < px(1.),
        "{toast:?}"
    );
}

#[gpui::test]
fn a_narrow_agent_row_keeps_the_role_and_reflows_the_state(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        let facts = AgentRowFacts {
            role: "Spec planner".into(),
            detail: "Opus".into(),
            phase: None,
            task: None,
            rounds: None,
            state: "Finished".into(),
            glyph: StateGlyph::Finished,
            doing: Some(DoingLine::Text(
                "Inspecting the contract and recording the result".into(),
            )),
        };
        div()
            .w(px(190.))
            .child(agent_row("narrow-agent", &facts, &Palette::dark()))
            .into_any_element()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let role = cx.debug_bounds("narrow-agent-role").unwrap();
    let state = cx.debug_bounds("narrow-agent-state").unwrap();
    assert!(state.top() >= role.bottom(), "{role:?}, {state:?}");
}

#[gpui::test]
fn the_light_switch_draws_its_enabled_accent_track(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| super::toggle("light-switch", true, |_, _, _| {}));
    cx.update(|window, cx| {
        Theme::set(Appearance::Light, cx);
        window.draw(cx).clear(cx);
    });
    let control = cx.debug_bounds("light-switch").unwrap();
    cx.update(|window, cx| {
        let accent = crate::theme::Theme::palette(cx).attention;
        let control = control.scale(window.scale_factor());
        assert!(window.painted_quads().iter().any(|quad| {
            control.contains(&quad.bounds.center()) && quad.background == accent.into()
        }));
    });
}
