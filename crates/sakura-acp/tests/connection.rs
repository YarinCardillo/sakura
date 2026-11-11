//! The connection against the fake agent, as a child process over stdio: every scripted session ends in the
//! entries the conversation shows, and every way the agent can fail ends in one typed event, never a hang.

use sakura_acp::connection::{Agent, AgentConfig, AgentEvent, Deadlines};
use sakura_acp::model::{
    OptionKind, PlanStatus, StopReason, ToolContent, ToolKind, ToolStatus, Update,
};
use sakura_acp::policy::Reason;
use sakura_acp::thread::{
    Entry, EntryKind, PermissionOutcome, ProblemKind, Thread, ThreadEvent, ToolCall, Turn,
};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

/// No test waits longer than this for an event.
const PATIENCE: Duration = Duration::from_secs(20);
/// A deadline short enough to be reached in a test.
const SHORT: Duration = Duration::from_millis(300);

/// An agent, its events, and the thread they reduce to.
struct Harness {
    agent: Agent,
    events: Receiver<AgentEvent>,
    thread: Thread,
    /// Chunks of the agent's answer seen so far.
    answer_chunks: usize,
}

fn config(flags: &[&str], deadlines: Deadlines) -> AgentConfig {
    let mut command = vec![env!("CARGO_BIN_EXE_sakura-fake-agent").to_string()];
    command.extend(["--delay-ms", "0"].map(String::from));
    command.extend(flags.iter().map(|flag| flag.to_string()));
    AgentConfig {
        command,
        env: Vec::new(),
        workspace: "/work/demo".into(),
        deadlines,
        max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
    }
}

impl Harness {
    fn start(flags: &[&str]) -> Self {
        Self::with(config(flags, Deadlines::default()))
    }

    fn with(config: AgentConfig) -> Self {
        let (agent, events) = Agent::start(config, &tokio::runtime::Handle::current());
        Self {
            agent,
            events,
            thread: Thread::new(),
            answer_chunks: 0,
        }
    }

    /// The next event, applied to the thread when it is for it.
    async fn next(&mut self) -> AgentEvent {
        let event = tokio::time::timeout(PATIENCE, self.events.recv())
            .await
            .expect("an event arrives in time")
            .expect("the connection has not ended without its last event");
        if let AgentEvent::Thread(event) = &event {
            self.thread.apply(event.clone());
        }
        if matches!(
            event,
            AgentEvent::Thread(ThreadEvent::Update(Update::AgentText(_)))
        ) {
            self.answer_chunks += 1;
        }
        event
    }

    /// Reads events until one matches.
    async fn until(&mut self, is_wanted: impl Fn(&AgentEvent) -> bool) -> AgentEvent {
        loop {
            let event = self.next().await;
            if is_wanted(&event) {
                return event;
            }
            assert!(
                !matches!(event, AgentEvent::Ended(_)),
                "ended before the awaited event: {event:?}"
            );
        }
    }

    /// A harness with a new session open.
    async fn opened(flags: &[&str]) -> Self {
        let mut harness = Self::start(flags);
        harness.agent.open_session(None).unwrap();
        harness
            .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
            .await;
        harness
    }

    /// Sends a prompt as the window does: the thread shows it at once, then the agent hears it.
    fn prompt(&mut self, text: &str) {
        self.thread.apply(ThreadEvent::Prompted(text.to_string()));
        self.agent.prompt(text.to_string()).unwrap();
    }

    /// Sends a prompt and reads until the turn ends; returns why.
    async fn turn(&mut self, text: &str) -> StopReason {
        self.prompt(text);
        let ended = self
            .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
            .await;
        let AgentEvent::Thread(ThreadEvent::TurnEnded(reason)) = ended else {
            unreachable!("until returned a turn end");
        };
        reason
    }

    /// Reads to the last event and checks nothing follows it.
    async fn ended(&mut self) -> Option<sakura_acp::thread::Problem> {
        let AgentEvent::Ended(problem) = self.until_ended().await else {
            unreachable!("until_ended returned the end");
        };
        let after = tokio::time::timeout(PATIENCE, self.events.recv()).await;
        assert_eq!(
            after.expect("the channel closes"),
            None,
            "nothing follows the end"
        );
        problem
    }

    async fn until_ended(&mut self) -> AgentEvent {
        loop {
            let event = self.next().await;
            if matches!(event, AgentEvent::Ended(_)) {
                return event;
            }
        }
    }

    fn kinds(&self) -> Vec<&'static str> {
        let name = |entry: &Entry| match entry.kind {
            EntryKind::User(_) => "user",
            EntryKind::Assistant(_) => "assistant",
            EntryKind::Thought(_) => "thought",
            EntryKind::Tool(_) => "tool",
            EntryKind::Plan(_) => "plan",
            EntryKind::Permission(_) => "permission",
            EntryKind::Problem(_) => "problem",
        };
        self.thread.entries().iter().map(name).collect()
    }

    fn tool(&self, id: &str) -> ToolCall {
        let found = self
            .thread
            .entries()
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::Tool(call) if call.id == id => Some(call.clone()),
                _ => None,
            });
        found.unwrap_or_else(|| panic!("no tool call {id}"))
    }

    fn assistant_text(&self) -> String {
        let texts = self
            .thread
            .entries()
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Assistant(message) => Some(message.text.as_str()),
                _ => None,
            });
        texts.collect()
    }
}

fn is_asked(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::Thread(ThreadEvent::PermissionAsked { .. })
    )
}

fn is_failed(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::Thread(ThreadEvent::Failed(_)))
}

fn is_update(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::Thread(ThreadEvent::Update(_)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_connects_and_opens_a_new_session() {
    let mut harness = Harness::start(&[]);
    let connected = harness.next().await;
    let AgentEvent::Connected { name, version } = connected else {
        panic!("connected first, got {connected:?}");
    };
    assert_eq!(name, "sakura-fake-agent");
    assert!(!version.is_empty());
    harness.agent.open_session(None).unwrap();
    let opened = harness.next().await;
    assert!(
        matches!(
            opened,
            AgentEvent::SessionOpened {
                is_loaded: false,
                ..
            }
        ),
        "{opened:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_answer_streams_into_one_message() {
    let mut harness = Harness::opened(&[]).await;
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
    assert_eq!(harness.kinds(), ["user", "assistant"]);
    assert!(harness.assistant_text().starts_with("I read the module"));
    assert!(harness.assistant_text().ends_with("normalized form."));
    assert_eq!(harness.thread.turn(), Turn::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_then_answer_are_a_thought_and_a_message() {
    let mut harness = Harness::opened(&[]).await;
    harness.turn("think").await;
    assert_eq!(harness.kinds(), ["user", "thought", "assistant"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_calls_with_an_update_before_its_call_end_in_their_own_states() {
    let mut harness = Harness::opened(&[]).await;
    harness.turn("tools").await;
    assert_eq!(harness.kinds(), ["user", "tool", "tool", "tool"]);
    let search = harness.tool("call-search");
    assert_eq!(search.title, "Search \"sort_by\" in src");
    assert_eq!(search.status, ToolStatus::Failed);
    assert_eq!(harness.tool("call-read").status, ToolStatus::Completed);
    assert_eq!(
        harness.tool("call-test").status,
        ToolStatus::Canceled,
        "left running at the turn's end"
    );
    assert_eq!(
        harness.tool("call-test").command(),
        Some("cargo test folders")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diff_arrives_as_hunks() {
    let mut harness = Harness::opened(&[]).await;
    harness.turn("diff").await;
    let call = harness.tool("call-edit");
    let [ToolContent::Diff(diff)] = call.content.as_slice() else {
        panic!("one diff, got {:?}", call.content);
    };
    let patch = diff.patch.as_ref().expect("a patch");
    assert_eq!(diff.path, "/work/demo/src/folders/sort.rs");
    assert_eq!((patch.added, patch.removed), (2, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_output_ends_whole_and_completed() {
    let mut harness = Harness::opened(&[]).await;
    harness.turn("terminal").await;
    let call = harness.tool("call-test");
    let [ToolContent::Text { text, .. }] = call.content.as_slice() else {
        panic!("one text block, got {:?}", call.content);
    };
    assert_eq!(text.lines().count(), 10);
    assert!(text.ends_with("3 passed; 0 failed; 0 ignored"));
    assert_eq!(call.status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_sent_three_times_is_one_entry_with_the_last_states() {
    let mut harness = Harness::opened(&[]).await;
    harness.turn("plan").await;
    assert_eq!(harness.kinds(), ["user", "plan"]);
    let EntryKind::Plan(entries) = &harness.thread.entries()[1].kind else {
        panic!("a plan");
    };
    let statuses: Vec<PlanStatus> = entries.iter().map(|entry| entry.status).collect();
    assert_eq!(
        statuses,
        [
            PlanStatus::Completed,
            PlanStatus::Completed,
            PlanStatus::InProgress
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_for_a_fetch_reaches_the_person_and_an_allow_lets_the_call_finish() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("permission");
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked {
        request,
        reason,
        options,
        subject,
        ..
    }) = asked
    else {
        unreachable!("until returned a permission request");
    };
    assert_eq!(reason, Reason::Network);
    assert_eq!(
        subject.kind,
        Some(ToolKind::Fetch),
        "the subject is what the policy decided on"
    );
    let address = "crates.io/api/v1/crates";
    assert!(subject
        .raw_input
        .is_some_and(|input| input.contains(address)));
    assert_eq!(options.len(), 3);
    assert!(harness.thread.waits_for_person());
    assert_eq!(
        harness.tool("call-fetch").status,
        ToolStatus::WaitingForConfirmation
    );
    harness
        .agent
        .answer(request, "allow-once".to_string())
        .unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Completed);
    assert_eq!(
        harness.kinds(),
        ["user", "assistant", "tool", "permission", "assistant"]
    );
    let EntryKind::Permission(permission) = &harness.thread.entries()[3].kind else {
        panic!("a permission");
    };
    assert_eq!(
        permission.outcome,
        PermissionOutcome::Selected(OptionKind::AllowOnce)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_refused_leaves_the_call_rejected() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("permission");
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked { request, .. }) = asked else {
        unreachable!("until returned a permission request");
    };
    harness
        .agent
        .answer(request, "no-such-option".to_string())
        .unwrap();
    harness
        .agent
        .answer(request, "reject-once".to_string())
        .unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Rejected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_for_an_edit_in_the_workspace_is_allowed_without_the_person() {
    let mut harness = Harness::opened(&[]).await;
    assert_eq!(harness.turn("permission-safe").await, StopReason::EndTurn);
    assert_eq!(harness.kinds(), ["user", "tool", "assistant"]);
    assert_eq!(harness.tool("call-safe-edit").status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_whose_options_share_an_id_is_refused_not_answered_by_that_id() {
    let mut harness = Harness::opened(&[]).await;
    // The edit is one the policy allows by itself; the one id stands for "once" and for "always".
    let reason = harness.turn("permission-twins").await;
    assert_eq!(
        reason,
        StopReason::Cancelled,
        "the request was answered cancelled"
    );
    assert_eq!(
        harness.kinds(),
        ["user"],
        "the edit was not approved, and the person is shown no buttons whose answer would be ambiguous"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_in_the_middle_of_a_stream_ends_the_turn_as_cancelled() {
    let mut harness = Harness::opened(&["--delay-ms", "2"]).await;
    harness.prompt("slow");
    harness.until(is_update).await;
    harness.thread.apply(ThreadEvent::CancelRequested);
    assert_eq!(harness.thread.turn(), Turn::Cancelling);
    harness.agent.cancel().unwrap();
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(
        ended,
        AgentEvent::Thread(ThreadEvent::TurnEnded(StopReason::Cancelled))
    );
    assert_eq!(harness.thread.turn(), Turn::Idle);
    assert!(
        harness.assistant_text().split(' ').count() < 400,
        "the stream was cut short"
    );
    assert_eq!(
        harness.turn("plain").await,
        StopReason::EndTurn,
        "the session goes on after a cancel"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_answers_the_open_permission_request_as_cancelled() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("permission");
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked { request, .. }) = asked else {
        unreachable!("until returned a permission request");
    };
    harness.agent.cancel().unwrap();
    let answered = harness.next().await;
    let cancelled = ThreadEvent::PermissionAnswered {
        request,
        outcome: PermissionOutcome::Cancelled,
    };
    assert_eq!(answered, AgentEvent::Thread(cancelled));
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(
        ended,
        AgentEvent::Thread(ThreadEvent::TurnEnded(StopReason::Cancelled))
    );
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Canceled);
    assert!(!harness.thread.waits_for_person());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_that_arrives_after_the_cancel_is_refused_not_approved() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("permission-late");
    harness.until(is_update).await;
    harness.agent.cancel().unwrap();
    let mut after_cancel = Vec::new();
    loop {
        let event = harness.next().await;
        if matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))) {
            break;
        }
        after_cancel.push(event);
    }
    let is_permission = |event: &AgentEvent| {
        let allowed = matches!(
            event,
            AgentEvent::Thread(ThreadEvent::PermissionAllowed { .. })
        );
        allowed || is_asked(event)
    };
    // The edit is one the policy allows by itself while a turn runs.
    assert!(
        !after_cancel.iter().any(is_permission),
        "a cancelled turn approves nothing and asks nothing: {after_cancel:?}"
    );
    assert_eq!(
        harness.tool("call-late-edit").status,
        ToolStatus::Failed,
        "the agent was told no"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_session_is_replayed_before_it_opens() {
    let mut harness = Harness::start(&[]);
    harness
        .agent
        .open_session(Some("fake-session-1".to_string()))
        .unwrap();
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    let loaded = AgentEvent::SessionOpened {
        session_id: "fake-session-1".to_string(),
        is_loaded: true,
    };
    assert_eq!(opened, loaded);
    let kinds = harness.kinds();
    assert_eq!(kinds.first(), Some(&"user"));
    assert_eq!(&kinds[kinds.len() - 2..], ["user", "assistant"]);
    assert!(harness
        .thread
        .entries()
        .iter()
        .all(|entry| entry.is_replayed));
    assert_eq!(harness.tool("call-test").status, ToolStatus::Completed);
    assert_eq!(harness.thread.turn(), Turn::Idle);
    assert_eq!(
        harness.turn("plain").await,
        StopReason::EndTurn,
        "the loaded session takes a prompt"
    );
    assert!(!harness.thread.entries().last().unwrap().is_replayed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_session_the_agent_no_longer_lists_becomes_a_new_one() {
    let mut harness = Harness::start(&[]);
    harness
        .agent
        .open_session(Some("forgotten".to_string()))
        .unwrap();
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert!(
        matches!(
            opened,
            AgentEvent::SessionOpened {
                is_loaded: false,
                ..
            }
        ),
        "{opened:?}"
    );
    assert!(harness.thread.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_session_that_fails_to_load_without_a_list_becomes_a_new_one() {
    let mut harness = Harness::start(&["--no-list"]);
    harness
        .agent
        .open_session(Some("forgotten".to_string()))
        .unwrap();
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert!(
        matches!(
            opened,
            AgentEvent::SessionOpened {
                is_loaded: false,
                ..
            }
        ),
        "{opened:?}"
    );
    assert!(harness.thread.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_without_load_gets_a_new_session_even_with_a_stored_one() {
    let mut harness = Harness::start(&["--no-load"]);
    harness
        .agent
        .open_session(Some("fake-session-1".to_string()))
        .unwrap();
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert!(
        matches!(
            opened,
            AgentEvent::SessionOpened {
                is_loaded: false,
                ..
            }
        ),
        "{opened:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_required_is_its_own_event_and_the_connection_stays_up() {
    let mut harness = Harness::start(&["--auth-required"]);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| *event == AgentEvent::AuthRequired)
        .await;
    harness
        .agent
        .open_session(Some("fake-session-1".to_string()))
        .unwrap();
    harness
        .until(|event| *event == AgentEvent::AuthRequired)
        .await;
    harness.agent.shutdown().unwrap();
    assert_eq!(harness.ended().await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_thousand_chunks_arrive_whole_and_in_order_through_the_bounded_channel() {
    let mut harness = Harness::opened(&[]).await;
    assert_eq!(harness.turn("stream5000").await, StopReason::EndTurn);
    assert_eq!(harness.kinds(), ["user", "assistant"]);
    assert_eq!(harness.answer_chunks, 5000);
    let text = harness.assistant_text();
    assert!(text.starts_with("## How sorting works\n\nThe folder list is built"));
    assert!(text.matches("## How sorting works").count() > 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_that_goes_on_while_the_window_does_not_read_ends_the_connection_not_memory() {
    let config = AgentConfig {
        max_unread_bytes: 32 * 1024,
        ..config(&[], Deadlines::default())
    };
    let mut harness = Harness::with(config);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    harness.prompt("stream5000");
    // The window does not read: the event channel fills, and the agent goes on writing its 5000 chunks.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let last = loop {
        let event = harness.next().await;
        let is_whole_turn = matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_)));
        assert!(!is_whole_turn, "every chunk was kept while nobody read");
        if let AgentEvent::Ended(problem) = event {
            break problem.expect("the connection ended with a problem");
        }
    };
    assert_eq!(last.kind, ProblemKind::Overrun);
    assert!(last.detail.contains("32768 bytes"), "{}", last.detail);
    assert!(harness.answer_chunks < 5000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_in_the_middle_of_a_turn_is_one_last_event_with_the_exit_code_and_the_last_words() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("crash");
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(problem.kind, ProblemKind::Died);
    assert!(problem.detail.contains("exit code 9"), "{}", problem.detail);
    assert!(
        problem
            .detail
            .contains("simulated crash in the middle of a turn"),
        "{}",
        problem.detail
    );
    assert_eq!(
        harness.assistant_text(),
        "Starting work",
        "what arrived before the crash is kept"
    );
}

/// Whether the process `pid` exists.
fn is_alive(pid: &str) -> bool {
    let probe = std::process::Command::new("kill")
        .args(["-0", pid])
        .output();
    probe.is_ok_and(|output| output.status.success())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_that_dies_while_a_process_it_started_holds_its_output_ends_and_takes_it_along() {
    let folder = tempfile::tempdir().unwrap();
    let pid_file = folder.path().join("worker.pid");
    // A wrapper leaves a worker behind with the agent's output still open, then becomes the agent, which
    // dies while the session is idle: its output never reaches its end.
    let script = r#"sleep 300 & echo $! > "$0"; exec "$@""#;
    let fake = env!("CARGO_BIN_EXE_sakura-fake-agent");
    let words = ["/bin/sh", "-c", script, pid_file.to_str().unwrap(), fake];
    let flags = ["--delay-ms", "0", "--exit-after-ms", "700"];
    let config = AgentConfig {
        command: words
            .iter()
            .chain(&flags)
            .map(|word| word.to_string())
            .collect(),
        ..config(&[], Deadlines::default())
    };
    let mut harness = Harness::with(config);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    let worker = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_string();
    assert!(is_alive(&worker), "the worker runs beside the agent");
    let ended = tokio::time::timeout(Duration::from_secs(10), harness.until_ended()).await;
    let ended = ended.expect("the session ends when the agent's process is gone");
    let AgentEvent::Ended(Some(problem)) = ended else {
        panic!("a problem says the agent died: {ended:?}");
    };
    assert_eq!(problem.kind, ProblemKind::Died);
    assert!(problem.detail.contains("exit code 9"), "{}", problem.detail);
    tokio::time::sleep(SHORT).await;
    assert!(!is_alive(&worker), "nothing the agent started outlives it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_at_start_is_one_last_event() {
    let mut harness = Harness::start(&["--crash-at-start"]);
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(problem.kind, ProblemKind::Died);
    assert!(problem.detail.contains("exit code 3"), "{}", problem.detail);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn program_that_does_not_exist_is_not_started() {
    let mut config = config(&[], Deadlines::default());
    config.command = vec!["/nonexistent/sakura-agent".to_string()];
    let mut harness = Harness::with(config);
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(problem.kind, ProblemKind::NotStarted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_command_is_not_started() {
    let mut config = config(&[], Deadlines::default());
    config.command = Vec::new();
    let mut harness = Harness::with(config);
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(problem.kind, ProblemKind::NotStarted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_that_never_answers_initialize_times_out_and_is_stopped() {
    let deadlines = Deadlines {
        request: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&["--mute"], deadlines));
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "initialize")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_that_falls_silent_times_out_and_the_session_goes_on() {
    let deadlines = Deadlines {
        idle_turn: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&[], deadlines));
    harness.agent.open_session(None).unwrap();
    harness.prompt("stall");
    let failed = harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("until returned a failure");
    };
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "session/prompt")
    );
    assert_eq!(harness.thread.turn(), Turn::Idle);
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_timed_out_turn_still_says_never_reaches_the_next_turn() {
    let deadlines = Deadlines {
        idle_turn: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&[], deadlines));
    harness.agent.open_session(None).unwrap();
    harness.prompt("stall-late");
    harness.until(is_failed).await;
    // The person goes on at once, while the old turn has not confirmed that it stopped.
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
    // Whatever the old turn still had to say has arrived by now.
    tokio::time::sleep(SHORT * 2).await;
    while let Ok(event) = harness.events.try_recv() {
        if let AgentEvent::Thread(event) = event {
            harness.thread.apply(event);
        }
    }
    let said = harness.assistant_text();
    assert!(
        !said.contains("late"),
        "the old turn spoke into the new one: {said}"
    );
    assert_eq!(
        harness.kinds().last(),
        Some(&"assistant"),
        "the new turn was answered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_permission_request_does_not_count_as_a_stall() {
    let deadlines = Deadlines {
        idle_turn: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&[], deadlines));
    harness.agent.open_session(None).unwrap();
    harness.prompt("permission");
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked { request, .. }) = asked else {
        unreachable!("until returned a permission request");
    };
    tokio::time::sleep(SHORT * 3).await;
    harness
        .agent
        .answer(request, "allow-once".to_string())
        .unwrap();
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(
        ended,
        AgentEvent::Thread(ThreadEvent::TurnEnded(StopReason::EndTurn))
    );
    assert_eq!(
        harness.kinds().last(),
        Some(&"assistant"),
        "no problem was reported"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_the_agent_does_not_confirm_times_out() {
    let deadlines = Deadlines {
        cancel: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&[], deadlines));
    harness.agent.open_session(None).unwrap();
    harness.prompt("deaf");
    harness.until(is_update).await;
    harness.agent.cancel().unwrap();
    // An agent that goes on after a cancel cannot be given another turn: the connection ends.
    let problem = harness.ended().await.expect("a problem says why");
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "session/cancel")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_that_falls_silent_and_never_confirms_its_end_ends_the_connection() {
    let deadlines = Deadlines {
        idle_turn: SHORT,
        cancel: SHORT,
        ..Deadlines::default()
    };
    let mut harness = Harness::with(config(&[], deadlines));
    harness.agent.open_session(None).unwrap();
    harness.prompt("deaf");
    let failed = harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("until returned a failure");
    };
    assert_eq!(problem.detail, "session/prompt", "the turn timed out first");
    let problem = harness.ended().await.expect("a problem says why");
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "session/cancel")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_ends_cleanly_also_in_the_middle_of_a_turn() {
    let mut harness = Harness::opened(&[]).await;
    harness.prompt("stall");
    harness.until(is_update).await;
    harness.agent.shutdown().unwrap();
    assert_eq!(harness.ended().await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_ends_also_when_the_agent_stopped_reading_what_is_being_written_to_it() {
    let deadlines = Deadlines {
        shutdown: SHORT,
        ..Deadlines::default()
    };
    // The agent reads `initialize` and `session/new` and nothing after them.
    let mut harness = Harness::with(config(&["--read-lines", "2"], deadlines));
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    // A prompt far larger than a pipe holds: writing it never completes.
    harness.agent.prompt("x".repeat(4 * 1024 * 1024)).unwrap();
    tokio::time::sleep(SHORT).await;
    harness.agent.shutdown().unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(8), harness.until_ended()).await;
    assert_eq!(
        ended.expect("the connection ends within its shutdown deadlines"),
        AgentEvent::Ended(None)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_handle_shuts_the_agent_down() {
    let Harness {
        agent, mut events, ..
    } = Harness::opened(&[]).await;
    drop(agent);
    let last = tokio::time::timeout(PATIENCE, events.recv())
        .await
        .expect("an event");
    assert_eq!(last, Some(AgentEvent::Ended(None)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn program_that_speaks_no_protocol_and_never_exits_is_killed() {
    let deadlines = Deadlines {
        request: SHORT,
        ..Deadlines::default()
    };
    let mut config = config(&[], deadlines);
    config.command = ["/bin/sleep", "600"].map(String::from).to_vec();
    let mut harness = Harness::with(config);
    let problem = harness.ended().await.expect("a problem");
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "initialize")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_without_a_session_fails_without_ending_the_connection() {
    let mut harness = Harness::start(&[]);
    harness.prompt("plain");
    let failed = harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("until returned a failure");
    };
    assert_eq!(problem.kind, ProblemKind::RequestFailed);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lines_that_are_not_json_do_not_break_the_session() {
    let mut harness = Harness::opened(&["--noise"]).await;
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
    assert!(harness.assistant_text().starts_with("I read the module"));
}
