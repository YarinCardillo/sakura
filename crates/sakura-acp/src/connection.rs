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

mod process;
mod session;
mod shared;

use crate::thread::{Problem, ProblemKind, ThreadEvent};
use crate::transport::Unread;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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
    /// The agent has no signed-in account: no session can open until the person signs in.
    AuthRequired,
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

#[derive(Debug)]
enum Command {
    Open { stored: Option<String> },
    Prompt(String),
    Cancel,
    Answer { request: u64, option_id: String },
    Shutdown,
}

/// The handle to a running agent. Dropping it shuts the agent down.
#[derive(Debug)]
pub struct Agent {
    commands: mpsc::Sender<Command>,
}

impl Agent {
    /// Starts the agent on `runtime` and returns its handle and its events. `initialize` is sent at once;
    /// [`AgentEvent::Connected`] or [`AgentEvent::Ended`] follows.
    pub fn start(
        config: AgentConfig,
        runtime: &tokio::runtime::Handle,
    ) -> (Agent, mpsc::Receiver<AgentEvent>) {
        let (commands, received) = mpsc::channel(COMMAND_BUFFER);
        let (events, read) = mpsc::channel(EVENT_BUFFER);
        runtime.spawn(run(config, received, events));
        (Agent { commands }, read)
    }

    /// Opens the task's session: the stored one when the agent still has it (its history is replayed),
    /// otherwise a new one.
    pub fn open_session(&self, stored: Option<String>) -> Result<(), SendError> {
        self.send(Command::Open { stored })
    }

    /// Sends the person's prompt; a turn starts.
    pub fn prompt(&self, text: String) -> Result<(), SendError> {
        self.send(Command::Prompt(text))
    }

    /// Asks the agent to stop the running turn.
    pub fn cancel(&self) -> Result<(), SendError> {
        self.send(Command::Cancel)
    }

    /// Answers the permission request `request` with one of its options.
    pub fn answer(&self, request: u64, option_id: String) -> Result<(), SendError> {
        self.send(Command::Answer { request, option_id })
    }

    /// Ends the session and the process; [`AgentEvent::Ended`] with no problem follows.
    pub fn shutdown(&self) -> Result<(), SendError> {
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
    /// The agent closed its output.
    AgentClosed,
    /// The connection cannot go on.
    Failed(Problem),
}

/// The whole life of one agent: start, serve, and the one last event.
async fn run(
    config: AgentConfig,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AgentEvent>,
) {
    let problem = match process::Process::start(&config.command, &config.env) {
        Ok(process) => serve(process, config, commands, events.clone()).await,
        Err(error) => Some(Problem {
            kind: ProblemKind::NotStarted,
            detail: error.to_string(),
        }),
    };
    events.send(AgentEvent::Ended(problem)).await.ok();
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
    let shared = Arc::new(shared::Shared::new(
        events,
        config.workspace.clone(),
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
        Ok(LoopEnd::Failed(problem)) => {
            process.stop(Duration::ZERO).await;
            Some(problem)
        }
        Ok(LoopEnd::AgentClosed) => Some(process.died(grace, None).await),
        Err(error) => Some(process.died(grace, Some(error.message)).await),
    }
}
