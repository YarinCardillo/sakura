//! Play driven on GPUI's test platform over a source that notes what is asked: the control that
//! starts and stops, the port that asks for the browser, the log read only when it should be, the
//! answers that come late, and where things stand on the layout grid in both languages.

use super::source::PlaySource;
use super::{header_control, Play, PlayEvent};
use crate::theme::{self, Appearance, Theme};
use crate::ui::files::Reading;
use crate::ui::real_text;
use gpui::{
    div, px, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext,
    Window,
};
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::play::{self, Action, Availability, Log, Outcome, Run};
use sakura_core::said::Said;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// The pane's width in the desk at first launch, and the window's height.
const WIDTH: f32 = 451.;
const WINDOW_HEIGHT: f32 = 700.;
/// How far two edges may stand apart and still be on one line.
const NEAR: f32 = 0.5;

/// A source that answers from memory and notes what is asked: `start`, `stop`, `status`, `log`.
#[derive(Default)]
struct Fake {
    asked: RefCell<Vec<&'static str>>,
    is_running: Cell<bool>,
    was_started: Cell<bool>,
    /// A start fails, as when the command ends at once.
    is_failing: Cell<bool>,
    /// `box` says the task is in a box.
    is_in_box: Cell<bool>,
    log: RefCell<String>,
    /// While set, an answer waits until the test lets it go.
    is_holding: Cell<bool>,
    held: RefCell<Vec<Box<dyn FnOnce()>>>,
}

impl Fake {
    fn answer<T: Send + 'static>(&self, answer: BridgeResult<T>) -> Reading<T> {
        if !self.is_holding.get() {
            return Box::pin(std::future::ready(answer));
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        self.held
            .borrow_mut()
            .push(Box::new(move || drop(send.send(answer))));
        Box::pin(async move { receive.await.unwrap_or_else(|_| Err(failed())) })
    }

    /// Lets the answer that has waited longest go.
    fn release(&self) {
        let let_go = self.held.borrow_mut().remove(0);
        let_go();
    }

    /// Lets the answer requested last go, before the ones requested earlier.
    fn release_newest(&self) {
        let let_go = self.held.borrow_mut().pop().expect("an answer waits");
        let_go();
    }

    fn count(&self, what: &str) -> usize {
        self.asked
            .borrow()
            .iter()
            .filter(|asked| **asked == what)
            .count()
    }

    fn state(&self) -> play::Play {
        let run = match self.is_running.get() {
            true => Run::Running {
                pid: 4711,
                since: None,
            },
            false => Run::Stopped,
        };
        let ports = if self.was_started.get() {
            vec![56000, 56001]
        } else {
            Vec::new()
        };
        play::Play { ports, run }
    }
}

fn failed() -> BridgeError {
    BridgeError::failed(Said::new("error.internal"))
        .with_details(Some("box: it ended at once".into()))
}

impl PlaySource for Fake {
    fn play(&self, action: Action) -> Reading<Outcome> {
        let (word, runs) = match action {
            Action::Start => ("start", Some(true)),
            Action::Stop => ("stop", Some(false)),
            Action::Status => ("status", None),
        };
        self.asked.borrow_mut().push(word);
        if self.is_in_box.get() {
            return self.answer(Ok(Outcome::NotInBox));
        }
        if self.is_failing.get() && action == Action::Start {
            return self.answer(Err(failed()));
        }
        if let Some(runs) = runs {
            self.is_running.set(runs);
            self.was_started.set(self.was_started.get() || runs);
        }
        self.answer(Ok(Outcome::State(self.state())))
    }

    fn log(&self) -> Reading<Log> {
        self.asked.borrow_mut().push("log");
        self.answer(Ok(Log::of(&self.log.borrow())))
    }
}

/// The header's control over the "Play" view, in a pane as wide as the desk's right pane.
struct Host {
    play: Entity<Play>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .debug_selector(|| "header".into())
            .h(theme::HEADER_HEIGHT)
            .flex()
            .items_center()
            .justify_end()
            .child(header_control(&self.play, cx));
        div()
            .debug_selector(|| "pane".into())
            .w(px(WIDTH))
            .h(px(WINDOW_HEIGHT))
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(self.play.clone()))
    }
}

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    play: Entity<Play>,
    source: Rc<Fake>,
    events: Rc<RefCell<Vec<PlayEvent>>>,
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(cx: &mut TestAppContext, source: Rc<Fake>, availability: Availability) -> Opened<'_> {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let events: Rc<RefCell<Vec<PlayEvent>>> = Rc::default();
    let noted = events.clone();
    let asked: Rc<dyn PlaySource> = source.clone();
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|cx| {
            let play = cx.new(|cx| Play::new(asked, availability, window, cx));
            cx.subscribe(&play, move |_, _, event: &PlayEvent, _| {
                noted.borrow_mut().push(event.clone());
            })
            .detach();
            *kept.borrow_mut() = Some(play.clone());
            Host { play }
        });
        gpui_component::Root::new(host, window, cx)
    });
    let play: Entity<Play> = slot.borrow_mut().take().expect("Play was built");
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened {
        cx,
        play,
        source,
        events,
    }
}

fn ready(cx: &mut TestAppContext) -> Opened<'_> {
    open(cx, Rc::new(Fake::default()), Availability::Ready)
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn is_there(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let centre = bounds(cx, selector).center();
    cx.simulate_click(centre, Modifiers::none());
    draw(cx);
}

impl Opened<'_> {
    fn show(&mut self) {
        self.play.update(self.cx, |play, cx| play.shown(true, cx));
        draw(self.cx);
    }

    fn is_running(&mut self) -> bool {
        self.play.read_with(self.cx, |play, _| play.is_running())
    }

    fn log_text(&mut self) -> String {
        self.play
            .read_with(self.cx, |play, cx| play.text.read(cx).value().to_string())
    }

    fn rest(&mut self, minutes: u64) {
        for _ in 0..minutes {
            self.cx.executor().advance_clock(Duration::from_secs(60));
            self.cx.run_until_parked();
        }
    }
}

#[gpui::test]
fn box_is_asked_once_where_play_stands_and_never_for_a_task_that_cannot_play(
    cx: &mut TestAppContext,
) {
    let mut opened = ready(cx);
    assert_eq!(*opened.source.asked.borrow(), ["status"]);
    opened.rest(5);
    assert_eq!(
        opened.source.asked.borrow().len(),
        1,
        "nothing is asked at rest"
    );
}

#[gpui::test]
fn a_task_in_a_box_says_so_and_asks_nothing(cx: &mut TestAppContext) {
    let mut opened = open(cx, Rc::new(Fake::default()), Availability::InBox);
    opened.show();
    click(opened.cx, "play-control");
    opened.play.update(opened.cx, |play, cx| play.refresh(cx));
    draw(opened.cx);
    assert!(opened.source.asked.borrow().is_empty());
    assert!(is_there(opened.cx, "play-unavailable"));
    assert!(
        !is_there(opened.cx, "play-project-settings"),
        "a box has no run command to set"
    );
    assert!(!is_there(opened.cx, "play-refresh"));
}

#[gpui::test]
fn a_project_without_a_run_command_says_so_and_leads_to_its_settings(cx: &mut TestAppContext) {
    let mut opened = open(cx, Rc::new(Fake::default()), Availability::NoRunCommand);
    opened.show();
    click(opened.cx, "play-control");
    assert!(opened.source.asked.borrow().is_empty());
    click(opened.cx, "play-project-settings");
    assert_eq!(*opened.events.borrow(), [PlayEvent::ProjectSettingsAsked]);
    // The project got its command: Play is asked where it stands, once.
    opened.play.update(opened.cx, |play, cx| {
        play.set_availability(Availability::Ready, cx)
    });
    draw(opened.cx);
    assert_eq!(*opened.source.asked.borrow(), ["status"]);
    assert!(!is_there(opened.cx, "play-unavailable"));
}

#[gpui::test]
fn play_starts_shows_its_port_and_stops(cx: &mut TestAppContext) {
    let mut opened = ready(cx);
    assert!(!is_there(opened.cx, "play-port"));
    opened.source.is_holding.set(true);
    click(opened.cx, "play-control");
    // While the start is on its way a second press asks nothing more.
    click(opened.cx, "play-control");
    assert_eq!(opened.source.count("start"), 1);
    assert!(!opened.is_running());
    opened.source.is_holding.set(false);
    opened.source.release();
    draw(opened.cx);
    assert!(opened.is_running());
    click(opened.cx, "play-port");
    let open_port = PlayEvent::OpenUrl("http://localhost:56000".into());
    assert_eq!(*opened.events.borrow(), [PlayEvent::Changed, open_port]);
    click(opened.cx, "play-control");
    assert_eq!(opened.source.count("stop"), 1);
    assert!(!opened.is_running());
    assert!(
        !is_there(opened.cx, "play-port"),
        "a Play that does not run has no port to open"
    );
}

#[gpui::test]
fn the_log_is_read_when_it_is_shown_when_play_starts_or_stops_and_on_refresh_only(
    cx: &mut TestAppContext,
) {
    let mut opened = ready(cx);
    *opened.source.log.borrow_mut() = "ready on 56000\nGET / 200\n".into();
    assert_eq!(
        opened.source.count("log"),
        0,
        "a view that is not on screen reads no log"
    );
    opened.show();
    assert_eq!(opened.source.count("log"), 1);
    assert_eq!(opened.log_text(), "ready on 56000\nGET / 200");
    opened.show();
    assert_eq!(
        opened.source.count("log"),
        1,
        "shown again while it is shown: nothing is read"
    );
    click(opened.cx, "play-control");
    assert_eq!(opened.source.count("log"), 2, "a start writes to the log");
    click(opened.cx, "play-control");
    assert_eq!(opened.source.count("log"), 3, "so does a stop");
    click(opened.cx, "play-refresh");
    assert_eq!(
        (opened.source.count("log"), opened.source.count("status")),
        (4, 2)
    );
    opened.rest(5);
    assert_eq!(
        opened.source.asked.borrow().len(),
        8,
        "nothing is asked at rest"
    );
    // Off screen, a start reads no log.
    opened
        .play
        .update(opened.cx, |play, cx| play.shown(false, cx));
    click(opened.cx, "play-control");
    assert_eq!(opened.source.count("log"), 4);
}

#[gpui::test]
fn an_older_log_never_replaces_a_newer_one(cx: &mut TestAppContext) {
    let mut opened = ready(cx);
    opened.source.is_holding.set(true);
    *opened.source.log.borrow_mut() = "older\n".into();
    opened.show();
    *opened.source.log.borrow_mut() = "newer\n".into();
    opened.play.update(opened.cx, |play, cx| play.read_log(cx));
    opened.source.release_newest();
    draw(opened.cx);
    assert_eq!(opened.log_text(), "newer");
    opened.source.release();
    draw(opened.cx);
    assert_eq!(
        opened.log_text(),
        "newer",
        "the answer that came late is dropped"
    );
}

#[gpui::test]
fn a_start_that_fails_is_said_in_place_and_is_not_asked_again(cx: &mut TestAppContext) {
    let mut opened = ready(cx);
    opened.source.is_failing.set(true);
    click(opened.cx, "play-control");
    assert!(is_there(opened.cx, "play-failed"));
    assert!(!opened.is_running());
    opened.rest(5);
    assert_eq!(
        opened.source.count("start"),
        1,
        "nothing is asked again by itself"
    );
    // The person presses again: that is the retry, and what failed is no longer said.
    opened.source.is_failing.set(false);
    click(opened.cx, "play-control");
    assert!(opened.is_running());
    assert!(!is_there(opened.cx, "play-failed"));
}

#[gpui::test]
fn a_task_box_itself_finds_in_a_box_says_so(cx: &mut TestAppContext) {
    let source = Rc::new(Fake::default());
    source.is_in_box.set(true);
    let mut opened = open(cx, source, Availability::Ready);
    opened.show();
    assert!(is_there(opened.cx, "play-unavailable"));
    click(opened.cx, "play-control");
    assert_eq!(
        *opened.source.asked.borrow(),
        ["status"],
        "and nothing more is asked"
    );
}

#[gpui::test]
fn an_empty_log_and_a_cut_one_say_so(cx: &mut TestAppContext) {
    let mut opened = ready(cx);
    opened.show();
    assert!(is_there(opened.cx, "play-log-empty"));
    let long: String = (0..play::LOG_MAX_LINES + 10)
        .map(|n| format!("line {n}\n"))
        .collect();
    *opened.source.log.borrow_mut() = long;
    click(opened.cx, "play-refresh");
    assert!(is_there(opened.cx, "play-log"));
    let text = opened.log_text();
    assert_eq!(text.lines().count(), play::LOG_MAX_LINES);
    assert!(text.ends_with(&format!("line {}", play::LOG_MAX_LINES + 9)));
}

#[gpui::test]
fn a_log_that_has_the_keyboard_is_not_drawn_again_while_nothing_happens(cx: &mut TestAppContext) {
    let mut opened = ready(cx);
    *opened.source.log.borrow_mut() = "one\ntwo\nthree\n".into();
    opened.show();
    click(opened.cx, "play-log");
    let text = opened
        .play
        .read_with(opened.cx, |play, _| play.text.clone());
    let has_keyboard = text.read_with(opened.cx, |text, cx| {
        gpui::Focusable::focus_handle(text, cx)
    });
    assert!(
        opened
            .cx
            .update(|window, _| has_keyboard.is_focused(window)),
        "the press gave it the keyboard"
    );
    let asked = Rc::new(Cell::new(0));
    let (of_play, of_text, play) = (asked.clone(), asked.clone(), opened.play.clone());
    opened.cx.update(|_, cx| {
        cx.observe(&play, move |_, _| of_play.set(of_play.get() + 1))
            .detach();
        cx.observe(&text, move |_, _| of_text.set(of_text.get() + 1))
            .detach();
    });
    for _ in 0..20 {
        opened
            .cx
            .executor()
            .advance_clock(Duration::from_millis(500));
        opened.cx.run_until_parked();
    }
    assert_eq!(asked.get(), 0, "no caret timer asks for a frame at rest");
}

fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    let apart = (f32::from(found) - f32::from(wanted)).abs();
    assert!(apart <= NEAR, "{what}: {found:?}, wanted {wanted:?}");
}

#[test]
fn the_control_keeps_its_size_and_the_view_stands_on_the_pane_s_inset() {
    real_text::run(|cx| {
        let mut opened = ready(cx);
        *opened.source.log.borrow_mut() = "ready\n".into();
        opened.show();
        let (pane, at_rest) = (bounds(opened.cx, "pane"), bounds(opened.cx, "play-control"));
        assert_near(
            at_rest.size.height,
            theme::CONTROL_HEIGHT,
            "the control's height",
        );
        assert_near(at_rest.right(), pane.right(), "the control ends the header");
        let refresh = bounds(opened.cx, "play-refresh");
        assert_near(
            refresh.right(),
            pane.right() - theme::SIDE_PANE_INSET,
            "Refresh ends on the inset",
        );
        opened.source.is_holding.set(true);
        click(opened.cx, "play-control");
        assert_eq!(
            bounds(opened.cx, "play-control"),
            at_rest,
            "starting moves nothing"
        );
        opened.source.is_holding.set(false);
        opened.source.release();
        draw(opened.cx);
        assert_eq!(
            bounds(opened.cx, "play-control"),
            at_rest,
            "running moves nothing"
        );
        let port = bounds(opened.cx, "play-port");
        assert_near(
            at_rest.left() - port.right(),
            theme::GAP_WIDE,
            "the port stands beside the control",
        );
        let in_view = bounds(opened.cx, "play-view-port");
        assert_near(
            in_view.center().y,
            refresh.center().y,
            "the view's port is on the bar's line",
        );
    });
}
