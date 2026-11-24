use super::{AgentEvent, Command};
use crate::model::{Selector, SessionChoices, StopReason, Update};
use crate::thread::{Problem, ProblemKind, ThreadEvent};
use sakura_core::engine::master::{MasterAcp, MasterBinding};
use sakura_core::process::capture_interruptibly;
use serde::Deserialize;
use std::future::Future;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const OPERATOR_DEADLINE: Duration = Duration::from_secs(220);

pub(super) async fn run(
    launch: impl Future<Output = Result<MasterAcp, Problem>>,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AgentEvent>,
) {
    let result = match launch.await {
        Ok(launch) => serve(&launch, &mut commands, &events).await,
        Err(problem) => Err(problem),
    };
    events.send(AgentEvent::Ended(result.err())).await.ok();
}

async fn serve(
    launch: &MasterAcp,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<AgentEvent>,
) -> Result<(), Problem> {
    if events
        .send(AgentEvent::Connected {
            name: launch.harness.clone(),
            version: launch.protocol_version.to_string(),
        })
        .await
        .is_err()
    {
        return Ok(());
    }
    let mut session = None;
    let mut is_open = false;
    while let Some(command) = commands.recv().await {
        match command {
            Command::Open { stored, .. } => {
                if let Some(opened) = open(launch, commands, events, stored).await? {
                    session = opened;
                    is_open = true;
                } else {
                    break;
                }
            }
            Command::Prompt { text, attachments } if is_open && attachments.is_empty() => {
                if !prompt(launch, commands, events, &mut session, text).await {
                    break;
                }
            }
            Command::Shutdown => break,
            _ => {}
        }
    }
    Ok(())
}

async fn open(
    launch: &MasterAcp,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<AgentEvent>,
    stored: Option<String>,
) -> Result<Option<Option<String>>, Problem> {
    let expected = launch
        .conversation
        .as_ref()
        .map(|row| row.session_id.as_str());
    if stored.as_deref().is_some_and(|id| Some(id) != expected) {
        return Err(unavailable());
    }
    let response = match exchange(launch, commands, None).await {
        Exchange::Finished(result) => result?,
        Exchange::Cancelled | Exchange::Closed => return Ok(None),
    };
    if response.outcome == "empty" && expected.is_none() && response.launch_id == launch.launch_id {
        events.send(AgentEvent::SessionReady).await.ok();
        return Ok(Some(None));
    }
    if response.outcome != "replayed" || !response.belongs_to(launch, expected) {
        return Err(unavailable());
    }
    replay(events, response.messages).await;
    let session = response.session_id.ok_or_else(unavailable)?;
    opened(launch, events, &session, true).await;
    Ok(Some(Some(session)))
}

async fn prompt(
    launch: &MasterAcp,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<AgentEvent>,
    session: &mut Option<String>,
    text: String,
) -> bool {
    match exchange(launch, commands, Some(text)).await {
        Exchange::Finished(result) => complete(launch, events, session, result).await,
        Exchange::Cancelled => {
            send_thread(events, ThreadEvent::TurnEnded(StopReason::Cancelled)).await
        }
        Exchange::Closed => return false,
    }
    true
}

enum Exchange {
    Finished(Result<Box<Response>, Problem>),
    Cancelled,
    Closed,
}

async fn exchange(
    launch: &MasterAcp,
    commands: &mut mpsc::Receiver<Command>,
    prompt: Option<String>,
) -> Exchange {
    let (interrupt, requested) = oneshot::channel();
    let mut interrupt = Some(interrupt);
    let request = invoke(launch, prompt, async {
        if requested.await.is_err() {
            std::future::pending::<()>().await;
        }
    });
    tokio::pin!(request);
    loop {
        tokio::select! {
            result = &mut request => return if interrupt.is_some() { Exchange::Finished(result.map(Box::new)) } else { Exchange::Cancelled },
            command = commands.recv() => match command {
                Some(Command::Cancel) => {
                    if let Some(interrupt) = interrupt.take() {
                        interrupt.send(()).ok();
                    }
                }
                Some(Command::Shutdown) | None => {
                    if let Some(interrupt) = interrupt.take() {
                        interrupt.send(()).ok();
                    }
                    // The engine unwinds the adapter's separate process group on interruption.
                    request.await.ok();
                    return Exchange::Closed;
                }
                _ => {}
            },
        }
    }
}

async fn complete(
    launch: &MasterAcp,
    events: &mpsc::Sender<AgentEvent>,
    session: &mut Option<String>,
    result: Result<Box<Response>, Problem>,
) {
    match result {
        Ok(response)
            if response.outcome == "completed"
                && response.belongs_to(launch, session.as_deref()) =>
        {
            bind_completed(launch, events, session, &response).await;
            if let Some(reply) = response.reply {
                send_thread(events, ThreadEvent::Update(Update::AgentText(reply))).await;
            }
            send_thread(events, ThreadEvent::TurnEnded(StopReason::EndTurn)).await;
        }
        _ => {
            send_thread(
                events,
                ThreadEvent::Failed(problem(ProblemKind::RequestFailed)),
            )
            .await
        }
    }
}

async fn bind_completed(
    launch: &MasterAcp,
    events: &mpsc::Sender<AgentEvent>,
    session: &mut Option<String>,
    response: &Response,
) {
    if session.is_none() && response.outcome == "completed" && response.belongs_to(launch, None) {
        *session = response.session_id.clone();
        if let Some(id) = session {
            opened(launch, events, id, false).await;
        }
    }
}

async fn replay(events: &mpsc::Sender<AgentEvent>, messages: Vec<ReplayMessage>) {
    send_thread(events, ThreadEvent::ReplayStarted).await;
    for message in messages {
        let update = match message.role.as_str() {
            "user" => Update::UserText(message.text),
            "assistant" => Update::AgentText(message.text),
            _ => continue,
        };
        send_thread(events, ThreadEvent::Update(update)).await;
    }
    send_thread(events, ThreadEvent::ReplayFinished).await;
}

async fn opened(
    launch: &MasterAcp,
    events: &mpsc::Sender<AgentEvent>,
    session: &str,
    is_loaded: bool,
) {
    if let Some(choices) = &launch.start_choices {
        let selector = |current: &str| Selector {
            current: current.to_string(),
            ..Selector::default()
        };
        let choices = SessionChoices {
            models: Some(selector(&choices.model)),
            efforts: (!choices.effort.is_empty()).then(|| selector(&choices.effort)),
            ..SessionChoices::default()
        };
        events.send(AgentEvent::Choices(choices)).await.ok();
    }
    events
        .send(AgentEvent::SessionOpened {
            session_id: session.to_string(),
            is_loaded,
        })
        .await
        .ok();
}

async fn send_thread(events: &mpsc::Sender<AgentEvent>, event: ThreadEvent) {
    events.send(AgentEvent::Thread(event)).await.ok();
}

#[derive(Deserialize)]
struct Response {
    outcome: String,
    launch_id: Option<String>,
    session_id: Option<String>,
    binding: Option<MasterBinding>,
    #[serde(default)]
    messages: Vec<ReplayMessage>,
    reply: Option<String>,
}

impl Response {
    fn belongs_to(&self, launch: &MasterAcp, session: Option<&str>) -> bool {
        self.launch_id == launch.launch_id
            && self.binding == launch.binding
            && self.session_id.as_deref().is_some_and(|id| {
                !id.is_empty() && id.len() <= 512 && session.is_none_or(|kept| id == kept)
            })
    }
}

#[derive(Deserialize)]
struct ReplayMessage {
    role: String,
    text: String,
}

async fn invoke(
    launch: &MasterAcp,
    prompt: Option<String>,
    interrupt: impl Future<Output = ()> + Send,
) -> Result<Response, Problem> {
    let (Some((program, args)), Some(cwd)) = (launch.argv.split_first(), &launch.cwd) else {
        return Err(problem(ProblemKind::NotStarted));
    };
    let mut command = tokio::process::Command::new(program);
    command.args(args).current_dir(cwd).env_clear();
    command.envs(
        launch
            .env_allow
            .iter()
            .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
    );
    command.envs(&launch.env);
    let request = prompt.map_or_else(
        || serde_json::json!({}),
        |prompt| serde_json::json!({ "prompt": prompt }),
    );
    let captured = capture_interruptibly(
        &mut command,
        request.to_string().as_bytes(),
        OPERATOR_DEADLINE,
        RESPONSE_LIMIT,
        |_| {},
        interrupt,
    )
    .await
    .map_err(|_| problem(ProblemKind::RequestFailed))?;
    if !captured.status.success() {
        return Err(problem(ProblemKind::RequestFailed));
    }
    serde_json::from_slice(&captured.stdout).map_err(|_| problem(ProblemKind::RequestFailed))
}

fn unavailable() -> Problem {
    problem(ProblemKind::SessionMissing)
}

fn problem(kind: ProblemKind) -> Problem {
    Problem {
        kind,
        detail: String::new(),
    }
}
