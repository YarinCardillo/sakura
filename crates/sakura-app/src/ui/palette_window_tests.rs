//! The command palette in the desk's window, on the sample data: Cmd+K lists the commands for where the
//! window is, and a command chosen is carried out on the lavoro it was listed for.

use super::desk::Desk;
use super::interaction_tests::{click, draw, open_with, pane_view, wait_for};
use super::right_pane::PaneView;
use crate::data::source::Recorder;
use crate::l10n::{self, Language};
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;

/// A lavoro of the sample data with commits not taken, and one whose work was taken.
const UNTAKEN: &str = "sample-accent";
const TAKEN: &str = "headings-26";

/// The desk on the sample data, in English, on the lavoro with commits not taken.
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>, Arc<Recorder>) {
    l10n::set_for_test(Language::English);
    let (cx, desk, recorder) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    click(cx, "row-sample-accent");
    (cx, desk, recorder)
}

fn press(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    draw(cx);
}

/// Opens the palette by its key, types `words` and chooses the command they leave first.
fn choose(cx: &mut VisualTestContext, words: &str) {
    press(cx, "cmd-k");
    cx.simulate_input(words);
    draw(cx);
    press(cx, "enter");
}

fn is_open(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> bool {
    desk.read_with(cx, |desk, cx| desk.palette.read(cx).is_open())
}

#[gpui::test]
fn cmd_k_opens_the_palette_with_the_caret_and_a_lavoro_chosen_is_shown(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    assert!(!is_open(cx, &desk));
    press(cx, "cmd-k");
    assert!(is_open(cx, &desk));
    assert!(cx.debug_bounds("palette-panel").is_some());
    cx.simulate_input("headings");
    draw(cx);
    let typed = desk.read_with(cx, |desk, cx| desk.palette.read(cx).query(cx));
    assert_eq!(
        typed.as_ref(),
        "headings",
        "typing goes to its field without a click"
    );
    press(cx, "enter");
    assert!(!is_open(cx, &desk));
    let shown = desk.read_with(cx, |desk, cx| desk.shown(cx));
    assert_eq!(shown.as_deref(), Some(TAKEN));
}

#[gpui::test]
fn a_view_chosen_in_the_palette_is_shown_in_the_right_pane_of_that_lavoro(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    desk.update(cx, |desk, cx| desk.toggle_changes(cx));
    draw(cx);
    choose(cx, "Show Git");
    assert_eq!(pane_view(cx, &desk, UNTAKEN), PaneView::Git);
    assert_eq!(
        pane_view(cx, &desk, TAKEN),
        PaneView::default(),
        "only the one shown"
    );
    let panes = desk.read_with(cx, |desk, _| desk.panes_shown());
    assert!(
        panes.changes,
        "a pane that was hidden comes back to show it"
    );
}

#[gpui::test]
fn archive_in_the_palette_asks_for_the_hold_when_work_would_be_lost(cx: &mut TestAppContext) {
    let (cx, _, recorder) = open(cx);
    choose(cx, "Archive");
    assert!(recorder.calls().is_empty(), "three commits not taken");
    assert!(cx.debug_bounds("hold-remove-sample-accent").is_some());
    click(cx, "remove-cancel");

    click(cx, "row-headings-26");
    choose(cx, "Archive");
    cx.run_until_parked();
    assert_eq!(
        recorder.calls(),
        vec![format!("rm {TAKEN}")],
        "nothing to lose"
    );
}

#[gpui::test]
fn the_palette_lists_what_the_window_offers_now(cx: &mut TestAppContext) {
    let (cx, desk, _) = open(cx);
    let situation = desk.read_with(cx, |desk, cx| desk.situation(cx));
    let shown = situation.shown.expect("a lavoro is shown");
    assert_eq!(shown.name, UNTAKEN);
    assert!(!shown.can_publish, "in a box and not taken");
    assert!(shown.can_archive);
    assert!(situation.tasks.iter().any(|task| task == TAKEN));
    assert!(situation.panes.sidebar && situation.panes.changes);

    click(cx, "row-headings-26");
    let situation = desk.read_with(cx, |desk, cx| desk.situation(cx));
    let shown = situation.shown.expect("a lavoro is shown");
    assert!(shown.can_publish, "its head was taken");
    assert!(
        !situation.can_close_tab,
        "the lavoro chosen took the one tab"
    );
}
