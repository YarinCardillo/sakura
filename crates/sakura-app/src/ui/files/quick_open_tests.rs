//! Quick open driven on GPUI's test platform: its key opens it with the caret in its field, what is
//! typed is asked of the source, Enter picks the first file and Esc leaves.

use super::quick_open::{self, QuickOpen, QuickOpenEvent, Target};
use super::tests::{draw, task, Fake};
use crate::theme::{Appearance, Theme};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    source: Rc<Fake>,
    /// The task and the path of every file picked.
    picked: Rc<RefCell<Vec<(String, String)>>>,
}

/// Where the tests' panel starts: a sidebar's width from the window's left edge.
const PANEL_LEFT: gpui::Pixels = gpui::px(240.);

/// `source` as the files of the task `task`.
fn target(task: &str, source: &Rc<Fake>) -> Target {
    Target {
        task: task.to_string(),
        source: source.clone(),
        panel_left: PANEL_LEFT,
    }
}

/// A window whose only view is quick open over the files of the tests' task, with its key bound.
fn open(cx: &mut TestAppContext) -> Opened<'_> {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let source = task();
    let files = target("sort-folders", &source);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let chosen = picked.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        cx.subscribe(&quick_open, move |_, _, event: &QuickOpenEvent, _| {
            let QuickOpenEvent::Picked { task, path } = event;
            chosen.borrow_mut().push((task.clone(), path.clone()));
        })
        .detach();
        quick_open::install(&quick_open, move |_| Some(files.clone()), window, cx);
        gpui_component::Root::new(quick_open, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened { cx, source, picked }
}

#[gpui::test]
fn the_key_opens_it_and_what_is_typed_is_asked_of_box(cx: &mut TestAppContext) {
    let opened = open(cx);
    assert!(opened.cx.debug_bounds("quick-open-panel").is_none());
    opened.cx.simulate_keystrokes("cmd-p");
    draw(opened.cx);
    assert!(opened.cx.debug_bounds("quick-open-panel").is_some());
    assert!(
        opened.source.read().is_empty(),
        "nothing typed, nothing asked"
    );
    opened.cx.simulate_input("lib");
    draw(opened.cx);
    let asked = opened.source.read();
    assert_eq!(asked.last().map(String::as_str), Some("search lib"));
    let is_typed = |call: &String| "search lib".starts_with(call.as_str());
    assert!(asked.iter().all(is_typed), "only what was typed: {asked:?}");
}

#[gpui::test]
fn it_stands_at_the_centre_of_the_panel_not_of_the_window(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened.cx.simulate_keystrokes("cmd-p");
    draw(opened.cx);
    let panel = opened.cx.debug_bounds("quick-open-panel").expect("it");
    let window = opened.cx.update(|window, _| window.viewport_size());
    let across = (PANEL_LEFT + window.width) / 2.;
    assert!(
        (panel.center().x - across).abs() <= gpui::px(1.),
        "across: {:?}, the panel's centre is {across:?}",
        panel.center().x
    );
}

#[gpui::test]
fn enter_picks_the_first_file_found_and_esc_leaves_without_one(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened.cx.simulate_keystrokes("cmd-p");
    draw(opened.cx);
    opened.cx.simulate_input("lib");
    draw(opened.cx);
    opened.cx.simulate_keystrokes("enter");
    draw(opened.cx);
    let picked = ("sort-folders".to_string(), "src/lib.rs".to_string());
    assert_eq!(*opened.picked.borrow(), [picked]);
    assert!(
        opened.cx.debug_bounds("quick-open-panel").is_none(),
        "it left"
    );
    opened.cx.simulate_keystrokes("cmd-p");
    draw(opened.cx);
    opened.cx.simulate_keystrokes("escape");
    draw(opened.cx);
    assert!(
        opened.cx.debug_bounds("quick-open-panel").is_none(),
        "Esc closes it"
    );
    assert_eq!(opened.picked.borrow().len(), 1);
}

#[gpui::test]
fn an_answer_for_the_task_it_was_closed_on_never_shows_for_another(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let (first, second) = (task(), task());
    first.set_file("first/query.ts", "");
    second.set_file("second/query.ts", "");
    let on_screen = Rc::new(RefCell::new(target("first", &first)));
    let (shown, picked) = (on_screen.clone(), Rc::new(RefCell::new(Vec::new())));
    let chosen = picked.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let quick_open = cx.new(|cx| QuickOpen::new(window, cx));
        cx.subscribe(&quick_open, move |_, _, event: &QuickOpenEvent, _| {
            let QuickOpenEvent::Picked { task, path } = event;
            chosen.borrow_mut().push((task.clone(), path.clone()));
        })
        .detach();
        quick_open::install(
            &quick_open,
            move |_| Some(shown.borrow().clone()),
            window,
            cx,
        );
        gpui_component::Root::new(quick_open, window, cx)
    });
    cx.update(|window, _| window.activate_window());
    first.is_holding.set(true);
    cx.simulate_keystrokes("cmd-p");
    draw(cx);
    cx.simulate_input("query");
    draw(cx);
    assert_eq!(first.read().first().map(String::as_str), Some("search q"));
    cx.simulate_keystrokes("cmd-p");
    draw(cx);
    *on_screen.borrow_mut() = target("second", &second);
    second.is_holding.set(true);
    cx.simulate_keystrokes("cmd-p");
    draw(cx);
    cx.simulate_input("query");
    draw(cx);
    assert_eq!(
        second.read(),
        ["search q"],
        "the other task is asked, one search at a time"
    );
    first.release();
    draw(cx);
    assert!(
        cx.debug_bounds("quick-open-file-first/query.ts").is_none(),
        "not this task's file"
    );
    while second.is_waiting() {
        second.release();
        draw(cx);
    }
    assert!(cx.debug_bounds("quick-open-file-second/query.ts").is_some());
    cx.simulate_keystrokes("enter");
    draw(cx);
    let picked_file = ("second".to_string(), "second/query.ts".to_string());
    assert_eq!(
        *picked.borrow(),
        [picked_file],
        "the event names the task it was opened for"
    );
}
