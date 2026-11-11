//! Selection never moves anything: selecting, hovering, focusing or pressing changes fills, colours and
//! words, never the size or the position of an element. Checked on the bounds of every element of the
//! header and of the sidebar before and after.

use super::conversation_tests::{ask_a_question, QUESTION};
use super::grid_tests::bounds;
use super::interaction_tests::{centre, click, draw, open_with, wait_for};
use super::launch_tests::{fake_agentbox, open_live, Names};
use super::real_text;
use crate::data::actions::ActionKey;
use crate::data::changes::Scope;
use crate::theme;
use gpui::{Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext};

const TASK: &str = "row-sample-accent";
const TABS: [&str; 4] = [
    "pane-view-files",
    "pane-view-git",
    "pane-view-checks",
    "pane-view-changes",
];
/// Every element of the header of the fixture's task, and the right pane's row of views.
const HEADER: [&str; 16] = [
    "page-header",
    "header-glyph",
    "centre-views",
    "centre-view-agent",
    "header-branch",
    "header-state",
    "pane-views",
    "pane-view-files",
    "pane-view-changes",
    "pane-view-checks",
    "pane-view-git",
    "pane-count-changes",
    "header-figures",
    "take",
    "task-menu",
    "task-menu-icon",
];
/// Every element of the sidebar's rows and labels (but the trailing figures: the age of old facts joins
/// them with time, not with a selection).
const SIDEBAR: [&str; 13] = [
    "group-review",
    "row-sample-folders",
    "glyph-sample-folders",
    "label-sample-folders",
    "row-sample-accent",
    "glyph-sample-accent",
    "label-sample-accent",
    "row-headings-26",
    "glyph-headings-26",
    "label-headings-26",
    "row-broken-check",
    "new-task",
    "new-task-icon",
];

/// Every element of the status bar.
const STATUS: [&str; 8] = [
    "status-bar",
    "toggle-sidebar",
    "toggle-sidebar-icon",
    "status-line",
    "status-terminal",
    "status-terminal-icon",
    "toggle-changes",
    "toggle-changes-icon",
];

fn open(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (cx, _, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| cx.debug_bounds(TASK).is_some());
    click(cx, TASK);
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    cx
}

/// The bounds of each of `selectors`. A row's name ends where its trailing figures start, and those grow
/// with the age of the facts, whatever the pointer does: of a name only the place and the height count.
fn all(cx: &mut VisualTestContext, selectors: &[&'static str]) -> Vec<Bounds<Pixels>> {
    selectors
        .iter()
        .map(|selector| {
            let mut bounds = bounds(cx, selector);
            if selector.starts_with("label-") {
                bounds.size.width = Pixels::ZERO;
            }
            bounds
        })
        .collect()
}

#[track_caller]
fn assert_unmoved(
    before: &[Bounds<Pixels>],
    after: &[Bounds<Pixels>],
    selectors: &[&'static str],
    what: &str,
) {
    for ((before, after), selector) in before.iter().zip(after).zip(selectors) {
        assert_eq!(before, after, "{selector} moved or changed size {what}");
    }
}

#[test]
fn selecting_a_view_tab_moves_nothing_in_the_header() {
    real_text::run(|cx| {
        let cx = open(cx);
        let before = all(cx, &HEADER);
        for tab in TABS {
            click(cx, tab);
            let after = all(cx, &HEADER);
            assert_unmoved(&before, &after, &HEADER, &format!("after selecting {tab}"));
        }
    });
}

#[test]
fn selecting_a_sidebar_row_moves_nothing_in_the_sidebar_and_the_header_comes_back_the_same() {
    real_text::run(|cx| {
        let cx = open(cx);
        let (sidebar, header) = (all(cx, &SIDEBAR), all(cx, &HEADER));
        for row in [
            "row-sample-folders",
            "row-headings-26",
            "row-broken-check",
            TASK,
        ] {
            click(cx, row);
            let after = all(cx, &SIDEBAR);
            assert_unmoved(
                &sidebar,
                &after,
                &SIDEBAR,
                &format!("after selecting {row}"),
            );
        }
        let after = all(cx, &HEADER);
        assert_unmoved(&header, &after, &HEADER, "after going round the rows");
    });
}

#[test]
fn a_row_s_name_has_the_same_weight_selected_and_not() {
    real_text::run(|cx| {
        let cx = open(cx);
        // The fixture's task is the one selected; another row is not.
        let (was_selected, was_not) = ("name-sample-accent", "name-headings-26");
        let before = (bounds(cx, was_selected).size, bounds(cx, was_not).size);
        click(cx, "row-headings-26");
        let after = (bounds(cx, was_selected).size, bounds(cx, was_not).size);
        assert_eq!(
            before, after,
            "a name is as wide selected as not: selection is the fill, never a heavier word"
        );
    });
}

#[test]
fn hovering_and_pressing_move_nothing() {
    real_text::run(|cx| {
        let cx = open(cx);
        let selectors: Vec<&'static str> = HEADER
            .iter()
            .chain(&SIDEBAR)
            .chain(&STATUS)
            .copied()
            .collect();
        let before = all(cx, &selectors);
        let targets = [
            "pane-view-files",
            "pane-view-changes",
            "pane-view-checks",
            "pane-view-git",
            "take",
            "task-menu",
            "row-headings-26",
            "new-task",
            "toggle-sidebar",
            "toggle-changes",
        ];
        for target in targets {
            let at = centre(cx, target);
            cx.simulate_mouse_move(at, None, Modifiers::none());
            draw(cx);
            let hovered = all(cx, &selectors);
            assert_unmoved(&before, &hovered, &selectors, &format!("under {target}"));
            cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
            draw(cx);
            let pressed = all(cx, &selectors);
            assert_unmoved(&before, &pressed, &selectors, &format!("pressing {target}"));
            // The press is let go outside the window's content, so it is no click.
            cx.simulate_mouse_up(
                gpui::point(at.x, gpui::px(-10.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw(cx);
        }
    });
}

#[test]
fn hiding_a_pane_and_bringing_it_back_moves_nothing_in_the_status_bar() {
    real_text::run(|cx| {
        let cx = open(cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        let before = all(cx, &STATUS);
        for toggle in ["toggle-sidebar", "toggle-changes"] {
            click(cx, toggle);
            let hidden = all(cx, &STATUS);
            assert_unmoved(
                &before,
                &hidden,
                &STATUS,
                &format!("after {toggle} hid its pane"),
            );
            click(cx, toggle);
            let shown = all(cx, &STATUS);
            assert_unmoved(
                &before,
                &shown,
                &STATUS,
                &format!("after {toggle} brought it back"),
            );
        }
    });
}

#[test]
fn showing_the_terminal_and_going_back_moves_nothing_in_the_status_bar_nor_the_rows_of_views() {
    real_text::run(|cx| {
        let cx = open(cx);
        let watched: Vec<&'static str> = STATUS
            .iter()
            .copied()
            // The switch grows by the terminal's tab, at its end: nothing before or after it moves.
            .chain([
                "page-header",
                "centre-view-agent",
                "header-branch",
                "pane-views",
            ])
            .collect();
        let before = all(cx, &watched);
        // The item opens the lavoro's terminal and takes the fill of a chosen view.
        click(cx, "status-terminal");
        wait_for(cx, "the terminal's view", |cx| {
            cx.debug_bounds("centre-view-terminal").is_some()
        });
        let on_terminal = all(cx, &watched);
        assert_unmoved(&before, &on_terminal, &watched, "with the terminal shown");
        click(cx, "status-terminal");
        let back = all(cx, &watched);
        assert_unmoved(&before, &back, &watched, "back on the agent");
    });
}

/// The lavoro of the fixture whose agent runs.
const RUNNING_TASK: &str = "sample-folders";
/// A `box` whose `stop` takes five seconds: a press on "Stop" really starts the action, and its pending
/// word stays on screen meanwhile.
const STOPPING_BOX: &str = "  stop) sleep 5 ;;";

#[test]
fn a_press_that_changes_a_buttons_word_changes_no_size() {
    real_text::run(|cx| {
        let (_dir, script) = fake_agentbox(Names::Listed, "0", STOPPING_BOX);
        let (cx, desk, _) = open_live(cx, script, Some(RUNNING_TASK), false);
        wait_for(cx, "the stop button of the header", |cx| {
            cx.debug_bounds("header-stop").is_some()
        });
        // This `box` has no changes to count on "Changes", and the work of this lavoro is not verified,
        // so the hold stands where "Take" would: every other element of the header, and the button.
        let header = HEADER
            .iter()
            .copied()
            .filter(|name| !["pane-count-changes", "take"].contains(name));
        let watched: Vec<&'static str> = header.chain(["header-stop"]).collect();
        let before = all(cx, &watched);
        click(cx, "header-stop");
        let is_stopping = desk.read_with(cx, |desk, cx| {
            let key = ActionKey::stop_agent(RUNNING_TASK);
            desk.actions.read(cx).is_running(&key)
        });
        assert!(
            is_stopping,
            "the press asked to stop the agent: the pending word is on screen"
        );
        let after = all(cx, &watched);
        assert_unmoved(&before, &after, &watched, "while the agent stops");
    });
}

#[test]
fn hovering_pressing_and_writing_in_the_conversation_move_nothing() {
    real_text::run(|cx| {
        let (cx, desk, _) = open_with(cx, None);
        wait_for(cx, "the rows", |cx| cx.debug_bounds(TASK).is_some());
        click(cx, TASK);
        wait_for(cx, "the conversation", |cx| {
            cx.debug_bounds("thread-composer-box").is_some()
        });
        ask_a_question(cx, &desk, "sample-accent");
        let parts = [
            "thread-prompt",
            "thread-composer-box",
            "thread-composer",
            "thread-send",
        ];
        let watched: Vec<&'static str> = parts.iter().chain(&QUESTION).copied().collect();
        let before = all(cx, &watched);
        // A press is held and let go off the control, so nothing is answered or sent.
        let away = centre(cx, "thread-prompt");
        for target in QUESTION[1..].iter().copied().chain(["thread-send"]) {
            let at = centre(cx, target);
            cx.simulate_mouse_move(at, None, Modifiers::none());
            draw(cx);
            let hovered = all(cx, &watched);
            assert_unmoved(&before, &hovered, &watched, &format!("under {target}"));
            cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
            draw(cx);
            let pressed = all(cx, &watched);
            assert_unmoved(&before, &pressed, &watched, &format!("pressing {target}"));
            cx.simulate_mouse_move(away, Some(MouseButton::Left), Modifiers::none());
            cx.simulate_mouse_up(away, MouseButton::Left, Modifiers::none());
            draw(cx);
        }
        click(cx, "thread-composer");
        let focused = all(cx, &watched);
        assert_unmoved(&before, &focused, &watched, "with the composer focused");
        cx.simulate_input("poi");
        draw(cx);
        let written = all(cx, &watched);
        assert_unmoved(&before, &written, &watched, "with words in the composer");
    });
}

#[test]
fn moving_the_keyboard_focus_moves_nothing() {
    real_text::run(|cx| {
        let cx = open(cx);
        let selectors: Vec<&'static str> = HEADER.iter().chain(&SIDEBAR).copied().collect();
        let before = all(cx, &selectors);
        for _ in 0..6 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            let after = all(cx, &selectors);
            assert_unmoved(&before, &after, &selectors, "with the focus moved");
        }
    });
}

#[test]
fn a_count_keeps_the_room_of_two_figures_so_one_more_digit_moves_nothing() {
    real_text::run(|cx| {
        let (cx, desk, _) = open_with(cx, None);
        wait_for(cx, "the rows", |cx| cx.debug_bounds(TASK).is_some());
        click(cx, TASK);
        wait_for(cx, "the whole patch", |cx| {
            cx.debug_bounds("changes-header-9").is_some()
        });
        let ten = bounds(cx, "pane-count-changes");
        let after_ten = bounds(cx, "pane-view-checks").left() - ten.right();
        // One commit's patch is not the count of the task's files: the figure is the nine `box` counted.
        let changes = desk.read_with(cx, |desk, _| desk.changes.clone());
        cx.update(|_, cx| {
            changes.update(cx, |changes, cx| {
                let scope = Scope::Commit("not-a-commit".into());
                changes.set_scope("sample-accent", scope, cx)
            });
            desk.update(cx, |_, cx| cx.notify());
        });
        draw(cx);
        let nine = bounds(cx, "pane-count-changes");
        assert_eq!(nine.size.width, ten.size.width, "9 and 10");
        assert!(nine.size.width >= theme::TAB_COUNT_WIDTH);
        let after_nine = bounds(cx, "pane-view-checks").left() - nine.right();
        assert_eq!(
            after_ten, after_nine,
            "the next tab is as far from the count"
        );
    });
}
