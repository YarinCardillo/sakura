//! What the app asks of `box` and of the screen while nobody touches it: a fake `box` logs every call, a
//! window is opened and ten minutes of ticks are played on the test clock. After the launch nothing is
//! asked, except the one thing no file tells: whether an agent that no terminal of the app is attached to
//! still runs, once a minute. And nothing is drawn: the first page, an empty desk, a desk of idle lavori
//! and an open conversation build no frame and ask for none. The Settings window has the same test beside
//! its own (`settings::tests`).

use super::desk::Desk;
use super::interaction_tests::{click, draw, motion_ended, wait_for};
use super::launch_tests::{calls, fake_agentbox, open_live, settled_calls, Names, TASK};
use super::thread::Phase;
use crate::data::source::Signal;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::models::{AgentPhase, BoxStatus};
use sakura_core::watcher::TICK;
use std::time::{Duration, Instant};

const STATUS: &str = include_str!("../../../sakura-core/tests/fixtures/status.json");
/// The fixture task whose agent runs.
const AT_WORK: &str = "sample-folders";
/// A fixture task with no agent in a terminal: its page is the conversation.
const TALKED: &str = "headings-26";
const TEN_MINUTES: Duration = Duration::from_secs(600);
/// The `box` processes of a launch on a task without an agent: the names, the facts of the task shown,
/// the status, the journal's end, the accounts, whether `box` can cancel, then the task's patch and log,
/// read once.
const AT_LAUNCH: usize = 8;

/// A fake `box` with five tasks; the agent of [`AT_WORK`] runs only when `is_agent_at_work`.
fn five_tasks(is_agent_at_work: bool, cases: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, script) = fake_agentbox(Names::Listed, "0", cases);
    let mut status: BoxStatus = serde_json::from_str(STATUS).expect("the fixture parses");
    let mut fifth = status.tasks[1].clone();
    fifth.name = "fifth".into();
    status.tasks.push(fifth);
    for task in &mut status.tasks {
        if let Some(agent) = task.agent.as_mut().filter(|_| !is_agent_at_work) {
            agent.state = AgentPhase::Exited;
        }
    }
    status.runner.current = None;
    status.runner.queue.clear();
    let write = |name: String, text: String| std::fs::write(dir.path().join(name), text);
    let names: Vec<_> = status
        .tasks
        .iter()
        .map(|t| serde_json::json!({"name": t.name, "profile": t.profile, "base": t.base}))
        .collect();
    write(
        "tasks.json".into(),
        serde_json::Value::from(names).to_string(),
    )
    .expect("the names");
    for task in &status.tasks {
        let facts = serde_json::to_string(task).expect("the facts");
        write(format!("facts-{}.json", task.name), facts).expect("the facts");
    }
    write(
        "status.json".into(),
        serde_json::to_string(&status).expect("the status"),
    )
    .expect("it");
    (dir, script)
}

/// Ten minutes of ticks, one every [`TICK`], each at its own moment of the test clock, so every timer
/// of the app that is due fires. Gives how many frames the window requested meanwhile.
fn idle_ten_minutes(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> usize {
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    let start = Instant::now();
    let mut elapsed = TICK;
    let mut frames = 0;
    while elapsed <= TEN_MINUTES {
        cx.executor().advance_clock(TICK);
        facts.update(cx, |facts, cx| {
            facts.signal_at(Signal::Tick, start + elapsed, cx)
        });
        // A read the tick requested takes its real time; the next tick comes after it, as it would.
        loop {
            cx.run_until_parked();
            if !facts.read_with(cx, |facts, _| facts.is_reading()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        frames += cx.update(|window, cx| window.simulate_next_frame(cx));
        elapsed += TICK;
    }
    frames
}

/// What a window did over ten idle minutes.
#[derive(Debug)]
struct Rest {
    /// The frames it built.
    draws: usize,
    /// The frames it requested.
    frames: usize,
    /// The `box` processes it started.
    asked: Vec<String>,
    is_breathing: bool,
}

impl Rest {
    fn assert_still(&self) {
        assert_eq!(self.draws, 0, "frames were built at rest: {self:#?}");
        assert_eq!(self.frames, 0, "frames were requested at rest: {self:#?}");
        assert!(self.asked.is_empty(), "box was asked at rest: {self:#?}");
        assert!(
            !self.is_breathing,
            "the breath ticks with nothing to breathe"
        );
    }
}

/// How long the motions of an opening last at most, played before the rest is measured: the reveal of
/// the mark, a slide.
const OPENING: Duration = Duration::from_secs(3);

/// Lets what the opening started end (its reads, its one reveal), then plays ten idle minutes.
fn rest(cx: &mut VisualTestContext, desk: &Entity<Desk>, dir: &tempfile::TempDir) -> Rest {
    let mut played = Duration::ZERO;
    while played < OPENING {
        cx.executor().advance_clock(crate::theme::DURATION_INSTANT);
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        played += crate::theme::DURATION_INSTANT;
    }
    let before = settled_calls(cx, dir).len();
    motion_ended(cx);
    let drawn = desk.read_with(cx, |desk, _| desk.draws());
    let frames = idle_ten_minutes(cx, desk);
    let after = settled_calls(cx, dir);
    Rest {
        draws: desk.read_with(cx, |desk, _| desk.draws()) - drawn,
        frames,
        asked: after[before..].to_vec(),
        is_breathing: desk.read_with(cx, |desk, cx| desk.is_breathing(cx)),
    }
}

fn open_settled<'a>(
    cx: &'a mut TestAppContext,
    dir: &tempfile::TempDir,
    script: std::path::PathBuf,
    has_terminals: bool,
) -> (&'a mut VisualTestContext, Entity<Desk>, usize) {
    let (cx, desk, _) = open_live(cx, script, Some(TASK), has_terminals);
    wait_for(cx, "the page", |cx| {
        cx.debug_bounds("changes-body").is_some() || cx.debug_bounds("page-header").is_some()
    });
    let at_launch = settled_calls(cx, dir).len();
    (cx, desk, at_launch)
}

#[gpui::test]
fn ten_idle_minutes_with_five_tasks_start_no_box_process_after_the_launch(cx: &mut TestAppContext) {
    let (dir, script) = five_tasks(false, "");
    let (cx, desk, at_launch) = open_settled(cx, &dir, script, false);
    assert!(
        at_launch <= AT_LAUNCH,
        "{at_launch} at launch: {:#?}",
        calls(&dir)
    );
    // Shown with `--nocapture`: what a launch starts, one line per process.
    println!("box processes of a launch: {:#?}", calls(&dir));
    idle_ten_minutes(cx, &desk);
    let after = settled_calls(cx, &dir);
    assert_eq!(
        after.len(),
        at_launch,
        "asked while idle: {:#?}",
        &after[at_launch..]
    );
    // The projects are read when "New task" opens or when there is no lavoro at all, never for a
    // window that has lavori to show.
    let of_projects = |call: &&String| call.starts_with("project") || call.starts_with("branches");
    assert_eq!(after.iter().filter(of_projects).count(), 0, "{after:#?}");
}

#[gpui::test]
fn an_agent_at_work_that_no_terminal_is_attached_to_is_checked_once_a_minute(
    cx: &mut TestAppContext,
) {
    let (dir, script) = five_tasks(true, "");
    let (cx, desk, at_launch) = open_settled(cx, &dir, script, false);
    idle_ten_minutes(cx, &desk);
    let after = settled_calls(cx, &dir);
    let asked = &after[at_launch..];
    let check = format!("facts {AT_WORK} --json");
    assert!(asked.iter().all(|call| *call == check), "{asked:#?}");
    assert!(asked.len() <= 10, "{} checks in ten minutes", asked.len());
    assert!(asked.len() >= 9, "it is still checked: {}", asked.len());
}

#[gpui::test]
fn an_agent_whose_terminal_is_attached_is_never_asked_about(cx: &mut TestAppContext) {
    let (dir, script) = five_tasks(true, "  attach) printf 'attached\\n'; exec cat ;;");
    let (cx, desk, _) = open_settled(cx, &dir, script, true);
    click(cx, "row-sample-folders");
    wait_for(cx, "the attach", |_| {
        calls(&dir).contains(&format!("attach {AT_WORK}"))
    });
    let attached = settled_calls(cx, &dir).len();
    idle_ten_minutes(cx, &desk);
    let after = settled_calls(cx, &dir);
    assert_eq!(
        after.len(),
        attached,
        "asked while idle: {:#?}",
        &after[attached..]
    );
}

#[gpui::test]
fn the_journal_is_read_when_it_changes_and_never_on_a_tick(cx: &mut TestAppContext) {
    let (dir, script) = five_tasks(false, "");
    let (cx, desk, at_launch) = open_settled(cx, &dir, script, false);
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    facts.update(cx, |facts, cx| {
        facts.signal_at(Signal::Changed(None), Instant::now(), cx)
    });
    let after = settled_calls(cx, &dir);
    let asked = &after[at_launch..];
    assert_eq!(asked.len(), 1, "{asked:#?}");
    assert!(asked[0].starts_with("journal --json"), "{asked:#?}");
}

#[gpui::test]
fn the_first_page_at_rest_builds_no_frame_and_asks_nothing_though_a_check_runs(
    cx: &mut TestAppContext,
) {
    // No lavoro and no project; the sample status still says a verification runs, which no glyph on
    // this page shows.
    let (dir, script) = super::project_setup_tests::empty_box("");
    let (cx, desk, _) = open_live(cx, script, None, false);
    wait_for(cx, "the first page", |cx| {
        cx.debug_bounds("project-choose").is_some()
    });
    let is_verifying = desk.read_with(cx, |desk, cx| {
        let runner = desk.facts.read(cx).runner();
        runner.is_some_and(|runner| runner.current.is_some())
    });
    assert!(is_verifying, "the fixture has a check running");
    rest(cx, &desk, &dir).assert_still();
}

#[gpui::test]
fn an_empty_desk_at_rest_builds_no_frame_and_asks_nothing(cx: &mut TestAppContext) {
    let project = r#"  projects) echo '[{"name":"demo-site","base":"main","where":"mac"}]' ;;"#;
    let (dir, script) = fake_agentbox(Names::Listed, "0", project);
    std::fs::write(dir.path().join("tasks.json"), "[]").expect("no names");
    let empty = r#"{"runner":{"state":"stopped","current":null,"queue":[]},"tasks":[]}"#;
    std::fs::write(dir.path().join("status.json"), empty).expect("no tasks");
    let (cx, desk, _) = open_live(cx, script, None, false);
    wait_for(cx, "the field in place", |cx| {
        cx.debug_bounds("first-task").is_some()
    });
    rest(cx, &desk, &dir).assert_still();
}

#[gpui::test]
fn a_desk_of_idle_lavori_at_rest_builds_no_frame(cx: &mut TestAppContext) {
    let (dir, script) = five_tasks(false, "");
    let (cx, desk, _) = open_settled(cx, &dir, script, false);
    rest(cx, &desk, &dir).assert_still();
}

#[gpui::test]
fn an_open_conversation_at_rest_builds_no_frame_and_asks_nothing(cx: &mut TestAppContext) {
    let agent = super::conversation_tests::fake_agent();
    let cases = format!(
        r#"  accounts) echo '[{{"name":"personal","loggedIn":true}}]' ;;
  acp) exec "{}" --delay-ms 2 ;;
  shell|attach|login) exec cat ;;"#,
        agent.display()
    );
    let (dir, script) = five_tasks(false, &cases);
    let (cx, desk, _) = open_live(cx, script, Some(TALKED), true);
    wait_for(cx, "the conversation", |cx| {
        cx.debug_bounds("thread-composer").is_some()
    });
    let conversation = super::conversation_tests::conversation(cx, &desk, TALKED);
    wait_for(cx, "the conversation to be at rest", |cx| {
        let phase = conversation.read_with(cx, |conversation, _| conversation.phase().clone());
        !matches!(phase, Phase::Preparing | Phase::Connecting | Phase::Loading)
    });
    rest(cx, &desk, &dir).assert_still();
}

#[gpui::test]
fn a_working_lavoro_breathes_only_while_its_glyph_is_on_screen(cx: &mut TestAppContext) {
    let (dir, script) = five_tasks(true, "");
    let (cx, desk, _) = open_settled(cx, &dir, script, false);
    wait_for(cx, "the breath of the row at work", |cx| {
        draw(cx);
        desk.read_with(cx, |desk, cx| desk.is_breathing(cx))
    });
    // The sidebar goes: the row at work is no longer on screen, and the breath ends with it.
    desk.update(cx, |desk, cx| desk.toggle_sidebar(cx));
    wait_for(cx, "the breath to end with the sidebar hidden", |cx| {
        cx.executor().advance_clock(crate::theme::DURATION_INSTANT);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        !desk.read_with(cx, |desk, cx| desk.is_breathing(cx))
    });
    motion_ended(cx);
    let drawn = desk.read_with(cx, |desk, _| desk.draws());
    cx.executor().advance_clock(TEN_MINUTES);
    cx.run_until_parked();
    let frames = cx.update(|window, cx| window.simulate_next_frame(cx));
    assert_eq!(desk.read_with(cx, |desk, _| desk.draws()), drawn);
    assert_eq!(frames, 0);
}
