//! One agent process and the session held with it over ACP. The caller starts it with an argument vector
//! (never a shell), sends commands through an [`Agent`] handle and reads typed [`AgentEvent`]s from a
//! bounded channel. Every request has a deadline, the agent's lines and its stderr are read into bounded
//! buffers, and whatever way the process ends becomes exactly one last event, [`AgentEvent::Ended`].
//!
//! The client declares no file system and no terminal capability: the agent's workspace lives in its
//! container, so the agent reads, writes and runs there with its own tools.
//!
//! Runs on a tokio runtime (the one the app already gives `sakura-core`). The channel is the bridge to the
//! window. When the window stops reading, the agent's messages wait between the pipe and the handlers,
//! counted (`transport::Unread`); an agent that goes on past the count's limit ends the connection with
//! a problem instead of filling memory.

mod codex;
mod master;
mod process;
mod session;
mod shared;

pub use codex::{
    start_codex_login, CodexConfig, CodexLogin, CodexLoginConfig, LoginEnd, LoginEvent,
    LoginFailure,
};

use crate::attachment::Attachment;
use crate::model::{PromptSupport, SessionChoices, SlashCommand};
use crate::policy::decider::Decider;
use crate::policy::Runs;
use crate::thread::{Problem, ProblemKind, ThreadEvent};
use crate::transport::Unread;
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Events waiting for the window before the agent's output is held back.
const EVENT_BUFFER: usize = 256;
/// Commands waiting for the connection; the window sends a handful per turn.
const COMMAND_BUFFER: usize = 32;

/// How to start an agent and hold a session with it.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The program and its arguments, run as given: `box acp <task> <account>`.
    pub command: Vec<String>,
    /// Environment variables set for the program on top of the app's own.
    pub env: Vec<(String, String)>,
    /// The agent's working directory as the agent sees it (`/work/<task>` in its container): the session's
    /// `cwd`, and the workspace the confinement policy allows without asking.
    pub workspace: PathBuf,
    pub deadlines: Deadlines,
    /// The most bytes of the agent's messages held before the window has been given them
    /// ([`crate::transport::MAX_UNREAD_BYTES`] unless a test needs less): past it the connection ends.
    pub max_unread_bytes: usize,
    /// The agent asks before each tool it is told to (a read, a command, a write, a fetch), whatever its
    /// login's own settings allow, so the confinement policy answers what stays inside and the person the
    /// rest; the policy also judges as on the Mac. For a task on the Mac, where nothing else stands
    /// between the agent and the person's files; in a box the box is the boundary.
    pub asks_beyond_workspace: bool,
    /// What a master adds to the sessions of its agent; empty for any other agent.
    pub session: SessionSetup,
}

/// What a session is opened with and what is done around it, beyond a plain agent's: the engine writes it
/// for its master and the app only passes it on.
#[derive(Debug, Clone, Default)]
pub struct SessionSetup {
    /// The `_meta` of `session/new` and `session/load` (for the Claude adapter, `{"claudeCode":
    /// {"options": ...}}`). It stands in place of the asking options of `asks_beyond_workspace`.
    pub meta: Option<Value>,
    /// The MCP servers of `session/new` and `session/load`, in the protocol's shape, sent as they are.
    pub mcp_servers: Vec<Value>,
    /// The mode set with `session/set_mode` as soon as a session is open, before any prompt: the adapter
    /// starts a session in the mode of the person's own settings.
    pub start_mode: Option<String>,
    /// The model set with `session/set_config_option` right after the start mode, before any prompt, when the
    /// session offers a model that matches it (by its id, else by a word of its name): the adapter starts a
    /// session on its own first model, whatever the options of `session/new` say. The person's own pick in
    /// the conversation comes later and wins over it.
    pub start_model: Option<String>,
    /// The reasoning effort set the same way, after the model (which decides the efforts that are offered).
    pub start_effort: Option<String>,
    /// Refuse a new session whose reported model or effort differs from the requested start.
    pub requires_start_choices: bool,
    /// Whether a session is closed with `session/close` when it is replaced and when the connection ends:
    /// the adapter keeps a session's child processes until then.
    pub closes_sessions: bool,
    /// The names of the variables of the app's own environment the agent inherits, and nothing else: the
    /// agent starts from an empty environment. `None` leaves it the app's whole environment.
    pub env_allow: Option<Vec<String>>,
    /// Variables of the app's own environment the agent must not inherit; `env` is set after them.
    pub unset_env: Vec<String>,
    /// Decides each permission request in place of the confinement policy.
    pub decider: Option<Decider>,
}

/// How long each kind of request may take.
#[derive(Debug, Clone, Copy)]
pub struct Deadlines {
    /// `initialize`, `session/new` and `session/list`.
    pub request: Duration,
    /// `session/load`, which replays a whole conversation before it answers.
    pub load: Duration,
    /// A turn with no message from the agent for this long, and no question open, has stalled.
    pub idle_turn: Duration,
    /// How long the agent has to end a turn after `session/cancel`.
    pub cancel: Duration,
    /// How long the process has to exit once its input is closed, before it is killed.
    pub shutdown: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(30),
            load: Duration::from_secs(120),
            idle_turn: Duration::from_secs(15 * 60),
            cancel: Duration::from_secs(10),
            shutdown: Duration::from_secs(2),
        }
    }
}

/// What the connection tells the window.
// The large variant is the common one (every streamed chunk is a thread event), so boxing it would cost
// an allocation per chunk to save space in a channel that holds a few hundred events at most.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The agent answered `initialize`.
    Connected { name: String, version: String },
    /// A session is open. `is_loaded` says it is a stored one, replayed before this event; the caller
    /// stores `session_id` for the task.
    SessionOpened { session_id: String, is_loaded: bool },
    /// The prepared launch accepts a first prompt, which will allocate the session.
    SessionReady,
    /// The stored session could not be loaded, and the caller requested no replacement.
    SessionUnavailable { session_id: String },
    /// The agent has no signed-in account: no session can open until the person signs in.
    AuthRequired,
    /// What the agent's prompts take besides text; sent once, after `initialize`.
    Supports(PromptSupport),
    /// The slash commands the agent announces now, all of them: they replace the ones before.
    Commands(Vec<SlashCommand>),
    /// What the session lets the person pick, as far as the agent said it: a part it did not mention is
    /// left as it was.
    Choices(SessionChoices),
    /// The session is in another mode now (the person set it, or the agent did).
    ModeChanged(String),
    /// Something for the conversation's reducer.
    Thread(ThreadEvent),
    /// The connection is over and nothing follows. A problem says why, unless the caller asked to stop.
    Ended(Option<Problem>),
}

/// Why a command could not be handed to the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The connection has ended.
    Gone,
    /// Too many commands wait already.
    Busy,
}

/// What the window asks of the connection; the agents of both protocols obey the same commands.
#[derive(Debug)]
enum Command {
    Open {
        stored: Option<String>,
        allows_replacement: bool,
    },
    Prompt {
        text: String,
        attachments: Vec<Attachment>,
    },
    SetMode(String),
    SetOption {
        config_id: String,
        value: String,
    },
    Cancel,
    Answer {
        request: u64,
        option_id: String,
    },
    Shutdown,
}

/// The handle to a running agent. Dropping it shuts the agent down.
#[derive(Debug)]
pub struct Agent {
    commands: mpsc::Sender<Command>,
    shutdown: Option<watch::Sender<()>>,
}

impl Agent {
    /// Starts the agent on `runtime` and returns its handle and its events. `initialize` is sent at once;
    /// [`AgentEvent::Connected`] or [`AgentEvent::Ended`] follows.
    pub fn start(
        config: AgentConfig,
        runtime: &tokio::runtime::Handle,
    ) -> (Agent, mpsc::Receiver<AgentEvent>) {
        Self::start_resolving(async move { Ok(config) }, runtime)
    }

    /// Like [`Agent::start`], for an agent whose configuration is not known yet (the engine writes a
    /// master's files when it is requested them): commands sent meanwhile wait, and a configuration that
    /// cannot be had ends the connection with that problem.
    pub fn start_resolving(
        config: impl Future<Output = Result<AgentConfig, Problem>> + Send + 'static,
        runtime: &tokio::runtime::Handle,
    ) -> (Agent, mpsc::Receiver<AgentEvent>) {
        let (commands, received) = mpsc::channel(COMMAND_BUFFER);
        let (events, read) = mpsc::channel(EVENT_BUFFER);
        runtime.spawn(run(config, received, events));
        (
            Agent {
                commands,
                shutdown: None,
            },
            read,
        )
    }

    /// Starts an engine-owned ACP Master. The operator owns its adapter and session lifecycle.
    pub fn start_master_resolving(
        launch: impl Future<Output = Result<sakura_core::engine::master::MasterAcp, Problem>>
            + Send
            + 'static,
        runtime: &tokio::runtime::Handle,
    ) -> (Agent, mpsc::Receiver<AgentEvent>) {
        let (commands, received) = mpsc::channel(COMMAND_BUFFER);
        let (events, read) = mpsc::channel(EVENT_BUFFER);
        runtime.spawn(master::run(launch, received, events));
        (
            Agent {
                commands,
                shutdown: None,
            },
            read,
        )
    }

    /// Opens the task's session: the stored one when the agent still has it (its history is replayed),
    /// otherwise a new one.
    pub fn open_session(&self, stored: Option<String>) -> Result<(), SendError> {
        self.send(Command::Open {
            stored,
            allows_replacement: true,
        })
    }

    /// Loads a stored session without starting a replacement when it is unavailable.
    pub fn load_session(&self, session_id: String) -> Result<(), SendError> {
        self.send(Command::Open {
            stored: Some(session_id),
            allows_replacement: false,
        })
    }

    /// Sends the person's prompt; a turn starts.
    pub fn prompt(&self, text: String) -> Result<(), SendError> {
        self.prompt_with(text, Vec::new())
    }

    /// Sends the person's prompt with files and images attached; the caller checked them against
    /// [`AgentEvent::Supports`].
    pub fn prompt_with(&self, text: String, attachments: Vec<Attachment>) -> Result<(), SendError> {
        self.send(Command::Prompt { text, attachments })
    }

    /// Puts the session in the mode `mode_id` (`session/set_mode`); [`AgentEvent::Choices`] follows.
    pub fn set_mode(&self, mode_id: String) -> Result<(), SendError> {
        self.send(Command::SetMode(mode_id))
    }

    /// Sets the session config option `config_id` (the model, say) to `value`
    /// (`session/set_config_option`); [`AgentEvent::Choices`] follows.
    pub fn set_option(&self, config_id: String, value: String) -> Result<(), SendError> {
        self.send(Command::SetOption { config_id, value })
    }

    /// Asks the agent to stop the running turn.
    pub fn cancel(&self) -> Result<(), SendError> {
        self.send(Command::Cancel)
    }

    /// Answers the permission request `request` with one of its options.
    pub fn answer(&self, request: u64, option_id: String) -> Result<(), SendError> {
        self.send(Command::Answer { request, option_id })
    }

    /// Like [`Agent::start_resolving`], for the master's other agent: Codex, over its app-server protocol,
    /// run on a thread of its own. It sends the same events and obeys the same commands as an agent over ACP, so the conversation
    /// cannot tell them apart; it has no modes and no slash commands to offer.
    pub fn start_codex_resolving(
        config: impl Future<Output = Result<CodexConfig, Problem>> + Send + 'static,
    ) -> (Agent, mpsc::Receiver<AgentEvent>) {
        let (commands, received) = mpsc::channel(COMMAND_BUFFER);
        let (events, read) = mpsc::channel(EVENT_BUFFER);
        let (shutdown, closing) = watch::channel(());
        codex::spawn(config, received, closing, events);
        (
            Agent {
                commands,
                shutdown: Some(shutdown),
            },
            read,
        )
    }

    /// Ends the session and the process; [`AgentEvent::Ended`] with no problem follows.
    pub fn shutdown(&self) -> Result<(), SendError> {
        if let Some(shutdown) = &self.shutdown {
            return shutdown.send(()).map_err(|_| SendError::Gone);
        }
        self.send(Command::Shutdown)
    }

    fn send(&self, command: Command) -> Result<(), SendError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => SendError::Busy,
                mpsc::error::TrySendError::Closed(_) => SendError::Gone,
            })
    }
}

/// How the session loop ended.
enum LoopEnd {
    /// The caller asked to stop, or dropped the handle.
    Shutdown,
    /// The caller left while the agent still owed an answer: nothing is waited for, the agent is ended at
    /// once. Only the Codex connection, which has requests of its own to wait on, ends this way.
    Abandoned,
    /// The agent closed its output.
    AgentClosed,
    /// The connection cannot go on.
    Failed(Problem),
}

/// The whole life of one agent: start, serve, and the one last event.
async fn run(
    config: impl Future<Output = Result<AgentConfig, Problem>>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AgentEvent>,
) {
    let problem = match config.await {
        Ok(config) => start_and_serve(config, commands, events.clone()).await,
        Err(problem) => Some(problem),
    };
    events.send(AgentEvent::Ended(problem)).await.ok();
}

async fn start_and_serve(
    config: AgentConfig,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AgentEvent>,
) -> Option<Problem> {
    let started = process::Process::start(
        &config.command,
        &config.env,
        &config.session.unset_env,
        config.session.env_allow.as_deref(),
    );
    match started {
        Ok(process) => serve(process, config, commands, events).await,
        Err(error) => Some(Problem {
            kind: ProblemKind::NotStarted,
            detail: error.to_string(),
        }),
    }
}

/// Holds the connection until it ends, then stops the process in the way that fits how it ended.
async fn serve(
    mut process: process::Process,
    config: AgentConfig,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AgentEvent>,
) -> Option<Problem> {
    let grace = config.deadlines.shutdown;
    let unread = Unread::new(config.max_unread_bytes);
    let lines = process.take_lines(unread.clone());
    let runs = match config.asks_beyond_workspace {
        true => Runs::OnMac,
        false => Runs::InBox,
    };
    let shared = Arc::new(shared::Shared::new(
        events,
        config.workspace.clone(),
        runs,
        config.session.decider.clone(),
        unread.clone(),
    ));
    let drive = session::drive(lines, shared, commands, config);
    tokio::pin!(drive);
    let end = tokio::select! {
        end = &mut drive => end,
        // The process is gone. Its output normally ends with it and the session loop says so; when
        // something it started keeps that output open, the loop gets `grace` to read what is left.
        () = process.exited() => match tokio::time::timeout(grace, &mut drive).await {
            Ok(end) => end,
            Err(_) => Ok(LoopEnd::AgentClosed),
        },
    };
    if let Some(detail) = unread.overrun() {
        // The SDK reports a broken transport without its reason: the count itself says what happened.
        process.stop(Duration::ZERO).await;
        let kind = ProblemKind::Overrun;
        return Some(Problem { kind, detail });
    }
    match end {
        Ok(LoopEnd::Shutdown) => {
            process.stop(grace).await;
            None
        }
        Ok(LoopEnd::Abandoned) => {
            process.stop(Duration::ZERO).await;
            None
        }
        Ok(LoopEnd::Failed(problem)) => {
            process.stop(Duration::ZERO).await;
            Some(problem)
        }
        Ok(LoopEnd::AgentClosed) => Some(process.died(grace, None).await),
        Err(error) => Some(process.died(grace, Some(error.message)).await),
    }
}

#[cfg(test)]
mod goose_master_contract_tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_goose_slot_opens_without_inventing_a_session() {
        let launch = serde_json::from_value(serde_json::json!({
            "protocol_version": 1, "harness": "goose", "slot": "fixture-tab",
            "readiness": "ready", "launch_id": "issued",
            "argv": ["/bin/sh", "-c", "cat >/dev/null; printf '%s' '{\"outcome\":\"empty\",\"launch_id\":\"issued\"}'"],
            "cwd": "/private/tmp",
            "binding": {
                "harness": "goose", "account": "local", "context": "master",
                "storage_namespace": "master:goose:test"
            }
        }))
        .unwrap();
        let (agent, mut events) =
            Agent::start_master_resolving(async { Ok(launch) }, &tokio::runtime::Handle::current());
        assert!(matches!(
            events.recv().await,
            Some(AgentEvent::Connected { .. })
        ));
        agent.open_session(None).unwrap();
        assert_eq!(events.recv().await, Some(AgentEvent::SessionReady));
    }
    #[tokio::test]
    async fn cancelling_a_goose_turn_interrupts_the_operator_before_it_finishes() {
        let folder = tempfile::tempdir().unwrap();
        let launch = serde_json::from_value(serde_json::json!({
            "protocol_version": 1, "harness": "goose", "slot": "fixture-tab",
            "readiness": "ready", "launch_id": "issued",
            "argv": ["/bin/sh", "-c", r#"
                input=$(cat)
                if [ "$input" = '{}' ]; then
                    printf '%s' '{"outcome":"empty","launch_id":"issued"}'
                    exit
                fi
                trap 'touch interrupted; exit 130' INT
                touch started
                while [ ! -e completed ]; do sleep 0.05; done
            "#],
            "cwd": folder.path(),
            "binding": {
                "harness": "goose", "account": "local", "context": "master",
                "storage_namespace": "master:goose:test"
            }
        }))
        .unwrap();
        let (agent, mut events) =
            Agent::start_master_resolving(async { Ok(launch) }, &tokio::runtime::Handle::current());
        assert!(matches!(
            events.recv().await,
            Some(AgentEvent::Connected { .. })
        ));
        agent.open_session(None).unwrap();
        assert_eq!(events.recv().await, Some(AgentEvent::SessionReady));
        agent.prompt("Work".into()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !folder.path().join("started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        agent.cancel().unwrap();
        let cancelled = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;
        std::fs::write(folder.path().join("completed"), "").unwrap();
        assert_eq!(
            cancelled.unwrap(),
            Some(AgentEvent::Thread(ThreadEvent::TurnEnded(
                crate::model::StopReason::Cancelled
            )))
        );
        assert!(folder.path().join("interrupted").is_file());
    }
}
