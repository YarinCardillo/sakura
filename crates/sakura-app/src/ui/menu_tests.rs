//! The menus of the window, measured in a test window on a fake `box`: every item's label starts on one
//! line whether the item is checked or not, the items' highlights stand as far from the menu's edge on
//! every side, and a trigger is a control of the one height whose words sit on the grid.

use super::interaction_tests::{click, draw, wait_for};
use super::launch_tests::{fake_agentbox, open_live, Names, TASK};
use super::real_text;
use crate::theme;
use gpui::{point, px, size, Bounds, Modifiers, Pixels, Point, TestAppContext, VisualTestContext};

/// The fake `box` lists one commit ahead.
const LOG: &str = r#"  log) cat "$d/log.json" ;;"#;
const SHA: &str = "7924b73a5d7f2468ace013579bdf02468ace1357";
/// How far two lines that must coincide may be apart.
const HALF_A_POINT: f32 = 0.5;

/// Every filled box of the last frame, in points.
fn filled(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let in_points = |quad: &gpui::Quad| {
            let (origin, extent) = (quad.bounds.origin, quad.bounds.size);
            Bounds::new(
                point(px(origin.x.0 / scale), px(origin.y.0 / scale)),
                size(px(extent.width.0 / scale), px(extent.height.0 / scale)),
            )
        };
        let quads = window.painted_quads();
        let painted = quads
            .iter()
            .filter(|quad| !quad.background.is_transparent());
        painted.map(in_points).collect()
    })
}

/// The boxes of `now` that were not in `before`.
fn appeared(before: &[Bounds<Pixels>], now: Vec<Bounds<Pixels>>) -> Vec<Bounds<Pixels>> {
    let is_new = |quad: &Bounds<Pixels>| !before.contains(quad);
    now.into_iter().filter(is_new).collect()
}

fn area(quad: &Bounds<Pixels>) -> f32 {
    f32::from(quad.size.width) * f32::from(quad.size.height)
}

/// Presses the trigger `selector` and gives the frame of the menu that opens: the largest box the press
/// brought (the kit names no element of its menus for a test; what it paints is read instead).
fn open_menu(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    let before = filled(cx);
    click(cx, selector);
    let new = appeared(&before, filled(cx));
    let largest = new.into_iter().max_by(|a, b| area(a).total_cmp(&area(b)));
    largest.expect("the press opened a menu")
}

/// Moves the pointer to `at` and gives the highlight of the item under it: the smallest box the move
/// brought around the pointer.
fn item_at(cx: &mut VisualTestContext, at: Point<Pixels>) -> Bounds<Pixels> {
    let elsewhere = point(px(2.), px(2.));
    cx.simulate_mouse_move(elsewhere, None, Modifiers::none());
    draw(cx);
    let before = filled(cx);
    cx.simulate_mouse_move(at, None, Modifiers::none());
    draw(cx);
    let around = |quad: &Bounds<Pixels>| quad.contains(&at);
    let new = appeared(&before, filled(cx)).into_iter().filter(around);
    let smallest = new.min_by(|a, b| area(a).total_cmp(&area(b)));
    smallest.expect("the item under the pointer is highlighted")
}

#[track_caller]
fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    assert!(
        (f32::from(found) - f32::from(wanted)).abs() <= HALF_A_POINT,
        "{what}: {found:?}, expected {wanted:?}"
    );
}

/// The highlights of the first and the last item stand as far from the menu's edge at the top, at the
/// bottom and at both sides: one inset all round, and no room left under the last item.
#[track_caller]
fn assert_one_inset(frame: Bounds<Pixels>, first: Bounds<Pixels>, last: Bounds<Pixels>) {
    let inset = first.top() - frame.top();
    assert!(inset > Pixels::ZERO, "the items are inside the menu");
    assert_near(frame.bottom() - last.bottom(), inset, "under the last item");
    for item in [first, last] {
        assert_near(item.left() - frame.left(), inset, "left of an item");
        assert_near(frame.right() - item.right(), inset, "right of an item");
    }
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    super::grid_tests::bounds(cx, selector)
}

/// Opens the window on a `box` with one commit ahead and shows "Changes" with its commit menu's trigger.
fn open_changes(cx: &mut TestAppContext) -> (&mut VisualTestContext, tempfile::TempDir) {
    let (dir, script) = fake_agentbox(Names::Listed, "0", LOG);
    let commit = serde_json::json!([{
        "sha": SHA, "subject": "attack", "time": "2026-11-10T06:43:31Z",
        "filesChanged": 1, "insertions": 1, "deletions": 0
    }]);
    std::fs::write(dir.path().join("log.json"), commit.to_string()).expect("the log");
    let (cx, _, _) = open_live(cx, script, Some(TASK), false);
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("pane-view-changes").is_some()
    });
    click(cx, "pane-view-changes");
    wait_for(cx, "the commit menu's trigger", |cx| {
        cx.debug_bounds("commit-picker").is_some()
    });
    (cx, dir)
}

const ALL: &str = "commit-choice-all";
const COMMIT: &str = "commit-choice-7924b73";

/// With the commit menu open: the frame holds its two choices with one inset all round, their labels
/// start on one line, as far inside their highlights. Gives that line.
#[track_caller]
fn assert_commit_menu_is_on_its_lines(cx: &mut VisualTestContext, frame: Bounds<Pixels>) -> Pixels {
    let (all, commit) = (bounds(cx, ALL), bounds(cx, COMMIT));
    let first = item_at(cx, all.center());
    let last = item_at(cx, commit.center());
    assert_one_inset(frame, first, last);
    assert_near(
        all.left(),
        commit.left(),
        "the two choices start on one line",
    );
    assert_near(
        all.left() - first.left(),
        commit.left() - last.left(),
        "as far inside their highlights",
    );
    all.left()
}

#[test]
fn the_commit_menu_s_choices_start_on_one_line_checked_or_not_with_one_inset_all_round() {
    real_text::run(|cx| {
        let (cx, _dir) = open_changes(cx);
        // "Tutte le modifiche" is the choice in force: it has the check, the commit has none.
        let frame = open_menu(cx, "commit-picker");
        let line = assert_commit_menu_is_on_its_lines(cx, frame);
        // The commit is chosen: now it has the check.
        click(cx, COMMIT);
        wait_for(cx, "the menu to go", |cx| cx.debug_bounds(ALL).is_none());
        let frame = open_menu(cx, "commit-picker");
        let moved = assert_commit_menu_is_on_its_lines(cx, frame);
        assert_near(moved, line, "the line did not move with the check");
    });
}

#[test]
fn the_commit_menu_s_trigger_is_a_quiet_control_with_its_words_on_the_pane_s_inset() {
    real_text::run(|cx| {
        let (cx, _dir) = open_changes(cx);
        let trigger = bounds(cx, "commit-picker");
        assert_eq!(trigger.size.height, theme::CONTROL_HEIGHT);
        let words = bounds(cx, "commit-picker-label");
        let line = bounds(cx, "right-pane").left() + theme::SIDE_PANE_INSET;
        assert_near(
            words.left(),
            line,
            "its words start on the right pane's inset, where its first view does",
        );
        assert_near(
            words.left() - trigger.left(),
            theme::CONTROL_PADDING_X,
            "its fill reaches one control padding past them",
        );
        let at_rest = filled(cx);
        assert!(
            !at_rest.contains(&trigger),
            "no fill at rest: the trigger is quiet"
        );
    });
}

#[test]
fn the_lavoro_s_menu_has_one_inset_all_round_and_nothing_under_its_last_item() {
    real_text::run(|cx| {
        let (cx, _dir) = open_changes(cx);
        // A lavoro whose agent runs: every item of its menu can be pressed.
        click(cx, "row-sample-folders");
        wait_for(cx, "its header", |cx| {
            cx.debug_bounds("task-menu").is_some()
        });
        let frame = open_menu(cx, "task-menu");
        let reach = theme::ROW_HEIGHT / 2.;
        let first = item_at(cx, point(frame.center().x, frame.top() + reach));
        let last = item_at(cx, point(frame.center().x, frame.bottom() - reach));
        assert!(last.top() > first.bottom(), "two different items");
        assert_one_inset(frame, first, last);
    });
}
