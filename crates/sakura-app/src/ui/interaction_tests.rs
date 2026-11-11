//! The window driven like a person would, on GPUI's test platform: simulated pointer and keys on a test
//! window over the built-in fixture. Actions are recorded instead of reaching `box`, and never answer, so an
//! action that started stays running.

use crate::data::actions::Actions;
use crate::data::changes::Changes;
use crate::data::facts::Facts;
use crate::data::sessions::Sessions;
use crate::data::source::{Recorder, Source, Started};
use crate::theme::{self, Appearance, Theme};
use crate::ui::breath::BreathMarks;
use crate::ui::console::Consoles;
use crate::ui::desk::{Desk, DeskData};
use crate::ui::right_pane::PaneView;
use gpui::{
    point, px, AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext,
};
use sakura_core::box_cli::BoxCli;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

const TASK: &str = "sample-accent";
const HOLD_TASK: &str = "sample-folders";
const WAIT_STEP: Duration = Duration::from_millis(10);
/// How long a test waits for a state before it calls it a hang. These tests wait on real processes, on a
/// machine that may be running every other test: the deadline is never what decides a pass.
pub(super) const HANG: Duration = Duration::from_secs(120);

/// Opens the desk on the fixture in a test window and draws its first frame.
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>, Arc<Recorder>) {
    open_with(cx, None)
}

/// Opens the desk on the fixture; terminals run `fake_box` when given, else none can open.
pub(super) fn open_with(
    cx: &mut TestAppContext,
    fake_box: Option<std::path::PathBuf>,
) -> (&mut VisualTestContext, Entity<Desk>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let actions = recorder.clone();
    let start = move |runtime| Source::for_test(runtime, actions);
    let (cx, desk) = open_on(cx, start, fake_box, None);
    (cx, desk, recorder)
}

/// Window tests run together, except one that measures time: it waits for the others to end and keeps new
/// ones waiting, so a machine busy with the rest of the suite does not decide its outcome.
static WINDOW_TESTS: std::sync::RwLock<()> = std::sync::RwLock::new(());

thread_local! {
    /// This test's place among the window tests, given back when its thread ends.
    static PLACE: RefCell<Option<Box<dyn std::any::Any>>> = const { RefCell::new(None) };
}

/// Takes this test's place among the window tests: alone when `is_timed`, else beside the others.
pub(super) fn take_place(is_timed: bool) {
    PLACE.with_borrow_mut(|place| place.take());
    let held: Box<dyn std::any::Any> = if is_timed {
        Box::new(WINDOW_TESTS.write().unwrap_or_else(PoisonError::into_inner))
    } else {
        Box::new(WINDOW_TESTS.read().unwrap_or_else(PoisonError::into_inner))
    };
    PLACE.with_borrow_mut(|place| *place = Some(held));
}

/// Opens the desk on the source `start` builds and draws its first frame; `first` is the task the
/// preferences remember as shown last.
pub(super) fn open_on(
    cx: &mut TestAppContext,
    start: impl FnOnce(Arc<tokio::runtime::Runtime>) -> Started,
    fake_box: Option<std::path::PathBuf>,
    first: Option<String>,
) -> (&mut VisualTestContext, Entity<Desk>) {
    let preferences = crate::prefs::Preferences {
        last_task: first,
        ..Default::default()
    };
    open_remembering(cx, start, fake_box, preferences)
}

/// Opens the desk as [`open_on`] does, with everything the window remembers given.
pub(super) fn open_remembering(
    cx: &mut TestAppContext,
    start: impl FnOnce(Arc<tokio::runtime::Runtime>) -> Started,
    fake_box: Option<std::path::PathBuf>,
    preferences: crate::prefs::Preferences,
) -> (&mut VisualTestContext, Entity<Desk>) {
    if PLACE.with_borrow(Option::is_none) {
        take_place(false);
    }
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        crate::menus::install(cx);
        crate::ui::keyboard::install(cx);
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a test runtime");
    let started = start(Arc::new(runtime));
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let desk = desk(started, fake_box, preferences, window, cx);
        *kept.borrow_mut() = Some(desk.clone());
        gpui_component::Root::new(desk, window, cx)
    });
    let desk = slot.borrow_mut().take().expect("the desk was built");
    // Keys reach only the active window; activating it gives no element focus.
    cx.update(|window, _| window.activate_window());
    draw(cx);
    (cx, desk)
}

fn desk(
    started: Started,
    fake_box: Option<std::path::PathBuf>,
    preferences: crate::prefs::Preferences,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> Entity<Desk> {
    let Started {
        source,
        events,
        watcher,
    } = started;
    let first = preferences.last_task.clone();
    let facts = cx.new(|cx| Facts::new(source.clone(), events, watcher, first, cx));
    let actions = cx.new(|cx| Actions::new(source.clone(), facts.clone(), cx));
    let projects = cx.new(|_| crate::data::projects::Projects::new(source.clone()));
    let router = sakura_terminal::CoreRouter::new();
    // Conversations are kept beside the fake `box`: a folder of the test's own.
    let folder = fake_box.as_deref().and_then(std::path::Path::parent);
    let sessions = Sessions::new(folder, source.runtime().clone());
    let terminals = fake_box.map(|script| {
        let cwd = script
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        sakura_core::terminal::Terminals::new(BoxCli::at(script), cwd, router.core_sink())
    });
    let consoles = cx
        .new(|cx| Consoles::with_terminals(terminals, router, facts.clone(), actions.clone(), cx));
    let changes = cx.new(|cx| Changes::new(source.clone(), facts.clone(), cx));
    let data = DeskData {
        facts,
        actions,
        projects,
        changes,
        consoles,
        source,
        sessions,
    };
    preferences.clone().install(None, cx);
    cx.new(|cx| Desk::new(data, &preferences, window, cx))
}

pub(super) fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

pub(super) fn centre(
    cx: &mut VisualTestContext,
    selector: &'static str,
) -> gpui::Point<gpui::Pixels> {
    match cx.debug_bounds(selector) {
        Some(bounds) => bounds.center(),
        None => panic!("{selector} is not on screen"),
    }
}

/// A click on the element, then the next frame.
pub(super) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = centre(cx, selector);
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
}

/// Waits for work on other threads (fixture reads on the runtime) until `is_ready`.
pub(super) fn wait_for(
    cx: &mut VisualTestContext,
    what: &str,
    is_ready: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = std::time::Instant::now() + HANG;
    while std::time::Instant::now() < deadline {
        cx.run_until_parked();
        draw(cx);
        if is_ready(cx) {
            return;
        }
        std::thread::sleep(WAIT_STEP);
    }
    panic!("timed out waiting for {what}");
}

/// The view the right pane of `task` shows.
/// Waits until a frame that was drawn asks for no other: a slide measures real time, so it ends when
/// it ends, not after a count of frames.
pub(super) fn motion_ended(cx: &mut VisualTestContext) {
    wait_for(cx, "the motion to end", |cx| {
        cx.update(|window, cx| window.simulate_next_frame(cx)) == 0
    });
    cx.run_until_parked();
}

pub(super) fn pane_view(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> PaneView {
    desk.read_with(cx, |desk, _| desk.pane_view(task))
}

#[gpui::test]
fn nothing_takes_focus_when_the_window_opens(cx: &mut TestAppContext) {
    let (cx, _, _) = open(cx);
    cx.update(|window, cx| assert!(window.focused(cx).is_none()));
}

fn selected(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<String> {
    desk.read_with(cx, |desk, cx| desk.shown(cx))
}

#[gpui::test]
fn the_window_opens_on_the_first_row_of_da_guardare_without_taking_focus(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("page-header").is_some()
    });
    assert_eq!(selected(cx, &desk).as_deref(), Some(TASK));
    cx.update(|window, cx| assert!(window.focused(cx).is_none()));
}

#[gpui::test]
fn the_window_opens_on_the_task_shown_last(cx: &mut TestAppContext) {
    let start = |runtime| Source::for_test(runtime, Arc::new(Recorder::default()));
    let (cx, desk) = open_on(cx, start, None, Some("headings-26".into()));
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("page-header").is_some()
    });
    assert_eq!(selected(cx, &desk).as_deref(), Some("headings-26"));
}

#[gpui::test]
fn selecting_a_row_shows_that_task_and_remembers_it(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    click(cx, "row-sample-folders");
    assert_eq!(selected(cx, &desk).as_deref(), Some(HOLD_TASK));
    assert!(
        cx.debug_bounds("page-header").is_some(),
        "its page is drawn"
    );
    let last = cx.update(|_, cx| crate::prefs::Preferences::current(cx).last_task);
    assert_eq!(last.as_deref(), Some(HOLD_TASK));
}

#[gpui::test]
fn a_view_of_the_right_pane_is_shown_in_the_frame_of_the_click_and_kept_per_lavoro(
    cx: &mut TestAppContext,
) {
    let (cx, desk, _) = open(cx);
    click(cx, "row-sample-accent");
    assert_eq!(pane_view(cx, &desk, TASK), PaneView::Changes);
    assert!(cx.debug_bounds("changes-body").is_some());
    click(cx, "pane-view-checks");
    assert_eq!(pane_view(cx, &desk, TASK), PaneView::Checks);
    assert!(
        cx.debug_bounds("verify-test").is_some(),
        "drawn in the first frame after the click"
    );
    assert!(cx.debug_bounds("changes-body").is_none());
    click(cx, "row-sample-folders");
    assert_eq!(
        pane_view(cx, &desk, HOLD_TASK),
        PaneView::Changes,
        "another lavoro has its own view"
    );
    click(cx, "row-sample-accent");
    assert_eq!(
        pane_view(cx, &desk, TASK),
        PaneView::Checks,
        "and this one kept its"
    );
}

#[gpui::test]
fn a_double_press_on_esegui_queues_once(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    click(cx, "row-sample-accent");
    click(cx, "pane-view-checks");
    let at = centre(cx, "run-test");
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
    assert_eq!(
        recorder.calls(),
        vec!["verify sample-accent test".to_string()]
    );
}

#[gpui::test]
fn annulla_takes_the_place_of_esegui_on_a_running_check_and_asks_box_once(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    click(cx, "row-sample-folders");
    click(cx, "pane-view-checks");
    wait_for(cx, "Annulla", |cx| cx.debug_bounds("cancel-test").is_some());
    assert!(cx.debug_bounds("run-test").is_none(), "not beside Esegui");
    assert!(
        cx.debug_bounds("run-rules").is_some(),
        "a verb at rest keeps Esegui"
    );
    let at = centre(cx, "cancel-test");
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
    assert_eq!(recorder.calls(), vec![format!("cancel {HOLD_TASK} test")]);
}

#[gpui::test]
fn the_hold_takes_only_after_its_duration(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    click(cx, "row-sample-folders");
    let at = centre(cx, "hold-sample-folders");
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.executor()
        .advance_clock(theme::HOLD - Duration::from_millis(100));
    cx.run_until_parked();
    assert!(recorder.calls().is_empty(), "not yet");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(recorder.calls(), vec![format!("take {HOLD_TASK}")]);
}

#[gpui::test]
fn releasing_the_hold_early_cancels_it(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    click(cx, "row-sample-folders");
    let at = centre(cx, "hold-sample-folders");
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.executor().advance_clock(theme::HOLD / 2);
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    cx.executor().advance_clock(theme::HOLD * 2);
    cx.run_until_parked();
    assert!(recorder.calls().is_empty());
}

#[gpui::test]
fn a_bare_click_on_the_hold_never_takes(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    click(cx, "row-sample-folders");
    click(cx, "hold-sample-folders");
    cx.executor().advance_clock(theme::HOLD * 2);
    cx.run_until_parked();
    assert!(recorder.calls().is_empty());
}

#[gpui::test]
fn tab_focuses_by_keyboard_so_the_ring_shows(cx: &mut TestAppContext) {
    let (cx, _, _) = open(cx);
    click(cx, "row-sample-accent");
    click(cx, "pane-view-checks");
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        assert!(window.focused(cx).is_some(), "tab reached a control");
        assert!(
            window.last_input_was_keyboard(),
            "keyboard focus shows the ring"
        );
    });
}

/// The pointer drag that selects code is not driven here: on GPUI's test platform it never begins a
/// selection (its text system lays out no glyphs, and with the real macOS one the gesture still did not
/// start), and the kit's own selection tests bypass the gesture too. What is checked is the part that was
/// missing: a press in the list gives it the keyboard, so Cmd+C reaches the kit's copy.
#[gpui::test]
fn a_file_opens_in_place_and_a_press_gives_the_list_the_keyboard(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    click(cx, "row-sample-accent");
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    let closed = pane_rows(cx, &desk);
    click(cx, "changes-header-0");
    assert!(pane_rows(cx, &desk) > closed, "the file opened in place");
    cx.update(|window, cx| {
        assert!(
            window.focused(cx).is_some(),
            "the press gave the list the keyboard"
        );
        assert!(!window.last_input_was_keyboard(), "with no ring");
    });
    click(cx, "changes-header-0");
    assert_eq!(pane_rows(cx, &desk), closed, "and closes again");
}

fn pane_rows(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> usize {
    desk.read_with(cx, |desk, cx| desk.pane_rows(TASK, cx))
}

#[gpui::test]
fn a_drag_across_code_lines_selects_them_and_cmd_c_copies_them(cx: &mut TestAppContext) {
    let (cx, _, _) = open(cx);
    click(cx, "row-sample-accent");
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    click(cx, "changes-header-0");
    let body = cx.debug_bounds("changes-body").expect("the list");
    // The header (36 px) and the hunk header (28 px) come first; code lines are 20 px tall.
    let first_line = body.top() + px(36. + 28. + 10.);
    let from = point(body.left() + px(200.), first_line);
    let to = point(body.left() + px(240.), first_line + px(20.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(cx);
    cx.simulate_keystrokes("cmd-c");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    let copied = copied.expect("the selection was copied");
    assert!(copied.contains('\n'), "two lines: {copied:?}");
}

#[gpui::test]
fn a_second_stop_while_the_first_runs_asks_box_once(cx: &mut TestAppContext) {
    let (cx, desk, recorder) = open(cx);
    click(cx, "row-sample-folders");
    for _ in 0..2 {
        desk.update(cx, |desk, cx| desk.stop_agent(HOLD_TASK.to_string(), cx));
    }
    draw(cx);
    assert_eq!(recorder.calls(), vec![format!("stop {HOLD_TASK}")]);
}

#[gpui::test]
fn removing_untaken_work_waits_for_the_hold(cx: &mut TestAppContext) {
    let (cx, desk, recorder) = open(cx);
    click(cx, "row-sample-accent");
    desk.update(cx, |desk, cx| desk.ask_remove(TASK, cx));
    draw(cx);
    assert!(recorder.calls().is_empty(), "three commits not taken");
    click(cx, "hold-remove-sample-accent");
    cx.executor().advance_clock(theme::HOLD * 2);
    cx.run_until_parked();
    assert!(recorder.calls().is_empty(), "a bare click never removes");
    let at = centre(cx, "hold-remove-sample-accent");
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.executor()
        .advance_clock(theme::HOLD + Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(recorder.calls(), vec![format!("rm {TASK} --force")]);
}

#[gpui::test]
fn annulla_puts_the_removal_away(cx: &mut TestAppContext) {
    let (cx, desk, recorder) = open(cx);
    click(cx, "row-sample-accent");
    desk.update(cx, |desk, cx| desk.ask_remove(TASK, cx));
    draw(cx);
    click(cx, "remove-cancel");
    assert!(cx.debug_bounds("hold-remove-sample-accent").is_none());
    assert!(recorder.calls().is_empty());
}

#[gpui::test]
fn the_task_menu_opens_from_the_header(cx: &mut TestAppContext) {
    let (cx, _, _) = open(cx);
    click(cx, "row-sample-accent");
    click(cx, "task-menu");
    cx.run_until_parked();
    draw(cx);
    let is_open = cx.update(|window, cx| window.focused(cx).is_some());
    assert!(is_open, "the open menu holds the keyboard");
}

#[gpui::test]
fn a_breath_tick_draws_neither_the_sidebar_nor_the_header(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    click(cx, "row-sample-folders");
    for _ in 0..20 {
        cx.run_until_parked();
        draw(cx);
        std::thread::sleep(WAIT_STEP);
    }
    let before = desk.read_with(cx, |desk, cx| desk.renders(cx));
    for _ in 0..10 {
        cx.executor().advance_clock(theme::DURATION_INSTANT);
        cx.run_until_parked();
        draw(cx);
    }
    let after = desk.read_with(cx, |desk, cx| desk.renders(cx));
    assert_eq!(after.0, before.0, "the sidebar was drawn again");
    assert_eq!(after.1, before.1, "the header was drawn again");
    assert!(desk.read_with(cx, |desk, cx| desk.is_breathing(cx)));
    let marks = cx.update(|_, cx| BreathMarks::count(cx));
    assert_eq!(
        marks, 2,
        "the sidebar row and the header of the task at work"
    );
}

#[gpui::test]
fn with_reduced_motion_nothing_ticks_and_the_glyph_is_drawn_still(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    click(cx, "row-sample-folders");
    for _ in 0..20 {
        cx.run_until_parked();
        draw(cx);
        std::thread::sleep(WAIT_STEP);
    }
    let before = desk.read_with(cx, |desk, cx| desk.renders(cx));
    for _ in 0..10 {
        cx.executor().advance_clock(theme::DURATION_INSTANT);
        cx.run_until_parked();
        draw(cx);
    }
    let after = desk.read_with(cx, |desk, cx| desk.renders(cx));
    assert_eq!(after, before, "nothing drew again");
    assert!(
        !desk.read_with(cx, |desk, cx| desk.is_breathing(cx)),
        "nothing ticks"
    );
    assert_eq!(cx.update(|_, cx| BreathMarks::count(cx)), 0);
}
