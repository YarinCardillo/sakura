//! The ChatGPT login of Codex, over the app-server's own methods: `initialize`, `account/login/start` (which
//! answers with the address to open in a browser and an id), the notification `account/login/completed`
//! (which says whether it worked) and `account/login/cancel`. The app opens the address and waits; no
//! terminal is involved, and no credential ever passes through here: the server writes its own login into the
//! `CODEX_HOME` it was started with.
//!
//! The login runs on a thread of its own, as the master's Codex connection does, and reports through a
//! channel.

use super::super::process::Process;
use super::wire::{self, Incoming};
use crate::transport;
use futures::{Stream, StreamExt as _};
use sakura_core::process::sanitize_line;
use serde_json::{json, Value};
use std::io;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncWriteExt as _, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How many events wait for the window: the address, then how it ended.
const EVENT_BUFFER: usize = 4;
/// How long the server gets to answer a request of the handshake or of the login's start.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);
/// How long the server gets to end by itself once the login is over, before it is stopped.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// The longest reason of the server that is passed on.
const MAX_REASON_CHARS: usize = 200;
/// What an address to open begins with.
const SECURE_ADDRESS: &str = "https://";
/// The ChatGPT sign-in, as `account/login/start` names it.
const CHATGPT: &str = "chatgpt";

/// How to start the login.
#[derive(Debug, Clone)]
pub struct CodexLoginConfig {
    /// The program and its arguments (`codex app-server`).
    pub command: Vec<String>,
    /// What is added to the program's environment: the `CODEX_HOME` the login is written to.
    pub env: Vec<(String, String)>,
    /// The variables of the app's own environment the program may keep, all of them when none; and the ones
    /// that are removed from it.
    pub env_allow: Option<Vec<String>>,
    pub unset_env: Vec<String>,
    /// How long the person has to finish in the browser.
    pub patience: Duration,
}

/// What a running login tells the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// The address to open in the browser; the person signs in there.
    Browser(String),
    /// The login is over, and this is how. Nothing follows.
    Ended(LoginEnd),
}

/// How a login ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEnd {
    /// The login is in the Codex home.
    Done,
    /// The person cancelled it.
    Cancelled,
    /// It did not work.
    Failed(LoginFailure),
}

/// Why a login did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginFailure {
    /// The program did not start.
    NotStarted(String),
    /// The server answered a request with an error; its words.
    Refused(String),
    /// The server said the login did not succeed; its reason, when it gave one.
    Declined(Option<String>),
    /// The server ended, or spoke in a way that cannot be followed, before the login was over.
    Broke,
    /// Nobody finished it in the browser in time.
    TimedOut,
}

/// A running login, which can be cancelled.
#[derive(Debug, Clone)]
pub struct CodexLogin {
    cancel: mpsc::Sender<()>,
}

impl CodexLogin {
    /// Asks the login to stop: the server is told, and the login ends [`LoginEnd::Cancelled`].
    pub fn cancel(&self) {
        // A login that has ended already has nothing left to cancel.
        self.cancel.try_send(()).ok();
    }
}

/// Starts the login on a thread of its own and returns its handle and its events.
pub fn start_codex_login(config: CodexLoginConfig) -> (CodexLogin, mpsc::Receiver<LoginEvent>) {
    let (cancel, cancelled) = mpsc::channel(1);
    let (events, read) = mpsc::channel(EVENT_BUFFER);
    let ended = events.clone();
    let started = std::thread::Builder::new()
        .name("codex-login".to_string())
        .spawn(move || run_on_a_runtime(config, cancelled, events));
    if let Err(error) = started {
        let end = LoginEnd::Failed(LoginFailure::NotStarted(error.to_string()));
        ended.try_send(LoginEvent::Ended(end)).ok();
    }
    (CodexLogin { cancel }, read)
}

fn run_on_a_runtime(
    config: CodexLoginConfig,
    cancelled: mpsc::Receiver<()>,
    events: mpsc::Sender<LoginEvent>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let end = match runtime {
        Ok(runtime) => runtime.block_on(run(config, cancelled, &events)),
        Err(error) => LoginEnd::Failed(LoginFailure::NotStarted(error.to_string())),
    };
    events.blocking_send(LoginEvent::Ended(end)).ok();
}

async fn run(
    config: CodexLoginConfig,
    cancelled: mpsc::Receiver<()>,
    events: &mpsc::Sender<LoginEvent>,
) -> LoginEnd {
    let started = Process::start(
        &config.command,
        &config.env,
        &config.unset_env,
        config.env_allow.as_deref(),
    );
    let mut process = match started {
        Ok(process) => process,
        Err(error) => return LoginEnd::Failed(LoginFailure::NotStarted(error.to_string())),
    };
    let (stdin, stdout) = process.take_pipes();
    let mut server = Server::new(stdin, stdout);
    let end = server
        .login(config.patience, cancelled, events)
        .await
        .unwrap_or_else(LoginEnd::Failed);
    // Closing its input is how the server is asked to end.
    drop(server);
    process.stop(SHUTDOWN_GRACE).await;
    end
}

/// The app-server's stdio, as the login talks to it.
struct Server {
    writer: ChildStdin,
    lines: Pin<Box<dyn Stream<Item = io::Result<String>> + Send>>,
    next_id: u64,
}

impl Server {
    fn new(writer: ChildStdin, stdout: ChildStdout) -> Self {
        let lines = transport::incoming(BufReader::new(stdout), transport::MAX_LINE_BYTES);
        Self {
            writer,
            lines: Box::pin(lines),
            next_id: 1,
        }
    }

    /// The whole login: the handshake, the start, then the wait for how it ends. An `Err` is a login that
    /// did not work; an `Ok` is how it ended otherwise.
    async fn login(
        &mut self,
        patience: Duration,
        mut cancelled: mpsc::Receiver<()>,
        events: &mpsc::Sender<LoginEvent>,
    ) -> Result<LoginEnd, LoginFailure> {
        let client = json!({ "clientInfo": { "name": "sakura-desk", "title": null, "version": env!("CARGO_PKG_VERSION") } });
        self.call("initialize", client).await?;
        self.send(wire::notification("initialized")).await?;
        let started = self
            .call("account/login/start", json!({ "type": CHATGPT }))
            .await?;
        let (url, login_id) = address_of(&started).ok_or(LoginFailure::Broke)?;
        events.send(LoginEvent::Browser(url)).await.ok();
        let deadline = Instant::now() + patience;
        tokio::select! {
            ended = self.wait_for_the_end(&login_id) => ended,
            () = tokio::time::sleep_until(deadline) => {
                self.cancel_at_the_server(&login_id).await;
                Err(LoginFailure::TimedOut)
            }
            _ = cancelled.recv() => {
                self.cancel_at_the_server(&login_id).await;
                Ok(LoginEnd::Cancelled)
            }
        }
    }

    /// Reads until the server says the login `login_id` is over.
    async fn wait_for_the_end(&mut self, login_id: &str) -> Result<LoginEnd, LoginFailure> {
        loop {
            let Some(Incoming::Notification { method, params }) = self.read().await? else {
                continue;
            };
            let is_ours = params["loginId"].as_str().is_none_or(|id| id == login_id);
            if method == "account/login/completed" && is_ours {
                return match params["success"].as_bool() {
                    Some(true) => Ok(LoginEnd::Done),
                    _ => Err(LoginFailure::Declined(reason_of(&params["error"]))),
                };
            }
        }
    }

    /// Tells the server the login is no longer wanted; whether it heard is not waited for.
    async fn cancel_at_the_server(&mut self, login_id: &str) {
        let id = self.take_id();
        let line = wire::request(id, "account/login/cancel", json!({ "loginId": login_id }));
        self.send(line).await.ok();
    }

    /// A request and its answer; what arrives meanwhile and is not the answer is dropped.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, LoginFailure> {
        let id = self.take_id();
        self.send(wire::request(id, method, params)).await?;
        let deadline = Instant::now() + ANSWER_WITHIN;
        loop {
            let heard = tokio::time::timeout_at(deadline, self.read()).await;
            match heard.map_err(|_| LoginFailure::TimedOut)?? {
                Some(Incoming::Response {
                    id: answered,
                    result,
                }) if answered == id => return Ok(result),
                Some(Incoming::Failure {
                    id: answered,
                    message,
                    ..
                }) if answered == id => {
                    return Err(LoginFailure::Refused(sanitize(&message)));
                }
                _ => {}
            }
        }
    }

    /// The next message of the server; none when it was not one.
    async fn read(&mut self) -> Result<Option<Incoming>, LoginFailure> {
        match self.lines.next().await {
            Some(Ok(line)) => Ok(wire::parse(&line)),
            Some(Err(_)) | None => Err(LoginFailure::Broke),
        }
    }

    async fn send(&mut self, mut line: String) -> Result<(), LoginFailure> {
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|_| LoginFailure::Broke)?;
        self.writer.flush().await.map_err(|_| LoginFailure::Broke)
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// The address and the id of the login `account/login/start` answered with. The address comes from a program's
/// output: only a secure web address is ever handed on to be opened.
fn address_of(answer: &Value) -> Option<(String, String)> {
    let url = answer["authUrl"]
        .as_str()
        .filter(|url| url.starts_with(SECURE_ADDRESS))?;
    let id = answer["loginId"].as_str()?;
    Some((url.to_string(), id.to_string()))
}

/// The reason the server gave for a login that did not succeed.
fn reason_of(error: &Value) -> Option<String> {
    error
        .as_str()
        .map(sanitize)
        .filter(|reason| !reason.is_empty())
}

/// A text of the server as one short line without control characters.
fn sanitize(text: &str) -> String {
    sanitize_line(text).chars().take(MAX_REASON_CHARS).collect()
}
