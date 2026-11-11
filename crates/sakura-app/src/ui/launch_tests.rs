//! The first second of the window, on the real chain of reads with a fake `box` script in place of
//! agentbox: `box tasks` answers at once, the facts of one task take a moment, `box status` takes seconds.

use super::desk::Desk;
use super::interaction_tests::{click, draw, open_on, take_place, wait_for};
use crate::data::source::{Recorder, Source};
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::models::BoxStatus;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const STATUS: &str = include_str!("../../../sakura-core/tests/fixtures/status.json");
pub(super) const TASK: &str = "sample-accent";
const FIRST: &str = "sample-folders";
/// How long after the first frame a row may take to be on screen and selectable.
const SELECTABLE_WITHIN: Duration = Duration::from_secs(1);
/// The fake `box status` takes eight seconds: a row selectable in half of that did not wait for it.
const WITHOUT_STATUS: Duration = Duration::from_secs(4);

/// What the fake `box` does for `tasks`.
pub(super) enum Names {
    Listed,
    /// An older `box`: its usage and exit code 2.
    Unknown,
}

/// A `box` that answers `tasks` as `names` says, `facts` after a moment, and `status` after
/// `status_seconds`.
fn slow_box(names: Names, status_seconds: &str) -> (tempfile::TempDir, PathBuf) {
    fake_agentbox(names, status_seconds, "")
}

/// The same `box`, logging its arguments to `calls` beside it, with `cases` (lines of a shell `case`)
/// tried before its own.
pub(super) fn fake_agentbox(
    names: Names,
    status_seconds: &str,
    cases: &str,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("sakura-launch")
        .tempdir()
        .expect("a temporary directory");
    write_answers(dir.path());
    let tasks = match names {
        Names::Listed => r#"cat "$d/tasks.json""#,
        Names::Unknown => "echo 'usage: box <command>' >&2; exit 2",
    };
    let body = format!(
        r#"#!/bin/sh
d="$(dirname "$0")"
echo "$@" >> "$d/calls"
trap 'echo "$1" >> "$d/ended"' EXIT
case "$1" in
{cases}
  tasks) {tasks} ;;
  status) sleep {status_seconds}; cat "$d/status.json" ;;
  facts) sleep 0.2; cat "$d/facts-$2.json" ;;
  diff) ;;
  cancel) echo 'usage: box <command>' >&2; exit 2 ;;
  *) echo '[]' ;;
esac
"#
    );
    std::fs::write(dir.path().join("ended"), "").expect("the list of calls that ended");
    let script = dir.path().join("box");
    sakura_core::test_support::write_script(&script, &body);
    (dir, script)
}

/// The fixture status, its names as `box tasks` prints them, and each task's facts.
fn write_answers(dir: &std::path::Path) {
    let status: BoxStatus = serde_json::from_str(STATUS).expect("the fixture parses");
    let names: Vec<serde_json::Value> = status
        .tasks
        .iter()
        .map(|task| {
            serde_json::json!({"name": task.name, "profile": task.profile, "base": task.base, "taken": null})
        })
        .collect();
    let write =
        |name: String, text: String| std::fs::write(dir.join(name), text).expect("an answer");
    write("status.json".into(), STATUS.into());
    write(
        "tasks.json".into(),
        serde_json::Value::from(names).to_string(),
    );
    for task in &status.tasks {
        let facts = serde_json::to_string(task).expect("the facts");
        write(format!("facts-{}.json", task.name), facts);
    }
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    script: PathBuf,
    first: Option<&str>,
) -> (&'a mut VisualTestContext, Entity<Desk>, Arc<Recorder>) {
    open_live(cx, script, first, false)
}

/// Opens the desk on the `box` at `script`; with `has_terminals` its consoles run the same script.
pub(super) fn open_live<'a>(
    cx: &'a mut TestAppContext,
    script: PathBuf,
    first: Option<&str>,
    has_terminals: bool,
) -> (&'a mut VisualTestContext, Entity<Desk>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let reads = recorder.clone();
    let terminals = has_terminals.then(|| script.clone());
    let start = move |runtime| Source::live_for_test(runtime, script, reads);
    let (cx, desk) = open_on(cx, start, terminals, first.map(str::to_string));
    (cx, desk, recorder)
}

/// What the fake `box` was asked, one line per call.
pub(super) fn calls(dir: &tempfile::TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.path().join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// How many calls of the fake `box` have not answered yet. A call that stays (an agent, a terminal, a
/// sign-in: the script becomes that process) never ends and is not counted.
pub(super) fn in_flight(dir: &tempfile::TempDir) -> usize {
    let stays = |call: &&String| {
        ["acp", "attach", "shell", "login", "agent"]
            .iter()
            .any(|command| call.split(' ').next() == Some(command))
    };
    let all = calls(dir);
    let ending = all.len() - all.iter().filter(stays).count();
    // Only a script that notes its ends has the file: without it nothing is known to be running.
    match std::fs::read_to_string(dir.path().join("ended")) {
        Ok(ended) => ending.saturating_sub(ended.lines().count()),
        Err(_) => 0,
    }
}

/// How long the fake `box` must stay unasked, with every call answered, to be called settled.
const QUIET: Duration = Duration::from_secs(1);

/// Waits until every call of the fake `box` has answered and nothing new was requested a moment, and
/// gives what it was asked. A call still running is waited for, however slow the machine is.
pub(super) fn settled_calls(cx: &mut VisualTestContext, dir: &tempfile::TempDir) -> Vec<String> {
    let deadline = Instant::now() + super::interaction_tests::HANG;
    let mut seen = calls(dir);
    let mut quiet_since = Instant::now();
    loop {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(25));
        let now = calls(dir);
        if now.len() != seen.len() || in_flight(dir) > 0 {
            seen = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= QUIET {
            return now;
        }
        assert!(Instant::now() < deadline, "box never settled: {seen:#?}");
    }
}

fn is_pending(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> Option<bool> {
    desk.read_with(cx, |desk, cx| {
        desk.facts.read(cx).row(task).map(|row| row.is_pending)
    })
}

/// How many times the timed launch is tried on a machine too busy to answer in time. The others in the
/// suite already wait for it; this covers what else runs on the machine. A launch that waited for `box
/// status` would take eight seconds every time.
const TIMED_ATTEMPTS: usize = 3;

/// One launch on a `box` whose status takes eight seconds: how long after the first frame a row was on
/// screen, clicked and selected. The rest of what the first second must do is asserted on the way.
fn timed_launch(cx: &mut TestAppContext, kept: &mut Vec<tempfile::TempDir>) -> Duration {
    let (dir, script) = slow_box(Names::Listed, "8");
    kept.push(dir);
    let (cx, desk, recorder) = open(cx, script, None);
    let first_frame = Instant::now();
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    click(cx, "row-sample-accent");
    let selected = desk.read_with(cx, |desk, cx| desk.shown(cx));
    assert_eq!(selected.as_deref(), Some(TASK));
    let elapsed = first_frame.elapsed();
    assert_eq!(
        is_pending(cx, &desk, TASK),
        Some(true),
        "its state is being read"
    );
    wait_for(cx, "the selected task's facts", |cx| {
        is_pending(cx, &desk, TASK) == Some(false)
    });
    assert_eq!(
        recorder.reads(),
        vec![
            "tasks".to_string(),
            "status".into(),
            format!("facts {TASK}")
        ],
        "a click on a row being read asks its facts at once"
    );
    assert_eq!(
        is_pending(cx, &desk, FIRST),
        Some(true),
        "status still runs"
    );
    elapsed
}

#[gpui::test]
fn rows_are_selectable_within_a_second_of_the_first_frame_while_status_takes_eight(
    cx: &mut TestAppContext,
) {
    take_place(true);
    let mut kept = Vec::new();
    let mut fastest = Duration::MAX;
    for _ in 0..TIMED_ATTEMPTS {
        fastest = fastest.min(timed_launch(cx, &mut kept));
        if fastest < SELECTABLE_WITHIN {
            break;
        }
    }
    // The one-second measure is a measure of the app only on a machine that has a core to give it. On
    // one that is busy with something else (other builds, a small CI runner) the test still proves the
    // thing the measure is for, that a row never waits for `box status` (eight seconds here), and says
    // that the second was not measured.
    if is_machine_busy() && fastest >= SELECTABLE_WITHIN {
        eprintln!("not measured: machine loaded; a row was selectable after {fastest:?}");
        assert!(fastest < WITHOUT_STATUS, "selected after {fastest:?}");
        return;
    }
    assert!(fastest < SELECTABLE_WITHIN, "selected after {fastest:?}");
}

/// Whether the machine's load is above what its cores can take: a time measured now says how busy the
/// machine is, not how fast the app is. A load that cannot be read counts as busy.
fn is_machine_busy() -> bool {
    sakura_core::machine::read(std::path::Path::new("/"))
        .map_or(true, |machine| sakura_core::model::is_load_high(&machine))
}

#[gpui::test]
fn the_task_the_window_opens_on_is_read_before_every_other(cx: &mut TestAppContext) {
    let (_dir, script) = slow_box(Names::Listed, "8");
    let (cx, desk, recorder) = open(cx, script, Some(FIRST));
    wait_for(cx, "the first task's facts", |cx| {
        is_pending(cx, &desk, FIRST) == Some(false)
    });
    assert_eq!(
        recorder.reads(),
        vec![
            "tasks".to_string(),
            format!("facts {FIRST}"),
            "status".into()
        ]
    );
    assert_eq!(is_pending(cx, &desk, TASK), Some(true));
}

#[gpui::test]
fn a_status_fills_every_row_the_names_listed(cx: &mut TestAppContext) {
    let (_dir, script) = slow_box(Names::Listed, "0.5");
    let (cx, desk, recorder) = open(cx, script, None);
    wait_for(cx, "the status", |cx| {
        is_pending(cx, &desk, TASK) == Some(false) && is_pending(cx, &desk, FIRST) == Some(false)
    });
    assert_eq!(recorder.reads(), vec!["tasks", "status"]);
}

#[gpui::test]
fn without_a_last_task_the_window_waits_for_the_groups_then_opens_on_da_guardare(
    cx: &mut TestAppContext,
) {
    let (_dir, script) = slow_box(Names::Listed, "0.6");
    let (cx, desk, _) = open(cx, script, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    let selected = |cx: &mut VisualTestContext| desk.read_with(cx, |desk, cx| desk.shown(cx));
    wait_for(cx, "the opening", |cx| selected(cx).is_some());
    assert_eq!(selected(cx).as_deref(), Some(TASK));
    assert_eq!(
        is_pending(cx, &desk, TASK),
        Some(false),
        "chosen from its group"
    );
}

#[gpui::test]
fn a_row_the_person_clicked_while_the_groups_were_read_stays_selected(cx: &mut TestAppContext) {
    let (_dir, script) = slow_box(Names::Listed, "0.6");
    let (cx, desk, _) = open(cx, script, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-headings-26").is_some()
    });
    click(cx, "row-headings-26");
    wait_for(cx, "the status", |cx| {
        is_pending(cx, &desk, FIRST) == Some(false)
    });
    let selected = desk.read_with(cx, |desk, cx| desk.shown(cx));
    assert_eq!(selected.as_deref(), Some("headings-26"));
}

#[gpui::test]
fn with_a_project_and_no_task_the_one_field_is_shown_in_place_in_the_panel(
    cx: &mut TestAppContext,
) {
    let project = r#"  projects) echo '[{"name":"demo-site","base":"main","where":"mac"}]' ;;"#;
    let (dir, script) = fake_agentbox(Names::Listed, "0", project);
    std::fs::write(dir.path().join("tasks.json"), "[]").expect("no names");
    let empty = r#"{"runner":{"state":"stopped","current":null,"queue":[]},"tasks":[]}"#;
    std::fs::write(dir.path().join("status.json"), empty).expect("no tasks");
    let (cx, desk, _) = open(cx, script, Some("gone"));
    wait_for(cx, "the field in place", |cx| {
        cx.debug_bounds("first-task").is_some() && cx.debug_bounds("new-task-field").is_some()
    });
    // The one field is the page itself, centred in the panel; no sheet opens over it.
    let field = cx.debug_bounds("first-task").expect("the field in place");
    let window = cx.update(|window, _| window.viewport_size());
    let panel_left = desk.read_with(cx, |desk, _| desk.panel_left_now());
    let across = (panel_left + window.width) / 2.;
    assert!(
        (field.center().x - across).abs() <= gpui::px(1.),
        "across: {:?}, the panel's centre is {across:?}",
        field.center().x
    );
    assert!(
        cx.debug_bounds("new-task-icon").is_some(),
        "the plus of the sidebar's header is there with no lavoro at all"
    );
    let label = cx.debug_bounds("sidebar-tasks");
    let label = label.expect("the header says what the list is");
    let plus = cx.debug_bounds("new-task-icon").expect("the plus");
    assert!(
        (label.center().y - plus.center().y).abs() <= gpui::px(1.),
        "in the plus's own row: {label:?}, {plus:?}"
    );
    let line = cx.debug_bounds("sidebar-empty");
    let line = line.expect("one quiet line for the empty list");
    assert!(line.top() >= label.bottom(), "under the header");
    // The sidebar's plus leads to the same field: it takes the caret, and nothing opens over it.
    click(cx, "new-task");
    let sheet = super::new_task_tests::sheet(cx, &desk);
    let has_caret = cx.update(|window, cx| sheet.read(cx).has_caret(window, cx));
    assert!(has_caret, "the field in place has the caret");
    let is_in_place = desk.read_with(cx, |desk, _| {
        desk.new_task
            .as_ref()
            .is_some_and(|open| open.is_in_place())
    });
    assert!(is_in_place, "no sheet over the page");
}

/// Rewrites the results of `TASK` in the fake `box`'s answers.
fn with_results(dir: &tempfile::TempDir, change: impl Fn(&mut sakura_core::models::Task)) {
    let mut status: BoxStatus = serde_json::from_str(STATUS).expect("the fixture parses");
    let task = status.tasks.iter_mut().find(|task| task.name == TASK);
    let task = task.expect("the fixture task");
    change(task);
    let facts = serde_json::to_string(task).expect("the facts");
    std::fs::write(dir.path().join(format!("facts-{TASK}.json")), facts).expect("the facts");
    let status = serde_json::to_string(&status).expect("the status");
    std::fs::write(dir.path().join("status.json"), status).expect("the status");
}

#[gpui::test]
fn a_verified_commit_that_changes_the_checks_is_a_hold_and_its_test_row_has_the_count(
    cx: &mut TestAppContext,
) {
    let (dir, script) = slow_box(Names::Listed, "0");
    with_results(&dir, |task| {
        let test = task.results.get_mut("test").expect("a test result");
        test.changes_checks = Some(true);
        test.checks_touched = Some(vec!["vitest.config.ts".into()]);
        test.tests = Some(sakura_core::models::TestCounts {
            ran: 7880,
            base: Some(7880),
            ..Default::default()
        });
    });
    let (cx, desk, _) = open(cx, script, Some(TASK));
    wait_for(cx, "the facts", |cx| {
        is_pending(cx, &desk, TASK) == Some(false)
    });
    wait_for(cx, "the hold", |cx| {
        cx.debug_bounds("hold-sample-accent").is_some()
    });
    assert!(
        cx.debug_bounds("take-note").is_none(),
        "no sentence in the header"
    );
    click(cx, "pane-view-checks");
    assert!(
        cx.debug_bounds("verify-summary-reason").is_some(),
        "the reason is in Verifiche"
    );
    assert!(cx.debug_bounds("verify-tests-test").is_some(), "the count");
    assert!(cx.debug_bounds("verify-tests-rules").is_none());
    assert!(cx.debug_bounds("verify-tests-touched").is_none());
    click(cx, "verify-test");
    wait_for(cx, "the files", |cx| {
        cx.debug_bounds("reserve-files").is_some()
    });
}

#[gpui::test]
fn a_verified_commit_that_touches_existing_tests_is_a_hold_and_the_row_says_so_under_the_count(
    cx: &mut TestAppContext,
) {
    let (dir, script) = slow_box(Names::Listed, "0");
    with_results(&dir, |task| {
        let test = task.results.get_mut("test").expect("a test result");
        test.changes_checks = Some(false);
        test.tests = Some(sakura_core::models::TestCounts {
            ran: 7880,
            base: Some(7880),
            ..Default::default()
        });
        test.tests_touched = Some(sakura_core::models::TestsTouched {
            modified: 3,
            deleted: 1,
            files: vec!["src/a.spec.ts".into()],
        });
    });
    let (cx, desk, _) = open(cx, script, Some(TASK));
    wait_for(cx, "the facts", |cx| {
        is_pending(cx, &desk, TASK) == Some(false)
    });
    wait_for(cx, "the hold", |cx| {
        cx.debug_bounds("hold-sample-accent").is_some()
    });
    click(cx, "pane-view-checks");
    let count = cx.debug_bounds("verify-tests-test").expect("the count");
    let said = cx
        .debug_bounds("verify-tests-touched")
        .expect("the sentence");
    assert!(said.top() > count.top(), "under the count");
    click(cx, "verify-test");
    wait_for(cx, "the files", |cx| {
        cx.debug_bounds("reserve-files").is_some()
    });
}

#[gpui::test]
fn a_box_without_the_cancel_command_never_offers_annulla(cx: &mut TestAppContext) {
    let (_dir, script) = slow_box(Names::Listed, "0");
    let (cx, desk, _) = open(cx, script, Some(FIRST));
    wait_for(cx, "the status", |cx| {
        is_pending(cx, &desk, FIRST) == Some(false)
    });
    click(cx, "pane-view-checks");
    wait_for(cx, "the rows", |cx| cx.debug_bounds("run-test").is_some());
    assert!(cx.debug_bounds("cancel-test").is_none());
}

#[gpui::test]
fn a_box_without_the_tasks_command_lists_the_rows_when_status_answers(cx: &mut TestAppContext) {
    let (_dir, script) = slow_box(Names::Unknown, "0.3");
    let (cx, desk, _) = open(cx, script, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    assert_eq!(is_pending(cx, &desk, TASK), Some(false));
}

#[gpui::test]
fn the_new_lavoro_button_is_in_the_sidebar_before_any_lavoro_is_listed_and_with_none(
    cx: &mut TestAppContext,
) {
    // An older `box` has no names to give, and its status takes three seconds: nothing is listed yet.
    let (_dir, script) = slow_box(Names::Unknown, "3");
    let (cx, desk, _) = open(cx, script, None);
    draw(cx);
    let is_listing = desk.read_with(cx, |desk, cx| !desk.facts.read(cx).rows().is_empty());
    assert!(!is_listing, "no lavoro is listed yet");
    assert!(
        cx.debug_bounds("new-task").is_some(),
        "the button of the sidebar's header is there while the lavori are read"
    );
    click(cx, "new-task");
    wait_for(cx, "the sheet", |cx| {
        cx.debug_bounds("new-task-field").is_some()
    });
}

#[gpui::test]
fn at_the_narrowest_window_a_row_with_trust_notes_keeps_its_action_and_its_notes_inside(
    cx: &mut TestAppContext,
) {
    let (dir, script) = slow_box(Names::Listed, "0");
    with_results(&dir, |task| {
        let test = task.results.get_mut("test").expect("a test result");
        test.tests = Some(sakura_core::models::TestCounts {
            ran: 7876,
            skipped: Some(4),
            base: Some(7880),
            base_skipped: Some(0),
        });
        test.tests_touched = Some(sakura_core::models::TestsTouched {
            modified: 3,
            deleted: 1,
            files: vec!["src/a.spec.ts".into()],
        });
    });
    let (cx, desk, _) = open(cx, script, Some(TASK));
    wait_for(cx, "the facts", |cx| {
        is_pending(cx, &desk, TASK) == Some(false)
    });
    cx.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    // The header is at rest before anything in it is pressed: the count on "Modifiche" leaves when the
    // patch is read (this `box` has no changes), and the tabs after it move then.
    wait_for(cx, "the changes to be read", |cx| {
        desk.read_with(cx, |desk, cx| {
            let changes = desk.changes.read(cx);
            changes
                .task(TASK)
                .is_some_and(|task| task.patch().is_some())
        })
    });
    // The window is laid out at its new size.
    draw(cx);
    click(cx, "pane-view-checks");
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("verify-test").is_some()
    });
    let row = cx.debug_bounds("verify-test").expect("the row");
    let inside = |selector: &'static str, cx: &mut VisualTestContext| {
        let bounds = cx.debug_bounds(selector).expect(selector);
        assert!(
            bounds.left() >= row.left() && bounds.right() <= row.right(),
            "{selector} at {:?} to {:?} leaves the row, {:?} to {:?}",
            bounds.left(),
            bounds.right(),
            row.left(),
            row.right()
        );
        bounds
    };
    let action = inside("run-test", cx);
    let count = inside("verify-tests-test", cx);
    let said = inside("verify-tests-touched", cx);
    assert!(
        count.right() <= action.left(),
        "the count ends before the action"
    );
    assert!(
        said.right() <= action.left() || said.top() >= action.bottom(),
        "the sentence is beside the action or under it, never across it"
    );
    let other = cx
        .debug_bounds("run-rules")
        .expect("the other row's action");
    assert_eq!(
        action.left(),
        other.left(),
        "the action keeps its place on every row"
    );
}
