//! The connection against the fake agent, as a child process over stdio: every scripted session ends in the
//! entries the conversation shows, and every way the agent can fail ends in one typed event, never a hang.

use sakura_acp::attachment::Attachment;
use sakura_acp::connection::{Agent, AgentConfig, AgentEvent, Deadlines, SessionSetup};
use sakura_acp::model::{
    OptionKind, PlanStatus, PromptSupport, SessionChoices, StopReason, ToolContent, ToolKind,
    ToolStatus, Update,
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
    /// What the session last said it lets the person pick.
    choices: Option<SessionChoices>,
    /// The commands the session last announced.
    commands: Vec<sakura_acp::model::SlashCommand>,
    /// The workspace on disk of a task on the Mac, held while the agent works in it.
    _workspace: Option<tempfile::TempDir>,
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
        asks_beyond_workspace: false,
        session: SessionSetup::default(),
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
            choices: None,
            commands: Vec::new(),
            _workspace: None,
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
        if let AgentEvent::Choices(choices) = &event {
            self.choices = Some(choices.clone());
        }
        if let AgentEvent::Commands(commands) = &event {
            self.commands.clone_from(commands);
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
    let supports = harness.next().await;
    assert!(matches!(supports, AgentEvent::Supports(_)), "{supports:?}");
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

/// A harness for a task on the Mac, its session open: `stored` is the session it opens again, if any.
/// Its workspace is a folder on disk, as a task's is, and the fake agent's paths are in it.
async fn opened_on_the_mac(flags: &[&str], stored: Option<&str>) -> Harness {
    let workspace = tempfile::tempdir().unwrap();
    let mut on_the_mac = config(flags, Deadlines::default());
    on_the_mac.asks_beyond_workspace = true;
    on_the_mac.workspace = workspace.path().into();
    let folder = workspace.path().to_string_lossy().into_owned();
    on_the_mac.env = vec![("SAKURA_FAKE_WORKSPACE".to_string(), folder)];
    let mut harness = Harness::with(on_the_mac);
    harness._workspace = Some(workspace);
    harness
        .agent
        .open_session(stored.map(str::to_string))
        .unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    harness
}

/// The fake agent's login lets every tool run without asking, as a person's own settings can.
const LOGIN_ALLOWS_ALL: &str = "--login-allows-all";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_what_leaves_the_task_is_asked_whatever_the_login_allows() {
    for stored in [None, Some("fake-session-1")] {
        let mut harness = opened_on_the_mac(&[LOGIN_ALLOWS_ALL], stored).await;
        harness.prompt("permission");
        let asked = harness.until(is_asked).await;
        let AgentEvent::Thread(ThreadEvent::PermissionAsked { reason, .. }) = asked else {
            unreachable!("until returned a permission request");
        };
        assert_eq!(
            reason,
            sakura_acp::policy::Reason::Network,
            "the fetch reaches the person's card ({stored:?})"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_an_edit_inside_the_task_goes_on_unasked() {
    let mut harness = opened_on_the_mac(&[LOGIN_ALLOWS_ALL], None).await;
    assert_eq!(harness.turn("permission-safe").await, StopReason::EndTurn);
    assert_eq!(harness.kinds(), ["user", "tool", "assistant"]);
    assert_eq!(harness.tool("call-safe-edit").status, ToolStatus::Completed);
}

/// The reason the next permission request reaches the person's card with.
async fn asked_reason(harness: &mut Harness) -> Reason {
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked { reason, .. }) = asked else {
        unreachable!("until returned a permission request");
    };
    reason
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_a_read_outside_the_task_reaches_the_card_whatever_the_login_allows() {
    let mut harness = opened_on_the_mac(&[LOGIN_ALLOWS_ALL], None).await;
    harness.prompt("permission-read-outside");
    assert_eq!(asked_reason(&mut harness).await, Reason::OutsideWorkspace);
}

/// What the app does with a tool it does not know when the agent asks. Whether the real agent asks for
/// such a tool under a login that allows it (the `*` ask rule) is not proven here: the fake agent would
/// only play back the rule it was written with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_a_tool_the_app_does_not_know_reaches_the_card_when_the_agent_asks() {
    let mut harness = opened_on_the_mac(&[], None).await;
    harness.prompt("permission-mcp");
    assert_eq!(asked_reason(&mut harness).await, Reason::UnknownKind);
}

/// Plays `script` on the Mac and checks that its one tool call was judged by the policy, allowed without
/// a card, and completed.
async fn allowed_on_the_mac_without_a_card(script: &str, call: &str) {
    let mut harness = opened_on_the_mac(&[LOGIN_ALLOWS_ALL], None).await;
    harness.prompt(script);
    let judged = harness
        .until(|event| {
            let allowed = matches!(
                event,
                AgentEvent::Thread(ThreadEvent::PermissionAllowed { .. })
            );
            allowed || is_asked(event)
        })
        .await;
    assert!(
        matches!(
            judged,
            AgentEvent::Thread(ThreadEvent::PermissionAllowed { .. })
        ),
        "{script} is allowed by the policy, not shown: {judged:?}"
    );
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.tool(call).status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_a_read_inside_the_task_is_judged_by_the_app_and_goes_on_without_a_card() {
    allowed_on_the_mac_without_a_card("permission-read", "call-read").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_a_grep_of_the_task_whose_regex_looks_like_a_path_goes_on_without_a_card() {
    allowed_on_the_mac_without_a_card("permission-grep", "call-grep").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_mac_a_known_command_reaches_the_card_whatever_the_login_allows() {
    let mut harness = opened_on_the_mac(&[LOGIN_ALLOWS_ALL], None).await;
    harness.prompt("permission-command");
    assert_eq!(asked_reason(&mut harness).await, Reason::CommandOnMac);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_a_box_a_known_command_goes_on_without_a_card() {
    let mut harness = Harness::opened(&[]).await;
    assert_eq!(
        harness.turn("permission-command").await,
        StopReason::EndTurn
    );
    assert!(!harness.kinds().contains(&"permission"));
    assert_eq!(harness.tool("call-command").status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_a_box_the_login_s_own_settings_stand() {
    let mut harness = Harness::opened(&[LOGIN_ALLOWS_ALL]).await;
    assert_eq!(harness.turn("permission").await, StopReason::EndTurn);
    assert!(
        !harness.kinds().contains(&"permission"),
        "the box is the boundary: nothing is asked"
    );
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Completed);
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

#[cfg_attr(
    windows,
    ignore = "Unix shell and process-group fixture: requires /bin/sh and kill"
)]
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
    let config = config(&["--read-lines", "0"], deadlines);
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

// The engine's master: what its sessions are opened with, what is done around them, and who decides its
// permission requests.

/// A harness over an agent that logs what it receives, with `setup` as the master's session setup.
struct Logged {
    harness: Harness,
    log: std::path::PathBuf,
    _folder: tempfile::TempDir,
}

impl Logged {
    fn start(setup: SessionSetup) -> Self {
        Self::start_with(&[], setup)
    }

    fn start_with(flags: &[&str], setup: SessionSetup) -> Self {
        let folder = tempfile::tempdir().expect("a folder");
        let log = folder.path().join("agent.log");
        let mut config = config(flags, Deadlines::default());
        config.env = vec![
            ("SAKURA_FAKE_LOG".to_string(), log.display().to_string()),
            ("SAKURA_FAKE_ENV_NAMES".to_string(), "HOME".to_string()),
        ];
        config.session = setup;
        Self {
            harness: Harness::with(config),
            log,
            _folder: folder,
        }
    }

    async fn opened(setup: SessionSetup) -> Self {
        let logged = Self::start(setup);
        logged.harness.agent.open_session(None).unwrap();
        let mut logged = logged;
        logged
            .harness
            .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
            .await;
        logged
    }

    /// What the agent received so far, one JSON object per message.
    fn received(&self) -> Vec<serde_json::Value> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).expect("a log line"))
            .collect()
    }

    /// The methods the agent received, in order.
    fn methods(&self) -> Vec<String> {
        let methods = self.received();
        let name = |message: &serde_json::Value| message["method"].as_str().map(str::to_string);
        methods.iter().filter_map(name).collect()
    }
}

fn engine_setup() -> SessionSetup {
    SessionSetup {
        meta: Some(serde_json::json!({
            "claudeCode": { "options": { "model": "haiku", "settingSources": [] } }
        })),
        mcp_servers: vec![serde_json::json!({
            "name": "sakura-master",
            "command": "/engine/bin/sakura",
            "args": ["master", "serve", "/engine/policy.json"],
            "env": [{ "name": "SAKURA_ENGINE_HOME", "value": "/engine/home" }],
        })],
        start_mode: Some("default".to_string()),
        closes_sessions: true,
        ..SessionSetup::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_new_carries_engine_meta_and_mcp_servers() {
    let logged = Logged::opened(engine_setup()).await;
    let received = logged.received();
    let new = received
        .iter()
        .find(|message| message["method"] == "session/new")
        .expect("a session/new");
    let options = &new["params"]["_meta"]["claudeCode"]["options"];
    assert_eq!(options["model"], "haiku");
    assert_eq!(options["settingSources"], serde_json::json!([]));
    let server = &new["params"]["mcpServers"][0];
    assert_eq!(server["name"], "sakura-master");
    assert_eq!(server["command"], "/engine/bin/sakura");
    assert_eq!(
        server["args"],
        serde_json::json!(["master", "serve", "/engine/policy.json"])
    );
    assert_eq!(server["env"][0]["name"], "SAKURA_ENGINE_HOME");
    assert_eq!(server["env"][0]["value"], "/engine/home");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_mode_is_set_before_the_first_prompt() {
    let mut logged = Logged::opened(engine_setup()).await;
    logged.harness.turn("plain").await;
    let methods = logged.methods();
    let at = |name: &str| methods.iter().position(|method| method == name).unwrap();
    assert!(at("session/new") < at("session/set_mode"));
    assert!(at("session/set_mode") < at("session/prompt"));
    let received = logged.received();
    let set = received
        .iter()
        .find(|message| message["method"] == "session/set_mode")
        .expect("a set_mode");
    assert_eq!(set["params"]["modeId"], "default");
}

/// A logged agent that offers a Haiku and a Sonnet model (it starts on Haiku), opened with `start_model`.
async fn opened_with_start_model(start_model: Option<&str>) -> Logged {
    let setup = SessionSetup {
        start_mode: Some("default".to_string()),
        start_model: start_model.map(str::to_string),
        ..SessionSetup::default()
    };
    let logged = Logged::start_with(&["--choices"], setup);
    logged.harness.agent.open_session(None).unwrap();
    let mut logged = logged;
    logged
        .harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    logged
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_start_model_is_set_before_the_first_prompt_and_the_choices_say_it() {
    let mut logged = opened_with_start_model(Some("sonnet")).await;
    let choices = logged.harness.choices.clone().expect("the choices");
    assert_eq!(choices.models.expect("models").current, "sonnet");
    logged.harness.turn("plain").await;
    let methods = logged.methods();
    let at = |name: &str| methods.iter().position(|method| method == name).unwrap();
    assert!(at("session/new") < at("session/set_config_option"));
    assert!(at("session/set_config_option") < at("session/prompt"));
    let received = logged.received();
    let set = received
        .iter()
        .find(|message| message["method"] == "session/set_config_option")
        .expect("a set_config_option");
    assert_eq!(set["params"]["configId"], "model");
    assert_eq!(set["params"]["value"], "sonnet");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_model_the_session_is_already_on_or_does_not_offer_sends_nothing() {
    for start_model in [Some("haiku"), Some("opus"), None] {
        let logged = opened_with_start_model(start_model).await;
        assert!(
            !logged
                .methods()
                .contains(&"session/set_config_option".to_string()),
            "{start_model:?}"
        );
        let choices = logged.harness.choices.clone().expect("the choices");
        assert_eq!(choices.models.expect("models").current, "haiku");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_is_replaced_and_the_one_left_at_the_end_are_closed() {
    let mut logged = Logged::opened(engine_setup()).await;
    logged.harness.agent.open_session(None).unwrap();
    logged
        .harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    logged.harness.agent.shutdown().unwrap();
    logged.harness.ended().await;
    let methods = logged.methods();
    let sessions: Vec<&str> = methods
        .iter()
        .map(String::as_str)
        .filter(|method| ["session/new", "session/close"].contains(method))
        .collect();
    assert_eq!(
        sessions,
        [
            "session/new",
            "session/close",
            "session/new",
            "session/close"
        ],
        "the first session is closed before the second opens, the second at the end"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_agent_is_sent_no_mode_and_no_close() {
    let mut logged = Logged::opened(SessionSetup::default()).await;
    logged.harness.agent.shutdown().unwrap();
    logged.harness.ended().await;
    let methods = logged.methods();
    assert!(!methods.contains(&"session/set_mode".to_string()));
    assert!(!methods.contains(&"session/close".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn variables_the_setup_unsets_do_not_reach_the_agent() {
    let setup = SessionSetup {
        unset_env: vec!["HOME".to_string()],
        ..SessionSetup::default()
    };
    let logged = Logged::start(setup);
    logged.harness.agent.open_session(None).unwrap();
    let mut logged = logged;
    logged
        .harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    let started = &logged.received()[0];
    assert_eq!(started["method"], "started");
    assert_eq!(started["environment"]["HOME"], false);
    let without = Logged::opened(SessionSetup::default()).await;
    assert_eq!(without.received()[0]["environment"]["HOME"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_server_the_protocol_cannot_read_fails_the_open_not_the_connection() {
    let setup = SessionSetup {
        mcp_servers: vec![serde_json::json!({ "name": 7 })],
        ..SessionSetup::default()
    };
    let mut logged = Logged::start(setup);
    logged.harness.agent.open_session(None).unwrap();
    let failed = logged.harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("until returned a failure");
    };
    assert_eq!(problem.kind, ProblemKind::RequestFailed);
    assert!(!logged.methods().contains(&"session/new".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configuration_that_cannot_be_had_ends_the_connection_with_its_problem() {
    let problem = sakura_acp::thread::Problem {
        kind: ProblemKind::NotStarted,
        detail: "no adapter is named".to_string(),
    };
    let wanted = problem.clone();
    let (agent, events) = Agent::start_resolving(
        async move { Err(wanted) },
        &tokio::runtime::Handle::current(),
    );
    let mut harness = Harness {
        agent,
        events,
        thread: Thread::new(),
        answer_chunks: 0,
        choices: None,
        commands: Vec::new(),
        _workspace: None,
    };
    assert_eq!(harness.ended().await, Some(problem));
}

/// A decider that answers every request with `verdict` and keeps the requests it was given.
fn deciding(
    verdict: sakura_acp::policy::decider::Verdict,
) -> (
    sakura_acp::policy::decider::Decider,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = seen.clone();
    let decider = sakura_acp::policy::decider::Decider::new(move |request| {
        kept.lock().expect("the requests").push(request);
        Box::pin(async move { verdict })
    });
    (decider, seen)
}

async fn decided_by(verdict: sakura_acp::policy::decider::Verdict) -> (Harness, Seen) {
    let (decider, seen) = deciding(verdict);
    let setup = SessionSetup {
        decider: Some(decider),
        ..SessionSetup::default()
    };
    let mut config = config(&[], Deadlines::default());
    config.session = setup;
    let mut harness = Harness::with(config);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    (harness, seen)
}

type Seen = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_deny_never_reaches_the_person() {
    use sakura_acp::policy::decider::Verdict;
    let (mut harness, seen) = decided_by(Verdict::Deny).await;
    harness.prompt("permission");
    let ended = harness
        .until(|event| {
            is_asked(event) || matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_)))
        })
        .await;
    assert!(!is_asked(&ended), "a denied request is never shown");
    assert_eq!(
        harness.tool("call-fetch").status,
        ToolStatus::Failed,
        "the agent was refused: its call fails, and nothing was asked"
    );
    assert!(
        !harness.kinds().contains(&"permission"),
        "no card, not even a closed one"
    );
    assert_eq!(seen.lock().expect("the requests").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_command_context_is_refused_before_the_decider() {
    use sakura_acp::policy::decider::Verdict;
    let (mut harness, seen) = decided_by(Verdict::Allow).await;
    harness.turn("permission-command-context").await;
    assert_eq!(harness.tool("call-command").status, ToolStatus::Failed);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_allow_is_answered_without_asking() {
    use sakura_acp::policy::decider::Verdict;
    let (mut harness, _) = decided_by(Verdict::Allow).await;
    harness.prompt("permission");
    let ended = harness
        .until(|event| {
            is_asked(event) || matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_)))
        })
        .await;
    assert!(
        !is_asked(&ended),
        "an allowed request is not put to the person"
    );
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_ask_reaches_the_person_who_answers_it() {
    use sakura_acp::policy::decider::Verdict;
    let (mut harness, _) = decided_by(Verdict::Ask).await;
    harness.prompt("permission");
    let asked = harness.until(is_asked).await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked {
        request, reason, ..
    }) = asked
    else {
        unreachable!("until returned a permission request");
    };
    assert_eq!(reason, Reason::EngineAsks);
    harness
        .agent
        .answer(request, "allow-once".to_string())
        .unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.tool("call-fetch").status, ToolStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_decider_is_given_the_request_as_the_agent_sent_it() {
    use sakura_acp::policy::decider::Verdict;
    let (mut harness, seen) = decided_by(Verdict::Allow).await;
    harness.turn("permission").await;
    let seen = seen.lock().expect("the requests");
    let request = &seen[0];
    assert_eq!(request["method"], "session/request_permission");
    let call = &request["params"]["toolCall"];
    assert_eq!(call["_meta"]["claudeCode"]["toolName"], "WebFetch");
    assert_eq!(
        call["rawInput"]["url"],
        "https://crates.io/api/v1/crates/unicode-normalization"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_whose_mode_could_not_be_set_is_forgotten_closed_and_never_prompted() {
    let mut logged = Logged::start_with(&["--fail-set-mode"], engine_setup());
    logged.harness.agent.open_session(None).unwrap();
    let failed = logged.harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("until returned a failure");
    };
    assert_eq!(problem.kind, ProblemKind::RequestFailed);
    assert!(problem.detail.contains("session/set_mode"), "{problem:?}");
    // The prompt of a person who did not see the failure is refused, and nothing reaches the agent.
    logged.harness.agent.prompt("plain".to_string()).unwrap();
    let refused = logged.harness.until(is_failed).await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = refused else {
        unreachable!("until returned a failure");
    };
    assert!(problem.detail.contains("no session is open"), "{problem:?}");
    logged.harness.agent.shutdown().unwrap();
    logged.harness.ended().await;
    let methods = logged.methods();
    assert!(
        !methods.contains(&"session/prompt".to_string()),
        "{methods:?}"
    );
    let close = methods.iter().filter(|method| *method == "session/close");
    assert_eq!(
        close.count(),
        1,
        "the unusable session is closed once: {methods:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kept_session_whose_mode_could_not_be_set_is_not_opened() {
    let mut logged = Logged::start_with(&["--fail-set-mode"], engine_setup());
    logged
        .harness
        .agent
        .open_session(Some("fake-session-1".to_string()))
        .unwrap();
    logged.harness.until(is_failed).await;
    logged.harness.agent.prompt("plain".to_string()).unwrap();
    logged.harness.until(is_failed).await;
    assert!(!logged.methods().contains(&"session/prompt".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_with_an_allow_list_starts_from_an_empty_environment_holding_only_it() {
    let setup = SessionSetup {
        env_allow: Some(vec!["HOME".to_string()]),
        ..SessionSetup::default()
    };
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("agent.log");
    let mut config = config(&[], Deadlines::default());
    config.env = vec![
        ("SAKURA_FAKE_LOG".to_string(), log.display().to_string()),
        (
            "SAKURA_FAKE_ENV_NAMES".to_string(),
            "HOME,PATH,CARGO_HOME".to_string(),
        ),
    ];
    config.session = setup;
    let mut harness = Harness::with(config);
    harness.agent.open_session(None).unwrap();
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    let text = std::fs::read_to_string(&log).expect("the log");
    let started: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(
        started["environment"]["HOME"], true,
        "an allowed name is passed on"
    );
    assert_eq!(
        started["environment"]["PATH"], false,
        "a name that is not allowed is not"
    );
    assert_eq!(started["environment"]["CARGO_HOME"], false);
}

/// A session of an agent that offers commands and choices, its log kept for what it was sent.
async fn offering(flags: &[&str]) -> Logged {
    let mut logged = Logged::start_with(flags, engine_setup());
    logged.harness.agent.open_session(None).unwrap();
    logged
        .harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    logged
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_s_prompt_capabilities_are_told_once_after_it_connects() {
    let mut harness = Harness::start(&[]);
    harness
        .until(|event| matches!(event, AgentEvent::Connected { .. }))
        .await;
    let support = harness.next().await;
    assert_eq!(
        support,
        AgentEvent::Supports(PromptSupport {
            has_images: true,
            has_embedded_context: true
        })
    );
    let mut bare = Harness::start(&["--no-attachments"]);
    let told = bare
        .until(|event| matches!(event, AgentEvent::Supports(_)))
        .await;
    assert_eq!(told, AgentEvent::Supports(PromptSupport::default()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn announced_commands_arrive_with_their_descriptions_and_hints() {
    let mut logged = offering(&["--commands"]).await;
    logged.harness.turn("plain").await;
    let commands = logged.harness.commands.clone();
    let names: Vec<&str> = commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(names, ["compact", "review", "mcp:docs:search", "context"]);
    assert_eq!(commands[0].hint, None);
    assert_eq!(commands[1].hint.as_deref(), Some("[pull request]"));
    assert_eq!(commands[1].description, "Review a pull request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_kind_of_announced_command_runs_and_leaves_the_session_usable() {
    // One of each kind the adapter announces: no input, an argument, an MCP prompt, a local-only one.
    let prompts = [
        "/compact",
        "/review 12",
        "/mcp:docs:search tabs",
        "/context",
    ];
    let mut logged = offering(&["--commands"]).await;
    for prompt in prompts {
        assert_eq!(
            logged.harness.turn(prompt).await,
            StopReason::EndTurn,
            "{prompt}"
        );
        let said = logged
            .harness
            .thread
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = said else {
            panic!("{prompt} was answered with a message, got {said:?}");
        };
        assert_eq!(message.text, format!("Ran {prompt}."));
    }
    assert_eq!(
        logged.harness.turn("plain").await,
        StopReason::EndTurn,
        "the session goes on"
    );
    let sent: Vec<String> = logged
        .received()
        .iter()
        .filter(|message| message["method"] == "session/prompt")
        .map(|message| {
            message["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        sent,
        [
            "/compact",
            "/review 12",
            "/mcp:docs:search tabs",
            "/context",
            "plain"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modes_and_models_the_session_offers_arrive_with_what_is_picked() {
    let logged = offering(&["--choices"]).await;
    let choices = logged
        .harness
        .choices
        .clone()
        .expect("the choices arrived before the session opened");
    let modes = choices.modes.expect("modes");
    assert_eq!(modes.current, "default", "the start mode is the picked one");
    assert_eq!(modes.choices.len(), 3);
    assert_eq!(modes.config_id, None);
    let models = choices.models.expect("models");
    assert_eq!(
        (models.config_id.as_deref(), models.current.as_str()),
        (Some("model"), "haiku")
    );
    assert_eq!(
        models
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Haiku", "Sonnet"]
    );
    logged.harness.agent.shutdown().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picking_a_mode_sends_set_mode_and_says_which_mode_the_session_is_in() {
    let mut logged = offering(&["--choices"]).await;
    logged.harness.agent.set_mode("plan".to_string()).unwrap();
    let told = logged
        .harness
        .until(|event| matches!(event, AgentEvent::ModeChanged(_)))
        .await;
    assert_eq!(told, AgentEvent::ModeChanged("plan".to_string()));
    let received = logged.received();
    let set = received
        .iter()
        .rev()
        .find(|m| m["method"] == "session/set_mode")
        .unwrap();
    assert_eq!(set["params"]["modeId"], "plan");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn picking_a_model_sends_set_config_option_and_returns_the_options_as_they_stand() {
    let mut logged = offering(&["--choices"]).await;
    logged
        .harness
        .agent
        .set_option("model".to_string(), "sonnet".to_string())
        .unwrap();
    logged.harness.until(|event| matches!(event, AgentEvent::Choices(c) if c.models.as_ref().is_some_and(|m| m.current == "sonnet"))).await;
    let received = logged.received();
    let set = received
        .iter()
        .find(|m| m["method"] == "session/set_config_option")
        .unwrap();
    assert_eq!(
        (
            set["params"]["configId"].as_str(),
            set["params"]["value"].as_str()
        ),
        (Some("model"), Some("sonnet"))
    );
    assert_eq!(
        logged.harness.turn("plain").await,
        StopReason::EndTurn,
        "the session goes on"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_carries_its_attachments_as_content_blocks_after_its_text() {
    let mut logged = offering(&[]).await;
    let text = Attachment::from_bytes("notes.md", "text/plain", b"# Notes".to_vec()).unwrap();
    let image = Attachment::from_bytes("shot.png", "image/png", vec![1, 2, 3]).unwrap();
    logged
        .harness
        .agent
        .prompt_with("plain look at these".to_string(), vec![text, image])
        .unwrap();
    logged
        .harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    let received = logged.received();
    let prompt = received
        .iter()
        .find(|m| m["method"] == "session/prompt")
        .unwrap();
    let blocks = prompt["params"]["prompt"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["text"], "plain look at these");
    assert_eq!(blocks[1]["resource"]["text"], "# Notes");
    assert_eq!(blocks[2]["type"], "image");
    assert_eq!(blocks[2]["mimeType"], "image/png");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kept_load_tries_directly_when_session_list_has_more_pages() {
    let mut harness = Harness::start(&["--more-session-pages"]);
    harness.agent.load_session("fake-session-1".into()).unwrap();
    let reply = harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::SessionOpened { .. } | AgentEvent::SessionUnavailable { .. }
            )
        })
        .await;
    assert_eq!(
        reply,
        AgentEvent::SessionOpened {
            session_id: "fake-session-1".into(),
            is_loaded: true,
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kept_load_reports_a_temporary_acp_failure_without_replacing() {
    let mut harness = Harness::start(&["--fail-load"]);
    harness.agent.load_session("fake-session-1".into()).unwrap();
    let reply = harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::Failed(_)) | AgentEvent::SessionUnavailable { .. }
            )
        })
        .await;
    assert!(
        matches!(&reply, AgentEvent::Thread(ThreadEvent::Failed(problem))
        if problem.kind == ProblemKind::RequestFailed),
        "{reply:?}"
    );
}
