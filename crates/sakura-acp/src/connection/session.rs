//! The session loop: `initialize`, then the caller's commands (open a session, prompt, cancel, answer,
//! shut down) while a turn may be running, until the caller stops or the agent closes its output.

use super::process::AgentLines;
use super::shared::Shared;
use super::{AgentConfig, AgentEvent, Command, Deadlines, LoopEnd};
use crate::convert;
use crate::thread::{Problem, ProblemKind, ThreadEvent};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities, Implementation,
    InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    PromptResponse, RequestPermissionRequest, SessionNotification, TextContent,
};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::{Agent as AgentRole, Client, Error, ErrorCode};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant;

/// The name the client gives the agent.
const CLIENT_NAME: &str = "sakura-desk";
/// Pages of `session/list` read while looking for the stored session.
const MAX_LIST_PAGES: usize = 20;
/// How often a running turn is checked for a stall, at most.
const STALL_CHECK: Duration = Duration::from_secs(1);
/// A stalled turn is noticed within this share of its limit.
const STALL_CHECK_SHARE: u32 = 4;

/// The answer to a prompt, whenever it comes.
type PromptAnswer = Pin<Box<dyn Future<Output = Result<PromptResponse, Error>> + Send>>;

/// How a turn ended.
enum TurnEnd {
    Answered(Result<PromptResponse, Error>),
    /// The agent fell silent. The answer to the prompt is still owed: until it comes, the agent may go on
    /// with the old turn.
    Stalled(PromptAnswer),
    /// The agent was told to stop and did not end the turn in time.
    CancelIgnored,
}

/// A running turn: the wait for its end, and the way to tell that wait a cancel was sent.
struct Turn {
    end: Pin<Box<dyn Future<Output = TurnEnd> + Send>>,
    cancelled: Arc<Notify>,
}

/// A turn that timed out and was told to stop, until the agent confirms it by answering its prompt. While
/// it settles, what the agent sends belongs to that old turn and is dropped, and the next prompt waits.
struct Settling {
    answer: PromptAnswer,
    limit: Instant,
}

/// What the agent said it can do.
#[derive(Default)]
struct Capabilities {
    can_load: bool,
    can_list: bool,
}

/// Connects to the agent over `lines` and runs the session loop to its end. Once the loop has ended, the
/// SDK goes on until everything it accepted is written to the agent; an agent that stopped reading would
/// hold it there for ever, so that last stretch has the shutdown deadline, after which the connection is
/// dropped as it is (which closes the agent's input) and the caller stops the process.
pub(super) async fn drive(
    lines: AgentLines,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Command>,
    config: AgentConfig,
) -> Result<LoopEnd, Error> {
    let teardown = config.deadlines.shutdown;
    let (ended, mut loop_end) = oneshot::channel();
    let connected = connect(lines, shared, commands, config, ended);
    tokio::pin!(connected);
    let end = tokio::select! {
        result = &mut connected => {
            let end = loop_end.try_recv().unwrap_or(LoopEnd::AgentClosed);
            return result.map(|()| end);
        }
        end = &mut loop_end => end.unwrap_or(LoopEnd::AgentClosed),
    };
    match tokio::time::timeout(teardown, connected).await {
        Ok(result) => result.map(|()| end),
        Err(_) => Ok(end),
    }
}

/// The connection with its two handlers; `ended` is told how the session loop ended as soon as it has.
async fn connect(
    lines: AgentLines,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Command>,
    config: AgentConfig,
    ended: oneshot::Sender<LoopEnd>,
) -> Result<(), Error> {
    let (updates, permissions) = (shared.clone(), shared.clone());
    Client
        .v1()
        .name(CLIENT_NAME)
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                updates.on_update(notification).await;
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                permissions.on_permission(request, responder).await;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(lines, async |connection: ConnectionTo<AgentRole>| {
            let session = Session {
                connection,
                shared,
                commands,
                config,
                capabilities: Capabilities::default(),
                turn: None,
                settling: None,
                waiting_prompt: None,
            };
            // The receiver is gone only when nobody waits for the end any more.
            ended.send(session.run().await).ok();
            Ok(())
        })
        .await
}

struct Session {
    connection: ConnectionTo<AgentRole>,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<Command>,
    config: AgentConfig,
    capabilities: Capabilities,
    turn: Option<Turn>,
    settling: Option<Settling>,
    /// The prompt sent while a timed-out turn settles: it goes out when the agent has confirmed.
    waiting_prompt: Option<String>,
}

impl Session {
    async fn run(mut self) -> LoopEnd {
        match self.initialize().await {
            Ok(()) => {}
            Err(problem) if problem.kind == ProblemKind::Died => return LoopEnd::AgentClosed,
            Err(problem) => return LoopEnd::Failed(problem),
        }
        loop {
            tokio::select! {
                () = self.connection.incoming_closed() => return LoopEnd::AgentClosed,
                end = turn_end(&mut self.turn) => {
                    if let Some(end) = self.turn_ended(end).await {
                        return end;
                    }
                }
                is_confirmed = settled(&mut self.settling) => {
                    if let Some(end) = self.settle(is_confirmed).await {
                        return end;
                    }
                }
                command = self.commands.recv() => match command {
                    None | Some(Command::Shutdown) => return self.shut_down().await,
                    Some(command) => self.obey(command).await,
                },
            }
        }
    }

    /// `initialize`: the client reads and writes no files and owns no terminal.
    async fn initialize(&mut self) -> Result<(), Problem> {
        let capabilities = ClientCapabilities::new()
            .fs(FileSystemCapabilities::new()
                .read_text_file(false)
                .write_text_file(false))
            .terminal(false);
        let request = InitializeRequest::new(ProtocolVersion::V1)
            .client_capabilities(capabilities)
            .client_info(Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION")));
        let sent = self.connection.send_request(request).block_task();
        let response = within(self.deadlines().request, "initialize", sent).await?;
        let agent = response.agent_capabilities;
        self.capabilities = Capabilities {
            can_load: agent.load_session,
            can_list: agent.session_capabilities.list.is_some(),
        };
        let info = response.agent_info;
        let (name, version) = info
            .map(|info| (info.name, info.version))
            .unwrap_or_default();
        self.shared
            .emit(AgentEvent::Connected { name, version })
            .await;
        Ok(())
    }

    async fn obey(&mut self, command: Command) {
        match command {
            Command::Open { stored } => self.open(stored).await,
            Command::Prompt(text) => self.prompt(text).await,
            Command::Cancel => self.cancel().await,
            Command::Answer { request, option_id } => self.shared.answer(request, &option_id).await,
            Command::Shutdown => {}
        }
    }

    /// Opens the stored session when the agent still has it, a new one otherwise.
    async fn open(&mut self, stored: Option<String>) {
        let outcome = match stored {
            Some(stored) if self.capabilities.can_load => self.load_or_new(stored).await,
            _ => self.new_session().await,
        };
        match outcome {
            Ok(event) => self.shared.emit(event).await,
            Err(problem) if problem.kind == ProblemKind::AuthRequired => {
                self.shared.emit(AgentEvent::AuthRequired).await;
            }
            // The agent is gone: the loop sees its output close and says so once.
            Err(problem) if problem.kind == ProblemKind::Died => {}
            Err(problem) => self.fail(problem).await,
        }
    }

    async fn load_or_new(&mut self, stored: String) -> Result<AgentEvent, Problem> {
        if !self.is_listed(&stored).await? {
            return self.new_session().await;
        }
        self.shared.set_session(Some(stored.clone()));
        self.shared.emit_thread(ThreadEvent::ReplayStarted).await;
        let request = LoadSessionRequest::new(stored.clone(), self.config.workspace.clone());
        let sent = self.connection.send_request(request).block_task();
        let loaded = within(self.deadlines().load, "session/load", sent).await;
        match loaded {
            Ok(_) => {
                self.shared.emit_thread(ThreadEvent::ReplayFinished).await;
                Ok(AgentEvent::SessionOpened {
                    session_id: stored,
                    is_loaded: true,
                })
            }
            Err(problem) => {
                // What was replayed before the failure is dropped: an empty replay clears it.
                self.shared.set_session(None);
                self.shared.emit_thread(ThreadEvent::ReplayStarted).await;
                self.shared.emit_thread(ThreadEvent::ReplayFinished).await;
                match problem.kind {
                    ProblemKind::RequestFailed => self.new_session().await,
                    _ => Err(problem),
                }
            }
        }
    }

    /// Whether the agent lists `session` for this workspace. An agent that cannot list is asked to load
    /// directly; a list that fails for a reason other than sign-in is taken as "try to load".
    async fn is_listed(&mut self, session: &str) -> Result<bool, Problem> {
        if !self.capabilities.can_list {
            return Ok(true);
        }
        let mut cursor = None;
        for _ in 0..MAX_LIST_PAGES {
            let request = ListSessionsRequest::new()
                .cwd(self.config.workspace.clone())
                .cursor(cursor.take());
            let sent = self.connection.send_request(request).block_task();
            let page = match within(self.deadlines().request, "session/list", sent).await {
                Ok(page) => page,
                Err(problem) if problem.kind == ProblemKind::RequestFailed => return Ok(true),
                Err(problem) => return Err(problem),
            };
            if page
                .sessions
                .iter()
                .any(|listed| &*listed.session_id.0 == session)
            {
                return Ok(true);
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(false)
    }

    async fn new_session(&mut self) -> Result<AgentEvent, Problem> {
        let request = NewSessionRequest::new(self.config.workspace.clone());
        let sent = self.connection.send_request(request).block_task();
        let response = within(self.deadlines().request, "session/new", sent).await?;
        let session_id = response.session_id.0.to_string();
        self.shared.set_session(Some(session_id.clone()));
        Ok(AgentEvent::SessionOpened {
            session_id,
            is_loaded: false,
        })
    }

    /// Starts a turn. The loop goes on reading commands while it runs.
    async fn prompt(&mut self, text: String) {
        let Some(session) = self.shared.session() else {
            return self
                .fail(request_failed("session/prompt", "no session is open"))
                .await;
        };
        if self.turn.is_some() {
            return eprintln!("sakura: a prompt arrived while a turn was running and was not sent");
        }
        if self.settling.is_some() {
            // The agent has not confirmed that the timed-out turn stopped: the prompt waits for that.
            self.waiting_prompt = Some(text);
            return;
        }
        let block = ContentBlock::Text(TextContent::new(text));
        let sent = self
            .connection
            .send_request(PromptRequest::new(session, vec![block]));
        self.shared.touch();
        self.shared.open_turn();
        let cancelled = Arc::new(Notify::new());
        let end = wait_for_turn(
            Box::pin(sent.block_task()),
            self.shared.clone(),
            cancelled.clone(),
            self.deadlines(),
        );
        self.turn = Some(Turn {
            end: Box::pin(end),
            cancelled,
        });
    }

    /// `session/cancel`: the agent is told, every open question is answered "cancelled", and the turn's
    /// wait starts counting the time the agent has to confirm.
    async fn cancel(&mut self) {
        let Some(turn) = self.turn.as_ref() else {
            return;
        };
        turn.cancelled.notify_one();
        self.send_cancel();
        self.shared.close_turn().await;
    }

    fn send_cancel(&self) {
        let Some(session) = self.shared.session() else {
            return;
        };
        if let Err(error) = self
            .connection
            .send_notification(CancelNotification::new(session))
        {
            eprintln!("sakura: cannot send session/cancel: {}", error.message);
        }
    }

    /// The turn is over; `Some` when the connection cannot go on after it.
    async fn turn_ended(&mut self, end: TurnEnd) -> Option<LoopEnd> {
        self.turn = None;
        self.shared.close_turn().await;
        match end {
            TurnEnd::Answered(Ok(response)) => {
                let reason = convert::stop_reason(response.stop_reason);
                self.shared
                    .emit_thread(ThreadEvent::TurnEnded(reason))
                    .await;
            }
            TurnEnd::Answered(Err(error)) => match problem_of("session/prompt", &error) {
                // The agent is gone: the loop sees its output close and says so once.
                problem if problem.kind == ProblemKind::Died => {}
                problem => self.fail(problem).await,
            },
            TurnEnd::Stalled(answer) => {
                self.send_cancel();
                self.fail(timed_out("session/prompt")).await;
                self.shared.set_settling(true);
                let limit = Instant::now() + self.deadlines().cancel;
                self.settling = Some(Settling { answer, limit });
            }
            // An agent that ignores a cancel may still be working on the old turn: nothing it sends
            // from here on can be told apart from a new turn's, so the connection ends.
            TurnEnd::CancelIgnored => return Some(LoopEnd::Failed(timed_out("session/cancel"))),
        }
        None
    }

    /// The timed-out turn was confirmed stopped (its prompt was answered), or its time to do so passed.
    /// Confirmed, the session goes on with the prompt that waited; otherwise the connection ends.
    async fn settle(&mut self, is_confirmed: bool) -> Option<LoopEnd> {
        self.settling = None;
        if !is_confirmed {
            return Some(LoopEnd::Failed(timed_out("session/cancel")));
        }
        self.shared.set_settling(false);
        if let Some(text) = self.waiting_prompt.take() {
            self.prompt(text).await;
        }
        None
    }

    /// The caller is done: a running turn is cancelled and the loop ends. The process is stopped by the
    /// caller of the loop, once the connection has let go of the agent's input.
    async fn shut_down(&mut self) -> LoopEnd {
        if self.turn.take().is_some() {
            self.send_cancel();
            self.shared.close_turn().await;
        }
        LoopEnd::Shutdown
    }

    async fn fail(&mut self, problem: Problem) {
        self.shared.emit_thread(ThreadEvent::Failed(problem)).await;
    }

    fn deadlines(&self) -> Deadlines {
        self.config.deadlines
    }
}

/// The answer to a request, or the problem when it fails or passes `limit`. Giving up on a request drops
/// it, which tells the agent to stop working on it.
async fn within<T>(
    limit: Duration,
    method: &'static str,
    sent: impl Future<Output = Result<T, Error>>,
) -> Result<T, Problem> {
    match tokio::time::timeout(limit, sent).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(problem_of(method, &error)),
        Err(_) => Err(timed_out(method)),
    }
}

/// The end of the running turn; never, when none runs.
async fn turn_end(turn: &mut Option<Turn>) -> TurnEnd {
    match turn.as_mut() {
        Some(turn) => turn.end.as_mut().await,
        None => std::future::pending().await,
    }
}

/// Whether the turn that settles was confirmed stopped (its prompt answered, whatever the answer) before
/// its limit; never resolves when none settles.
async fn settled(settling: &mut Option<Settling>) -> bool {
    let Some(settling) = settling.as_mut() else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = settling.answer.as_mut() => true,
        () = tokio::time::sleep_until(settling.limit) => false,
    }
}

/// Waits for the prompt's answer, with two deadlines: the agent must not fall silent while nothing waits
/// for the person, and after a cancel it must end the turn within the cancel deadline.
async fn wait_for_turn(
    mut answer: PromptAnswer,
    shared: Arc<Shared>,
    cancelled: Arc<Notify>,
    deadlines: Deadlines,
) -> TurnEnd {
    let check = (deadlines.idle_turn / STALL_CHECK_SHARE).min(STALL_CHECK);
    let mut cancel_limit: Option<Instant> = None;
    loop {
        let confirm = async {
            match cancel_limit {
                Some(limit) => tokio::time::sleep_until(limit).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = answer.as_mut() => return TurnEnd::Answered(result),
            () = cancelled.notified(), if cancel_limit.is_none() => {
                cancel_limit = Some(Instant::now() + deadlines.cancel);
            }
            () = confirm => return TurnEnd::CancelIgnored,
            () = tokio::time::sleep(check) => {
                if cancel_limit.is_none() && shared.is_stalled(deadlines.idle_turn) {
                    return TurnEnd::Stalled(answer);
                }
            }
        }
    }
}

/// Whether a request failed because the connection is gone, which the loop reports by itself.
fn is_closed(error: &Error) -> bool {
    agent_client_protocol::is_incoming_transport_closed(error)
        || agent_client_protocol::is_connection_terminated(error)
}

/// The problem behind a failed request: the agent is gone, it wants a sign-in, or it answered with an
/// error. The agent's message is untrusted text, shown as it is.
fn problem_of(method: &str, error: &Error) -> Problem {
    let kind = if is_closed(error) {
        ProblemKind::Died
    } else if error.code == ErrorCode::AUTH_REQUIRED {
        ProblemKind::AuthRequired
    } else {
        return request_failed(method, &error.message);
    };
    Problem {
        kind,
        detail: String::new(),
    }
}

fn request_failed(method: &str, message: &str) -> Problem {
    Problem {
        kind: ProblemKind::RequestFailed,
        detail: format!("{method}: {message}"),
    }
}

fn timed_out(method: &str) -> Problem {
    Problem {
        kind: ProblemKind::TimedOut,
        detail: method.to_string(),
    }
}
