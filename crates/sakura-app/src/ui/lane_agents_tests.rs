//! The lane page's list of agents: one card per agent from the engine's runs (a resume folded into the run it
//! continues), the order, what each row says, and the words in both languages.

use super::agent_rig_tests::{calls_of, open_with, settle, AGENT_FEED, EMPTY_PAGE};
use super::engine_desk::Desk;
use super::engine_rig_tests::{Rig, SNAPSHOT};
use super::lane_agents::{cards_of, facts_of};
use super::lane_tests::{bounds, click};
use crate::theme::lane_page::AGENTS_TICK;
use crate::ui::kit::DoingLine;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::engine::agents::AgentRun;

const RUNS: &str = include_str!("../../../sakura-core/tests/fixtures/engine-agents.json");
/// Seconds after the running builder's tool started, in the fixture's clock (2026-11-19 09:14:05 UTC).
const TOOL_STARTED: i64 = 1_795_079_645;

fn runs() -> Vec<AgentRun> {
    serde_json::from_str(RUNS).expect("the agents fixture")
}

fn card_named(origin: &str) -> super::lane_agents::AgentCard {
    let cards = cards_of(&runs());
    let found = cards.into_iter().find(|card| card.origin == origin);
    found.expect("the card")
}

#[test]
fn the_fixture_lists_one_card_per_agent_and_folds_a_resume_into_the_run_it_continues() {
    let cards = cards_of(&runs());
    assert_eq!(cards.len(), 5, "six runs, one of them a resume");
    let builder = card_named("build-1");
    assert_eq!(builder.tail.name, "build-1.2");
    assert_eq!(builder.resumed, 1);
    assert!(cards
        .iter()
        .all(|card| card.tail.name != "build-1" || card.resumed == 0));
}

#[test]
fn cards_follow_the_roles_by_their_start_and_show_the_newest_agent_of_a_role_first() {
    let cards = cards_of(&runs());
    let origins: Vec<&str> = cards.iter().map(|card| card.origin.as_str()).collect();
    assert_eq!(
        origins,
        ["plan-1", "build-1", "review-2", "review-1", "check-1"]
    );
}

#[test]
fn the_second_line_names_the_model_and_the_account_when_there_is_one() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(
        facts_of(&card_named("build-1"), 0).detail,
        "Claude Sonnet, personal"
    );
    assert_eq!(facts_of(&card_named("review-2"), 0).detail, "Codex GPT-5.5");
}

#[test]
fn a_lane_whose_planner_and_builder_run_on_codex_reads_as_codex_with_no_claude_word() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut codex_runs = runs();
    for run in codex_runs
        .iter_mut()
        .filter(|run| matches!(run.role.as_str(), "planner" | "builder"))
    {
        run.provider = "codex".into();
        run.model = "gpt-5.5".into();
    }
    for card in cards_of(&codex_runs)
        .into_iter()
        .filter(|card| matches!(card.origin.as_str(), "plan-1" | "build-1"))
    {
        let facts = facts_of(&card, 0);
        assert!(
            facts.detail.starts_with("Codex GPT-5.5"),
            "{}: {}",
            card.origin,
            facts.detail
        );
        assert!(!facts.detail.contains("Claude"), "{}", facts.detail);
    }
}

#[test]
fn a_running_tool_says_how_long_it_has_run_and_the_time_moves_with_the_clock() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let card = card_named("build-1");
    let elapsed = |now: i64| match facts_of(&card, now).doing {
        Some(DoingLine::Tool { elapsed, .. }) => elapsed.to_string(),
        other => panic!("a running tool: {other:?}"),
    };
    assert_eq!(elapsed(TOOL_STARTED + 67), "1 min 07 s");
    assert_eq!(elapsed(TOOL_STARTED + 68), "1 min 08 s");
}

#[test]
fn a_finished_agent_says_the_first_line_of_what_it_wrote_last() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let facts = facts_of(&card_named("review-1"), 0);
    assert_eq!(facts.state, "Finished");
    let Some(DoingLine::Text(words)) = facts.doing else {
        panic!("its last words");
    };
    assert!(words.starts_with("REVISE: the query parser"), "{words}");
}

#[test]
fn the_row_of_a_resumed_agent_shows_its_resume_and_the_state_of_the_run_that_works_now() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let facts = facts_of(&card_named("build-1"), 0);
    assert_eq!(facts.state, "Working");
    assert_eq!(facts.rounds.as_deref(), Some("Continued once"));
    assert_eq!(facts_of(&card_named("plan-1"), 0).rounds, None);
}

#[test]
fn a_run_that_stopped_is_said_stopped_and_one_with_nothing_logged_has_no_second_line() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let facts = facts_of(&card_named("check-1"), 0);
    assert_eq!(facts.state, "Stopped");
    assert!(facts.doing.is_none());
}

#[test]
fn the_words_exist_in_italian_too() {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    let facts = facts_of(&card_named("build-1"), 0);
    assert_eq!(facts.state, "In corso");
    assert_eq!(facts.rounds.as_deref(), Some("Ripreso una volta"));
    assert_eq!(facts.detail, "Claude Sonnet, personal");
    assert_eq!(facts_of(&card_named("check-1"), 0).state, "Fermato");
}

type Shown<'a> = (
    Rig,
    Entity<Desk>,
    String,
    &'a mut VisualTestContext,
    tempfile::TempDir,
);

/// The lane `demo` shown with its overview, once the agents are listed.
fn show<'a>(cx: &'a mut TestAppContext) -> Shown<'a> {
    let repo = tempfile::tempdir().expect("a repository folder");
    let pages = [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE];
    let (rig, desk, key, cx) = open_with(cx, repo.path(), SNAPSHOT, pages);
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    (rig, desk, key, cx, repo)
}

#[gpui::test]
fn the_overview_lists_one_row_per_agent_in_order_with_what_each_does(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx);
    let tops: Vec<f32> = ["plan-1", "build-1", "review-2", "review-1", "check-1"]
        .iter()
        .map(|run| {
            let row = bounds(cx, &format!("agent-{run}")).expect("a row per agent");
            f32::from(row.top())
        })
        .collect();
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
    assert!(
        bounds(cx, "agent-build-1.2").is_none(),
        "a resume is no row of its own"
    );
    assert!(bounds(cx, "agent-build-1-doing").is_some());
    assert!(bounds(cx, "agent-build-1-detail").is_some());
    assert!(
        bounds(cx, "agent-review-2-latest").is_none()
            && bounds(cx, "agent-review-1-latest").is_none(),
        "no badge between rows of different agents"
    );
}

#[gpui::test]
fn the_overview_has_the_goal_and_the_agents_and_no_composer(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx);
    assert!(bounds(cx, "lane-goal").is_some(), "the goal");
    assert!(bounds(cx, "lane-agents").is_some());
    assert!(bounds(cx, "lane-header").is_some(), "the header stays");
    assert!(
        bounds(cx, "lane-composer").is_none(),
        "one place to write: an agent's page"
    );
    assert!(bounds(cx, "lane-feed").is_none(), "no merged feed");
}

#[gpui::test]
fn the_list_is_read_again_every_three_seconds_while_the_page_is_shown(cx: &mut TestAppContext) {
    let (rig, _desk, _key, cx, _repo) = show(cx);
    for _ in 0..2 {
        let before = calls_of(&rig, "flow sessions").len();
        settle(cx, "another read", |cx| {
            cx.executor().advance_clock(AGENTS_TICK);
            calls_of(&rig, "flow sessions").len() > before
        });
    }
    let calls = calls_of(&rig, "flow sessions");
    assert_eq!(calls[0], "flow sessions --json -- demo");
}

#[gpui::test]
fn pressing_an_agent_asks_the_page_to_open_its_conversation(cx: &mut TestAppContext) {
    let (_rig, desk, key, cx, _repo) = show(cx);
    click(cx, "agent-review-1");
    let open = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx)
            .expect("the page")
            .read(cx)
            .open_agent()
    });
    assert_eq!(open.as_deref(), Some("review-1"));
}

#[test]
fn what_an_agent_wrote_never_reaches_the_row_with_a_hidden_or_bidirectional_control() {
    let mut run = runs().remove(1);
    run.doing = Some(sakura_core::engine::agents::Doing::Text(
        "safe \u{202e}gnp.exe\u{200b} words".into(),
    ));
    run.account = Some("per\u{202e}sonal".into());
    let card = cards_of(&[run]).remove(0);
    let facts = facts_of(&card, 0);
    let Some(DoingLine::Text(words)) = facts.doing else {
        panic!("its last words");
    };
    assert!(
        !words.contains('\u{202e}') && !words.contains('\u{200b}'),
        "{words}"
    );
    assert!(!facts.detail.contains('\u{202e}'), "{}", facts.detail);
}

/// A run of `role` in `phase`, started at `minute` past nine, finished.
fn run_of(name: &str, role: &str, phase: &str, minute: u32) -> AgentRun {
    run_with(name, (role, phase, "haiku"), minute, None)
}

/// A run of `(role, phase, model)` that `resumes` another run when it says which.
fn run_with(
    name: &str,
    (role, phase, model): (&str, &str, &str),
    minute: u32,
    resumes: Option<&str>,
) -> AgentRun {
    let started = format!("2026-11-19T09:{minute:02}:00+00:00");
    let resumes = resumes.map_or("null".to_string(), |run| format!("\"{run}\""));
    let text = format!(
        r#"{{"name": "{name}", "role": "{role}", "phase": "{phase}", "provider": "claude", "model": "{model}",
        "state": "finished", "started_at": "{started}", "listening": "resume", "resumes": {resumes}}}"#
    );
    serde_json::from_str(&text).expect("a run")
}

#[test]
fn a_builder_of_a_fix_round_says_what_it_fixes_and_which_round_it_is() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let runs = [
        of_agent(
            run_of("build-1", "builder", "build", 1),
            "build:build:T1",
            1,
            "T1:Count lines",
        ),
        of_agent(
            run_of("build-2", "builder", "build", 2),
            "build:build:final-1",
            1,
            "",
        ),
        of_agent(
            run_of("build-3", "builder", "build", 3),
            "build:build:fix-2",
            1,
            "",
        ),
    ];
    let cards = cards_of(&runs);
    let task_of = |origin: &str| {
        let card = cards
            .iter()
            .find(|card| card.origin == origin)
            .expect("a card");
        facts_of(card, 0).task.map(|words| words.to_string())
    };
    assert_eq!(task_of("build-1").as_deref(), Some("Count lines"));
    assert_eq!(
        task_of("build-2").as_deref(),
        Some("Fixing the review's findings, round 1")
    );
    assert_eq!(
        task_of("build-3").as_deref(),
        Some("Fixing what the checks found, round 2")
    );
}

#[test]
fn a_planner_row_names_its_own_phase_whatever_step_the_engine_says_it_serves() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    // The engine says the step each of the three planner sessions serves is the plan: the phase tells them apart.
    let planner = |name: &str, phase: &str, minute: u32| {
        let run = run_of(name, "planner", phase, minute);
        AgentRun {
            step: "plan".into(),
            ..of_agent(run, &format!("planner:{phase}"), 1, "")
        }
    };
    let runs = [
        planner("intake-1", "intake", 1),
        planner("contract-1", "contract", 2),
        planner("plan-1", "plan", 3),
        of_agent(run_of("build-1", "builder", "build", 4), "build:T1", 1, ""),
    ];
    let cards = cards_of(&runs);
    let phases: Vec<Option<String>> = cards
        .iter()
        .map(|card| facts_of(card, 0).phase.map(|words| words.to_string()))
        .collect();
    assert_eq!(
        phases,
        [
            Some("Plan".into()),
            Some("Contract".into()),
            Some("Spec".into()),
            None
        ]
    );
}

/// `run` as a run of the engine's agent `agent`, in its round `round` (counted from one), for the task `task`.
fn of_agent(run: AgentRun, agent: &str, round: u32, task: &str) -> AgentRun {
    AgentRun {
        agent: agent.into(),
        round,
        task_id: task.split(':').next().unwrap_or_default().into(),
        task_title: task.split_once(':').map_or("", |(_, title)| title).into(),
        ..run
    }
}

#[test]
fn the_fix_rounds_of_one_agent_are_one_row_showing_the_newest_round_and_its_number() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let runs = [
        run_of("plan-1", "planner", "plan", 1),
        of_agent(run_of("build-1", "builder", "build", 2), "build:T1", 1, ""),
        of_agent(run_of("build-2", "builder", "build", 3), "build:T1", 2, ""),
        of_agent(run_of("build-3", "builder", "build", 4), "build:T1", 3, ""),
    ];
    let cards = cards_of(&runs);
    assert_eq!(cards.len(), 2, "a planner and one builder");
    let builder = &cards[1];
    assert_eq!(
        builder.origin, "build-1",
        "the agent is known by its first round"
    );
    assert_eq!(builder.tail.name, "build-3", "the row is the newest round");
    let origins: Vec<&str> = builder
        .rounds
        .iter()
        .map(|round| round.origin.as_str())
        .collect();
    assert_eq!(origins, ["build-1", "build-2", "build-3"]);
    assert_eq!(facts_of(builder, 0).rounds.as_deref(), Some("Round 3"));
    assert_eq!(
        facts_of(&cards[0], 0).rounds,
        None,
        "one round says no round"
    );
}

#[test]
fn two_parallel_builders_are_two_rows_each_naming_its_task_and_a_fix_round_counts_for_its_own() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let runs = [
        of_agent(
            run_of("build-1", "builder", "build", 1),
            "build:T1",
            1,
            "T1:Count lines",
        ),
        of_agent(
            run_of("build-2", "builder", "build", 2),
            "build:T2",
            1,
            "T2:Count characters",
        ),
        of_agent(
            run_of("build-3", "builder", "build", 3),
            "build:T1",
            2,
            "T1:Count lines",
        ),
    ];
    let cards = cards_of(&runs);
    assert_eq!(cards.len(), 2, "one row per agent");
    let lines = cards
        .iter()
        .find(|card| card.origin == "build-1")
        .expect("T1");
    let characters = cards
        .iter()
        .find(|card| card.origin == "build-2")
        .expect("T2");
    assert_eq!(
        lines.tail.name, "build-3",
        "the fix round of T1 is T1's newest run"
    );
    assert_eq!(facts_of(lines, 0).task.as_deref(), Some("Count lines"));
    assert_eq!(facts_of(lines, 0).rounds.as_deref(), Some("Round 2"));
    assert_eq!(
        facts_of(characters, 0).task.as_deref(),
        Some("Count characters")
    );
    assert_eq!(
        facts_of(characters, 0).rounds,
        None,
        "the other task has one round"
    );
}

#[test]
fn a_planner_s_spec_contract_and_plan_are_three_agents_and_no_rounds() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let runs = [
        of_agent(
            run_of("intake-1", "planner", "intake", 1),
            "planner:spec",
            1,
            "",
        ),
        of_agent(
            run_of("contract-1", "planner", "contract", 2),
            "planner:contract",
            1,
            "",
        ),
        of_agent(
            run_of("plan-1", "planner", "plan", 3),
            "planner:plan",
            1,
            "",
        ),
    ];
    let cards = cards_of(&runs);
    assert_eq!(cards.len(), 3);
    assert!(cards.iter().all(|card| facts_of(card, 0).rounds.is_none()));
}

#[test]
fn an_engine_that_names_no_agent_gets_one_row_per_run_with_its_resumes_folded() {
    let runs = [
        run_of("build-1", "builder", "build", 1),
        run_with(
            "build-1.2",
            ("builder", "build", "haiku"),
            2,
            Some("build-1"),
        ),
        run_of("build-2", "builder", "build", 3),
    ];
    let cards = cards_of(&runs);
    assert_eq!(cards.len(), 2, "build-1 with its resume, and build-2");
    assert_eq!(cards[1].tail.name, "build-1.2");
    assert!(cards.iter().all(|card| card.rounds.len() == 1));
}

#[test]
fn a_resume_stays_in_its_round_and_is_said_as_continued() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let runs = [
        of_agent(run_of("build-1", "builder", "build", 1), "build:T1", 1, ""),
        of_agent(run_of("build-2", "builder", "build", 3), "build:T1", 2, ""),
        of_agent(
            run_with(
                "build-2.2",
                ("builder", "build", "haiku"),
                4,
                Some("build-2"),
            ),
            "build:T1",
            2,
            "",
        ),
    ];
    let card = cards_of(&runs).remove(0);
    assert_eq!(card.rounds.len(), 2);
    assert_eq!(card.tail.name, "build-2.2");
    assert_eq!(
        facts_of(&card, 0).rounds.as_deref(),
        Some("Round 2, continued once")
    );
}

#[test]
fn a_plan_review_is_a_plan_reviewer_and_a_step_review_a_code_reviewer_as_details_names_them() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let plan = cards_of(&[run_of("review-1", "reviewer", "plan_review", 1)]).remove(0);
    assert_eq!(facts_of(&plan, 0).role, "Plan reviewer");
    let step = cards_of(&[run_of("review-2", "reviewer", "review", 2)]).remove(0);
    assert_eq!(facts_of(&step, 0).role, "Code reviewer");
    let own = cards_of(&[run_of("review-3", "plan_reviewer", "plan_review", 3)]).remove(0);
    assert_eq!(facts_of(&own, 0).role, "Plan reviewer");
}

#[test]
fn the_arbiter_is_an_agent_of_its_own_and_never_a_plan_reviewer() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut all = runs();
    let mut arbiter = all[0].clone();
    arbiter.name = "arbiter-1".into();
    arbiter.role = "plan_reviewer".into();
    arbiter.phase = "arbiter".into();
    arbiter.step = "plan_review".into();
    all.push(arbiter);
    let card = cards_of(&all)
        .into_iter()
        .find(|card| card.origin == "arbiter-1")
        .expect("the arbiter's card");
    assert_eq!(facts_of(&card, 0).role, "Arbiter");
}
