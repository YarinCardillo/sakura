//! The master's Codex agent against the fake app-server, as a child process over stdio: what a turn shows,
//! what the engine's decider and the person decide, and that nothing runs that the engine did not allow.

use sakura_acp::attachment::Attachment;
use sakura_acp::connection::{
    Agent, AgentConfig, AgentEvent, CodexConfig, Deadlines, SessionSetup,
};
use sakura_acp::model::{StopReason, ToolKind, ToolStatus, Update};
use sakura_acp::policy::decider::{Decider, Verdict};
use sakura_acp::thread::{
    EntryKind, PermissionOutcome, ProblemKind, Thread, ThreadEvent, ToolCall, Turn,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

/// No test waits longer than this for an event.
const PATIENCE: Duration = Duration::from_secs(20);

/// What the fake sees of the decider: every request put to it.
type Asked = Arc<Mutex<Vec<Value>>>;

/// A Codex agent, its events, and the thread they reduce to.
struct Harness {
    agent: Agent,
    events: Receiver<AgentEvent>,
    thread: Thread,
    log: tempfile::TempDir,
    asked: Asked,
}

fn decider(verdict: Verdict, asked: &Asked) -> Decider {
    let asked = asked.clone();
    Decider::new(move |request| {
        asked.lock().expect("the list").push(request);
        Box::pin(async move { verdict })
    })
}

fn config(log: &std::path::Path, decider: Option<Decider>, env: &[(&str, &str)]) -> CodexConfig {
    let workspace = log.canonicalize().unwrap();
    let mut variables = vec![
        (
            "SAKURA_FAKE_WORKSPACE".to_string(),
            workspace.display().to_string(),
        ),
        (
            "SAKURA_FAKE_LOG".to_string(),
            log.join("codex.log").display().to_string(),
        ),
    ];
    variables.extend(
        env.iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
    );
    let server = json!({ "name": "sakura-master", "command": "/bin/sakura", "args": ["master", "serve", "p"],
        "env": [{ "name": "SAKURA_ENGINE_HOME", "value": "/engine" }] });
    CodexConfig {
        agent: AgentConfig {
            command: vec![env!("CARGO_BIN_EXE_sakura-fake-codex").to_string()],
            env: variables,
            workspace,
            deadlines: Deadlines::default(),
            max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
            asks_beyond_workspace: false,
            session: SessionSetup {
                mcp_servers: vec![server],
                decider,
                ..SessionSetup::default()
            },
        },
        instructions: "You are the master.".to_string(),
        model: None,
        effort: None,
        collaboration: None,
        allowed_tools: vec![
            "mcp__sakura-master__lane_open".into(),
            "mcp__sakura-master__lanes".into(),
        ],
    }
}

impl Harness {
    fn start(verdict: Option<Verdict>, env: &[(&str, &str)]) -> Self {
        Self::start_with(verdict, env, |_| {})
    }

    /// The same, with `edit` changing the configuration first.
    fn start_with(
        verdict: Option<Verdict>,
        env: &[(&str, &str)],
        edit: impl FnOnce(&mut CodexConfig),
    ) -> Self {
        let log = tempfile::tempdir().expect("a folder");
        let asked = Asked::default();
        let decider = verdict.map(|verdict| decider(verdict, &asked));
        let mut config = config(log.path(), decider, env);
        edit(&mut config);
        let (agent, events) = Agent::start_codex_resolving(async move { Ok(config) });
        Self {
            agent,
            events,
            thread: Thread::new(),
            log,
            asked,
        }
    }

    async fn next(&mut self) -> AgentEvent {
        let event = tokio::time::timeout(PATIENCE, self.events.recv()).await;
        let event = event
            .expect("an event arrives in time")
            .expect("the last event comes first");
        if let AgentEvent::Thread(event) = &event {
            self.thread.apply(event.clone());
        }
        event
    }

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

    /// A harness with a new thread open.
    async fn opened(verdict: Option<Verdict>) -> Self {
        let mut harness = Self::start(verdict, &[]);
        harness.agent.open_session(None).expect("open");
        harness
            .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
            .await;
        harness
    }

    fn prompt(&mut self, text: &str) {
        self.thread.apply(ThreadEvent::Prompted(text.to_string()));
        self.agent.prompt(text.to_string()).expect("prompt");
    }

    /// Sends a prompt and reads until the turn ends; the reason.
    async fn turn(&mut self, text: &str) -> StopReason {
        self.prompt(text);
        let ended = self
            .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
            .await;
        let AgentEvent::Thread(ThreadEvent::TurnEnded(reason)) = ended else {
            unreachable!("a turn end");
        };
        reason
    }

    /// What the fake received, one message each.
    fn received(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.log.path().join("codex.log")).unwrap_or_default();
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }

    fn answer(&self) -> String {
        let last = self
            .thread
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::Assistant(message) => Some(message.text.clone()),
                _ => None,
            });
        last.unwrap_or_default()
    }

    fn tools(&self) -> Vec<ToolCall> {
        let tool = |entry: &sakura_acp::thread::Entry| match &entry.kind {
            EntryKind::Tool(call) => Some(call.clone()),
            _ => None,
        };
        self.thread.entries().iter().filter_map(tool).collect()
    }
}

#[tokio::test]
async fn codex_connects_opens_a_thread_and_offers_its_models() {
    let mut harness = Harness::start(None, &[]);
    harness.agent.open_session(None).expect("open");
    let connected = harness
        .until(|event| matches!(event, AgentEvent::Connected { .. }))
        .await;
    assert_eq!(
        connected,
        AgentEvent::Connected {
            name: "codex".into(),
            version: "9.9.9".into()
        }
    );
    let choices = harness
        .until(|event| matches!(event, AgentEvent::Choices(_)))
        .await;
    let AgentEvent::Choices(choices) = choices else {
        unreachable!()
    };
    let models = choices.models.expect("models");
    assert_eq!(models.current, "gpt-fast");
    assert_eq!(
        models
            .choices
            .iter()
            .map(|choice| choice.name.as_str())
            .collect::<Vec<_>>(),
        ["Fast", "Deep"]
    );
    let modes = choices.modes.expect("Codex has its collaboration modes");
    assert_eq!(modes.current, "default");
    assert_eq!(
        modes
            .choices
            .iter()
            .map(|mode| mode.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "plan"]
    );
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(
        opened,
        AgentEvent::SessionOpened {
            session_id: "thr-1".into(),
            is_loaded: false
        }
    );
    assert_eq!(
        harness.calls("initialized").len(),
        1,
        "the handshake was finished"
    );
}

#[tokio::test]
async fn every_thread_runs_read_only_with_the_masters_tools_and_nothing_that_runs_commands() {
    let harness = Harness::opened(None).await;
    let start = &harness.calls("thread/start")[0]["params"];
    assert_eq!(start["sandbox"], "read-only");
    assert_eq!(start["approvalPolicy"], "untrusted");
    assert_eq!(
        start["cwd"],
        json!(harness.log.path().canonicalize().unwrap())
    );
    assert_eq!(start["developerInstructions"], "You are the master.");
    let server = &start["config"]["mcp_servers"]["sakura-master"];
    assert_eq!(server["command"], "/bin/sakura");
    assert_eq!(server["args"], json!(["master", "serve", "p"]));
    assert_eq!(server["env"], json!({ "SAKURA_ENGINE_HOME": "/engine" }));
    assert_eq!(
        server["enabled_tools"],
        json!(["lane_open", "lanes"]),
        "only what the engine allows"
    );
    for feature in [
        "shell_tool",
        "unified_exec",
        "apply_patch_freeform",
        "view_image",
    ] {
        assert_eq!(
            start["config"]["features"][feature], false,
            "{feature} is off"
        );
    }
    assert_eq!(start["config"]["web_search"], "disabled");
}

/// What the thread's tools are, as the fake reports it after a turn: `servers name=tool+tool ...`.
async fn tool_inventory(mut harness: Harness) -> String {
    harness.turn("servers").await;
    harness.answer()
}

#[tokio::test]
async fn servers_the_persons_own_codex_configures_are_switched_off_in_a_new_thread() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_INHERITED", "notes,deploy")]);
    harness.agent.open_session(None).expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    let mcp = harness.calls("thread/start")[0]["params"]["config"]["mcp_servers"].clone();
    assert_eq!(mcp["notes"], json!({ "enabled": false }));
    assert_eq!(mcp["deploy"], json!({ "enabled": false }));
    assert_eq!(
        tool_inventory(harness).await,
        "servers sakura-master=lane_open+lanes",
        "nothing but the engine's tools is enabled"
    );
}

#[tokio::test]
async fn servers_the_persons_own_codex_configures_are_switched_off_in_a_resumed_thread() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_INHERITED", "notes")]);
    harness
        .agent
        .open_session(Some("thr-9".into()))
        .expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(
        tool_inventory(harness).await,
        "servers sakura-master=lane_open+lanes"
    );
}

#[tokio::test]
async fn a_server_the_engine_names_is_not_switched_off_with_the_persons_own() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_INHERITED", "sakura-master,notes")]);
    harness.agent.open_session(None).expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(
        tool_inventory(harness).await,
        "servers sakura-master=lane_open+lanes"
    );
}

#[tokio::test]
async fn an_initialize_refusal_ends_the_connection_before_a_thread_opens() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_REFUSE", "initialize")]);
    let AgentEvent::Ended(Some(problem)) = harness.next().await else {
        panic!("the handshake should fail before connecting");
    };
    assert_eq!(problem.kind, ProblemKind::RequestFailed);
    assert!(problem.detail.starts_with("initialize:"));
    assert!(harness.calls("thread/start").is_empty());
}

#[tokio::test]
async fn a_thread_start_refusal_never_retries_with_a_weaker_sandbox() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_REFUSE", "thread/start")]);
    harness.agent.open_session(None).unwrap();
    let failure = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::Failed(_))))
        .await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failure else {
        unreachable!();
    };
    assert!(problem.detail.starts_with("thread/start:"));
    let requests = harness.calls("thread/start");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["params"]["sandbox"], "read-only");
}

#[tokio::test]
async fn when_the_servers_configuration_cannot_be_read_no_thread_is_opened() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_REFUSE", "config/read")]);
    harness.agent.open_session(None).expect("open");
    let failed = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::Failed(_))))
        .await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!("a failure");
    };
    assert_eq!(problem.kind, ProblemKind::RequestFailed);
    assert!(problem.detail.contains("config/read"), "{}", problem.detail);
    assert!(harness.calls("thread/start").is_empty());
}

#[tokio::test]
async fn a_plain_turn_streams_one_answer_and_ends_as_a_normal_turn() {
    let mut harness = Harness::opened(None).await;
    assert_eq!(harness.turn("plain").await, StopReason::EndTurn);
    assert_eq!(
        harness.answer(),
        "Hello there.",
        "streamed once, not shown again when it completed"
    );
    assert_eq!(harness.thread.turn(), Turn::Idle);
    let turn = &harness.calls("turn/start")[0]["params"];
    assert_eq!(turn["threadId"], "thr-1");
    assert_eq!(turn["input"][0]["text"], "plain");
}

#[tokio::test]
async fn thoughts_stream_as_one_thought_with_a_break_between_paragraphs() {
    let mut harness = Harness::opened(None).await;
    harness.turn("think").await;
    let thought = harness
        .thread
        .entries()
        .iter()
        .find_map(|entry| match &entry.kind {
            EntryKind::Thought(message) => Some(message.text.clone()),
            _ => None,
        });
    assert_eq!(thought.as_deref(), Some("First, \n\nthen second."));
    assert_eq!(harness.answer(), "Thought it through.");
}

#[tokio::test]
async fn a_call_of_the_masters_tool_is_one_line_titled_as_the_adapter_titles_it_with_its_answer_as_text(
) {
    let mut harness = Harness::opened(None).await;
    harness.turn("tool").await;
    let tools = harness.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].title, "mcp__sakura-master__lane_open");
    assert_eq!(
        (tools[0].kind, tools[0].status),
        (ToolKind::Other, ToolStatus::Completed)
    );
    assert_eq!(tools[0].raw_input, Some(json!({ "project": "demo" })));
    let sakura_acp::model::ToolContent::Text { text, .. } = &tools[0].content[0] else {
        panic!("a text result");
    };
    assert_eq!(text, "{\"lane\": \"demo\"}");
}

#[tokio::test]
async fn a_command_the_engine_refuses_is_declined_and_never_reaches_the_person() {
    let mut harness = Harness::opened(Some(Verdict::Deny)).await;
    harness.turn("command").await;
    assert_eq!(harness.answer(), "command decline");
    let asked = harness.asked.lock().expect("the list").clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["params"]["toolCall"]["name"], "Bash");
    assert_eq!(
        asked[0]["params"]["toolCall"]["rawInput"]["command"],
        "git status"
    );
    assert_eq!(
        asked[0]["params"]["toolCall"]["rawInput"]["cwd"],
        json!(harness.log.path().canonicalize().unwrap()),
        "where it runs is judged too"
    );
    let has_card = harness
        .thread
        .entries()
        .iter()
        .any(|entry| matches!(entry.kind, EntryKind::Permission(_)));
    assert!(!has_card, "the person was not asked");
    assert_eq!(harness.tools()[0].status, ToolStatus::Rejected);
}

#[tokio::test]
async fn an_approval_that_names_no_command_is_declined_without_asking_the_engine_or_the_person() {
    let mut harness = Harness::opened(Some(Verdict::Ask)).await;
    harness.turn("nameless").await;
    assert_eq!(harness.answer(), "nameless decline");
    assert!(
        harness.asked.lock().expect("the list").is_empty(),
        "nothing the engine could judge"
    );
}

/// Sends `word` to a harness whose decider would let everything through to the person, and says what the
/// fake was answered and how many requests the engine was asked about.
async fn answered_to(word: &str) -> (String, usize) {
    let mut harness = Harness::opened(Some(Verdict::Ask)).await;
    harness.turn(word).await;
    let asked = harness.asked.lock().expect("the list").len();
    (harness.answer(), asked)
}

#[tokio::test]
async fn a_command_that_runs_outside_the_masters_folder_is_declined_without_asking_anyone() {
    assert_eq!(
        answered_to("foreigncwd").await,
        ("command decline".to_string(), 0)
    );
}

#[tokio::test]
async fn a_command_request_with_a_field_nobody_here_understands_is_declined() {
    assert_eq!(
        answered_to("unknownfield").await,
        ("command decline".to_string(), 0)
    );
    assert_eq!(
        answered_to("amendment").await,
        ("command decline".to_string(), 0)
    );
}

#[tokio::test]
async fn a_patch_that_moves_a_file_is_judged_at_its_destination_too_and_says_what_it_does() {
    let mut harness = Harness::opened(Some(Verdict::Ask)).await;
    harness.prompt("moveout");
    harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::PermissionAsked { .. })
            )
        })
        .await;
    let asked = harness.asked.lock().expect("the list").clone();
    let judged: Vec<(&str, &str)> = asked
        .iter()
        .map(|request| {
            let input = &request["params"]["toolCall"]["rawInput"];
            (
                input["file_path"].as_str().unwrap_or_default(),
                input["kind"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        judged,
        [("/work/a.txt", "update"), ("/outside/b.txt", "move")]
    );
}

#[tokio::test]
async fn a_patch_request_that_asks_for_a_root_or_carries_an_unknown_field_is_declined() {
    assert_eq!(
        answered_to("grantroot").await,
        ("patch decline".to_string(), 0)
    );
    assert_eq!(
        answered_to("patchunknown").await,
        ("patch decline".to_string(), 0)
    );
}

#[tokio::test]
async fn a_command_with_no_decider_is_declined() {
    let mut harness = Harness::opened(None).await;
    harness.turn("command").await;
    assert_eq!(harness.answer(), "command decline");
}

#[tokio::test]
async fn a_command_the_engine_only_asks_about_goes_to_the_person_and_runs_when_they_allow_it() {
    let mut harness = Harness::opened(Some(Verdict::Ask)).await;
    harness.prompt("command");
    let asked = harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::PermissionAsked { .. })
            )
        })
        .await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked {
        request, options, ..
    }) = asked
    else {
        unreachable!();
    };
    assert_eq!(
        options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["accept", "decline"]
    );
    harness
        .agent
        .answer(request, "accept".into())
        .expect("answer");
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.answer(), "command accept");
    let waiting = harness
        .thread
        .entries()
        .iter()
        .find_map(|entry| match &entry.kind {
            EntryKind::Permission(permission) => Some(permission.outcome),
            _ => None,
        });
    assert!(matches!(waiting, Some(PermissionOutcome::Selected(_))));
}

#[tokio::test]
async fn a_command_the_person_declines_is_declined() {
    let mut harness = Harness::opened(Some(Verdict::Ask)).await;
    harness.prompt("command");
    let asked = harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::PermissionAsked { .. })
            )
        })
        .await;
    let AgentEvent::Thread(ThreadEvent::PermissionAsked { request, .. }) = asked else {
        unreachable!();
    };
    harness
        .agent
        .answer(request, "decline".into())
        .expect("answer");
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(harness.answer(), "command decline");
}

#[tokio::test]
async fn a_patch_is_put_to_the_decider_as_an_edit_of_each_file_and_a_refusal_declines_it() {
    let mut harness = Harness::opened(Some(Verdict::Deny)).await;
    harness.turn("patch").await;
    assert_eq!(harness.answer(), "patch decline");
    let asked = harness.asked.lock().expect("the list").clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["params"]["toolCall"]["name"], "Edit");
    assert_eq!(
        asked[0]["params"]["toolCall"]["rawInput"]["file_path"],
        "/work/a.txt"
    );
}

#[tokio::test]
async fn a_permission_grant_is_refused_and_a_patch_with_no_decider_is_declined() {
    let mut harness = Harness::opened(None).await;
    harness.turn("grant").await;
    assert_eq!(harness.answer(), "grant {}", "nothing was granted");
    harness.turn("patch").await;
    assert_eq!(harness.answer(), "patch decline");
}

#[tokio::test]
async fn with_no_model_picked_no_model_is_sent_and_a_server_that_lists_only_its_own_answers() {
    // The app-server of a ChatGPT account rejects a model it does not list: nothing is sent unless the person
    // picked one of the list, and the thread answers with Codex's own default.
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_STRICT_MODELS", "1")]);
    harness.agent.open_session(None).expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    harness.turn("model").await;
    assert_eq!(
        harness.answer(),
        "model none",
        "no model was named to the server"
    );
}

#[tokio::test]
async fn a_model_the_server_does_not_list_is_refused_by_it_in_its_own_words() {
    let mut harness = Harness::start_with(None, &[("SAKURA_FAKE_STRICT_MODELS", "1")], |config| {
        config.model = Some("gpt-5.5".into());
    });
    harness.agent.open_session(None).expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    harness.prompt("plain");
    let problem = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::Failed(_))))
        .await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = problem else {
        unreachable!()
    };
    assert!(problem.detail.contains("not supported"), "{problem:?}");
}

/// The choices after `harness` changed something, once the next `Choices` event arrives.
async fn choices_now(harness: &mut Harness) -> sakura_acp::model::SessionChoices {
    let event = harness
        .until(|event| matches!(event, AgentEvent::Choices(_)))
        .await;
    let AgentEvent::Choices(choices) = event else {
        unreachable!()
    };
    choices
}

#[tokio::test]
async fn each_model_lists_the_efforts_it_accepts_and_the_one_in_force_is_the_models_own_default_until_picked(
) {
    let mut harness = Harness::opened(None).await;
    // Setting nothing makes the connection say what it offers now.
    harness
        .agent
        .set_option("effort".into(), String::new())
        .expect("set");
    let first = choices_now(&mut harness).await;
    let efforts = first.efforts.expect("the efforts of the first model");
    assert_eq!(
        efforts
            .choices
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["low", "medium", "high", "xhigh"]
    );
    assert_eq!(efforts.current, "medium", "the model's own default");
    harness
        .agent
        .set_option("effort".into(), "high".into())
        .expect("set");
    assert_eq!(
        choices_now(&mut harness)
            .await
            .efforts
            .expect("efforts")
            .current,
        "high"
    );
    // A model that does not accept the effort goes back to its own default, and keeps it when it does.
    harness
        .agent
        .set_option("effort".into(), "xhigh".into())
        .expect("set");
    choices_now(&mut harness).await;
    harness
        .agent
        .set_option("model".into(), "gpt-deep".into())
        .expect("set");
    let deep = choices_now(&mut harness).await.efforts.expect("efforts");
    assert_eq!(deep.choices.len(), 3);
    assert_eq!(
        deep.current, "low",
        "xhigh is not one of its efforts: its default"
    );
    harness
        .agent
        .set_option("effort".into(), "high".into())
        .expect("set");
    choices_now(&mut harness).await;
    harness
        .agent
        .set_option("model".into(), "gpt-fast".into())
        .expect("set");
    assert_eq!(
        choices_now(&mut harness)
            .await
            .efforts
            .expect("efforts")
            .current,
        "high",
        "kept: it accepts it"
    );
}

#[tokio::test]
async fn a_turn_names_only_what_the_person_chose_effort_speed_and_plan_included() {
    let mut harness = Harness::opened(None).await;
    harness.turn("settings").await;
    assert_eq!(
        harness.answer(),
        "settings model=none effort=none tier=none mode=none"
    );
    harness
        .agent
        .set_option("effort".into(), "high".into())
        .expect("set");
    harness
        .agent
        .set_option("service_tier".into(), "true".into())
        .expect("set");
    harness.agent.set_mode("plan".into()).expect("mode");
    harness.turn("settings").await;
    assert_eq!(
        harness.answer(),
        "settings model=none effort=high tier=priority mode=plan"
    );
    harness.agent.set_mode("default".into()).expect("mode");
    harness
        .agent
        .set_option("service_tier".into(), "false".into())
        .expect("set");
    harness.turn("settings").await;
    assert_eq!(
        harness.answer(),
        "settings model=none effort=high tier=none mode=default"
    );
}

#[tokio::test]
async fn the_fast_tier_is_offered_only_for_a_model_that_has_one() {
    let mut harness = Harness::opened(None).await;
    // Setting nothing makes the connection say what it offers now.
    harness
        .agent
        .set_option("effort".into(), String::new())
        .expect("set");
    let first = choices_now(&mut harness).await;
    let speed = first.others.first().expect("the fast switch");
    assert!(speed.is_toggle && speed.name == "Fast" && speed.current == "false");
    harness
        .agent
        .set_option("model".into(), "gpt-deep".into())
        .expect("set");
    assert!(
        choices_now(&mut harness).await.others.is_empty(),
        "the second model has no tier"
    );
}

#[tokio::test]
async fn the_model_the_person_picks_is_the_model_of_the_next_turns() {
    let mut harness = Harness::opened(None).await;
    harness
        .agent
        .set_option("model".into(), "gpt-deep".into())
        .expect("set");
    let choices = harness
        .until(|event| matches!(event, AgentEvent::Choices(_)))
        .await;
    let AgentEvent::Choices(choices) = choices else {
        unreachable!()
    };
    assert_eq!(choices.models.expect("models").current, "gpt-deep");
    harness.turn("model").await;
    assert_eq!(harness.answer(), "model gpt-deep");
}

#[tokio::test]
async fn stopping_a_turn_interrupts_it_and_the_turn_ends_as_cancelled() {
    let mut harness = Harness::opened(None).await;
    harness.prompt("slow");
    harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::Update(Update::AgentText(_)))
            )
        })
        .await;
    harness.thread.apply(ThreadEvent::CancelRequested);
    harness.agent.cancel().expect("cancel");
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    assert_eq!(
        ended,
        AgentEvent::Thread(ThreadEvent::TurnEnded(StopReason::Cancelled))
    );
    let interrupt = &harness.calls("turn/interrupt")[0]["params"];
    assert_eq!(
        (interrupt["threadId"].as_str(), interrupt["turnId"].as_str()),
        (Some("thr-1"), Some("turn-1"))
    );
}

#[tokio::test]
async fn a_turn_that_failed_says_why_in_the_servers_words() {
    let mut harness = Harness::opened(None).await;
    harness.prompt("fail");
    let failed = harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::Failed(_))))
        .await;
    let AgentEvent::Thread(ThreadEvent::Failed(problem)) = failed else {
        unreachable!()
    };
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::RequestFailed, "usage limit reached")
    );
    assert_eq!(harness.thread.turn(), Turn::Idle);
}

#[tokio::test]
async fn a_stored_thread_is_resumed_and_its_turns_are_replayed() {
    let mut harness = Harness::start(None, &[]);
    harness
        .agent
        .open_session(Some("thr-old".into()))
        .expect("open");
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(
        opened,
        AgentEvent::SessionOpened {
            session_id: "thr-old".into(),
            is_loaded: true
        }
    );
    let kinds: Vec<&str> = harness
        .thread
        .entries()
        .iter()
        .map(|entry| match &entry.kind {
            EntryKind::User(_) => "user",
            EntryKind::Thought(_) => "thought",
            EntryKind::Assistant(_) => "answer",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["user", "thought", "answer"]);
    assert!(harness
        .thread
        .entries()
        .iter()
        .all(|entry| entry.is_replayed));
    assert_eq!(
        harness.calls("thread/resume")[0]["params"]["threadId"],
        "thr-old"
    );
    harness.turn("plain").await;
    assert_eq!(
        harness.calls("turn/start")[0]["params"]["threadId"],
        "thr-old",
        "the conversation goes on in it"
    );
}

#[tokio::test]
async fn a_stored_thread_the_server_no_longer_has_is_replaced_by_a_new_one() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_GONE", "thr-old")]);
    harness
        .agent
        .open_session(Some("thr-old".into()))
        .expect("open");
    let opened = harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(
        opened,
        AgentEvent::SessionOpened {
            session_id: "thr-1".into(),
            is_loaded: false
        }
    );
    assert!(
        harness.thread.entries().is_empty(),
        "nothing of the lost conversation is shown"
    );
}

#[tokio::test]
async fn an_image_goes_as_a_data_address_and_a_text_file_as_text() {
    let mut harness = Harness::opened(None).await;
    let image = Attachment::from_bytes("shot.png", "image/png", vec![1, 2, 3]).expect("an image");
    let notes =
        Attachment::from_bytes("notes.md", "text/plain", b"# Notes".to_vec()).expect("text");
    harness.thread.apply(ThreadEvent::Prompted("look".into()));
    harness
        .agent
        .prompt_with("look".into(), vec![image, notes])
        .expect("prompt");
    harness
        .until(|event| matches!(event, AgentEvent::Thread(ThreadEvent::TurnEnded(_))))
        .await;
    let input = &harness.calls("turn/start")[0]["params"]["input"];
    assert_eq!(input[1]["url"], "data:image/png;base64,AQID");
    assert!(input[2]["text"]
        .as_str()
        .is_some_and(|text| text.contains("# Notes")));
}

#[tokio::test]
async fn shutting_down_ends_the_connection_with_no_problem() {
    let mut harness = Harness::opened(None).await;
    harness.agent.shutdown().expect("shutdown");
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Ended(_)))
        .await;
    assert_eq!(ended, AgentEvent::Ended(None));
}

/// A harness whose deadlines are short enough to be reached.
async fn hurried() -> Harness {
    let log = tempfile::tempdir().expect("a folder");
    let mut config = config(log.path(), None, &[]);
    let short = Duration::from_millis(300);
    config.agent.deadlines = Deadlines {
        idle_turn: short,
        cancel: short,
        ..Deadlines::default()
    };
    let (agent, events) = Agent::start_codex_resolving(async move { Ok(config) });
    let mut harness = Harness {
        agent,
        events,
        thread: Thread::new(),
        log,
        asked: Asked::default(),
    };
    harness.agent.open_session(None).expect("open");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    harness
}

/// A Codex that stops answering `thread/resume` and ignores the end of its input, asked to resume a thread,
/// with a long grace for a polite stop: only an immediate kill ends it in time. The process id once it hangs.
async fn hung_on_resume(log: &std::path::Path) -> (Agent, Receiver<AgentEvent>, i32) {
    let mut config = config(log, None, &[("SAKURA_FAKE_HANG", "thread/resume")]);
    config.agent.deadlines = Deadlines {
        shutdown: Duration::from_secs(60),
        ..Deadlines::default()
    };
    let (agent, events) = Agent::start_codex_resolving(async move { Ok(config) });
    agent.open_session(Some("thr-5".into())).expect("open");
    let pid_file = log.join("codex.log.pid");
    for _ in 0..200 {
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            return (agent, events, pid.trim().parse().expect("a pid"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the fake never reached thread/resume");
}

/// Waits for the last event; how long it took.
async fn ended_within(events: &mut Receiver<AgentEvent>, limit: Duration) -> Option<AgentEvent> {
    let wait = async {
        while let Some(event) = events.recv().await {
            if matches!(event, AgentEvent::Ended(_)) {
                return Some(event);
            }
        }
        None
    };
    tokio::time::timeout(limit, wait).await.ok().flatten()
}

/// Whether the process `pid` still exists.
fn is_alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

#[tokio::test]
async fn shutting_down_while_a_request_is_unanswered_ends_the_server_at_once() {
    let log = tempfile::tempdir().expect("a folder");
    let (agent, mut events, pid) = hung_on_resume(log.path()).await;
    agent.shutdown().expect("shutdown");
    let ended = ended_within(&mut events, Duration::from_secs(5)).await;
    assert_eq!(ended, Some(AgentEvent::Ended(None)));
    assert!(!is_alive(pid), "the server was killed with the connection");
}

#[tokio::test]
async fn dropping_the_agent_while_a_request_is_unanswered_ends_the_server_at_once() {
    let log = tempfile::tempdir().expect("a folder");
    let (agent, mut events, pid) = hung_on_resume(log.path()).await;
    drop(agent);
    let ended = ended_within(&mut events, Duration::from_secs(5)).await;
    assert_eq!(ended, Some(AgentEvent::Ended(None)));
    assert!(!is_alive(pid), "the server was killed with the connection");
}

#[tokio::test]
async fn a_command_sent_while_a_request_waits_is_still_served_after_its_answer() {
    let mut harness = Harness::start(None, &[]);
    harness.agent.open_session(None).expect("open");
    harness
        .agent
        .set_option("model".into(), "gpt-deep".into())
        .expect("option");
    harness
        .until(|event| matches!(event, AgentEvent::SessionOpened { .. }))
        .await;
    assert_eq!(harness.turn("model").await, StopReason::EndTurn);
    assert_eq!(harness.answer(), "model gpt-deep");
}

#[tokio::test]
async fn a_turn_that_falls_silent_ends_the_connection_with_a_timeout() {
    let mut harness = hurried().await;
    harness.prompt("silent");
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Ended(_)))
        .await;
    let AgentEvent::Ended(Some(problem)) = ended else {
        panic!("a problem, got {ended:?}")
    };
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "turn")
    );
}

#[tokio::test]
async fn a_stop_the_server_does_not_obey_ends_the_connection_with_a_timeout() {
    let mut harness = hurried().await;
    harness.prompt("deaf");
    harness
        .until(|event| {
            matches!(
                event,
                AgentEvent::Thread(ThreadEvent::Update(Update::AgentText(_)))
            )
        })
        .await;
    harness.agent.cancel().expect("cancel");
    let ended = harness
        .until(|event| matches!(event, AgentEvent::Ended(_)))
        .await;
    let AgentEvent::Ended(Some(problem)) = ended else {
        panic!("a problem, got {ended:?}")
    };
    assert_eq!(
        (problem.kind, problem.detail.as_str()),
        (ProblemKind::TimedOut, "turn/interrupt")
    );
}

#[tokio::test]
async fn a_program_that_is_not_there_is_a_problem_that_says_so() {
    let log = tempfile::tempdir().expect("a folder");
    let mut config = config(log.path(), None, &[]);
    config.agent.command = vec!["/no/such/codex".to_string(), "app-server".to_string()];
    let (_agent, mut events) = Agent::start_codex_resolving(async move { Ok(config) });
    let ended = tokio::time::timeout(PATIENCE, events.recv())
        .await
        .expect("in time")
        .expect("an event");
    let AgentEvent::Ended(Some(problem)) = ended else {
        panic!("an end with a problem, got {ended:?}");
    };
    assert_eq!(problem.kind, ProblemKind::NotStarted);
}

/// Types `text` in a conversation that has a thread and reads until the turn ends; the calls the server got of
/// `method`.
async fn slash(text: &str, method: &str) -> (Harness, Vec<Value>) {
    let mut harness = Harness::opened(None).await;
    harness.turn(text).await;
    let calls = harness.calls(method);
    (harness, calls)
}

#[tokio::test]
async fn codex_announces_the_commands_its_terminal_has_with_its_own_words() {
    let mut harness = Harness::start(None, &[]);
    harness.agent.open_session(None).expect("open");
    let AgentEvent::Commands(commands) = harness
        .until(|event| matches!(event, AgentEvent::Commands(_)))
        .await
    else {
        unreachable!("commands");
    };
    let names: Vec<&str> = commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    for wanted in [
        "compact", "review", "fork", "rename", "skills", "goal", "mcp", "status", "pdf",
    ] {
        assert!(names.contains(&wanted), "{wanted} in {names:?}");
    }
    let compact = commands
        .iter()
        .find(|command| command.name == "compact")
        .expect("compact");
    assert_eq!(
        compact.description,
        "summarize the current conversation now"
    );
    let pdf = commands
        .iter()
        .find(|command| command.name == "pdf")
        .expect("a skill");
    assert_eq!(pdf.description, "Read and write PDF files");
}

#[tokio::test]
async fn slash_compact_starts_a_compaction_of_the_thread_and_the_turn_ends() {
    let (harness, calls) = slash("/compact", "thread/compact/start").await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["params"]["threadId"], "thr-1");
    assert!(
        harness.calls("turn/start").is_empty(),
        "not sent as a message"
    );
}

#[tokio::test]
async fn slash_review_reviews_the_uncommitted_changes() {
    let (_harness, calls) = slash("/review", "review/start").await;
    assert_eq!(calls[0]["params"]["target"]["type"], "uncommittedChanges");
}

#[tokio::test]
async fn slash_fork_and_slash_rename_use_the_threads_own_requests() {
    let (harness, calls) = slash("/fork", "thread/fork").await;
    assert_eq!(calls[0]["params"]["threadId"], "thr-1");
    assert!(
        harness.answer().contains("thr-forked"),
        "{}",
        harness.answer()
    );
    let (_harness, calls) = slash("/rename A better name", "thread/name/set").await;
    assert_eq!(calls[0]["params"]["name"], "A better name");
}

#[tokio::test]
async fn slash_skills_lists_them_and_a_skill_runs_as_a_turn_with_the_skill_attached() {
    let (harness, _) = slash("/skills", "skills/list").await;
    assert!(harness.answer().contains("pdf"), "{}", harness.answer());
    let (harness, _) = slash("/pdf make it small", "turn/start").await;
    let sent = harness.calls("turn/start");
    let input = &sent[0]["params"]["input"];
    assert_eq!(input[0]["type"], "skill");
    assert_eq!(input[0]["name"], "pdf");
    assert_eq!(input[1]["text"], "make it small");
}

#[tokio::test]
async fn slash_goal_mcp_and_status_ask_their_own_requests_and_say_what_came_back() {
    let (_harness, calls) = slash("/goal ship it", "thread/goal/set").await;
    assert_eq!(calls[0]["params"]["objective"], "ship it");
    let (harness, calls) = slash("/mcp", "mcpServerStatus/list").await;
    assert_eq!(calls.len(), 1);
    assert!(
        harness.answer().contains("sakura-master"),
        "{}",
        harness.answer()
    );
    let (harness, calls) = slash("/status", "account/rateLimits/read").await;
    assert_eq!(calls.len(), 1);
    assert!(harness.answer().contains("12"), "{}", harness.answer());
}

#[tokio::test]
async fn kept_load_reports_a_temporary_codex_failure_without_replacing() {
    let mut harness = Harness::start(None, &[("SAKURA_FAKE_RESUME_FAILURE", "1")]);
    harness.agent.load_session("thr-old".into()).unwrap();
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
