//! The master's other agent: Codex, over its app-server protocol (`codex app-server`, JSON-RPC on stdio,
//! one message per line). It sends the same [`AgentEvent`]s and obeys the same commands as an agent over
//! ACP, so the conversation shows both alike. It runs in Codex's read-only sandbox and nothing beyond it
//! happens unless the engine's decider lets it (see `approvals`); the master's own tools are offered through
//! Codex's MCP configuration, pointing at the same server the Claude master gets.
//!
//! One thread of Codex is the session. A stored session is resumed and its turns are replayed; a thread
//! that is gone is replaced by a new one. A turn that falls silent, or that is stopped and does not end, ends
//! the connection with a problem, as it does over ACP.

mod approvals;
mod events;
mod items;
mod login;
mod session;
mod slash;
mod wire;

use super::process::Process;
use super::shared::{request_failed, timed_out};
use super::{AgentConfig, AgentEvent, Command, LoopEnd, COMMAND_BUFFER};
use crate::policy::decider::Decider;
use crate::thread::{Problem, ProblemKind, ThreadEvent};
use crate::transport;
use futures::{Stream, StreamExt as _};
pub use login::{
    start_codex_login, CodexLogin, CodexLoginConfig, LoginEnd, LoginEvent, LoginFailure,
};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncWriteExt as _, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use wire::Incoming;

/// The name the client gives the server.
const CLIENT_NAME: &str = "sakura-desk";
/// How often a running turn is checked for a stall and a cancel that was not obeyed.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// Approvals that may wait for the person at once; one more is declined.
const MAX_WAITING_APPROVALS: usize = 32;
/// Items whose fields are remembered, so an approval can name what it is about; the oldest goes first.
const MAX_KNOWN_ITEMS: usize = 512;
/// The sandbox and the approval policy every thread of the master has. The sandbox only reads; the policy
/// asks for whatever Codex does not know to be safe, and the answer is the engine's.
const SANDBOX: &str = "read-only";
const APPROVAL_POLICY: &str = "untrusted";

/// How to start Codex as the master's agent.
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// The program (`codex app-server`), its environment, the master's folder, the deadlines, the engine's
    /// decider and the master's MCP servers in the protocol's shape, as the engine wrote them.
    pub agent: AgentConfig,
    /// The engine's generated instructions for the master, given to every thread as its developer
    /// instructions.
    pub instructions: String,
    /// The model the first turns use; none leaves Codex's own default.
    pub model: Option<String>,
    /// The effort the first turns use, and the collaboration mode (`plan`) the conversation starts in; none
    /// leaves Codex's own.
    pub effort: Option<String>,
    pub collaboration: Option<String>,
    /// The master's tools the engine allows, as `mcp__<server>__<tool>`: the only tools of its server
    /// Codex is given, and those it may call without asking.
    pub allowed_tools: Vec<String>,
}

impl CodexConfig {
    /// The names of the MCP servers the person's own Codex configuration holds, from the answer to
    /// `config/read`.
    pub fn inherited_servers(config_read: &Value) -> Vec<String> {
        let servers = config_read.pointer("/config/mcp_servers");
        let servers = servers.and_then(Value::as_object);
        servers
            .into_iter()
            .flatten()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The `config` of a thread: the master's MCP servers with only the tools the engine allows, every
    /// server of the person's own configuration (`inherited`) that is not one of those switched off, and
    /// the tools of Codex that run commands, change files, look at files or search the web switched off,
    /// so nothing but the engine's tools and the model's own words is left to it.
    pub fn thread_config(&self, inherited: &[String]) -> Value {
        let mut servers: serde_json::Map<String, Value> = self
            .agent
            .session
            .mcp_servers
            .iter()
            .filter_map(|server| self.mcp_server(server))
            .collect();
        for name in inherited {
            servers
                .entry(name.clone())
                .or_insert_with(|| json!({ "enabled": false }));
        }
        json!({
            "mcp_servers": servers,
            "features": {
                "shell_tool": false,
                "unified_exec": false,
                "apply_patch_freeform": false,
                "view_image": false,
            },
            "web_search": "disabled",
        })
    }

    /// One MCP server of the spec as Codex's config writes it; none when the engine allows no tool of it.
    fn mcp_server(&self, server: &Value) -> Option<(String, Value)> {
        let name = server.get("name")?.as_str()?;
        let prefix = format!("mcp__{name}__");
        let tools: Vec<&str> = self
            .allowed_tools
            .iter()
            .filter_map(|tool| tool.strip_prefix(&prefix))
            .collect();
        if tools.is_empty() {
            return None;
        }
        let env: serde_json::Map<String, Value> = server
            .get("env")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pair| {
                Some((
                    pair.get("name")?.as_str()?.to_string(),
                    pair.get("value")?.clone(),
                ))
            })
            .collect();
        let entry = json!({
            "command": server.get("command")?,
            "args": server.get("args").cloned().unwrap_or_else(|| json!([])),
            "env": env,
            "enabled_tools": tools,
            "default_tools_approval_mode": "approve",
        });
        Some((name.to_string(), entry))
    }
}

/// Runs one Codex on a thread of its own, with a runtime of its own that holds nothing else: the
/// conversation with a third party's server is not held to the app's runtime, and its futures need not be
/// sendable between threads. Starting that thread or runtime can fail, which ends the connection with a
/// problem like any other start that fails.
pub(super) fn spawn(
    config: impl Future<Output = Result<CodexConfig, Problem>> + Send + 'static,
    commands: mpsc::Receiver<Command>,
    shutdown: watch::Receiver<()>,
    events: mpsc::Sender<AgentEvent>,
) {
    let ended = events.clone();
    let started = std::thread::Builder::new()
        .name("codex-master".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(run(config, commands, shutdown, events)),
                Err(error) => {
                    let problem = Problem {
                        kind: ProblemKind::NotStarted,
                        detail: error.to_string(),
                    };
                    events.blocking_send(AgentEvent::Ended(Some(problem))).ok();
                }
            }
        });
    if let Err(error) = started {
        let problem = Problem {
            kind: ProblemKind::NotStarted,
            detail: error.to_string(),
        };
        ended.try_send(AgentEvent::Ended(Some(problem))).ok();
    }
}

/// The whole life of one Codex: start, serve, and the one last event.
async fn run(
    config: impl Future<Output = Result<CodexConfig, Problem>>,
    commands: mpsc::Receiver<Command>,
    shutdown: watch::Receiver<()>,
    events: mpsc::Sender<AgentEvent>,
) {
    let problem = match config.await {
        Ok(config) => start_and_serve(config, commands, shutdown, events.clone()).await,
        Err(problem) => Some(problem),
    };
    events.send(AgentEvent::Ended(problem)).await.ok();
}

async fn start_and_serve(
    config: CodexConfig,
    commands: mpsc::Receiver<Command>,
    shutdown: watch::Receiver<()>,
    events: mpsc::Sender<AgentEvent>,
) -> Option<Problem> {
    let session = &config.agent.session;
    let started = Process::start(
        &config.agent.command,
        &config.agent.env,
        &session.unset_env,
        session.env_allow.as_deref(),
    );
    let mut process = match started {
        Ok(process) => process,
        Err(error) => {
            let (kind, detail) = (ProblemKind::NotStarted, error.to_string());
            return Some(Problem { kind, detail });
        }
    };
    let grace = config.agent.deadlines.shutdown;
    let (stdin, stdout) = process.take_pipes();
    let mut serving = Box::pin(Serve::new(stdin, stdout, config, commands, shutdown, events).run());
    let end = tokio::select! {
        end = &mut serving => end,
        () = process.exited() => match tokio::time::timeout(grace, &mut serving).await {
            Ok(end) => end,
            Err(_) => LoopEnd::AgentClosed,
        },
    };
    // Dropping the connection closes the server's input, which is how it is asked to end.
    drop(serving);
    match end {
        LoopEnd::Shutdown => {
            process.stop(grace).await;
            None
        }
        LoopEnd::Abandoned => {
            process.stop(Duration::ZERO).await;
            None
        }
        LoopEnd::Failed(problem) => {
            process.stop(Duration::ZERO).await;
            Some(problem)
        }
        LoopEnd::AgentClosed => Some(process.died(grace, None).await),
    }
}

type Lines = Pin<Box<dyn Stream<Item = io::Result<String>> + Send>>;

/// A turn of the thread that has not ended.
struct ActiveTurn {
    /// The server's id of the turn; known once the turn started.
    id: Option<String>,
    /// When the turn was told to stop, and so when it must have.
    cancel_limit: Option<Instant>,
}

/// An approval the person has not answered.
struct Waiting {
    /// The server's id of the request, to answer it with.
    server_id: Value,
    /// How the person's yes or no is written as the answer to this kind of request.
    decision: fn(bool) -> Value,
}

/// The connection to one app-server.
struct Serve {
    writer: tokio::process::ChildStdin,
    lines: Lines,
    events: mpsc::Sender<AgentEvent>,
    commands: mpsc::Receiver<Command>,
    shutdown: watch::Receiver<()>,
    /// Commands that arrived while a request was being answered, served first once it is.
    backlog: VecDeque<Command>,
    /// The caller left while a request was waiting for its answer.
    is_abandoned: bool,
    config: CodexConfig,
    next_id: u64,
    thread_id: Option<String>,
    turn: Option<ActiveTurn>,
    /// The model the person chose, when one was: the only one ever named to the server, so a model the
    /// server does not list for the person's account is never sent on the app's own account.
    model: Option<String>,
    /// The model the server says it runs a thread on when none was chosen: shown, never sent.
    reported_model: Option<String>,
    /// The skills the thread offers, each also a command.
    skills: Vec<slash::Skill>,
    /// The reasoning effort the person chose, when one was; none leaves the model's own default.
    effort: Option<String>,
    /// Whether the person chose the fast service tier of the model.
    is_fast: bool,
    /// The collaboration mode the person chose (`plan`), once one was: it is sent with every turn after.
    collaboration: Option<String>,
    /// The models the server offers, once listed.
    models: Vec<crate::model::Choice>,
    /// Items whose text was streamed, so it is not shown again when the item completes.
    streamed: std::collections::HashSet<String>,
    /// The summary parts seen of each reasoning item, to put a break between them.
    parts: HashMap<String, i64>,
    known: events::Known,
    waiting: HashMap<u64, Waiting>,
    next_request: u64,
    last_heard: Instant,
}

impl Serve {
    fn new(
        stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
        config: CodexConfig,
        commands: mpsc::Receiver<Command>,
        shutdown: watch::Receiver<()>,
        events: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let lines = transport::incoming(BufReader::new(stdout), transport::MAX_LINE_BYTES);
        Self {
            writer: stdin,
            lines: Box::pin(lines),
            events,
            commands,
            shutdown,
            backlog: VecDeque::new(),
            is_abandoned: false,
            model: config.model.clone(),
            reported_model: None,
            skills: Vec::new(),
            effort: config.effort.clone(),
            is_fast: false,
            collaboration: config.collaboration.clone(),
            config,
            next_id: 1,
            thread_id: None,
            turn: None,
            models: Vec::new(),
            streamed: Default::default(),
            parts: HashMap::new(),
            known: events::Known::default(),
            waiting: HashMap::new(),
            next_request: 1,
            last_heard: Instant::now(),
        }
    }

    /// `initialize`, then the commands until the caller stops, the server closes or the connection fails.
    async fn run(mut self) -> LoopEnd {
        if let Err(problem) = self.initialize().await {
            if self.is_abandoned {
                return LoopEnd::Abandoned;
            }
            return match problem.kind {
                ProblemKind::Died => LoopEnd::AgentClosed,
                _ => LoopEnd::Failed(problem),
            };
        }
        let mut check = tokio::time::interval(CHECK_EVERY);
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let end = tokio::select! {
                line = next_line(&mut self.lines) => self.read(line).await,
                command = next_command(&mut self.backlog, &mut self.commands) => self.command(command).await,
                _ = self.shutdown.changed() => Some(self.shut_down().await),
                _ = check.tick(), if self.turn.is_some() => self.check_turn().await,
            };
            if self.is_abandoned {
                return LoopEnd::Abandoned;
            }
            if let Some(end) = end {
                return end;
            }
        }
    }

    /// A command that came while a request waits for its answer. The caller leaving ends the wait, as
    /// there is nobody left to answer to; anything else is kept for after the answer. Whether the wait
    /// ends.
    fn keep_or_abandon(&mut self, command: Option<Command>) -> bool {
        match command {
            None | Some(Command::Shutdown) => self.is_abandoned = true,
            Some(command) => self.backlog.push_back(command),
        }
        self.is_abandoned
    }

    /// One line of the server's output.
    async fn read(&mut self, line: Option<io::Result<String>>) -> Option<LoopEnd> {
        match line {
            None => Some(LoopEnd::AgentClosed),
            Some(Err(error)) => Some(LoopEnd::Failed(request_failed(
                "the server's output",
                &error.to_string(),
            ))),
            Some(Ok(line)) => {
                if let Some(message) = wire::parse(&line) {
                    self.handle(message).await;
                }
                None
            }
        }
    }

    async fn command(&mut self, command: Option<Command>) -> Option<LoopEnd> {
        match command {
            None | Some(Command::Shutdown) => Some(self.shut_down().await),
            Some(Command::Open {
                stored,
                allows_replacement,
            }) => self.open(stored, allows_replacement).await,
            Some(Command::Prompt { text, attachments }) => {
                self.prompt(text, attachments).await;
                None
            }
            Some(Command::SetOption { config_id, value }) => {
                self.set_option(config_id, value).await;
                None
            }
            Some(Command::Cancel) => {
                self.interrupt().await;
                None
            }
            Some(Command::Answer { request, option_id }) => {
                self.answer(request, option_id).await;
                None
            }
            Some(Command::SetMode(mode)) => {
                self.set_collaboration(mode).await;
                None
            }
        }
    }

    /// `initialize` and `initialized`, then what the window learns of the server.
    async fn initialize(&mut self) -> Result<(), Problem> {
        let params = json!({
            "clientInfo": { "name": CLIENT_NAME, "title": null, "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true, "requestAttestation": false },
        });
        let limit = self.config.agent.deadlines.request;
        let answer = self.call("initialize", params, limit).await?;
        self.send(wire::notification("initialized")).await?;
        let agent = answer
            .get("userAgent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let version = agent
            .split_whitespace()
            .next()
            .and_then(|name| name.split('/').nth(1));
        let version = version.unwrap_or_default().to_string();
        self.emit(AgentEvent::Connected {
            name: "codex".to_string(),
            version,
        })
        .await;
        let support = crate::model::PromptSupport {
            has_images: true,
            has_embedded_context: true,
        };
        self.emit(AgentEvent::Supports(support)).await;
        Ok(())
    }

    /// Sends one request and reads until its answer. What else the server says meanwhile is dealt with as
    /// usual; the commands of the window wait until the answer is here, except that the window leaving
    /// ends the wait at once (the error is a death, and [`Serve::run`] says why).
    async fn call(
        &mut self,
        method: &'static str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, Problem> {
        let id = self.next_id;
        self.next_id += 1;
        let line = wire::request(id, method, params);
        self.send(line).await?;
        let deadline = Instant::now() + limit;
        loop {
            let heard = tokio::select! {
                _ = self.shutdown.changed() => Err(None),
                line = tokio::time::timeout_at(deadline, next_line(&mut self.lines)) => Ok(line),
                command = self.commands.recv(), if self.backlog.len() < COMMAND_BUFFER => Err(command),
            };
            let line = match heard {
                Ok(line) => line,
                Err(command) => match self.keep_or_abandon(command) {
                    true => return Err(died()),
                    false => continue,
                },
            };
            let line = line.map_err(|_| timed_out(method))?;
            let line = line.ok_or_else(died)?;
            let line = line.map_err(|error| request_failed(method, &error.to_string()))?;
            let Some(message) = wire::parse(&line) else {
                continue;
            };
            match reply_to(id, method, message) {
                Reply::Mine(answer) => return answer,
                Reply::Other(message) => self.handle(message).await,
            }
        }
    }

    /// Writes one line to the server.
    async fn send(&mut self, mut line: String) -> Result<(), Problem> {
        line.push('\n');
        if self.writer.write_all(line.as_bytes()).await.is_err() {
            return Err(died());
        }
        self.writer.flush().await.map_err(|_| died())
    }

    /// Sends a request whose answer nobody waits for.
    async fn notify_server(&mut self, method: &'static str, params: Value) {
        let id = self.next_id;
        self.next_id += 1;
        let line = wire::request(id, method, params);
        if self.send(line).await.is_err() {
            eprintln!("sakura: {method} was not sent: the server is gone");
        }
    }

    /// Sends an event to the window, waiting while its buffer is full; a window that is gone is not an
    /// error here.
    pub(super) async fn emit(&mut self, event: AgentEvent) {
        self.events.send(event).await.ok();
    }

    pub(super) async fn emit_thread(&mut self, event: ThreadEvent) {
        self.emit(AgentEvent::Thread(event)).await;
    }

    async fn fail(&mut self, problem: Problem) {
        self.emit_thread(ThreadEvent::Failed(problem)).await;
    }

    /// The caller is done: a running turn is told to stop and the loop ends. The process is stopped by the
    /// caller of the loop, once the writer is dropped.
    async fn shut_down(&mut self) -> LoopEnd {
        if self.turn.is_some() {
            self.interrupt().await;
        }
        LoopEnd::Shutdown
    }

    /// A running turn that has fallen silent, or that was told to stop and did not, ends the connection.
    async fn check_turn(&mut self) -> Option<LoopEnd> {
        let turn = self.turn.as_ref()?;
        let deadlines = self.config.agent.deadlines;
        let is_ignored = turn
            .cancel_limit
            .is_some_and(|limit| Instant::now() >= limit);
        if is_ignored {
            return Some(LoopEnd::Failed(timed_out("turn/interrupt")));
        }
        let is_silent = turn.cancel_limit.is_none()
            && self.waiting.is_empty()
            && self.last_heard.elapsed() >= deadlines.idle_turn;
        is_silent.then(|| LoopEnd::Failed(timed_out("turn")))
    }
}

/// The next command: one that came while a request waited, else the next that comes.
async fn next_command(
    backlog: &mut VecDeque<Command>,
    commands: &mut mpsc::Receiver<Command>,
) -> Option<Command> {
    match backlog.pop_front() {
        Some(command) => Some(command),
        None => commands.recv().await,
    }
}

/// The next line of the server's output; none when it has closed.
async fn next_line(lines: &mut Lines) -> Option<io::Result<String>> {
    lines.next().await
}

/// What a message is to the request `id` that is being waited for.
enum Reply {
    /// Its answer, or its error.
    Mine(Result<Value, Problem>),
    /// Something else, to be dealt with as usual.
    Other(Incoming),
}

fn reply_to(id: u64, method: &str, message: Incoming) -> Reply {
    match message {
        Incoming::Response {
            id: answered,
            result,
        } if answered == id => Reply::Mine(Ok(result)),
        Incoming::Failure {
            id: answered,
            code,
            message,
        } if answered == id => Reply::Mine(Err(request_problem(method, code, &message))),
        other => Reply::Other(other),
    }
}

fn request_problem(method: &str, code: Option<i64>, message: &str) -> Problem {
    const INVALID_REQUEST: i64 = -32600;
    let mut problem = request_failed(method, message);
    if method == "thread/resume"
        && code == Some(INVALID_REQUEST)
        && (message == "no rollout found" || message.starts_with("no rollout found for thread id "))
    {
        problem.kind = ProblemKind::SessionMissing;
    }
    problem
}

fn died() -> Problem {
    Problem {
        kind: ProblemKind::Died,
        detail: String::new(),
    }
}

/// The decider of `config`, when it has one.
fn decider_of(config: &CodexConfig) -> Option<&Decider> {
    config.agent.session.decider.as_ref()
}
