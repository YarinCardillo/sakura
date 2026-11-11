//! The commit graph driven on GPUI's test platform over a reader that counts its calls: the rows on
//! the grid in both languages at the widths the right pane has, only the rows on screen drawn, a press
//! that names its commit, and a graph that is read when asked and never by itself.

use super::{GitGraph, GitGraphEvent, LaneMark, ReadGraph};
use crate::theme::{self, Appearance, Theme};
use crate::ui::real_text;
use gpui::{
    div, px, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    Modifiers, ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext,
    Window,
};
use sakura_core::error::BridgeError;
use sakura_core::graph::{Commit, Graph, Lane};
use sakura_core::said::Said;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The pane's widths in the desk: at first launch, and the narrowest it gets.
const WIDTHS: [f32; 2] = [451., 212.];
/// Commits on the base of the long graph: more than a pane shows.
const MANY: usize = 500;
const NEAR: f32 = 0.5;

fn lane(task: Option<&str>, head: &str) -> Lane {
    Lane {
        lavoro: task.map(str::to_string),
        branch: task.map_or("main".into(), |task| format!("sakura/{task}")),
        place: task.map(|_| "mac".to_string()),
        head: Some(head.to_string()),
        base: None,
        readable: true,
    }
}

fn commit(sha: &str, parent: Option<&str>, task: Option<&str>) -> Commit {
    Commit {
        sha: sha.to_string(),
        parents: parent.into_iter().map(str::to_string).collect(),
        subject: format!("A subject long enough to be cut where the row ends, for commit {sha}"),
        time: "2026-11-10T16:00:00Z".to_string(),
        lavoro: task.map(str::to_string),
        branches: Vec::new(),
    }
}

/// A base of `count` commits with one task of two commits leaving its newest.
fn graph(count: usize) -> Graph {
    let base = |index: usize| format!("b{index:04}");
    let mut commits = vec![
        commit("t2", Some("t1"), Some("sort-folders")),
        commit("t1", Some(&base(0)), Some("sort-folders")),
    ];
    for index in 0..count {
        let parent = (index + 1 < count).then(|| base(index + 1));
        commits.push(commit(&base(index), parent.as_deref(), None));
    }
    let mut unreadable = lane(Some("broken-check"), "none");
    (unreadable.head, unreadable.readable) = (None, false);
    Graph {
        project: "app".into(),
        base: "main".into(),
        lanes: vec![
            lane(None, &base(0)),
            lane(Some("sort-folders"), "t2"),
            unreadable,
        ],
        commits,
    }
}

/// The graph in a column as wide as the desk's right pane.
struct Host {
    graph: Entity<GitGraph>,
    width: Pixels,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "graph-pane".into())
            .w(self.width)
            .h_full()
            .child(self.graph.clone())
    }
}

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    host: Entity<Host>,
    graph: Entity<GitGraph>,
    reads: Rc<Cell<usize>>,
    is_failing: Rc<Cell<bool>>,
    events: Rc<RefCell<Vec<GitGraphEvent>>>,
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open(cx: &mut TestAppContext, count: usize) -> Opened<'_> {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let (reads, is_failing) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(false)));
    let (counted, failing) = (reads.clone(), is_failing.clone());
    let read: ReadGraph = Rc::new(move || {
        counted.set(counted.get() + 1);
        let answer = match failing.get() {
            true => Err(BridgeError::failed(Said::new("error.internal"))),
            false => Ok(graph(count)),
        };
        Box::pin(std::future::ready(answer))
    });
    let mark: LaneMark = Rc::new(|_, _| Some(div().size(theme::MARK_SIZE).into_any_element()));
    let events = Rc::new(RefCell::new(Vec::new()));
    let noted = events.clone();
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|cx| {
            let graph = cx.new(|cx| GitGraph::new(read, mark, cx));
            cx.subscribe(&graph, move |_, _, event: &GitGraphEvent, _| {
                noted.borrow_mut().push(event.clone());
            })
            .detach();
            Host {
                graph,
                width: px(WIDTHS[0]),
            }
        });
        *kept.borrow_mut() = Some(host.clone());
        gpui_component::Root::new(host, window, cx)
    });
    let host: Entity<Host> = slot.borrow_mut().take().expect("the host was built");
    let graph = host.read_with(cx, |host, _| host.graph.clone());
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened {
        cx,
        host,
        graph,
        reads,
        is_failing,
        events,
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"))
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let centre = bounds(cx, selector).center();
    cx.simulate_click(centre, Modifiers::none());
    draw(cx);
}

fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    let apart = (f32::from(found) - f32::from(wanted)).abs();
    assert!(apart <= NEAR, "{what}: {found:?}, wanted {wanted:?}");
}

#[gpui::test]
fn a_press_on_a_task_s_commit_names_its_sha_and_one_on_the_base_s_does_nothing(
    cx: &mut TestAppContext,
) {
    let opened = open(cx, 3);
    click(opened.cx, "graph-row-t1");
    // `box` gives the diff of a task's own commits only.
    click(opened.cx, "graph-row-b0000");
    let picked = |sha: &str| GitGraphEvent::CommitPicked(sha.to_string());
    assert_eq!(*opened.events.borrow(), [picked("t1")]);
}

#[gpui::test]
fn only_the_rows_on_screen_are_drawn(cx: &mut TestAppContext) {
    let opened = open(cx, MANY);
    assert!(opened.cx.debug_bounds("graph-row-t2").is_some());
    assert!(opened.cx.debug_bounds("graph-row-b0000").is_some());
    assert!(
        opened.cx.debug_bounds("graph-row-b0499").is_none(),
        "far below"
    );
}

#[gpui::test]
fn the_graph_is_read_once_and_again_only_when_asked(cx: &mut TestAppContext) {
    let opened = open(cx, 3);
    assert_eq!(opened.reads.get(), 1);
    for _ in 0..5 {
        opened
            .cx
            .executor()
            .advance_clock(std::time::Duration::from_secs(60));
        draw(opened.cx);
    }
    assert_eq!(
        opened.reads.get(),
        1,
        "nothing is read while nothing changes"
    );
    opened
        .graph
        .update(opened.cx, |graph, cx| graph.refresh(cx));
    draw(opened.cx);
    assert_eq!(opened.reads.get(), 2);
}

#[gpui::test]
fn a_graph_that_cannot_be_read_says_so_and_a_shown_one_stays(cx: &mut TestAppContext) {
    let opened = open(cx, 3);
    opened.is_failing.set(true);
    opened
        .graph
        .update(opened.cx, |graph, cx| graph.refresh(cx));
    draw(opened.cx);
    assert!(
        opened.cx.debug_bounds("graph-row-t1").is_some(),
        "the one read before"
    );
    assert!(opened.cx.debug_bounds("graph-retry").is_none());
}

#[gpui::test]
fn a_first_read_that_fails_offers_to_try_again(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let attempts = Rc::new(Cell::new(0));
    let counted = attempts.clone();
    let read: ReadGraph = Rc::new(move || {
        counted.set(counted.get() + 1);
        let answer = match counted.get() {
            1 => Err(BridgeError::failed(Said::new("error.internal"))),
            _ => Ok(graph(2)),
        };
        Box::pin(std::future::ready(answer))
    });
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let graph = cx.new(|cx| GitGraph::new(read, Rc::new(|_, _| None), cx));
        gpui_component::Root::new(graph, window, cx)
    });
    draw(cx);
    assert!(cx.debug_bounds("graph-row-t1").is_none());
    click(cx, "graph-retry");
    assert_eq!(attempts.get(), 2);
    assert!(cx.debug_bounds("graph-row-t1").is_some());
}

#[test]
fn rows_stand_on_the_grid_in_both_languages_at_both_widths() {
    real_text::run(|cx| {
        let opened = open(cx, 3);
        for width in WIDTHS {
            opened.host.update(opened.cx, |host, cx| {
                host.width = px(width);
                cx.notify();
            });
            draw(opened.cx);
            let pane = bounds(opened.cx, "graph-pane");
            let inset = theme::SIDE_PANE_INSET - theme::ROW_PADDING_X;
            let (first, second) = (
                bounds(opened.cx, "graph-row-t2"),
                bounds(opened.cx, "graph-row-t1"),
            );
            assert_eq!(first.size.height, theme::ROW_HEIGHT, "one row pitch");
            assert_near(
                second.top() - first.top(),
                theme::ROW_HEIGHT,
                "row under row",
            );
            assert_near(
                first.left(),
                pane.left() + inset,
                "a row's fill starts before the inset",
            );
            assert_near(first.right(), pane.right() - inset, "and ends past it");
            let note = bounds(opened.cx, "graph-unreadable-broken-check");
            assert_eq!(note.size.height, theme::ROW_HEIGHT, "the note is one row");
            assert_near(first.top(), note.bottom(), "the rows start under the note");
            let subject = bounds(opened.cx, "graph-subject-t2");
            assert!(
                subject.size.width >= theme::ICON_SIZE * 4.,
                "the subject keeps its room"
            );
            assert!(subject.right() <= first.right(), "and stays inside the row");
            let (age, end) = (
                bounds(opened.cx, "graph-age-t2"),
                bounds(opened.cx, "graph-end-sort-folders"),
            );
            let chip = bounds(opened.cx, "graph-chip-sort-folders");
            assert!(
                chip.right() <= end.right() + px(NEAR),
                "a lane's name is cut inside its end, never drawn past it: {chip:?} in {end:?}"
            );
            let is_wide = width == WIDTHS[0];
            if is_wide {
                assert_near(
                    age.right(),
                    pane.right() - theme::SIDE_PANE_INSET,
                    "the age ends on the inset",
                );
                assert!(
                    end.right() <= age.left(),
                    "the lane's end stands before the age"
                );
                assert!(
                    subject.right() <= end.left(),
                    "the subject before the lane's end"
                );
            } else {
                assert!(
                    age.top() >= first.bottom(),
                    "a narrow row lets go of the age"
                );
            }
        }
    });
}

#[gpui::test]
fn commits_over_what_a_graph_holds_are_said_unreadable_and_none_is_drawn(cx: &mut TestAppContext) {
    let opened = open(cx, 10_001);
    assert!(
        opened.cx.debug_bounds("graph-row-t2").is_none(),
        "no part of it"
    );
    assert!(
        opened.cx.debug_bounds("graph-retry").is_some(),
        "the unreadable state"
    );
}

#[gpui::test]
fn a_history_with_a_cycle_is_said_unreadable_and_not_shown_as_empty(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let read: ReadGraph = Rc::new(|| {
        let mut cyclic = graph(1);
        cyclic.commits = vec![commit("a", Some("b"), None), commit("b", Some("a"), None)];
        Box::pin(std::future::ready(Ok(cyclic)))
    });
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let graph = cx.new(|cx| GitGraph::new(read, Rc::new(|_, _| None), cx));
        gpui_component::Root::new(graph, window, cx)
    });
    draw(cx);
    assert!(
        cx.debug_bounds("graph-retry").is_some(),
        "the unreadable state"
    );
    assert!(cx.debug_bounds("graph-row-a").is_none());
}
