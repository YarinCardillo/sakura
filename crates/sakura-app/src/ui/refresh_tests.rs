//! What a change tells the window to read again, on the real chain of reads with a fake `box`: no event is
//! lost because a read was under way when it came, and an answer that is older than what the window knows
//! changes nothing.

use super::desk::Desk;
use super::interaction_tests::wait_for;
use super::launch_tests::{calls, fake_agentbox, open_live, Names, TASK};
use crate::data::facts::Facts;
use crate::data::source::Signal;
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The fake `box` prints the journal from a file beside it (nothing until the test writes one), keeping
/// what it read at the start of the call, and answers after `JOURNAL_SECONDS`.
const JOURNAL: &str = r#"  journal) cat "$d/journal.json" 2>/dev/null || echo '[]'; sleep "$(cat "$d/journal-seconds" 2>/dev/null || echo 0)" ;;"#;

fn event(at: &str, kind: &str, task: &str) -> serde_json::Value {
    serde_json::json!({ "at": at, "event": kind, "task": task })
}

fn write_journal(dir: &tempfile::TempDir, events: &[serde_json::Value]) {
    let text = serde_json::Value::from(events.to_vec()).to_string();
    std::fs::write(dir.path().join("journal.json"), text).expect("the journal");
}

/// Opens the window on a `box` whose status takes `status_seconds`, and waits for the page.
fn open<'a>(
    cx: &'a mut TestAppContext,
    status_seconds: &str,
    cases: &str,
) -> (
    &'a mut VisualTestContext,
    Entity<Facts>,
    tempfile::TempDir,
    Entity<Desk>,
) {
    let cases = format!("{JOURNAL}\n{cases}");
    let (dir, script): (tempfile::TempDir, PathBuf) =
        fake_agentbox(Names::Listed, status_seconds, &cases);
    let (cx, desk, _) = open_live(cx, script, Some(TASK), false);
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("page-header").is_some()
    });
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    (cx, facts, dir, desk)
}

/// Waits until every call of the fake `box` has answered and it has been asked nothing new for a moment.
fn settle(cx: &mut VisualTestContext, dir: &tempfile::TempDir) {
    super::launch_tests::settled_calls(cx, dir);
}

/// How many times the fake `box` was requested `command` (its first words).
fn asked(dir: &tempfile::TempDir, command: &str) -> usize {
    calls(dir)
        .iter()
        .filter(|call| call.starts_with(command))
        .count()
}

/// Tells the window that something not tied to one task changed, as the watcher does for the journal.
fn journal_changed(facts: &Entity<Facts>, cx: &mut VisualTestContext) {
    facts.update(cx, |facts, cx| {
        facts.signal_at(Signal::Changed(None), Instant::now(), cx)
    });
    cx.run_until_parked();
}

#[gpui::test]
fn the_first_event_after_a_journal_that_began_empty_is_acted_on(cx: &mut TestAppContext) {
    let (cx, facts, dir, _desk) = open(cx, "0", "");
    settle(cx, &dir);
    let before = asked(&dir, "status");
    write_journal(
        &dir,
        &[event("2026-11-10T07:00:00Z", "agent-start", "headings-26")],
    );
    journal_changed(&facts, cx);
    settle(cx, &dir);
    assert_eq!(
        asked(&dir, "status"),
        before + 1,
        "an agent started: every task is read again; calls: {:#?}",
        calls(&dir)
    );
}

#[gpui::test]
fn an_event_that_comes_while_every_task_is_being_read_is_followed_by_one_more_read(
    cx: &mut TestAppContext,
) {
    // The first read of every task takes three seconds; the event comes while it runs.
    let (cx, facts, dir, _desk) = open(cx, "3", "");
    wait_for(
        cx,
        "the first look at the journal and the read of every task",
        |_| asked(&dir, "journal") >= 1 && asked(&dir, "status") >= 1,
    );
    assert_eq!(asked(&dir, "status"), 1);
    write_journal(
        &dir,
        &[event("2026-11-10T07:00:00Z", "verify-done", "headings-26")],
    );
    journal_changed(&facts, cx);
    settle(cx, &dir);
    assert_eq!(
        asked(&dir, "status"),
        2,
        "the read under way may predate the event: one more follows it; calls: {:#?}",
        calls(&dir)
    );
}

#[gpui::test]
fn a_journal_change_that_comes_while_the_journal_is_being_read_is_not_lost(
    cx: &mut TestAppContext,
) {
    let (cx, facts, dir, _desk) = open(cx, "0", "");
    settle(cx, &dir);
    std::fs::write(dir.path().join("journal-seconds"), "1").expect("the delay");
    let first = event("2026-11-10T07:00:00Z", "take", "headings-26");
    write_journal(&dir, std::slice::from_ref(&first));
    journal_changed(&facts, cx);
    wait_for(cx, "the journal read to start", |_| {
        asked(&dir, "journal") >= 2
    });
    // The read under way has printed the first event only; a second one is written now.
    std::thread::sleep(Duration::from_millis(200));
    let second = event("2026-11-10T07:00:05Z", "take", "broken-check");
    write_journal(&dir, &[first, second]);
    journal_changed(&facts, cx);
    settle(cx, &dir);
    assert_eq!(
        asked(&dir, "facts broken-check"),
        1,
        "the second take is read once the first look ends; calls: {:#?}",
        calls(&dir)
    );
}

/// The fake `box` answers the facts of one task after two seconds, once the file `slow` is beside it.
const SLOW_FACTS: &str = r#"  facts) if [ "$2" = broken-check ] && [ -f "$d/slow" ]; then sleep 2; fi; cat "$d/facts-$2.json" ;;"#;
const REMOVED: &str = "broken-check";

/// Rewrites the status the fake `box` prints without the task `name`.
fn remove_from_status(dir: &tempfile::TempDir, name: &str) {
    let path = dir.path().join("status.json");
    let text = std::fs::read_to_string(&path).expect("the status");
    let mut status: sakura_core::models::BoxStatus =
        serde_json::from_str(&text).expect("the status parses");
    status.tasks.retain(|task| task.name != name);
    let text = serde_json::to_string(&status).expect("the status");
    std::fs::write(path, text).expect("the status");
}

#[gpui::test]
fn facts_that_answer_after_their_task_was_removed_do_not_bring_it_back(cx: &mut TestAppContext) {
    let (cx, facts, dir, _desk) = open(cx, "0", SLOW_FACTS);
    settle(cx, &dir);
    let is_listed =
        |cx: &mut VisualTestContext| facts.read_with(cx, |facts, _| facts.has_task(REMOVED));
    assert!(is_listed(cx));
    std::fs::write(dir.path().join("slow"), "").expect("the delay");
    facts.update(cx, |facts, cx| facts.refresh_task(REMOVED.into(), cx));
    wait_for(cx, "the slow facts to be asked", |_| {
        asked(&dir, "facts broken-check") >= 1
    });
    // The task is removed meanwhile: a read of every task, started after those facts were asked, no
    // longer lists it.
    remove_from_status(&dir, REMOVED);
    facts.update(cx, |facts, cx| facts.refresh_all(cx));
    wait_for(cx, "the task to leave", |cx| !is_listed(cx));
    // The old facts answer now: wait until no read of facts is under way, however long that takes.
    wait_for(cx, "the old facts to answer", |cx| {
        !facts.read_with(cx, |facts, _| facts.is_reading())
    });
    assert!(
        !is_listed(cx),
        "facts older than the removal changed nothing"
    );
}

/// The fake `box` prints the log of a result from a file beside it.
const RESULT_LOG: &str = r#"  result-log) cat "$d/the-log" ;;"#;

/// The log shown for the test check of the task on screen, once read.
fn shown_log(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Option<String> {
    desk.read_with(cx, |desk, cx| {
        match desk.actions.read(cx).log(TASK, "test") {
            Some(crate::data::actions::LogTail::Loaded(text)) => Some(text.to_string()),
            _ => None,
        }
    })
}

/// Rewrites the facts the fake `box` prints for the task on screen with a test result that ended at `at`.
fn test_ended_again(dir: &tempfile::TempDir, at: &str) {
    let path = dir.path().join(format!("facts-{TASK}.json"));
    let text = std::fs::read_to_string(&path).expect("the facts");
    let mut task: sakura_core::models::Task = serde_json::from_str(&text).expect("the facts parse");
    let result = task.results.get_mut("test").expect("a test result");
    result.finished_at = at.to_string();
    std::fs::write(path, serde_json::to_string(&task).expect("the facts")).expect("the facts");
}

#[gpui::test]
fn an_open_log_is_read_again_when_its_check_has_a_newer_result(cx: &mut TestAppContext) {
    let (cx, facts, dir, desk) = open(cx, "0", RESULT_LOG);
    settle(cx, &dir);
    std::fs::write(dir.path().join("the-log"), "first run").expect("the log");
    desk.update(cx, |desk, cx| desk.toggle_log("test".into(), cx));
    wait_for(cx, "the log of the first run", |cx| {
        shown_log(&desk, cx).as_deref() == Some("first run")
    });
    // The check runs again and ends: the row above the log now shows the new result.
    std::fs::write(dir.path().join("the-log"), "second run").expect("the log");
    test_ended_again(&dir, "2026-11-10T09:00:00Z");
    facts.update(cx, |facts, cx| facts.refresh_task(TASK.into(), cx));
    wait_for(cx, "the log of the second run", |cx| {
        shown_log(&desk, cx).as_deref() == Some("second run")
    });
    let reads = asked(&dir, "result-log");
    facts.update(cx, |facts, cx| facts.refresh_task(TASK.into(), cx));
    settle(cx, &dir);
    assert_eq!(
        asked(&dir, "result-log"),
        reads,
        "the same result: the log is not read again"
    );
}

/// Waits until every read of the tasks has answered, lets the test's clock pass the gap a second read of
/// the changes waits for after the first (a timer of the app, which only the test's clock moves), and
/// then until `box` is asked nothing more.
fn settle_reads(facts: &Entity<Facts>, cx: &mut VisualTestContext, dir: &tempfile::TempDir) {
    wait_for(cx, "the reads to answer", |cx| {
        !facts.read_with(cx, |facts, _| facts.is_reading())
    });
    settle(cx, dir);
    cx.executor()
        .advance_clock(crate::data::changes::MIN_REFETCH_GAP * 2);
    settle(cx, dir);
}

/// Each `box diff` starts a container: a launch asks for the changes of the lavoro on screen once, however
/// the names, its own facts and the read of every lavoro follow one another.
fn assert_one_read_of_the_changes(dir: &tempfile::TempDir) {
    let all = calls(dir);
    assert_eq!(asked(dir, "diff"), 1, "box diff at launch: {all:#?}");
    assert_eq!(asked(dir, "log"), 1, "box log at launch: {all:#?}");
    assert_eq!(asked(dir, &format!("facts {TASK}")), 1, "{all:#?}");
    assert_eq!(asked(dir, "status"), 1, "{all:#?}");
}

#[gpui::test]
fn a_launch_reads_the_changes_of_the_lavoro_shown_once_when_every_lavoro_is_read_at_once(
    cx: &mut TestAppContext,
) {
    let (cx, facts, dir, _desk) = open(cx, "0", "");
    settle_reads(&facts, cx, &dir);
    assert_one_read_of_the_changes(&dir);
}

#[gpui::test]
fn a_launch_reads_the_changes_of_the_lavoro_shown_once_when_the_read_of_every_lavoro_is_slow(
    cx: &mut TestAppContext,
) {
    // The names, then the facts of the lavoro shown, then, two seconds later, every lavoro.
    let (cx, facts, dir, _desk) = open(cx, "2", "");
    settle_reads(&facts, cx, &dir);
    assert_one_read_of_the_changes(&dir);
}
