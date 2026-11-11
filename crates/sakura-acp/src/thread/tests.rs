use super::*;
use crate::model::{DiffContent, PlanStatus};
use crate::policy::Subject;
use serde_json::json;

fn agent(text: &str) -> ThreadEvent {
    ThreadEvent::Update(Update::AgentText(text.to_string()))
}

fn thought(text: &str) -> ThreadEvent {
    ThreadEvent::Update(Update::Thought(text.to_string()))
}

fn fields(id: &str) -> ToolCallFields {
    ToolCallFields::new(id)
}

fn declared(id: &str, title: &str, kind: ToolKind, status: ToolStatus) -> ThreadEvent {
    ThreadEvent::Update(Update::ToolCall(ToolCallFields {
        title: Some(title.to_string()),
        kind: Some(kind),
        status: Some(status),
        content: Some(Vec::new()),
        locations: Some(Vec::new()),
        ..fields(id)
    }))
}

fn updated(id: &str, status: ToolStatus) -> ThreadEvent {
    ThreadEvent::Update(Update::ToolCallUpdate(ToolCallFields {
        status: Some(status),
        ..fields(id)
    }))
}

fn with_content(id: &str, content: Vec<ToolContent>) -> ThreadEvent {
    ThreadEvent::Update(Update::ToolCallUpdate(ToolCallFields {
        content: Some(content),
        ..fields(id)
    }))
}

fn plan_of(states: [PlanStatus; 2]) -> ThreadEvent {
    let entry = |content: &str, status| PlanEntry {
        content: content.to_string(),
        status,
    };
    ThreadEvent::Update(Update::Plan(vec![
        entry("find", states[0]),
        entry("fix", states[1]),
    ]))
}

fn options() -> Vec<PermissionOption> {
    let option = |id: &str, kind| PermissionOption {
        id: id.to_string(),
        name: id.to_string(),
        kind,
    };
    vec![
        option("allow-once", OptionKind::AllowOnce),
        option("reject-once", OptionKind::RejectOnce),
    ]
}

fn thread_after(events: Vec<ThreadEvent>) -> Thread {
    let mut thread = Thread::new();
    for event in events {
        thread.apply(event);
    }
    thread
}

fn tool(thread: &Thread, id: &str) -> ToolCall {
    let found = thread.entries().iter().find_map(|entry| match &entry.kind {
        EntryKind::Tool(call) if call.id == id => Some(call.clone()),
        _ => None,
    });
    found.expect("the tool call has an entry")
}

fn kinds(thread: &Thread) -> Vec<&'static str> {
    let name = |entry: &Entry| match entry.kind {
        EntryKind::User(_) => "user",
        EntryKind::Assistant(_) => "assistant",
        EntryKind::Thought(_) => "thought",
        EntryKind::Tool(_) => "tool",
        EntryKind::Plan(_) => "plan",
        EntryKind::Permission(_) => "permission",
        EntryKind::Problem(_) => "problem",
    };
    thread.entries().iter().map(name).collect()
}

#[test]
fn plain_answer_chunks_become_one_streaming_message_that_settles_at_turn_end() {
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("wave".into()),
        agent("Buon"),
        agent("giorno"),
    ]);
    assert_eq!(kinds(&thread), ["user", "assistant"]);
    assert_eq!(thread.turn(), Turn::Running);
    let EntryKind::Assistant(message) = &thread.entries()[1].kind else {
        panic!("an assistant message");
    };
    assert_eq!(
        (message.text.as_str(), message.is_streaming),
        ("Buongiorno", true)
    );
    thread.apply(ThreadEvent::TurnEnded(StopReason::EndTurn));
    let EntryKind::Assistant(message) = &thread.entries()[1].kind else {
        panic!("an assistant message");
    };
    assert!(!message.is_streaming);
    assert_eq!(thread.turn(), Turn::Idle);
}

#[test]
fn thinking_then_answer_are_two_entries_and_the_thought_stops_streaming_when_the_answer_starts() {
    let thread = thread_after(vec![
        ThreadEvent::Prompted("sort".into()),
        thought("Devo "),
        thought("capire."),
        agent("Here."),
    ]);
    assert_eq!(kinds(&thread), ["user", "thought", "assistant"]);
    let EntryKind::Thought(thought) = &thread.entries()[1].kind else {
        panic!("a thought");
    };
    assert_eq!(
        (thought.text.as_str(), thought.is_streaming),
        ("Devo capire.", false)
    );
}

#[test]
fn tool_update_before_its_tool_call_creates_the_call_and_the_declaration_completes_it() {
    let content = vec![ToolContent::text("found 3")];
    let thread = thread_after(vec![
        ThreadEvent::Prompted("cerca".into()),
        with_content("t1", content.clone()),
        updated("t1", ToolStatus::InProgress),
        declared("t1", "Cerca sort_by", ToolKind::Search, ToolStatus::Pending),
    ]);
    assert_eq!(kinds(&thread), ["user", "tool"]);
    let call = tool(&thread, "t1");
    assert_eq!(call.title, "Cerca sort_by");
    assert_eq!(call.kind, ToolKind::Search);
    assert_eq!(
        call.status,
        ToolStatus::InProgress,
        "a stale pending does not take the call back"
    );
    assert_eq!(
        call.content, content,
        "an empty declaration does not empty the content"
    );
}

#[test]
fn tool_calls_keep_their_position_and_end_in_their_own_states() {
    let thread = thread_after(vec![
        ThreadEvent::Prompted("lavora".into()),
        declared("read", "Read", ToolKind::Read, ToolStatus::Pending),
        declared("search", "Cerca", ToolKind::Search, ToolStatus::InProgress),
        agent("Intanto"),
        updated("read", ToolStatus::Completed),
        updated("search", ToolStatus::Failed),
        updated("read", ToolStatus::InProgress),
    ]);
    assert_eq!(kinds(&thread), ["user", "tool", "tool", "assistant"]);
    assert_eq!(
        tool(&thread, "read").status,
        ToolStatus::Completed,
        "a finished call stays finished"
    );
    assert_eq!(tool(&thread, "search").status, ToolStatus::Failed);
}

#[test]
fn diff_content_replaces_the_content_of_its_call() {
    let patch = crate::diff::file_patch("/work/demo/a.rs", Some("a\n"), "b\n");
    let diff = ToolContent::Diff(DiffContent {
        path: "/work/demo/a.rs".into(),
        patch,
    });
    let thread = thread_after(vec![
        declared(
            "edit",
            "Modifica a.rs",
            ToolKind::Edit,
            ToolStatus::InProgress,
        ),
        with_content("edit", vec![ToolContent::text("working")]),
        with_content("edit", vec![diff.clone()]),
    ]);
    assert_eq!(tool(&thread, "edit").content, [diff]);
}

#[test]
fn terminal_output_arrives_whole_each_time_and_replaces_what_was_shown() {
    let thread = thread_after(vec![
        declared(
            "run",
            "cargo test",
            ToolKind::Execute,
            ToolStatus::InProgress,
        ),
        with_content("run", vec![ToolContent::text("line 1")]),
        with_content("run", vec![ToolContent::text("line 1\nline 2")]),
    ]);
    assert_eq!(
        tool(&thread, "run").content,
        [ToolContent::text("line 1\nline 2")]
    );
}

#[test]
fn plan_is_replaced_whole_within_a_turn_and_is_a_new_entry_after_a_new_prompt() {
    use PlanStatus::{Completed, InProgress, Pending};
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("one".into()),
        plan_of([InProgress, Pending]),
        agent("lavoro"),
        plan_of([Completed, InProgress]),
    ]);
    assert_eq!(kinds(&thread), ["user", "plan", "assistant"]);
    let EntryKind::Plan(entries) = &thread.entries()[1].kind else {
        panic!("a plan");
    };
    assert_eq!(entries[0].status, Completed);
    assert_eq!(entries[1].status, InProgress);
    thread.apply(ThreadEvent::TurnEnded(StopReason::EndTurn));
    thread.apply(ThreadEvent::Prompted("two".into()));
    thread.apply(plan_of([Pending, Pending]));
    assert_eq!(
        kinds(&thread),
        ["user", "plan", "assistant", "user", "plan"]
    );
}

#[test]
fn permission_request_makes_the_call_wait_and_an_allow_starts_it() {
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("scarica".into()),
        ThreadEvent::PermissionAsked {
            request: 7,
            tool: ToolCallFields {
                title: Some("Scarica".into()),
                kind: Some(ToolKind::Fetch),
                status: Some(ToolStatus::Pending),
                ..fields("fetch")
            },
            reason: Reason::Network,
            subject: Subject::default(),
            options: options(),
        },
    ]);
    assert_eq!(kinds(&thread), ["user", "tool", "permission"]);
    assert_eq!(
        tool(&thread, "fetch").status,
        ToolStatus::WaitingForConfirmation
    );
    assert!(thread.waits_for_person());
    thread.apply(updated("fetch", ToolStatus::InProgress));
    assert_eq!(
        tool(&thread, "fetch").status,
        ToolStatus::WaitingForConfirmation
    );
    thread.apply(ThreadEvent::PermissionAnswered {
        request: 7,
        outcome: PermissionOutcome::Selected(OptionKind::AllowOnce),
    });
    assert_eq!(tool(&thread, "fetch").status, ToolStatus::InProgress);
    assert!(!thread.waits_for_person());
}

#[test]
fn permission_refused_rejects_the_call_and_a_second_answer_changes_nothing() {
    let mut thread = thread_after(vec![ThreadEvent::PermissionAsked {
        request: 1,
        tool: fields("fetch"),
        reason: Reason::Network,
        subject: Subject::default(),
        options: options(),
    }]);
    let refuse = PermissionOutcome::Selected(OptionKind::RejectOnce);
    thread.apply(ThreadEvent::PermissionAnswered {
        request: 1,
        outcome: refuse,
    });
    thread.apply(ThreadEvent::PermissionAnswered {
        request: 1,
        outcome: PermissionOutcome::Selected(OptionKind::AllowOnce),
    });
    assert_eq!(tool(&thread, "fetch").status, ToolStatus::Rejected);
    let EntryKind::Permission(permission) = &thread.entries()[1].kind else {
        panic!("a permission");
    };
    assert_eq!(permission.outcome, refuse);
}

#[test]
fn permission_allowed_by_the_policy_starts_the_call_without_a_question() {
    let thread = thread_after(vec![ThreadEvent::PermissionAllowed {
        tool: ToolCallFields {
            title: Some("Modifica".into()),
            kind: Some(ToolKind::Edit),
            status: Some(ToolStatus::Pending),
            ..fields("edit")
        },
    }]);
    assert_eq!(kinds(&thread), ["tool"]);
    assert_eq!(tool(&thread, "edit").status, ToolStatus::InProgress);
    assert!(!thread.waits_for_person());
}

#[test]
fn cancel_marks_unfinished_calls_and_open_questions_at_once_and_keeps_finished_ones() {
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("lavora".into()),
        declared("done", "Read", ToolKind::Read, ToolStatus::Completed),
        declared("pending", "Cerca", ToolKind::Search, ToolStatus::Pending),
        declared(
            "running",
            "cargo test",
            ToolKind::Execute,
            ToolStatus::InProgress,
        ),
        ThreadEvent::PermissionAsked {
            request: 3,
            tool: fields("asking"),
            reason: Reason::Network,
            subject: Subject::default(),
            options: options(),
        },
        agent("sto"),
        ThreadEvent::CancelRequested,
    ]);
    assert_eq!(thread.turn(), Turn::Cancelling);
    assert_eq!(tool(&thread, "done").status, ToolStatus::Completed);
    for id in ["pending", "running", "asking"] {
        assert_eq!(tool(&thread, id).status, ToolStatus::Canceled, "{id}");
    }
    assert!(!thread.waits_for_person());
    thread.apply(updated("running", ToolStatus::InProgress));
    assert_eq!(
        tool(&thread, "running").status,
        ToolStatus::Canceled,
        "a stale update after the cancel"
    );
    thread.apply(updated("running", ToolStatus::Completed));
    assert_eq!(
        tool(&thread, "running").status,
        ToolStatus::Completed,
        "the call really ended"
    );
    thread.apply(ThreadEvent::TurnEnded(StopReason::Cancelled));
    assert_eq!(thread.turn(), Turn::Idle);
    assert_eq!(
        kinds(&thread).last(),
        Some(&"assistant"),
        "a cancel is not a problem"
    );
}

#[test]
fn replay_builds_the_thread_in_one_pass_with_nothing_streaming_or_running() {
    let mut thread = thread_after(vec![ThreadEvent::Prompted("old".into()), agent("old")]);
    thread.replay(vec![
        Update::UserText("Fix ".into()),
        Update::UserText("the sorting".into()),
        Update::Thought("thinking".into()),
        Update::AgentText("Done".into()),
        Update::ToolCall(ToolCallFields {
            title: Some("cargo test".into()),
            status: Some(ToolStatus::InProgress),
            ..fields("run")
        }),
        Update::UserText("Thanks".into()),
    ]);
    assert_eq!(
        kinds(&thread),
        ["user", "thought", "assistant", "tool", "user"]
    );
    assert_eq!(
        thread.entries()[0].kind,
        EntryKind::User("Fix the sorting".into())
    );
    assert!(thread.entries().iter().all(|entry| entry.is_replayed));
    let is_streaming = |entry: &Entry| match &entry.kind {
        EntryKind::Assistant(message) | EntryKind::Thought(message) => message.is_streaming,
        _ => false,
    };
    assert!(!thread.entries().iter().any(is_streaming));
    assert_eq!(tool(&thread, "run").status, ToolStatus::Canceled);
    assert_eq!(thread.turn(), Turn::Idle);
    assert!(thread.take_changes().is_reset);
}

#[test]
fn user_chunk_outside_a_replay_is_the_echo_of_the_prompt_and_is_dropped() {
    let thread = thread_after(vec![
        ThreadEvent::Prompted("wave".into()),
        ThreadEvent::Update(Update::UserText("wave".into())),
    ]);
    assert_eq!(kinds(&thread), ["user"]);
}

#[test]
fn auth_required_is_a_problem_entry_and_ends_the_turn() {
    let problem = Problem {
        kind: ProblemKind::AuthRequired,
        detail: String::new(),
    };
    let thread = thread_after(vec![
        ThreadEvent::Prompted("wave".into()),
        ThreadEvent::Failed(problem.clone()),
    ]);
    assert_eq!(thread.entries()[1].kind, EntryKind::Problem(problem));
    assert_eq!(thread.turn(), Turn::Idle);
}

#[test]
fn crash_in_the_middle_of_a_turn_cancels_what_ran_and_says_why() {
    let thread = thread_after(vec![
        ThreadEvent::Prompted("lavora".into()),
        agent("Starting"),
        declared(
            "run",
            "cargo test",
            ToolKind::Execute,
            ToolStatus::InProgress,
        ),
        ThreadEvent::Failed(Problem {
            kind: ProblemKind::Died,
            detail: "exit code 9".into(),
        }),
    ]);
    assert_eq!(kinds(&thread), ["user", "assistant", "tool", "problem"]);
    assert_eq!(tool(&thread, "run").status, ToolStatus::Canceled);
    assert_eq!(thread.turn(), Turn::Idle);
}

#[test]
fn stop_reasons_other_than_end_and_cancel_are_said() {
    for (reason, kind) in [
        (StopReason::Refusal, ProblemKind::Refusal),
        (StopReason::MaxTokens, ProblemKind::MaxTokens),
        (StopReason::MaxTurnRequests, ProblemKind::MaxTurnRequests),
    ] {
        let thread = thread_after(vec![
            ThreadEvent::Prompted("x".into()),
            ThreadEvent::TurnEnded(reason),
        ]);
        let EntryKind::Problem(problem) = &thread.entries()[1].kind else {
            panic!("a problem");
        };
        assert_eq!(problem.kind, kind);
    }
}

#[test]
fn refusal_keeps_the_providers_words_as_its_detail_and_not_as_an_answer() {
    let words = "API Error: unable to respond to this request (request req_011, see https://example.com/aup)";
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("x".into()),
        agent(words),
        ThreadEvent::TurnEnded(StopReason::Refusal),
    ]);
    assert_eq!(kinds(&thread), ["user", "problem"]);
    let EntryKind::Problem(problem) = &thread.entries()[1].kind else {
        panic!("a problem");
    };
    assert_eq!(problem.kind, ProblemKind::Refusal);
    assert_eq!(problem.detail, words);
    assert!(
        thread.take_changes().is_reset,
        "an entry left the conversation: it is drawn again from its entries"
    );
}

#[test]
fn refusal_takes_only_the_message_that_ends_the_turn() {
    let thread = thread_after(vec![
        ThreadEvent::Prompted("x".into()),
        agent("Guardo il file."),
        declared("read", "Read", ToolKind::Read, ToolStatus::Completed),
        ThreadEvent::TurnEnded(StopReason::Refusal),
    ]);
    assert_eq!(kinds(&thread), ["user", "assistant", "tool", "problem"]);
    let EntryKind::Problem(problem) = &thread.entries()[3].kind else {
        panic!("a problem");
    };
    assert_eq!(problem.detail, "");
}

#[test]
fn five_thousand_chunks_are_one_entry_and_each_change_names_only_that_entry() {
    let mut thread = thread_after(vec![ThreadEvent::Prompted("explain".into())]);
    thread.take_changes();
    for at in 0..5000 {
        thread.apply(agent("word "));
        let changes = thread.take_changes();
        match at {
            0 => assert_eq!(changes.first_new, Some(1)),
            _ => assert_eq!(changes.updated.iter().copied().collect::<Vec<_>>(), [1]),
        }
    }
    assert_eq!(thread.entries().len(), 2);
    let EntryKind::Assistant(message) = &thread.entries()[1].kind else {
        panic!("an assistant message");
    };
    assert_eq!(message.text.len(), 5000 * "word ".len());
}

#[test]
fn changes_name_new_entries_once_and_updated_older_ones() {
    let mut thread = thread_after(vec![
        ThreadEvent::Prompted("x".into()),
        declared("a", "A", ToolKind::Read, ToolStatus::Pending),
    ]);
    let first = thread.take_changes();
    assert_eq!((first.first_new, first.updated.len()), (Some(0), 0));
    thread.apply(updated("a", ToolStatus::Completed));
    thread.apply(agent("fatto"));
    let second = thread.take_changes();
    assert_eq!(second.first_new, Some(2));
    assert_eq!(second.updated.iter().copied().collect::<Vec<_>>(), [1]);
    assert!(thread.take_changes().is_empty());
}

#[test]
fn entry_ids_stay_unique_across_a_replay() {
    let mut thread = thread_after(vec![ThreadEvent::Prompted("x".into()), agent("y")]);
    let before: Vec<u64> = thread.entries().iter().map(|entry| entry.id).collect();
    thread.replay(vec![Update::AgentText("z".into())]);
    assert!(!before.contains(&thread.entries()[0].id));
}

#[test]
fn tool_text_over_the_limit_is_cut_on_a_character_boundary_and_says_so() {
    let text = "è".repeat(crate::model::MAX_TOOL_TEXT_BYTES);
    let ToolContent::Text { text, is_truncated } = ToolContent::text(&text) else {
        panic!("text content");
    };
    assert!(is_truncated);
    assert_eq!(text.len(), crate::model::MAX_TOOL_TEXT_BYTES);
}

#[test]
fn permission_keeps_the_subject_it_was_asked_with_when_the_call_changes_under_it() {
    let asked_about = ToolCallFields {
        title: Some("Esegui i test".into()),
        kind: Some(ToolKind::Execute),
        raw_input: Some(json!({ "command": "curl example.com" })),
        ..fields("run")
    };
    let subject = Subject::of(&asked_about);
    let mut thread = thread_after(vec![ThreadEvent::PermissionAsked {
        request: 1,
        tool: asked_about,
        reason: Reason::Network,
        subject: subject.clone(),
        options: options(),
    }]);
    thread.apply(ThreadEvent::Update(Update::ToolCallUpdate(
        ToolCallFields {
            title: Some("cargo test".into()),
            raw_input: Some(json!({ "command": "cargo test" })),
            ..fields("run")
        },
    )));
    assert_eq!(
        tool(&thread, "run").command(),
        Some("cargo test"),
        "the call's entry follows the agent"
    );
    let EntryKind::Permission(permission) = &thread.entries()[1].kind else {
        panic!("a permission");
    };
    assert_eq!(
        permission.subject, subject,
        "the request is still about what was asked"
    );
    assert_eq!(
        permission.subject.command.as_deref(),
        Some("curl example.com")
    );
}

fn small(entries: usize, bytes: usize) -> Thread {
    Thread::with_limits(Limits { entries, bytes })
}

fn full_notes(thread: &Thread) -> usize {
    let is_note = |entry: &&Entry| matches!(&entry.kind, EntryKind::Problem(problem) if problem.kind == ProblemKind::Full);
    thread.entries().iter().filter(is_note).count()
}

#[test]
fn text_past_the_byte_limit_is_not_kept_and_the_thread_says_once_that_it_is_full() {
    let mut thread = small(100, 64);
    thread.apply(ThreadEvent::Prompted("vai".into()));
    for _ in 0..40 {
        thread.apply(agent("dieci byte"));
        thread.apply(thought("dieci byte"));
    }
    assert!(thread.is_full());
    let held: usize = thread
        .entries()
        .iter()
        .map(|entry| match &entry.kind {
            EntryKind::Assistant(message) | EntryKind::Thought(message) => message.text.len(),
            _ => 0,
        })
        .sum();
    assert!(held <= 64, "{held} bytes of text were kept");
    assert_eq!(full_notes(&thread), 1);
    thread.take_changes();
    thread.apply(agent("ancora"));
    thread.apply(declared(
        "late",
        "Tardi",
        ToolKind::Read,
        ToolStatus::Pending,
    ));
    assert!(
        thread.take_changes().is_empty(),
        "a full thread takes in nothing"
    );
}

#[test]
fn tool_calls_past_the_entry_limit_are_not_kept_nor_remembered() {
    let mut thread = small(8, 1 << 20);
    thread.apply(ThreadEvent::Prompted("vai".into()));
    for at in 0..200 {
        thread.apply(declared(
            &format!("call-{at}"),
            "Legge",
            ToolKind::Read,
            ToolStatus::Pending,
        ));
    }
    assert!(thread.is_full());
    assert!(thread.entries().len() <= 9, "eight entries and the note");
    assert!(thread.tool_index.len() <= 8);
    assert_eq!(full_notes(&thread), 1);
}

#[test]
fn what_a_tool_call_shows_counts_as_long_as_it_is_shown() {
    let mut thread = small(100, 100);
    let output = |text: &str| vec![ToolContent::text(text)];
    thread.apply(declared(
        "a",
        "Esegue",
        ToolKind::Execute,
        ToolStatus::InProgress,
    ));
    for _ in 0..10 {
        thread.apply(with_content("a", output(&"x".repeat(60))));
    }
    assert!(
        !thread.is_full(),
        "output that replaces output holds the same room"
    );
    thread.apply(declared(
        "b",
        "Esegue",
        ToolKind::Execute,
        ToolStatus::InProgress,
    ));
    thread.apply(with_content("b", output(&"y".repeat(60))));
    assert!(thread.is_full(), "two outputs of 60 bytes pass 100");
}

#[test]
fn full_thread_still_shows_the_person_s_prompt_and_ends_its_turn() {
    let mut thread = small(2, 1 << 20);
    thread.apply(ThreadEvent::Prompted("one".into()));
    thread.apply(agent("answer"));
    thread.apply(thought("pensiero"));
    assert!(thread.is_full());
    thread.apply(ThreadEvent::TurnEnded(StopReason::EndTurn));
    thread.apply(ThreadEvent::Prompted("two".into()));
    assert_eq!(thread.turn(), Turn::Running);
    assert!(
        matches!(&thread.entries().last().unwrap().kind, EntryKind::User(text) if text == "two")
    );
    thread.apply(ThreadEvent::TurnEnded(StopReason::EndTurn));
    assert_eq!(thread.turn(), Turn::Idle);
}
