//! One agent's conversation: its entries in order, the text being written at the end, the time of a tool,
//! the box that writes to that agent alone and what it says before and after a send, and the way back.

use super::agent_chat::composer_words;
use super::agent_chat::rows::{
    folded, join_parts, result_words, rows_of, sort_by_id, verdict_of, Joined, Row, ToolRow,
    Verdict,
};
use super::agent_rig_tests::{
    calls_of, open_kept, open_with, settle, AGENTS, AGENT_FEED, EMPTY_PAGE, STEER_DELIVERED,
};
use super::engine_desk::Desk;
use super::engine_rig_tests::{Rig, SNAPSHOT};
use super::lane_tests::{bounds, click};
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::engine::agents::Listening;
use sakura_core::engine::feed::{AgentFeedPage, FeedEntry};
use sakura_core::engine::snapshot::Snapshot;

fn page() -> AgentFeedPage {
    serde_json::from_str(AGENT_FEED).expect("the agent feed fixture")
}

fn entry(kind: &str, text: &str) -> FeedEntry {
    FeedEntry {
        run: "build-1.2".into(),
        role: "builder".into(),
        workspace: "/work/demo".into(),
        kind: kind.into(),
        text: text.into(),
        ..FeedEntry::default()
    }
}

#[test]
fn the_fixture_entries_become_rows_in_order_each_of_its_kind() {
    let rows = rows_of(&page().entries);
    let kinds: Vec<&str> = rows
        .iter()
        .map(|row| match row {
            Row::You(_) => "you",
            Row::Engine(_) => "engine",
            Row::Words { .. } => "words",
            Row::Tool(_) => "tool",
            Row::Result { .. } => "result",
            Row::Round(_) => "round",
        })
        .collect();
    assert_eq!(kinds, ["you", "engine", "words", "tool", "tool", "tool"]);
    assert_eq!(rows[0], Row::You(page().entries[0].text.clone()));
}

#[test]
fn a_result_block_is_its_verdict_and_summary() {
    let mut result = entry("result", "");
    result.result = Some(sakura_core::engine::feed::FeedResult {
        verdict: "pass".into(),
        summary: "214 passed.".into(),
    });
    assert_eq!(
        rows_of(&[result]),
        [Row::Result {
            verdict: "pass".into(),
            summary: "214 passed.".into()
        }]
    );
}

#[test]
fn an_entry_of_a_kind_the_app_does_not_know_still_shows_its_words() {
    let rows = rows_of(&[entry("thought", "Something new.")]);
    assert_eq!(
        rows,
        [Row::Words {
            text: "Something new.".into(),
            is_cut: false
        }]
    );
}

#[test]
fn an_engine_line_shows_the_engine_s_plain_text_and_never_a_raw_ledger_line() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let said = |entry: FeedEntry| match rows_of(&[entry]).remove(0) {
        Row::Engine(words) => words,
        other => panic!("an engine line: {other:?}"),
    };
    let mut plain = entry("engine", "Your message resumed the builder.");
    plain.technical = "steer to build-4.2 resumed actor=app:me".into();
    assert_eq!(
        said(plain),
        "Your message resumed the builder.",
        "the engine's own plain text"
    );
    let raw = entry("engine", "steer to build-4.2 resumed actor=app:me");
    assert_eq!(
        said(raw),
        "Your message resumed the agent",
        "an older engine's ledger line, in words"
    );
    let odd = entry("engine", "something odd happened actor=app:me");
    assert_eq!(
        said(odd),
        "An engine event",
        "a ledger line nobody has words for is not shown"
    );
    let kept = entry("engine", "Resumed build-1 with your message.");
    assert_eq!(
        said(kept),
        "Resumed build-1 with your message.",
        "plain words stay as they are"
    );
}

#[test]
fn the_engine_s_own_files_are_no_step_to_follow() {
    let mut read = entry("tool", "");
    read.tool = "Read".into();
    read.path = "inbox/steer-1.md".into();
    let rows = rows_of(&[read]);
    let is_role_file = sakura_core::files::is_role_file("inbox/steer-1.md");
    assert_eq!(rows.is_empty(), is_role_file);
}

fn tool(started: Option<&str>, ended: Option<&str>) -> ToolRow {
    ToolRow {
        name: "Bash".into(),
        path: String::new(),
        line: None,
        command: "cargo test".into(),
        pattern: String::new(),
        target: String::new(),
        workspace: "/w".into(),
        started_at: started.and_then(sakura_core::engine::moment::iso_seconds),
        ended_at: ended.and_then(sakura_core::engine::moment::iso_seconds),
    }
}

#[test]
fn a_tool_row_shows_the_target_the_engine_names_and_the_name_alone_when_it_names_none() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut read = entry("tool", "");
    read.tool = "Read".into();
    read.target = "tally.py".into();
    let Row::Tool(row) = rows_of(&[read]).remove(0) else {
        panic!("a tool row");
    };
    assert_eq!(
        row.place(),
        "tally.py",
        "from target when no other field says it"
    );
    assert!(row.has_target());
    let mut bare = entry("tool", "");
    bare.tool = "Read".into();
    let Row::Tool(row) = rows_of(&[bare]).remove(0) else {
        panic!("a tool row");
    };
    assert!(!row.has_target(), "the name alone");
}

#[test]
fn a_tool_that_ended_always_says_its_time_even_when_it_was_quick() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let start = "2026-11-19T09:14:05+00:00";
    let quick = tool(Some(start), Some(start));
    assert_eq!(quick.elapsed_words(0).as_deref(), Some("0 s"));
}

#[test]
fn a_tool_names_its_target_a_search_its_pattern_and_one_with_none_has_no_place() {
    let mut search = tool(None, None);
    search.command = String::new();
    search.name = "Grep".into();
    search.pattern = "fn read_phrase".into();
    assert_eq!(search.place(), "fn read_phrase");
    search.pattern = String::new();
    assert_eq!(search.place(), "");
    assert!(!search.has_target());
    search.path = "src/parser.rs".into();
    assert!(search.has_target());
}

#[test]
fn a_tool_that_runs_counts_up_with_the_clock_and_one_that_ended_says_how_long_it_took() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let start = "2026-11-19T09:14:05+00:00";
    let at = |seconds: i64| sakura_core::engine::moment::iso_seconds(start).unwrap() + seconds;
    let running = tool(Some(start), None);
    assert_eq!(running.elapsed_words(at(7)).as_deref(), Some("7 s"));
    assert_eq!(running.elapsed_words(at(8)).as_deref(), Some("8 s"));
    let ended = tool(Some(start), Some("2026-11-19T09:15:15+00:00"));
    assert_eq!(ended.elapsed_words(at(900)).as_deref(), Some("1 min 10 s"));
    assert_eq!(
        tool(None, None).elapsed_words(at(1)),
        None,
        "an engine with no times"
    );
}

#[test]
fn the_box_says_who_it_writes_to_by_how_a_message_reaches_the_agent() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let words = |listening: &Listening| composer_words(listening, "builder");
    assert_eq!(
        words(&Listening::Live).as_deref(),
        Some("Write to the builder")
    );
    assert_eq!(
        words(&Listening::Resume).as_deref(),
        Some("Continue with the builder")
    );
    assert_eq!(
        composer_words(&Listening::Queue, "code_reviewer").as_deref(),
        Some("The code reviewer reads this when it finishes")
    );
    assert_eq!(words(&Listening::No("Why.".into())), None);
}

#[test]
fn the_box_words_exist_in_italian_too() {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(
        composer_words(&Listening::Live, "builder").as_deref(),
        Some("Scrivi al costruttore")
    );
}

#[test]
fn a_message_folds_past_twelve_lines_counting_a_long_line_as_the_rows_it_wraps_onto() {
    let lines = |count: usize| vec!["a line"; count].join("\n");
    assert_eq!(folded(&lines(12)), None, "twelve lines fit");
    let cut = folded(&lines(13)).expect("thirteen lines fold");
    assert_eq!(cut.lines().count(), 12);
    let wide = "x".repeat(30_000);
    let cut = folded(&wide).expect("one endless line folds");
    assert_eq!(cut.chars().count(), 12 * 90);
    let wrapped = format!("{}\n{}", "y".repeat(90 * 11), "second\nthird");
    assert_eq!(folded(&wrapped).expect("folds").lines().count(), 2);
    assert_eq!(folded(""), None);
}

type Shown<'a> = (
    Rig,
    Entity<Desk>,
    String,
    &'a mut VisualTestContext,
    tempfile::TempDir,
);

/// The lane `demo` with the conversation of the agent `run` open, over `feed` and the answer `steer` to a message.
fn show<'a>(cx: &'a mut TestAppContext, run: &str, feed: &str, steer: &str) -> Shown<'a> {
    let repo = tempfile::tempdir().expect("a repository folder");
    let (rig, desk, key, cx) = open_with(cx, repo.path(), SNAPSHOT, [feed, EMPTY_PAGE, EMPTY_PAGE]);
    std::fs::write(rig.dir.path().join("steer.json"), steer).expect("an answer");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| {
        bounds(cx, &format!("agent-{run}")).is_some()
    });
    click(cx, &format!("agent-{run}"));
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    (rig, desk, key, cx, repo)
}

#[gpui::test]
fn opening_an_agent_shows_its_conversation_in_order_with_the_header_of_the_lane_kept(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-5").is_some());
    let tops: Vec<f32> = (0..6)
        .map(|at| {
            let row = bounds(cx, &format!("chat-row-{at}")).expect("a row");
            f32::from(row.top())
        })
        .collect();
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
    assert!(bounds(cx, "chat-you-0").is_some());
    assert!(bounds(cx, "chat-engine-1").is_some());
    assert!(bounds(cx, "chat-answer-2").is_some());
    assert!(
        bounds(cx, "lane-header").is_some(),
        "the lane's header stays"
    );
    assert!(
        bounds(cx, "lane-agents").is_none(),
        "the overview is not shown"
    );
    assert!(bounds(cx, "lane-feed").is_none(), "no merged feed");
    let first = calls_of(&rig, "flow feed")[0].clone();
    assert_eq!(first, "flow feed --json --run=build-1 -- demo");
}

#[gpui::test]
fn the_header_says_the_role_the_model_and_the_account_and_the_state(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    for part in [
        "agent-chat-back",
        "agent-chat-role",
        "agent-chat-detail",
        "agent-chat-state",
    ] {
        assert!(bounds(cx, part).is_some(), "{part}");
    }
}

/// The run `name` as the engine lists a finished builder's round, of the agent `agent`.
fn round_json(name: &str, minute: u32, agent: &str) -> String {
    format!(
        r#"{{"name": "{name}", "role": "builder", "phase": "build", "provider": "claude", "model": "haiku",
        "state": "finished", "started_at": "2026-11-19T09:{minute:02}:00+00:00", "listening": "resume",
        "agent": "{agent}", "task_id": "T1", "task_title": "Count lines"}}"#
    )
}

/// The page of the round `name`: one message, and nothing more after its cursor.
fn write_round_feed(rig: &Rig, name: &str, words: &str) {
    let page = format!(
        r#"{{"entries": [{{"run": "{name}", "role": "builder", "workspace": "/w", "kind": "text",
        "text": "{words}", "tool": "", "path": "", "line": null, "id": "{name}@1:0"}}],
        "cursor": "c1", "live": null, "state": "finished"}}"#
    );
    let more = r#"{"entries": [], "cursor": "c1", "live": null, "state": "finished"}"#;
    let dir = rig.dir.path();
    std::fs::write(dir.join(format!("feed-{name}.json")), page).expect("a page");
    std::fs::write(dir.join(format!("feed-{name}-since.json")), more).expect("a page");
}

#[gpui::test]
fn an_agent_with_several_rounds_shows_every_round_in_order_with_a_quiet_divider_between(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let repo = tempfile::tempdir().expect("a repository folder");
    let (rig, desk, key, cx) = open_with(
        cx,
        repo.path(),
        SNAPSHOT,
        [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE],
    );
    let runs = ["build-1", "build-2", "build-3"];
    let list: Vec<String> = runs
        .iter()
        .zip(1..)
        .map(|(run, at)| round_json(run, at, "build:T1"))
        .collect();
    std::fs::write(
        rig.dir.path().join("agents.json"),
        format!("[{}]", list.join(",")),
    )
    .expect("agents");
    for (run, words) in runs
        .iter()
        .zip(["First round.", "Second round.", "Third round."])
    {
        write_round_feed(&rig, run, words);
    }
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    assert!(
        bounds(cx, "agent-build-2").is_none(),
        "one row for the three rounds"
    );
    click(cx, "agent-build-1");
    settle(cx, "the third round", |cx| {
        bounds(cx, "chat-answer-4").is_some()
    });
    let at = |cx: &mut VisualTestContext, id: &str| {
        f32::from(bounds(cx, id).unwrap_or_else(|| panic!("{id}")).top())
    };
    let order = [
        "chat-answer-0",
        "chat-round-1",
        "chat-answer-2",
        "chat-round-3",
        "chat-answer-4",
    ];
    let tops: Vec<f32> = order.iter().map(|id| at(cx, id)).collect();
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
    for run in runs {
        assert!(reads_of(&rig, run) >= 1, "the round {run} was read");
    }
}

#[gpui::test]
fn the_chat_of_one_of_two_parallel_builders_reads_and_writes_to_its_own_newest_run_only(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let repo = tempfile::tempdir().expect("a repository folder");
    let (rig, desk, key, cx) = open_with(
        cx,
        repo.path(),
        SNAPSHOT,
        [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE],
    );
    let list = [
        round_json("build-1", 1, "build:T1"),
        round_json("build-2", 2, "build:T2"),
        round_json("build-3", 3, "build:T1"),
    ];
    std::fs::write(
        rig.dir.path().join("agents.json"),
        format!("[{}]", list.join(",")),
    )
    .expect("agents");
    for (run, words) in [
        ("build-1", "T1 first."),
        ("build-2", "T2 only."),
        ("build-3", "T1 fix."),
    ] {
        write_round_feed(&rig, run, words);
    }
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    assert!(
        bounds(cx, "agent-build-2").is_some(),
        "the other task has its row"
    );
    assert!(bounds(cx, "agent-build-3").is_none(), "a round is no row");
    click(cx, "agent-build-1");
    settle(cx, "the fix round", |cx| {
        bounds(cx, "chat-answer-2").is_some()
    });
    assert_eq!(
        reads_of(&rig, "build-2"),
        0,
        "the other builder's conversation is not read"
    );
    // The person's line, once the engine has it in the newest run's log, shows in this chat.
    let mine = r#"{"entries": [{"run": "build-3", "role": "builder", "workspace": "/w", "kind": "you",
        "text": "Also add a test.", "tool": "", "path": "", "line": null, "id": "build-3@2:0"}],
        "cursor": "c2", "live": null, "state": "finished"}"#;
    std::fs::write(rig.dir.path().join("feed-build-3-since.json"), mine).expect("a page");
    write(cx, &desk, &key, "Also add a test.");
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    assert!(
        calls_of(&rig, "flow steer")[0].contains("--run=build-3"),
        "the box writes to the run this chat shows last: {:?}",
        calls_of(&rig, "flow steer")
    );
    settle(cx, "the line as mine", |cx| {
        bounds(cx, "chat-you-3").is_some()
    });
}

#[gpui::test]
fn the_text_being_written_stands_at_the_end_and_gives_way_to_the_message_when_it_is_complete(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the live text", |cx| bounds(cx, "chat-live").is_some());
    let complete = r#"{"entries": [{"run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "text", "text": "The empty query returns no results.", "tool": "", "path": "", "line": null}],
        "cursor": "c3", "live": null, "state": "running"}"#;
    std::fs::write(rig.dir.path().join("page2.json"), complete).expect("a page");
    settle(cx, "the message", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-answer-6").is_some() && bounds(cx, "chat-live").is_none()
    });
    let reads = calls_of(&rig, "flow feed");
    assert!(
        reads.last().is_some_and(|call| call.contains("--since=")),
        "{reads:?}"
    );
}

#[gpui::test]
fn a_tool_sent_again_when_it_ends_takes_the_place_of_its_running_row_and_is_not_added_twice(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-5").is_some());
    let ended = r#"{"entries": [
        {"id": "build-1.2@1500:0", "run": "build-1.2", "role": "builder", "workspace": "/w", "kind": "tool",
         "text": "", "tool": "Bash", "path": "", "command": "cargo test -p orchard-search -- --nocapture",
         "line": null, "started_at": "2026-11-19T09:14:05+00:00", "ended_at": "2026-11-19T09:15:17+00:00"},
        {"id": "build-1.2@1600:0", "run": "build-1.2", "role": "builder", "workspace": "/w", "kind": "text",
         "text": "The tests pass.", "tool": "", "path": "", "line": null}],
        "cursor": "c3", "live": null, "state": "running"}"#;
    std::fs::write(rig.dir.path().join("page2.json"), ended).expect("a page");
    settle(cx, "the new message", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-answer-6").is_some()
    });
    assert!(
        bounds(cx, "chat-row-7").is_none(),
        "the tool is not added twice"
    );
}

#[test]
fn an_entry_whose_id_is_already_shown_replaces_that_row_and_the_rest_are_added() {
    let shown =
        std::collections::HashMap::from([("a@1:0".to_string(), 0), ("a@2:0".to_string(), 1)]);
    let running = Row::Tool(tool(Some("2026-11-19T09:14:05+00:00"), None));
    let ended = Row::Tool(tool(
        Some("2026-11-19T09:14:05+00:00"),
        Some("2026-11-19T09:15:17+00:00"),
    ));
    assert_ne!(running, ended);
    let words = Row::Words {
        text: "The tests pass.".into(),
        is_cut: false,
    };
    let (replaced, added) = sort_by_id(
        &shown,
        vec![
            ("a@2:0".to_string(), ended.clone()),
            ("a@3:0".to_string(), words.clone()),
        ],
    );
    assert_eq!(replaced, vec![(1, ended)]);
    assert_eq!(added, vec![("a@3:0".to_string(), words)]);
}

#[gpui::test]
fn the_next_part_of_a_long_text_grows_the_message_already_shown(cx: &mut TestAppContext) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-5").is_some());
    // The text that was streaming ends with a tool call, so the long text that follows is not the one that
    // was being read.
    let tool = r#"{"entries": [{"id": "build-1.2@1550:0", "run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "tool", "text": "", "tool": "Bash", "command": "ls", "path": "", "line": null, "started_at": "2026-11-19T09:14:05+00:00"}],
        "cursor": "c2b", "live": null, "state": "running"}"#;
    std::fs::write(rig.dir.path().join("page2.json"), tool).expect("a page");
    settle(cx, "the tool", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-tool-6").is_some()
    });
    let first = r#"{"entries": [{"id": "build-1.2@1600:0", "run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "text", "text": "One line.\n", "continued": true, "tool": "", "path": "", "line": null}],
        "cursor": "c3", "live": null, "state": "running"}"#;
    std::fs::write(rig.dir.path().join("page2.json"), first).expect("a page");
    settle(cx, "the first part", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-answer-7").is_some()
    });
    assert!(
        bounds(cx, "chat-more-7").is_none(),
        "one line is not folded"
    );
    let rest = "More.\\n".repeat(30);
    let second = format!(
        r#"{{"entries": [{{"id": "build-1.2@1600:0+1", "run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "text", "text": "{rest}", "continued": false, "tool": "", "path": "", "line": null}},
        {{"id": "build-1.2@1700:0", "run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "text", "text": "Done.", "tool": "", "path": "", "line": null}}],
        "cursor": "c4", "live": null, "state": "running"}}"#
    );
    std::fs::write(rig.dir.path().join("page2.json"), second).expect("a page");
    settle(cx, "the message grown past the fold", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-more-7").is_some()
    });
    assert!(
        bounds(cx, "chat-row-9").is_none(),
        "the part is no new message"
    );
}

#[gpui::test]
fn a_message_of_thirty_thousand_characters_is_folded_and_opens_on_show_all(
    cx: &mut TestAppContext,
) {
    let long = format!(
        "```\n{}\n```",
        "let x = 1; // a long line of code\n".repeat(900)
    );
    let text = serde_json::Value::from(long).to_string();
    // An older message folds; the last one is the one being read and never does.
    let feed = format!(
        r#"{{"entries": [{{"run": "build-1", "role": "builder", "workspace": "/w", "kind": "text",
        "text": {text}, "tool": "", "path": "", "line": null, "id": "a"}},
        {{"run": "build-1", "role": "builder", "workspace": "/w", "kind": "text",
        "text": "Done.", "tool": "", "path": "", "line": null, "id": "b"}}],
        "cursor": "c1", "live": null, "state": "finished"}}"#
    );
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", &feed, STEER_DELIVERED);
    settle(cx, "the fold", |cx| bounds(cx, "chat-more-0").is_some());
    assert!(
        bounds(cx, "chat-more-1").is_none(),
        "the last message is open"
    );
    let folded = bounds(cx, "chat-fold-0").expect("the folded text");
    click(cx, "chat-more-0");
    settle(cx, "the whole message", |cx| {
        bounds(cx, "chat-more-0").is_none()
    });
    let whole = bounds(cx, "chat-body-0").expect("the whole text");
    assert!(whole.size.height > folded.size.height);
}

#[gpui::test]
fn the_latest_message_of_an_agent_is_open_when_the_chat_is_opened_after_it_ended(
    cx: &mut TestAppContext,
) {
    let long: String = (1..=30).map(|n| format!("Paragraph {n}.\n\n")).collect();
    let text = serde_json::Value::from(long).to_string();
    let feed = format!(
        r#"{{"entries": [{{"run": "build-1", "role": "builder", "workspace": "/w", "kind": "text",
        "text": {text}, "tool": "", "path": "", "line": null, "id": "a"}}],
        "cursor": "c1", "live": null, "state": "finished"}}"#
    );
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", &feed, STEER_DELIVERED);
    settle(cx, "the message", |cx| {
        bounds(cx, "chat-answer-0").is_some()
    });
    assert!(
        bounds(cx, "chat-more-0").is_none(),
        "its last message is not folded under Show all"
    );
}

#[gpui::test]
fn the_answer_being_read_as_it_streams_stays_open_when_it_ends_and_an_older_one_folds(
    cx: &mut TestAppContext,
) {
    let long: String = (1..=30)
        .map(|n| format!("Paragraph {n} of the answer.\n\n"))
        .collect();
    let text = serde_json::Value::from(long.clone()).to_string();
    let live = format!(
        r#"{{"entries": [], "cursor": "c1", "live": {{"text": {text}}}, "state": "running"}}"#
    );
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", &live, STEER_DELIVERED);
    settle(cx, "the live text", |cx| bounds(cx, "chat-live").is_some());
    let done = format!(
        r#"{{"entries": [{{"run": "build-1", "role": "builder", "workspace": "/w", "kind": "text",
        "text": {text}, "tool": "", "path": "", "line": null, "id": "build-1@9:0"}}],
        "cursor": "c2", "live": null, "state": "running"}}"#
    );
    std::fs::write(rig.dir.path().join("page2.json"), done).expect("a page");
    settle(cx, "the message", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "chat-answer-0").is_some() && bounds(cx, "chat-live").is_none()
    });
    assert!(
        bounds(cx, "chat-more-0").is_none(),
        "the message the person was reading is not folded under Show all"
    );
}

#[gpui::test]
fn the_box_of_an_agent_that_listens_is_there_and_one_that_cannot_be_reached_says_why(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    assert!(bounds(cx, "lane-composer").is_some());
    assert!(bounds(cx, "chat-not-listening").is_none());
    let (_rig, _desk, _key, cx, _repo) = show(cx, "check-1", EMPTY_PAGE, STEER_DELIVERED);
    assert!(
        bounds(cx, "lane-composer").is_none(),
        "nothing can read a message"
    );
    assert!(
        bounds(cx, "chat-not-listening").is_some(),
        "the words of the engine stand there"
    );
}

fn write(cx: &mut VisualTestContext, desk: &Entity<Desk>, key: &str, text: &str) {
    let field = desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(key, cx).expect("the page");
        page.read(cx).composer_field(cx).expect("the field")
    });
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    settle(cx, "the text in the box", |cx| {
        bounds(cx, "lane-send").is_some()
    });
}

#[gpui::test]
fn a_message_goes_to_that_agent_alone_and_the_box_empties_once_it_was_taken(
    cx: &mut TestAppContext,
) {
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    write(cx, &desk, &key, "Use argparse");
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    assert_eq!(
        calls_of(&rig, "flow steer")[0],
        format!(
            "flow steer --run=build-1.2 --actor={} --json -- demo Use argparse",
            sakura_core::engine::actor::app_actor()
        ),
        "to the run that works now, which is the agent"
    );
    settle(cx, "the box emptied", |cx| {
        desk.read_with(cx, |desk, cx| {
            let page = desk.lane_for_test(&key, cx).expect("the page");
            page.read(cx).draft(cx).is_empty()
        })
    });
    assert!(
        bounds(cx, "chat-outcome").is_none(),
        "a delivery needs no word"
    );
}

#[gpui::test]
fn the_message_shows_in_the_chat_in_the_frame_the_box_empties(cx: &mut TestAppContext) {
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-5").is_some());
    write(cx, &desk, &key, "Use argparse");
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    settle(cx, "the box emptied", |cx| {
        desk.read_with(cx, |desk, cx| {
            let page = desk.lane_for_test(&key, cx).expect("the page");
            page.read(cx).draft(cx).is_empty()
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        bounds(cx, "chat-you-6").is_some(),
        "the person's message stands where the next row goes, without waiting for the engine's feed"
    );
}

#[gpui::test]
fn pressing_send_twice_sends_once_and_the_box_waits_for_the_outcome(cx: &mut TestAppContext) {
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    std::fs::write(rig.dir.path().join("steer-delay"), "2").expect("a delay");
    write(cx, &desk, &key, "Use argparse");
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    if bounds(cx, "lane-send").is_some() {
        click(cx, "lane-send");
    }
    cx.run_until_parked();
    assert_eq!(calls_of(&rig, "flow steer").len(), 1, "one send, not two");
}

#[gpui::test]
fn a_refusal_is_said_in_the_engine_s_words_and_the_box_keeps_the_message(cx: &mut TestAppContext) {
    let refused =
        r#"{"outcome": "refused", "run": "build-1.2", "words": "The lane's folder is gone."}"#;
    let (_rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, refused);
    write(cx, &desk, &key, "Use argparse");
    click(cx, "lane-send");
    settle(cx, "the words", |cx| bounds(cx, "chat-outcome").is_some());
    let kept = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx)
            .expect("the page")
            .read(cx)
            .draft(cx)
    });
    assert_eq!(kept, "Use argparse");
}

#[gpui::test]
fn a_queued_message_says_it_waits_and_a_resume_says_it_started(cx: &mut TestAppContext) {
    let queued = r#"{"outcome": "queued", "run": "review-2", "words": "The reviewer reads it when it finishes."}"#;
    let (_rig, desk, key, cx, _repo) = show(cx, "review-2", AGENT_FEED, queued);
    write(cx, &desk, &key, "Also check the empty query");
    click(cx, "lane-send");
    settle(cx, "the words", |cx| bounds(cx, "chat-outcome").is_some());
}

#[gpui::test]
fn the_words_about_a_resumed_run_go_when_that_run_ends(cx: &mut TestAppContext) {
    let resumed =
        r#"{"outcome": "resumed", "run": "build-1.2", "words": "The builder is back at it."}"#;
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, resumed);
    write(cx, &desk, &key, "Summarize what you changed");
    click(cx, "lane-send");
    settle(cx, "the words", |cx| bounds(cx, "chat-outcome").is_some());
    // The run the words name finishes: they have nothing left to say.
    let ended = AGENTS.replacen("\"state\": \"running\"", "\"state\": \"finished\"", 1);
    std::fs::write(rig.dir.path().join("agents.json"), ended).expect("agents");
    settle(cx, "the words to go", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENTS_TICK);
        bounds(cx, "chat-outcome").is_none()
    });
}

/// The fixture whose lane `demo` has a goal of many lines, so that its overview scrolls.
fn with_a_long_goal() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let demo = items
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .expect("demo");
    demo["goal"] = "A line of the goal.\n\n".repeat(60).into();
    snapshot.to_string()
}

#[gpui::test]
fn the_back_arrow_returns_to_the_overview_which_kept_its_scroll(cx: &mut TestAppContext) {
    let repo = tempfile::tempdir().expect("a repository folder");
    let pages = [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE];
    let (_rig, desk, key, cx) = open_with(cx, repo.path(), &with_a_long_goal(), pages);
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, _| page.scroll_overview(gpui::px(-120.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let moved = page.read_with(cx, |page, _| page.overview_offset());
    assert!(f32::from(moved.y) < -1., "the overview scrolls: {moved:?}");
    page.update(cx, |page, cx| page.open_agent_for_test("build-1", cx));
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| {
        bounds(cx, "lane-overview").is_some()
    });
    assert_eq!(page.read_with(cx, |page, _| page.overview_offset()), moved);
}

#[gpui::test]
fn cmd_bracket_closes_the_conversation_and_leaves_the_tab_on_the_lane(cx: &mut TestAppContext) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    cx.simulate_keystrokes("cmd-[");
    settle(cx, "the overview", |cx| bounds(cx, "lane-agents").is_some());
    assert!(bounds(cx, "agent-chat").is_none());
    assert!(
        bounds(cx, "lane-header").is_some(),
        "the tab stays on the lane"
    );
}

/// Lets the clock run for `ticks` reading intervals, with the real time the fake engine's processes take.
fn run_clock(cx: &mut VisualTestContext, ticks: usize) {
    for _ in 0..ticks {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(40));
        cx.run_until_parked();
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
}

fn reads_of(rig: &Rig, run: &str) -> usize {
    let mark = format!("--run={run} ");
    calls_of(rig, "flow feed")
        .iter()
        .filter(|call| call.contains(&mark))
        .count()
}

fn lists_read(rig: &Rig) -> usize {
    calls_of(rig, "flow sessions").len()
}

#[gpui::test]
fn a_conversation_that_is_not_shown_stops_reading_and_reads_at_once_when_it_is_shown_again(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    run_clock(cx, 3);
    assert!(reads_of(&rig, "build-1") >= 2, "it reads while it is shown");
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| bounds(cx, "lane-agents").is_some());
    run_clock(cx, 1);
    let hidden = reads_of(&rig, "build-1");
    run_clock(cx, 8);
    assert_eq!(
        reads_of(&rig, "build-1"),
        hidden,
        "back on the overview it reads nothing"
    );
    click(cx, "agent-build-1");
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    assert!(
        bounds(cx, "chat-row-0").is_some(),
        "its rows were kept: they are there before the read answers"
    );
    settle(cx, "the read on opening", |_| {
        reads_of(&rig, "build-1") > hidden
    });
    let reads = calls_of(&rig, "flow feed");
    let last = reads
        .iter()
        .rfind(|call| call.contains("--run=build-1 "))
        .expect("a read");
    assert!(
        last.contains("--since="),
        "it reads on from the cursor it kept: {last}"
    );
}

#[gpui::test]
fn opening_another_agent_stops_the_first_one_reading(cx: &mut TestAppContext) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    run_clock(cx, 2);
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| {
        bounds(cx, "agent-review-1").is_some()
    });
    click(cx, "agent-review-1");
    settle(cx, "the other conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    run_clock(cx, 1);
    let (first, second) = (reads_of(&rig, "build-1"), reads_of(&rig, "review-1"));
    run_clock(cx, 6);
    assert_eq!(
        reads_of(&rig, "build-1"),
        first,
        "the one not shown reads nothing"
    );
    assert!(
        reads_of(&rig, "review-1") > second,
        "the one shown reads on"
    );
}

#[gpui::test]
fn a_lane_page_that_is_not_shown_reads_neither_its_agents_nor_a_conversation(
    cx: &mut TestAppContext,
) {
    let (rig, desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    run_clock(cx, 2);
    desk.update(cx, |desk, cx| {
        desk.navigate(sakura_core::tabs::Page::Inbox, false, cx)
    });
    cx.run_until_parked();
    run_clock(cx, 1);
    let (chat, list) = (reads_of(&rig, "build-1"), lists_read(&rig));
    run_clock(cx, 8);
    assert_eq!(
        reads_of(&rig, "build-1"),
        chat,
        "no conversation read while the page is away"
    );
    assert_eq!(
        lists_read(&rig),
        list,
        "no list read while the page is away"
    );
    desk.update(cx, |desk, cx| desk.go_back_for_test(cx));
    settle(cx, "the lane page again", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    settle(cx, "both read again", |_| {
        reads_of(&rig, "build-1") > chat && lists_read(&rig) > list
    });
}

#[gpui::test]
fn a_message_that_was_sent_keeps_its_outcome_while_its_conversation_is_not_shown(
    cx: &mut TestAppContext,
) {
    let refused =
        r#"{"outcome": "refused", "run": "build-1.2", "words": "The lane's folder is gone."}"#;
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, refused);
    std::fs::write(rig.dir.path().join("steer-delay"), "1").expect("a delay");
    write(cx, &desk, &key, "Use argparse");
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| bounds(cx, "lane-agents").is_some());
    std::thread::sleep(std::time::Duration::from_millis(1600));
    cx.run_until_parked();
    click(cx, "agent-build-1");
    settle(cx, "the outcome, said once", |cx| {
        bounds(cx, "chat-outcome").is_some()
    });
}

/// The key of the lane `demo` once the fake engine's snapshot is moved to the folder `repo`.
fn key_in(repo: &std::path::Path) -> String {
    let moved = SNAPSHOT.replace("/work/repo", &repo.display().to_string());
    let probe = Snapshot::parse(moved.as_bytes()).expect("the snapshot");
    let found = probe.items.iter().find(|item| item.task() == "demo");
    found.expect("demo").lane_key()
}

fn stored_drafts(cx: &mut VisualTestContext) -> sakura_core::drafts::Drafts {
    let kept = cx.update(|_, cx| crate::prefs::Preferences::current(cx).drafts);
    sakura_core::drafts::Drafts::restore(kept.as_ref())
}

fn pause(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(crate::theme::DRAFT_PAUSE + std::time::Duration::from_millis(1));
    cx.run_until_parked();
}

fn lane_page_of(
    desk: &Entity<Desk>,
    key: &str,
    cx: &mut VisualTestContext,
) -> Entity<super::lane_page::LanePage> {
    desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(key, cx).expect("the page").clone()
    })
}

/// Types `text` in the box of the open conversation, as a person does.
fn type_in_box(desk: &Entity<Desk>, key: &str, text: &str, cx: &mut VisualTestContext) {
    use gpui::Focusable as _;
    let field = desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(key, cx).expect("the page");
        page.read(cx).composer_field(cx).expect("the field")
    });
    cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    cx.simulate_input(text);
    cx.run_until_parked();
}

/// Opens the conversation of `run` from the overview and types `text` in its box.
fn type_to(desk: &Entity<Desk>, key: &str, run: &str, text: &str, cx: &mut VisualTestContext) {
    click(cx, &format!("agent-{run}"));
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    type_in_box(desk, key, text, cx);
}

#[gpui::test]
fn each_agent_keeps_its_own_unsent_message_in_the_preferences(cx: &mut TestAppContext) {
    let (_rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    type_in_box(&desk, &key, "to the builder", cx);
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| {
        bounds(cx, "agent-review-1").is_some()
    });
    type_to(&desk, &key, "review-1", "to the reviewer", cx);
    pause(cx);
    let kept = stored_drafts(cx);

    let lane = sakura_core::tabs::Page::Lane(key.clone());
    assert_eq!(kept.get_for(&lane, Some("build-1")), Some("to the builder"));
    assert_eq!(
        kept.get_for(&lane, Some("review-1")),
        Some("to the reviewer")
    );
}

/// The preferences of a launch that left a message to the builder and another to the reviewer of the lane `key`.
fn two_drafts_of(key: &str) -> crate::prefs::Preferences {
    let mut drafts = sakura_core::drafts::Drafts::default();
    let lane = sakura_core::tabs::Page::Lane(key.to_string());
    drafts.set_for(&lane, Some("build-1"), "left to the builder");
    drafts.set_for(&lane, Some("review-1"), "left to the reviewer");
    crate::prefs::Preferences {
        drafts: Some(drafts.to_json()),
        ..crate::prefs::Preferences::default()
    }
}

#[gpui::test]
fn a_draft_of_the_last_launch_goes_back_into_its_own_agent_s_box_and_no_other(
    cx: &mut TestAppContext,
) {
    let repo = tempfile::tempdir().expect("a repository folder");
    let lane = key_in(repo.path());
    let pages = [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE];
    let (_rig, desk, key, cx) = open_kept(cx, repo.path(), SNAPSHOT, pages, two_drafts_of(&lane));
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| {
        bounds(cx, "agent-review-1").is_some()
    });
    click(cx, "agent-review-1");
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    let page = lane_page_of(&desk, &key, cx);
    assert_eq!(
        page.read_with(cx, |page, cx| page.draft(cx)),
        "left to the reviewer"
    );
    click(cx, "agent-chat-back");
    settle(cx, "the overview", |cx| {
        bounds(cx, "agent-build-1").is_some()
    });
    click(cx, "agent-build-1");
    settle(cx, "the conversation", |cx| {
        bounds(cx, "agent-chat").is_some()
    });
    assert_eq!(
        page.read_with(cx, |page, cx| page.draft(cx)),
        "left to the builder"
    );
}

#[gpui::test]
fn sending_to_one_agent_forgets_its_draft_and_leaves_the_others(cx: &mut TestAppContext) {
    let repo = tempfile::tempdir().expect("a repository folder");
    let lane = key_in(repo.path());
    let pages = [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE];
    let (rig, desk, key, cx) = open_kept(cx, repo.path(), SNAPSHOT, pages, two_drafts_of(&lane));
    std::fs::write(rig.dir.path().join("steer.json"), STEER_DELIVERED).expect("an answer");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the agents", |cx| bounds(cx, "agent-build-1").is_some());
    click(cx, "agent-build-1");
    settle(cx, "the conversation", |cx| {
        bounds(cx, "lane-send").is_some()
    });
    click(cx, "lane-send");
    settle(cx, "the call", |_| calls_of(&rig, "flow steer").len() == 1);
    settle(cx, "the box emptied", |cx| {
        lane_page_of(&desk, &key, cx).read_with(cx, |page, cx| page.draft(cx).is_empty())
    });
    pause(cx);
    let kept = stored_drafts(cx);
    let page = sakura_core::tabs::Page::Lane(key);
    assert_eq!(kept.get_for(&page, Some("build-1")), None);
    assert_eq!(
        kept.get_for(&page, Some("review-1")),
        Some("left to the reviewer")
    );
}

#[gpui::test]
fn a_read_that_fails_after_the_chat_has_content_says_so_beside_it_keeps_it_and_retries(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows and the live text", |cx| {
        bounds(cx, "chat-row-5").is_some() && bounds(cx, "chat-live").is_some()
    });
    assert!(
        bounds(cx, "agent-chat-problem").is_none(),
        "nothing is wrong yet"
    );
    std::fs::write(rig.dir.path().join("feed-fails"), "").expect("a failing engine");
    settle(cx, "the words", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "agent-chat-problem").is_some()
    });
    assert!(bounds(cx, "chat-row-0").is_some(), "the rows stay");
    assert!(
        bounds(cx, "chat-live").is_some(),
        "so does the text being written"
    );
    let cursor = calls_of(&rig, "flow feed")
        .iter()
        .filter_map(|call| call.split("--since=").nth(1))
        .next()
        .map(|rest| rest.split(' ').next().unwrap_or_default().to_string())
        .expect("a read from the cursor");
    let before = calls_of(&rig, "flow feed").len();
    settle(cx, "the retry", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        calls_of(&rig, "flow feed").len() > before + 1
    });
    let failed = calls_of(&rig, "flow feed");
    assert!(
        failed
            .iter()
            .skip(before)
            .all(|call| call.contains(&format!("--since={cursor}"))),
        "every retry reads from the last good cursor: {failed:?}"
    );
    std::fs::remove_file(rig.dir.path().join("feed-fails")).expect("the engine is back");
    let more = r#"{"entries": [{"run": "build-1.2", "role": "builder", "workspace": "/w",
        "kind": "text", "text": "Back at it.", "tool": "", "path": "", "line": null, "id": "late"}],
        "cursor": "c9", "live": null, "state": "running"}"#;
    std::fs::write(rig.dir.path().join("page2.json"), more).expect("a page");
    settle(cx, "the words gone and the new row there", |cx| {
        cx.executor()
            .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
        bounds(cx, "agent-chat-problem").is_none() && bounds(cx, "chat-answer-6").is_some()
    });
}

fn words(text: &str) -> Row {
    Row::Words {
        text: text.into(),
        is_cut: false,
    }
}

#[test]
fn the_parts_of_a_long_text_join_into_one_message_within_a_page_and_across_pages() {
    let page = vec![
        ("a@1:0".to_string(), words("first "), true),
        ("a@1:0+1".to_string(), words("second "), true),
    ];
    assert_eq!(
        join_parts(false, page),
        Joined {
            tail: None,
            rows: vec![("a@1:0".to_string(), words("first second "))],
            awaits_part: true,
        }
    );
    let next = vec![
        ("a@1:0+2".to_string(), words("last."), false),
        ("a@2:0".to_string(), words("Next message."), false),
    ];
    assert_eq!(
        join_parts(true, next),
        Joined {
            tail: Some("last.".to_string()),
            rows: vec![("a@2:0".to_string(), words("Next message."))],
            awaits_part: false,
        }
    );
}

/// A page of a conversation that adds no entry and whose live text is `text`.
fn live_only(text: &str, cursor: &str) -> String {
    let text = serde_json::Value::from(text);
    format!(
        r#"{{"entries": [], "cursor": "{cursor}", "live": {{"text": {text}}}, "state": "running"}}"#
    )
}

#[gpui::test]
fn the_text_being_written_grows_in_place_the_same_answer_taking_new_words(cx: &mut TestAppContext) {
    let first = live_only("The plan: strip", "c1");
    let (rig, desk, key, cx, _repo) = show(cx, "build-1", &first, STEER_DELIVERED);
    settle(cx, "the live text", |cx| bounds(cx, "chat-live").is_some());
    let chat = open_chat_of(&desk, &key, cx);
    let part = chat.read_with(cx, |chat, _| chat.live_part_id());
    for (at, text) in [
        "The plan: strip the flag",
        "The plan: strip the flag and test it",
    ]
    .iter()
    .enumerate()
    {
        std::fs::write(
            rig.dir.path().join("page2.json"),
            live_only(text, &format!("c{}", at + 2)),
        )
        .expect("a page");
        settle(cx, "the growth", |cx| {
            cx.executor()
                .advance_clock(crate::theme::lane_page::AGENT_CHAT_LIVE_TICK);
            chat.read_with(cx, |chat, _| chat.growths() > at)
        });
    }
    assert_eq!(
        chat.read_with(cx, |chat, _| chat.growths()),
        2,
        "grown twice, in place"
    );
    assert_eq!(
        chat.read_with(cx, |chat, _| chat.live_part_id()),
        part,
        "the answer that streams is the same one: its new words fade in, a rebuilt one would show at once"
    );
}

fn open_chat_of(
    desk: &Entity<Desk>,
    key: &str,
    cx: &mut VisualTestContext,
) -> Entity<super::agent_chat::AgentChat> {
    let page = lane_page_of(desk, key, cx);
    page.read_with(cx, |page, _| page.open_chat().expect("an open chat"))
}

#[gpui::test]
fn revisiting_many_rows_keeps_the_markdown_cache_bounded(cx: &mut TestAppContext) {
    let (_rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    let chat = open_chat_of(&desk, &key, cx);
    let cached = chat.update(cx, |chat, cx| chat.parsed_cache_size_after(300, cx));
    assert_eq!(cached, 128);
}

#[gpui::test]
fn opening_many_agents_bounds_the_inactive_chats(cx: &mut TestAppContext) {
    let (_rig, desk, key, cx, _repo) = show(cx, "build-1", EMPTY_PAGE, STEER_DELIVERED);
    let page = lane_page_of(&desk, &key, cx);
    for index in 0..12 {
        page.update(cx, |page, cx| {
            page.open_agent_for_test(&format!("run-{index}"), cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert_eq!(page.read_with(cx, |page, _| page.cached_chats()), 8);
}

fn is_balanced(text: &str) -> bool {
    text.matches("**").count().is_multiple_of(2) && text.matches('`').count().is_multiple_of(2)
}

#[test]
fn a_fold_ends_at_a_block_boundary_and_never_inside_a_paragraph_with_its_raw_markers() {
    let plain = |n: usize| format!("Paragraph {n} says something plain.");
    let kept: Vec<String> = (0..5).map(plain).collect();
    let straddling = [
        "**Contract (`contract.json`, 10 rows, all `cli`,",
        "viewport null, `prover: check`, `passes:",
        "one`, two, three, four).** More words,",
        "and more words.",
        "The end of the block.",
    ]
    .join("\n");
    let text = format!(
        "{}\n\n{straddling}\n\nThe last paragraph.",
        kept.join("\n\n")
    );
    let cut = folded(&text).expect("it is longer than the fold");
    assert_eq!(
        cut,
        kept.join("\n\n"),
        "the paragraph that crosses the limit is left to Show all"
    );
    assert!(!cut.contains("**") && is_balanced(&cut));
}

#[test]
fn a_fold_inside_one_long_line_leaves_no_marker_that_was_never_closed() {
    let long = format!("**Bold that opens {}", "and goes on ".repeat(400));
    let cut = folded(&long).expect("one endless line folds");
    assert!(
        is_balanced(&cut),
        "no raw ** left open: {}",
        &cut[..cut.len().min(60)]
    );
    let code = format!("Look at `{}", "x".repeat(5000));
    assert!(is_balanced(&folded(&code).expect("folds")));
}

#[test]
fn a_blank_line_inside_a_code_block_is_no_block_boundary() {
    let fenced = format!(
        "```\n{}\n```",
        ["let a = 1;", "", "let b = 2;"].join("\n") + &"\n\nlet c = 3;".repeat(10)
    );
    let text = format!("An introduction.\n\n{fenced}");
    let cut = folded(&text).expect("it is longer than the fold");
    assert_eq!(
        cut, "An introduction.",
        "the fence is not cut in the middle at one of its blank lines"
    );
}

#[test]
fn a_verdict_is_read_in_any_case_and_an_approval_is_never_a_failure() {
    for raw in ["APPROVE", "Approve", "approved", "APPROVED", "lgtm"] {
        assert_eq!(verdict_of(raw), Verdict::Approved, "{raw}");
    }
    for raw in ["pass", "PASSED", "ok"] {
        assert_eq!(verdict_of(raw), Verdict::Passed, "{raw}");
    }
    for raw in ["fail", "FAILED", "ko"] {
        assert_eq!(verdict_of(raw), Verdict::Failed, "{raw}");
    }
    for raw in ["REJECT", "rejected"] {
        assert_eq!(verdict_of(raw), Verdict::Rejected, "{raw}");
    }
    assert_eq!(verdict_of("REVISE"), Verdict::ChangesRequested);
    assert_eq!(
        verdict_of("Needs Work"),
        Verdict::Other("Needs Work".into()),
        "a word nobody knows is shown as it is"
    );
}

#[test]
fn the_sentinels_the_engine_documents_are_said_in_words_and_never_as_they_are_written() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    for (raw, words) in [
        ("NO_CHANGE", "No change made"),
        ("ERROR", "Ended with an error"),
        ("DONE", "Done"),
        ("REJECT", "Declined"),
    ] {
        assert_eq!(verdict_of(raw).words(), words, "{raw}");
        assert!(!verdict_of(raw).words().contains('_'), "{raw}");
    }
    assert!(verdict_of("DONE").is_good(), "a finished run is a success");
    assert!(verdict_of("ERROR").is_bad(), "an error is a failure");
    let no_change = verdict_of("NO_CHANGE");
    assert!(
        !no_change.is_good() && !no_change.is_bad(),
        "no mark of good or bad"
    );
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(verdict_of("NO_CHANGE").words(), "Nessuna modifica");
}

/// The keys of a result block the engine summarises as "<count> <key>": the lists its roles' prompts define.
const RESULT_LIST_KEYS: [&str; 4] = ["findings", "rulings", "ruling_requests", "rows"];

#[test]
fn a_result_card_never_shows_an_engine_field_name_and_follows_what_was_asked() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    for language in [
        crate::l10n::Language::English,
        crate::l10n::Language::Italian,
    ] {
        crate::l10n::set_for_test(language);
        for key in RESULT_LIST_KEYS {
            for count in [1, 3] {
                let (heading, body) = result_words("DONE", &format!("{count} {key}"));
                let shown = format!("{heading} {}", body.unwrap_or_default());
                assert!(!shown.contains('_'), "{key}: {shown}");
            }
        }
        let (_, body) = result_words("DONE", "2 a_field_nobody_knows");
        assert_eq!(
            body, None,
            "a count of something unknown is hidden, not shown raw"
        );
    }
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(
        result_words("DONE", "1 ruling_requests"),
        (
            "Requested a ruling".to_string(),
            Some("1 request for a ruling".to_string())
        ),
        "a builder that requested a ruling did not finish"
    );
    assert_eq!(
        result_words("DONE", "3 findings").1.as_deref(),
        Some("3 findings")
    );
    assert_eq!(result_words("DONE", "1 rows").1.as_deref(), Some("1 check"));
    assert_eq!(
        result_words("REVISE", "The parser drops a quote.")
            .1
            .as_deref(),
        Some("The parser drops a quote.")
    );
    assert_eq!(result_words("DONE", "").0, "Done");
}

#[test]
fn only_a_failure_gets_the_failure_mark() {
    for good in [Verdict::Approved, Verdict::Passed] {
        assert!(good.is_good() && !good.is_bad());
    }
    for bad in [Verdict::Failed, Verdict::Rejected] {
        assert!(bad.is_bad());
    }
    for neither in [Verdict::ChangesRequested, Verdict::Other("x".into())] {
        assert!(
            !neither.is_good() && !neither.is_bad(),
            "{neither:?} is neither"
        );
    }
}

#[test]
fn the_verdict_words_exist_in_english_and_italian() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(verdict_of("APPROVE").words(), "Approved");
    assert_eq!(verdict_of("REVISE").words(), "Changes requested");
    assert_eq!(verdict_of("pass").words(), "Passed");
    assert_eq!(verdict_of("Needs Work").words(), "Needs Work");
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(verdict_of("APPROVE").words(), "Approvato");
    assert_eq!(verdict_of("REVISE").words(), "Modifiche richieste");
}

#[gpui::test]
fn an_approving_result_card_has_a_check_and_never_the_cross(cx: &mut TestAppContext) {
    let result = r#"{"entries": [{"run": "review-1", "role": "code_reviewer", "workspace": "/w", "kind": "result",
        "text": "", "tool": "", "path": "", "line": null, "id": "r1",
        "result": {"verdict": "APPROVE", "summary": "The parser change is right."}}],
        "cursor": "c1", "live": null, "state": "finished"}"#;
    let (_rig, _desk, _key, cx, _repo) = show(cx, "review-1", result, STEER_DELIVERED);
    settle(cx, "the card", |cx| bounds(cx, "chat-result-0").is_some());
    assert!(bounds(cx, "chat-verdict-pass-0").is_some(), "the check");
    assert!(
        bounds(cx, "chat-verdict-fail-0").is_none(),
        "never the cross"
    );
}

#[gpui::test]
fn the_words_of_a_lane_agent_s_chat_are_selected_with_the_mouse_and_copied_with_cmd_c(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-tool-5").is_some());
    for (what, selector) in [
        ("the agent's", "chat-answer-2"),
        ("the person's", "thread-prompt"),
    ] {
        let block = bounds(cx, selector).expect(selector);
        cx.simulate_mouse_down(
            gpui::point(block.left() + gpui::px(24.), block.top() + gpui::px(10.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        for step in 1..=8 {
            let x = block.left()
                + gpui::px(24.)
                + (block.size.width - gpui::px(26.)) * (step as f32 / 8.);
            cx.simulate_mouse_move(
                gpui::point(x, block.bottom() - gpui::px(4.)),
                Some(gpui::MouseButton::Left),
                gpui::Modifiers::none(),
            );
        }
        cx.simulate_mouse_up(
            gpui::point(block.right() - gpui::px(2.), block.bottom() - gpui::px(4.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.simulate_keystrokes("cmd-c");
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        assert!(!copied.is_empty(), "{what} words were copied");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    }
}

/// A feed of one answer of `lines` short paragraphs, "line 0" to "line N": far taller than the window.
fn long_answer_feed(lines: usize) -> String {
    let text: String = (0..lines).map(|n| format!("line {n}\n\n")).collect();
    let entry = serde_json::json!({
        "run": "build-1.2", "role": "builder", "workspace": "/w", "kind": "text", "text": text,
        "tool": "", "path": "", "line": null, "id": "build-1.2@1:0"
    });
    serde_json::json!({"entries": [entry], "cursor": "c1", "live": null, "state": "finished"})
        .to_string()
}

#[gpui::test]
fn a_drag_held_below_a_lane_agent_s_chat_scrolls_it_and_selects_to_the_last_line_of_a_message_taller_than_the_view(
    cx: &mut TestAppContext,
) {
    let feed = long_answer_feed(120);
    let (_rig, _desk, _key, cx, _repo) = show(cx, "build-1", &feed, STEER_DELIVERED);
    settle(cx, "the answer", |cx| bounds(cx, "chat-answer-0").is_some());
    let window = cx.update(|window, _| window.viewport_size());
    // The chat follows the end of the answer: the wheel brings its first line into view.
    let middle = gpui::point(window.width / 2., window.height / 2.);
    for _ in 0..6 {
        cx.simulate_event(gpui::ScrollEvent {
            position: middle,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(1500.))),
            touch_phase: gpui::TouchPhase::Moved,
            ..gpui::ScrollEvent::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let answer = bounds(cx, "chat-answer-0").expect("the answer");
    let start = gpui::point(answer.left() + gpui::px(4.), answer.top() + gpui::px(8.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    let below = gpui::point(answer.left() + gpui::px(40.), window.height - gpui::px(6.));
    cx.simulate_mouse_move(
        below,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::none(),
    );
    for _ in 0..150 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    cx.simulate_mouse_up(below, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_keystrokes("cmd-c");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    // The press lands a few pixels in, so the first letter is not part of the selection.
    assert!(
        copied.trim_start().starts_with("ine 0") && copied.trim_end().ends_with("line 119"),
        "the selection ran from the first to the last line: {} characters copied",
        copied.len()
    );
}

#[gpui::test]
fn cmd_c_copies_the_words_selected_in_a_lane_agent_s_chat_while_its_box_has_the_focus(
    cx: &mut TestAppContext,
) {
    use gpui::Focusable as _;
    let (_rig, desk, key, cx, _repo) = show(cx, "build-1", AGENT_FEED, STEER_DELIVERED);
    settle(cx, "the rows", |cx| bounds(cx, "chat-answer-2").is_some());
    // The box takes the focus when the conversation opens, and the person selects words with the mouse.
    let field = desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(&key, cx).expect("the page").clone();
        page.read(cx).composer_field(cx).expect("the field")
    });
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.set_value("a draft", window, cx));
        field.read(cx).focus_handle(cx).focus(window, cx);
    });
    let block = bounds(cx, "chat-answer-2").expect("the answer");
    cx.simulate_mouse_down(
        gpui::point(block.left() + gpui::px(24.), block.top() + gpui::px(10.)),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    // A press outside a box takes the focus away from it, as the box does for itself.
    cx.update(|window, cx| window.blur(cx));
    for step in 1..=8 {
        let x =
            block.left() + gpui::px(24.) + (block.size.width - gpui::px(26.)) * (step as f32 / 8.);
        cx.simulate_mouse_move(
            gpui::point(x, block.bottom() - gpui::px(4.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::none(),
        );
    }
    cx.simulate_mouse_up(
        gpui::point(block.right() - gpui::px(2.), block.bottom() - gpui::px(4.)),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    cx.simulate_keystrokes("cmd-c");
    let by_key = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        !by_key.is_empty(),
        "the selected words were copied by the key"
    );
    // The Mac's own Edit menu (where Cmd-C comes from) sends the action to whatever has the focus.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    cx.dispatch_action(gpui_component::input::Copy);
    let by_menu = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        !by_menu.is_empty(),
        "the selected words were copied by the menu"
    );
}

#[test]
fn a_record_that_says_running_with_no_process_is_the_end_of_an_answer_not_a_stop_when_the_feed_says_so(
) {
    use super::agent_chat::shown_state;
    use sakura_core::engine::agents::RunState::{Aborted, Finished, Orphaned, Running};
    // The moment between the process ending and the record saying so: no "Stopped" flash.
    assert_eq!(shown_state(Orphaned, Finished), Finished);
    assert_eq!(shown_state(Orphaned, Running), Running);
    // A run that was aborted, or an orphan the feed knows no more of, is stopped.
    assert_eq!(shown_state(Aborted, Finished), Aborted);
    assert_eq!(shown_state(Orphaned, Orphaned), Orphaned);
    assert_eq!(shown_state(Finished, Running), Finished);
}

#[gpui::test]
fn a_lane_whose_only_plan_review_run_is_the_arbiter_s_shows_no_plan_review_in_the_bar_or_in_details(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let repo = tempfile::tempdir().expect("a repository folder");
    let (rig, desk, key, cx) = open_with(
        cx,
        repo.path(),
        SNAPSHOT,
        [AGENT_FEED, EMPTY_PAGE, EMPTY_PAGE],
    );
    let mut runs: Vec<serde_json::Value> = serde_json::from_str(AGENTS).expect("the agents");
    let mut arbiter = runs[0].clone();
    arbiter["name"] = "arbiter-1".into();
    arbiter["role"] = "plan_reviewer".into();
    arbiter["phase"] = "arbiter".into();
    arbiter["step"] = "plan_review".into();
    runs.push(arbiter);
    std::fs::write(
        rig.dir.path().join("agents.json"),
        serde_json::to_string(&runs).expect("agents"),
    )
    .expect("agents");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    settle(cx, "the arbiter's card", |cx| {
        bounds(cx, "agent-arbiter-1").is_some()
    });
    assert!(bounds(cx, "step-seg-build").is_some(), "the bar is drawn");
    assert!(
        bounds(cx, "step-seg-plan_review").is_none(),
        "no plan review in the bar"
    );
    assert!(
        bounds(cx, "review-row-plan_review").is_none(),
        "none in Details"
    );
    assert!(
        bounds(cx, "review-row-task_review").is_some(),
        "the other reviews stay"
    );
}
