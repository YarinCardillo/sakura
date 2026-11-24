use super::*;
use gpui::{AppContext as _, Entity, TestAppContext};
use sakura_core::error::{BridgeError, ErrorCode};
use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Mutex;
use tokio::sync::watch::{channel, Sender};

const SNAPSHOT: &str = include_str!("../../../sakura-core/tests/fixtures/engine-snapshot.json");

/// A reader that answers from a script and counts what it was asked.
struct Scripted {
    answers: Mutex<VecDeque<BridgeResult<Snapshot>>>,
    reads: Mutex<usize>,
}

impl Scripted {
    fn new(answers: Vec<BridgeResult<Snapshot>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
            reads: Mutex::new(0),
        })
    }

    fn reads(&self) -> usize {
        *self.reads.lock().unwrap()
    }
}

impl EngineRead for Scripted {
    fn snapshot(&self) -> Pending<Snapshot> {
        *self.reads.lock().unwrap() += 1;
        let next = self.answers.lock().unwrap().pop_front();
        // The last answer is repeated: a test that reads more than it scripted is told so by its count.
        let answer = next.unwrap_or_else(|| Ok(snapshot()));
        Box::pin(std::future::ready(answer))
    }
}

fn snapshot() -> Snapshot {
    Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture is a snapshot")
}

fn broken_config() -> BridgeError {
    let said = Said::new("error.engine.failed").number("code", 2);
    BridgeError::new(ErrorCode::EngineFailed, said)
        .with_details(Some("config problem: broken.toml".to_string()))
}

fn start(cx: &mut TestAppContext, reader: &Arc<Scripted>) -> (Entity<EngineFacts>, Sender<()>) {
    let (sender, changes) = channel(());
    let reader = reader.clone();
    let facts = cx.new(|cx| EngineFacts::new(reader, changes, None, cx));
    cx.run_until_parked();
    (facts, sender)
}

fn pass(cx: &mut TestAppContext, time: Duration) {
    cx.executor().advance_clock(time);
    cx.run_until_parked();
}

#[gpui::test]
fn the_first_read_is_made_at_the_start(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![Ok(snapshot())]);
    let (facts, _changes) = start(cx, &reader);
    assert_eq!(reader.reads(), 1);
    let waits = facts.read_with(cx, |facts, _| facts.snapshot().map(|s| s.waits.len()));
    assert_eq!(waits, Some(5));
}

#[gpui::test]
fn failed_read_keeps_last_snapshot_and_says_why(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![Ok(snapshot()), Err(broken_config())]);
    let (facts, changes) = start(cx, &reader);
    changes.send(()).unwrap();
    pass(cx, CHANGE_DEBOUNCE);
    assert_eq!(reader.reads(), 2);
    facts.read_with(cx, |facts, _| {
        assert_eq!(
            facts.snapshot(),
            Some(&snapshot()),
            "the last snapshot stays"
        );
        let problem = facts.problem().expect("the failure is told");
        assert_eq!(problem.said.key.0, "error.engine.failed");
        assert_eq!(
            problem.details.as_deref(),
            Some("config problem: broken.toml")
        );
    });
}

#[gpui::test]
fn a_failure_that_repeats_is_told_once_and_a_good_read_clears_it(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![
        Err(broken_config()),
        Err(broken_config()),
        Ok(snapshot()),
    ]);
    let (facts, _changes) = start(cx, &reader);
    let told = Rc::new(Cell::new(0));
    let count = told.clone();
    let _watch = cx.update(|cx| cx.observe(&facts, move |_, _| count.set(count.get() + 1)));
    pass(cx, RECOVERY_TICK);
    assert_eq!(told.get(), 0, "the same problem is not told again");
    pass(cx, RECOVERY_TICK);
    assert_eq!(told.get(), 1, "the snapshot that came back is news");
    facts.read_with(cx, |facts, _| assert!(facts.problem().is_none()));
}

#[gpui::test]
fn one_change_event_reads_once(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![]);
    let (_facts, changes) = start(cx, &reader);
    assert_eq!(reader.reads(), 1);
    changes.send(()).unwrap();
    pass(cx, CHANGE_DEBOUNCE);
    assert_eq!(reader.reads(), 2);
    pass(cx, CHANGE_DEBOUNCE * 3);
    assert_eq!(
        reader.reads(),
        2,
        "nothing more is read until something changes"
    );
}

#[gpui::test]
fn two_events_within_debounce_read_once(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![]);
    let (_facts, changes) = start(cx, &reader);
    changes.send(()).unwrap();
    pass(cx, CHANGE_DEBOUNCE / 3);
    changes.send(()).unwrap();
    pass(cx, CHANGE_DEBOUNCE);
    assert_eq!(reader.reads(), 2);
}

#[gpui::test]
fn fix37_project_folder_moves_refresh_the_snapshot_once_each(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("project");
    let moved = directory.path().join("moved");
    std::fs::create_dir(&folder).unwrap();
    let mut present = snapshot();
    present.projects.truncate(1);
    present.projects[0].folder = folder.display().to_string();
    let mut missing = present.clone();
    missing.projects[0].folder_missing = true;
    let reader = Scripted::new(vec![Ok(present.clone()), Ok(missing), Ok(present)]);
    let (facts, _changes) = start(cx, &reader);
    facts.update(cx, EngineFacts::watch_project_folders);

    std::fs::write(folder.join("tracked-file"), "ordinary project work").unwrap();
    wait_for_folder_watch(cx, || false);
    assert_eq!(reader.reads(), 1);

    for (from, to, expected_reads, is_missing) in
        [(&folder, &moved, 2, true), (&moved, &folder, 3, false)]
    {
        std::fs::rename(from, to).unwrap();
        wait_for_folder_watch(cx, || reader.reads() >= expected_reads);
        assert_eq!(reader.reads(), expected_reads);
        assert_eq!(
            facts.read_with(cx, |facts, _| facts.snapshot().unwrap().projects[0]
                .folder_missing),
            is_missing,
        );
        pass(cx, CHANGE_DEBOUNCE * 2);
        assert_eq!(reader.reads(), expected_reads);
    }
}

fn wait_for_folder_watch(cx: &mut TestAppContext, done: impl Fn() -> bool) {
    for _ in 0..60 {
        pass(cx, CHANGE_DEBOUNCE / 3);
        if done() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
fn ages_trigger_a_repaint_event_every_fifteen_seconds(cx: &mut TestAppContext) {
    assert_eq!(AGES_TICK, Duration::from_secs(15));
    let reader = Scripted::new(vec![]);
    let (facts, _changes) = start(cx, &reader);
    let ticks = Rc::new(Cell::new(0));
    let count = ticks.clone();
    let _watch = cx.update(|cx| {
        cx.subscribe(&facts, move |_, _: &AgesChanged, _| {
            count.set(count.get() + 1);
        })
    });
    pass(cx, AGES_TICK);
    assert_eq!(ticks.get(), 1);
    pass(cx, AGES_TICK);
    assert_eq!(ticks.get(), 2);
}

#[gpui::test]
fn recovery_refreshes_the_snapshot_after_the_watch_stops(cx: &mut TestAppContext) {
    let mut updated = snapshot();
    updated.waits.clear();
    let reader = Scripted::new(vec![Ok(snapshot()), Ok(updated)]);
    let (facts, changes) = start(cx, &reader);
    drop(changes);
    pass(cx, RECOVERY_TICK);
    assert_eq!(reader.reads(), 2);
    facts.read_with(cx, |facts, _| {
        assert!(facts.snapshot().unwrap().waits.is_empty())
    });
}

#[gpui::test]
fn a_minute_of_age_updates_does_not_start_a_snapshot_read(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![]);
    let (_facts, _changes) = start(cx, &reader);
    pass(cx, Duration::from_secs(60));
    assert_eq!(reader.reads(), 1, "only the launch read");
}

#[gpui::test]
fn a_broken_config_problem_says_what_the_engine_wrote(cx: &mut TestAppContext) {
    let reader = Scripted::new(vec![Err(broken_config())]);
    let (facts, _changes) = start(cx, &reader);
    let text = facts.read_with(cx, |facts, _| facts.problem().expect("the problem").text());
    assert!(text.contains("The engine ended with code 2"), "{text}");
    assert!(text.contains("config problem: broken.toml"), "{text}");
}

#[gpui::test]
fn a_deadline_crossing_announces_the_wake_once_without_reading_the_engine(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["waits"][0]["is_snoozed"] = true.into();
    let deadline = "2099-11-22T18:00:00Z";
    document["waits"][0]["snoozed_until"] = deadline.into();
    let snapshot = Snapshot::parse(document.to_string().as_bytes()).unwrap();
    let reader = Scripted::new(vec![Ok(snapshot)]);
    let (facts, _changes) = start(cx, &reader);
    let wakes = Rc::new(Cell::new(0));
    let count = wakes.clone();
    let _watch = cx.update(|cx| {
        cx.subscribe(&facts, move |_, _: &WaitsChanged, _| {
            count.set(count.get() + 1)
        })
    });
    let wake = sakura_core::engine::moment::iso_seconds(deadline).unwrap();
    facts.update(cx, |facts, cx| facts.tick_ages(wake - 1, cx));
    assert_eq!(wakes.get(), 0);
    facts.update(cx, |facts, cx| facts.tick_ages(wake, cx));
    assert_eq!(wakes.get(), 1);
    facts.update(cx, |facts, cx| facts.tick_ages(wake + 1, cx));
    assert_eq!(wakes.get(), 1);
    assert_eq!(reader.reads(), 1);
}

#[gpui::test]
fn quiet_track_reads_do_not_notify_snapshot_observers(cx: &mut TestAppContext) {
    let current = || Snapshot::parse(br#"{"api":1,"tracks":[],"track_actions":[]}"#).unwrap();
    let reader = Scripted::new(vec![Ok(current()), Ok(current())]);
    let (facts, changes) = start(cx, &reader);
    let noticed = Rc::new(Cell::new(false));
    let flag = noticed.clone();
    let _watch = cx.update(|cx| cx.observe(&facts, move |_, _| flag.set(true)));
    changes.send(()).unwrap();
    pass(cx, CHANGE_DEBOUNCE);
    assert_eq!(reader.reads(), 2);
    assert!(!noticed.get());
    facts.read_with(cx, |facts, _| {
        assert_eq!(facts.snapshot(), Some(&current()));
        assert_ne!(facts.snapshot(), Some(&Snapshot::default()));
    });
}
