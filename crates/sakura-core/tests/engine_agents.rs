//! The lane's agents as `sakura flow sessions`, `flow feed --run` and `flow steer --run` print them: the
//! fixtures are written from the engine's contract, and every command runs against a script that stands
//! in for the engine.

use sakura_core::engine::agents::{Doing, Listening, Outcome, RunState};
use sakura_core::engine::cli::EngineCli;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const AGENTS: &str = include_str!("fixtures/engine-agents.json");
const AGENT_FEED: &str = include_str!("fixtures/engine-agent-feed.json");

/// Writes `text` to `path` as an executable script through `cp`, so this process never holds it open for
/// writing while another test's fork runs it ("text file busy").
fn write_script(path: &Path, text: &str) {
    let source = path.with_extension("txt");
    std::fs::write(&source, text).unwrap();
    let made = Command::new("/bin/sh")
        .args(["-c", r#"/bin/cp "$1" "$2" && /bin/chmod 755 "$2""#, "sh"])
        .args([&source, path])
        .status()
        .unwrap();
    assert!(made.success());
}

/// An engine that is a shell script with `body`, in `dir`; the arguments it was called with are kept in
/// `dir/args` and the folder it ran in in `dir/cwd`.
fn engine(dir: &Path, body: &str) -> EngineCli {
    let path = dir.join("sakura");
    let record = format!(
        "pwd -P > '{0}/cwd'\nprintf '%s\\n' \"$*\" > '{0}/args'\n",
        dir.display()
    );
    write_script(&path, &format!("#!/bin/sh\n{record}{body}\n"));
    EngineCli::at(path).with_deadline(Duration::from_secs(180))
}

/// An engine that prints `text`.
fn engine_saying(dir: &Path, text: &str) -> EngineCli {
    std::fs::write(dir.join("answer.json"), text).unwrap();
    engine(dir, &format!("cat '{}/answer.json'", dir.display()))
}

fn args_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("args"))
        .unwrap()
        .trim()
        .to_string()
}

#[tokio::test]
async fn agents_reads_every_run_of_the_fixture_in_the_engine_s_order() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let runs = engine_saying(dir.path(), AGENTS)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    let names: Vec<&str> = runs.iter().map(|run| run.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "plan-1",
            "build-1",
            "build-1.2",
            "review-1",
            "review-2",
            "check-1"
        ]
    );
    assert_eq!(
        args_of(dir.path()),
        "flow sessions --json -- add-search",
        "one command, in the lane's repository"
    );
}

#[tokio::test]
async fn agents_carry_role_model_account_and_times() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let runs = engine_saying(dir.path(), AGENTS)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    let build = &runs[1];
    assert_eq!(
        (
            build.role.as_str(),
            build.provider.as_str(),
            build.model.as_str()
        ),
        ("builder", "claude", "sonnet")
    );
    assert_eq!(build.account.as_deref(), Some("personal"));
    assert_eq!(build.state, RunState::Finished);
    assert_eq!(build.started_at, "2026-11-19T08:07:02+00:00");
    assert_eq!(build.ended_at.as_deref(), Some("2026-11-19T08:31:48+00:00"));
    let codex = &runs[4];
    assert_eq!(
        (codex.provider.as_str(), codex.account.as_deref()),
        ("codex", None)
    );
    assert_eq!(codex.state, RunState::Running);
    assert_eq!(codex.ended_at, None);
}

#[tokio::test]
async fn agents_say_what_each_one_does_now() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let runs = engine_saying(dir.path(), AGENTS)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    assert_eq!(
        runs[2].doing,
        Some(Doing::Tool {
            tool: "Bash".into(),
            detail: "cargo test -p orchard-search -- --nocapture".into(),
            since: "2026-11-19T09:14:05+00:00".into(),
        })
    );
    assert_eq!(
        runs[3].doing,
        Some(Doing::Text(
            "REVISE: the query parser drops a trailing quote, and nothing tests an empty query."
                .into()
        ))
    );
    assert_eq!(runs[5].doing, None, "nothing was logged");
}

#[tokio::test]
async fn agents_say_how_a_message_reaches_each_one() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let runs = engine_saying(dir.path(), AGENTS)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    assert_eq!(runs[2].listening, Listening::Live);
    assert_eq!(runs[4].listening, Listening::Queue);
    assert_eq!(runs[1].listening, Listening::Resume);
    let Listening::No(words) = &runs[5].listening else {
        panic!("the orphaned run has no channel: {:?}", runs[5].listening);
    };
    assert!(words.starts_with("This agent stopped"), "{words}");
}

#[tokio::test]
async fn a_resumed_run_names_the_run_it_continues() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let runs = engine_saying(dir.path(), AGENTS)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    assert_eq!(runs[2].resumes.as_deref(), Some("build-1"));
    assert_eq!(runs[1].resumes, None);
}

#[tokio::test]
async fn the_step_the_engine_names_for_a_run_is_its_phase_and_an_older_engine_s_phase_still_reads()
{
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let text = r#"[
        {"name": "review-1", "role": "reviewer", "phase": "contract", "step": "plan_review", "state": "finished"},
        {"name": "review-2", "role": "reviewer", "phase": "review", "state": "running"}
    ]"#;
    let runs = engine_saying(dir.path(), text)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    let steps: Vec<&str> = runs.iter().map(|run| run.step_or_phase()).collect();
    assert_eq!(steps, ["plan_review", "review"]);
    let phases: Vec<&str> = runs.iter().map(|run| run.phase.as_str()).collect();
    assert_eq!(
        phases,
        ["contract", "review"],
        "the phase is the session's own, whatever step it serves"
    );
}

#[tokio::test]
async fn a_run_names_its_agent_its_round_and_its_task_and_an_older_engine_says_none_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let text = r#"[
        {"name": "build-2", "role": "builder", "state": "finished", "agent": "build:T2", "round": 2,
         "task_id": "T2", "task_title": "Count characters"},
        {"name": "build-1", "role": "builder", "state": "finished"}
    ]"#;
    let runs = engine_saying(dir.path(), text)
        .agents(repo.path(), "add-search")
        .await
        .unwrap();
    assert_eq!(runs[0].agent, "build:T2");
    assert_eq!(runs[0].round, 2);
    assert_eq!(
        (runs[0].task_id.as_str(), runs[0].task_title.as_str()),
        ("T2", "Count characters")
    );
    assert_eq!(
        (runs[1].agent.as_str(), runs[1].round),
        ("", 0),
        "an older engine: none"
    );
}

#[tokio::test]
async fn an_agent_with_unknown_extra_fields_and_a_state_the_app_does_not_know_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let text = r#"[{"name": "build-1", "role": "builder", "provider": "claude", "model": "sonnet",
        "state": "paused-for-tea", "started_at": "2026-11-19T08:00:00+00:00", "listening": "no",
        "something_new": {"a": 1}}]"#;
    let runs = engine_saying(dir.path(), text)
        .agents(repo.path(), "demo")
        .await
        .unwrap();
    assert_eq!(runs[0].state, RunState::Unknown);
    assert_eq!(runs[0].listening, Listening::No(String::new()));
    assert_eq!(runs[0].account, None);
}

#[tokio::test]
async fn agents_refuse_a_task_that_reads_as_an_option_and_a_folder_that_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), "[]");
    let error = cli.agents(repo.path(), "--json").await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.input_refused");
    let gone = repo.path().join("orchard");
    let error = cli.agents(&gone, "add-search").await.unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.lane_folder_missing");
}

#[tokio::test]
async fn agent_feed_reads_every_kind_the_chat_shows() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let page = engine_saying(dir.path(), AGENT_FEED)
        .agent_feed(repo.path(), "add-search", "build-1.2", None)
        .await
        .unwrap();
    let kinds: Vec<&str> = page.entries.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["you", "engine", "text", "tool", "tool", "tool"]);
    assert!(page.entries[0].is_you() && page.entries[1].is_engine());
    assert!(page.entries[3].is_tool() && !page.entries[2].is_tool());
    assert_eq!(page.state, RunState::Running);
    assert!(page.cursor.starts_with("eyJ"));
    assert_eq!(
        args_of(dir.path()),
        "flow feed --json --run=build-1.2 -- add-search"
    );
}

#[tokio::test]
async fn agent_feed_tools_carry_their_times_and_a_running_one_has_no_end() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let page = engine_saying(dir.path(), AGENT_FEED)
        .agent_feed(repo.path(), "add-search", "build-1.2", None)
        .await
        .unwrap();
    let edit = &page.entries[4];
    assert_eq!(
        edit.started_at.as_deref(),
        Some("2026-11-19T09:13:41+00:00")
    );
    assert_eq!(edit.ended_at.as_deref(), Some("2026-11-19T09:13:43+00:00"));
    let running = &page.entries[5];
    assert_eq!(
        running.command,
        "cargo test -p orchard-search -- --nocapture"
    );
    assert!(running.started_at.is_some() && running.ended_at.is_none());
    assert_eq!(page.entries[2].started_at, None, "words have no times");
}

#[tokio::test]
async fn agent_feed_has_the_live_text_and_none_when_nothing_is_being_written() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let page = engine_saying(dir.path(), AGENT_FEED)
        .agent_feed(repo.path(), "add-search", "build-1.2", None)
        .await
        .unwrap();
    let live = page.live.expect("the fixture has a live tail");
    assert!(live.starts_with("While the tests run"), "{live}");
    let done = r#"{"entries": [], "cursor": "x", "live": null, "state": "finished"}"#;
    let page = engine_saying(dir.path(), done)
        .agent_feed(repo.path(), "add-search", "build-1", None)
        .await
        .unwrap();
    assert_eq!((page.live, page.state), (None, RunState::Finished));
}

#[tokio::test]
async fn agent_feed_after_a_cursor_sends_it_and_ignores_extra_fields() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    engine_saying(dir.path(), AGENT_FEED)
        .agent_feed(repo.path(), "add-search", "build-1.2", Some("eyJ4"))
        .await
        .unwrap();
    assert_eq!(
        args_of(dir.path()),
        "flow feed --json --run=build-1.2 --since=eyJ4 -- add-search"
    );
}

#[tokio::test]
async fn agent_feed_refuses_a_run_that_reads_as_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let error = engine_saying(dir.path(), AGENT_FEED)
        .agent_feed(repo.path(), "add-search", "--json", None)
        .await
        .unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.input_refused");
}

fn outcome_text(outcome: &str, run: &str, words: &str) -> String {
    format!(r#"{{"outcome": "{outcome}", "run": "{run}", "words": "{words}"}}"#)
}

#[tokio::test]
async fn write_to_agent_reads_each_outcome() {
    let cases = [
        ("delivered", Outcome::Delivered, "build-1.2"),
        ("queued", Outcome::Queued, "review-2"),
        ("resumed", Outcome::Resumed, "build-1.3"),
        ("refused", Outcome::Refused, "build-1"),
    ];
    for (name, expected, run) in cases {
        let dir = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let said = outcome_text(name, run, "One sentence.");
        let written = engine_saying(dir.path(), &said)
            .write_to_agent(repo.path(), "add-search", run, "Please go on.")
            .await
            .unwrap();
        assert_eq!(written.outcome, expected, "{name}");
        assert_eq!(written.run, run);
        assert_eq!(written.words, "One sentence.");
    }
}

#[tokio::test]
async fn write_to_agent_sends_the_text_after_a_double_dash_so_it_is_never_an_option() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let said = outcome_text("delivered", "build-1.2", "Sent.");
    engine_saying(dir.path(), &said)
        .write_to_agent(repo.path(), "add-search", "build-1.2", "--help me")
        .await
        .unwrap();
    let actor = sakura_core::engine::actor::app_actor();
    assert_eq!(
        args_of(dir.path()),
        format!("flow steer --run=build-1.2 --actor={actor} --json -- add-search --help me"),
        "the message names the person at the window, and the text stays after the double dash"
    );
}

#[tokio::test]
async fn write_to_agent_refuses_an_empty_text_and_an_outcome_nobody_knows() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let cli = engine_saying(dir.path(), &outcome_text("maybe", "build-1", "?"));
    let error = cli
        .write_to_agent(repo.path(), "add-search", "build-1", "   ")
        .await
        .unwrap_err();
    assert_eq!(error.said.key.0, "error.engine.input_refused");
    let unknown = cli
        .write_to_agent(repo.path(), "add-search", "build-1", "hi")
        .await;
    assert!(
        unknown.is_err(),
        "an outcome the app does not know is not a delivery"
    );
}
