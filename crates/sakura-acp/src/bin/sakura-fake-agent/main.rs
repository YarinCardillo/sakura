//! A fake agent that speaks ACP v1 over stdio, for tests and for looking at the conversation view without a
//! real agent. It is written against the protocol's wire format with plain JSON, not with the SDK the client
//! uses, so the two sides do not share a bug. A response of the client is matched to the permission request
//! it answers by that request's id (`asked.rs`). It is a development tool and is never bundled with the app.
//!
//! The first word of a prompt picks the scripted session (see `scripts.rs`). Flags:
//! `--delay-ms N` (pause between streamed steps, default 12), `--auth-required` (sessions answer
//! `auth_required`), `--auth-on-prompt` (sessions open, every prompt answers `auth_required`, as the Claude
//! adapter does without a login), `--crash-at-start` (exit before reading), `--no-list`, `--no-load` (capabilities off),
//! `--empty` (no stored session to list or load), `--mute` (reads and never answers), `--noise` (a line that
//! is not JSON before every answer), `--read-lines N` (stops reading its input after N lines and stays
//! alive), `--exit-after-ms N` (exits with the crash code N ms after it started, whatever it is doing).

mod asked;
mod scripts;
mod wire;

use asked::{Answer, Asked};
use scripts::Step;
use serde_json::{json, Value};
use std::cell::Cell;
use std::io::BufRead;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wire::Wire;

/// The session `session/list` reports and `session/load` replays.
const STORED_SESSION: &str = "fake-session-1";
/// JSON-RPC and ACP error codes the fake agent answers with.
const AUTH_REQUIRED: i64 = -32000;
const METHOD_NOT_FOUND: i64 = -32601;
const RESOURCE_NOT_FOUND: i64 = -32002;
/// Exit codes of the simulated crashes.
const EXIT_CRASH_AT_START: i32 = 3;
const EXIT_CRASH_IN_TURN: i32 = 9;
/// How often a stalled or waiting turn looks for a cancel.
const POLL: Duration = Duration::from_millis(5);
/// How long a permission request sent after a cancel waits for its answer.
const LATE_ANSWER_WAIT: Duration = Duration::from_secs(5);
/// How many polls a lingering turn waits before each of its late updates and before it ends.
const LINGER_POLLS: u32 = 8;
const DEFAULT_DELAY_MS: u64 = 12;
/// How far behind its schedule a turn may be before the schedule starts again.
const MAX_LAG: Duration = Duration::from_millis(250);

/// What the flags ask for.
#[derive(Clone)]
struct Options {
    delay: Duration,
    is_auth_required: bool,
    is_auth_asked_on_prompt: bool,
    has_list: bool,
    has_load: bool,
    has_stored_session: bool,
    is_mute: bool,
}

impl Options {
    fn from_args(args: &[String]) -> Self {
        let has = |flag: &str| args.iter().any(|arg| arg == flag);
        let delay_ms = Self::number_after(args, "--delay-ms").unwrap_or(DEFAULT_DELAY_MS);
        Self {
            delay: Duration::from_millis(delay_ms),
            is_auth_required: has("--auth-required"),
            is_auth_asked_on_prompt: has("--auth-on-prompt"),
            has_list: !has("--no-list"),
            has_load: !has("--no-load"),
            has_stored_session: !has("--empty"),
            is_mute: has("--mute"),
        }
    }
}

impl Options {
    /// The number that follows the last `flag`.
    fn number_after(args: &[String], flag: &str) -> Option<u64> {
        let at = args.iter().rposition(|arg| arg == flag)?;
        args.get(at + 1)?.parse().ok()
    }
}

/// What the reader thread and a running turn share.
struct Shared {
    wire: Wire,
    options: Options,
    /// How many `session/cancel` arrived. A turn is cancelled by one that arrives after it started, so a
    /// prompt that follows a cancel closely does not hide the cancel from the turn it was for.
    cancels: AtomicU64,
    /// The permission requests that wait for the client's answer.
    asked: Asked,
    next_session: Mutex<u32>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--crash-at-start") {
        eprintln!("sakura-fake-agent: simulated crash at start");
        std::process::exit(EXIT_CRASH_AT_START);
    }
    let shared = Arc::new(Shared {
        wire: Wire::stdout(args.iter().any(|arg| arg == "--noise")),
        options: Options::from_args(&args),
        cancels: AtomicU64::new(0),
        asked: Asked::default(),
        next_session: Mutex::new(1),
    });
    if let Some(after) = Options::number_after(&args, "--exit-after-ms") {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(after));
            eprintln!("sakura-fake-agent: simulated death while idle");
            std::process::exit(EXIT_CRASH_IN_TURN);
        });
    }
    let read_lines = Options::number_after(&args, "--read-lines");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut read = 0;
    loop {
        if read_lines.is_some_and(|limit| read >= limit) {
            stop_reading();
        }
        let Some(Ok(line)) = lines.next() else { break };
        read += 1;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        receive(&shared, message);
    }
}

/// The agent stops reading its input and stays alive: what the client writes next waits in the pipe.
fn stop_reading() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Routes one incoming message: a request, a notification, or the answer to our permission request.
fn receive(shared: &Arc<Shared>, message: Value) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        (Some(method), Some(id)) => request(shared, method, id, params),
        (Some("session/cancel"), None) => {
            shared.cancels.fetch_add(1, Ordering::SeqCst);
        }
        (Some(_), None) => {}
        (None, Some(_)) => answer_permission(shared, message),
        (None, None) => {}
    }
}

/// A response from the client: the answer to the permission request with its id, if one waits.
fn answer_permission(shared: &Shared, message: Value) {
    if !shared.asked.answer(&message) {
        eprintln!("sakura-fake-agent: a response answers no request that waits");
    }
}

fn request(shared: &Arc<Shared>, method: &str, id: Value, params: Value) {
    let options = &shared.options;
    if options.is_mute {
        return;
    }
    match method {
        "initialize" => shared.wire.result(&id, initialize_result(options)),
        "authenticate" => shared.wire.result(&id, json!({})),
        "session/new" | "session/load" if options.is_auth_required => {
            shared
                .wire
                .error(&id, AUTH_REQUIRED, "Authentication required")
        }
        "session/new" => shared
            .wire
            .result(&id, json!({ "sessionId": new_session(shared) })),
        "session/list" if options.has_list => {
            shared.wire.result(&id, list_result(options, &params))
        }
        "session/load" if options.has_load => load(shared, &id, &params),
        // The Claude adapter opens a session without a login and refuses the first prompt.
        "session/prompt" if options.is_auth_asked_on_prompt => {
            shared
                .wire
                .error(&id, AUTH_REQUIRED, "Authentication required")
        }
        "session/prompt" => prompt(shared, id, params),
        _ => shared.wire.error(&id, METHOD_NOT_FOUND, "Method not found"),
    }
}

fn initialize_result(options: &Options) -> Value {
    let list = if options.has_list {
        json!({})
    } else {
        Value::Null
    };
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": options.has_load,
            "sessionCapabilities": { "list": list },
        },
        "agentInfo": { "name": "sakura-fake-agent", "version": env!("CARGO_PKG_VERSION") },
        "authMethods": [{ "id": "fake-login", "name": "Fake login" }],
    })
}

fn new_session(shared: &Shared) -> String {
    let mut next = shared.next_session.lock().expect("the session counter");
    *next += 1;
    format!("fake-session-{}", *next)
}

fn list_result(options: &Options, params: &Value) -> Value {
    let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("/work");
    let sessions = match options.has_stored_session {
        true => json!([{
            "sessionId": STORED_SESSION,
            "cwd": cwd,
            "title": "Sample session",
            "updatedAt": "2026-11-10T08:00:00Z",
        }]),
        false => json!([]),
    };
    json!({ "sessions": sessions })
}

/// Replays the stored session as updates, then answers; an unknown session is an error.
fn load(shared: &Shared, id: &Value, params: &Value) {
    let session = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    if session != STORED_SESSION || !shared.options.has_stored_session {
        return shared
            .wire
            .error(id, RESOURCE_NOT_FOUND, "Session not found");
    }
    for update in scripts::history() {
        shared.wire.update(session, update);
    }
    shared.wire.result(id, json!({}));
}

/// Runs the scripted turn on its own thread, so the reader keeps seeing cancels and permission answers.
fn prompt(shared: &Arc<Shared>, id: Value, params: Value) {
    let session = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let text = prompt_text(&params);
    let cancels_before = shared.cancels.load(Ordering::SeqCst);
    let shared = shared.clone();
    std::thread::spawn(move || {
        let turn = Turn {
            shared: &shared,
            session: &session,
            cancels_before,
            two: Cell::new(Instant::now()),
        };
        let stop = turn.play(scripts::for_prompt(&text));
        shared.wire.result(&id, json!({ "stopReason": stop }));
    });
}

fn prompt_text(params: &Value) -> String {
    let blocks = params.get("prompt").and_then(Value::as_array);
    blocks
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One running turn.
struct Turn<'a> {
    shared: &'a Shared,
    session: &'a str,
    /// The cancels that had arrived when the turn started.
    cancels_before: u64,
    /// When the next step is two. Pauses are kept on this schedule, not added one after another, so a
    /// sleep that overruns is made up by the next ones and a stream lasts as long as its pauses say.
    two: Cell<Instant>,
}

impl Turn<'_> {
    /// Plays the steps in order and returns the stop reason; a cancel ends the turn at the next step.
    fn play(&self, steps: Vec<Step>) -> &'static str {
        for step in steps {
            if self.is_cancelled() {
                return "cancelled";
            }
            match step {
                Step::Update(update) => self.shared.wire.update(self.session, update),
                Step::Pause(times) => self.pause(times),
                Step::Permission {
                    request,
                    allowed,
                    refused,
                } => {
                    let Some(is_allowed) = self.ask(request) else {
                        return "cancelled";
                    };
                    let outcome = if is_allowed { allowed } else { refused };
                    self.shared.wire.update(self.session, outcome);
                }
                Step::LatePermission {
                    request,
                    allowed,
                    refused,
                } => {
                    while !self.is_cancelled() {
                        std::thread::sleep(POLL);
                    }
                    let is_allowed = self.ask_after_cancel(request);
                    let outcome = if is_allowed { allowed } else { refused };
                    self.shared.wire.update(self.session, outcome);
                    return "cancelled";
                }
                Step::Linger(updates) => {
                    while !self.is_cancelled() {
                        std::thread::sleep(POLL);
                    }
                    for update in updates {
                        std::thread::sleep(POLL * LINGER_POLLS);
                        self.shared.wire.update(self.session, update);
                    }
                    std::thread::sleep(POLL * LINGER_POLLS);
                    return "cancelled";
                }
                Step::Crash => crash(),
                Step::Refuse => return "refusal",
                Step::Stall => return self.stall(),
                Step::Deaf => loop {
                    std::thread::sleep(POLL);
                },
            }
        }
        "end_turn"
    }

    fn is_cancelled(&self) -> bool {
        self.shared.cancels.load(Ordering::SeqCst) > self.cancels_before
    }

    fn pause(&self, times: u32) {
        let now = Instant::now();
        let two = self.two.get() + self.shared.options.delay * times;
        // A turn that fell far behind (it waited for the person) starts its schedule again from now.
        let two = if now.saturating_duration_since(two) > MAX_LAG {
            now
        } else {
            two
        };
        self.two.set(two);
        std::thread::sleep(two.saturating_duration_since(now));
    }

    /// Sends a permission request and returns its id and where its answer will arrive.
    fn send_request(&self, mut request: Value) -> (u64, Receiver<Answer>) {
        request["sessionId"] = json!(self.session);
        let id = self.shared.wire.next_id();
        let answered = self.shared.asked.ask(id, &request["options"]);
        self.shared
            .wire
            .request(id, "session/request_permission", request);
        (id, answered)
    }

    /// Asks the client for permission and waits for its answer; `None` when the turn was cancelled.
    fn ask(&self, request: Value) -> Option<bool> {
        let (id, answered) = self.send_request(request);
        loop {
            match answered.recv_timeout(POLL) {
                Ok(Answer::Allowed) => return Some(true),
                Ok(Answer::Refused) => return Some(false),
                Ok(Answer::Cancelled) => return None,
                Err(_) if self.is_cancelled() => {
                    self.shared.asked.forget(id);
                    return None;
                }
                Err(_) => {}
            }
        }
    }

    /// Asks for permission although the turn was cancelled, and says whether the client allowed it. No
    /// answer within [`LATE_ANSWER_WAIT`] counts as a refusal.
    fn ask_after_cancel(&self, request: Value) -> bool {
        let (id, answered) = self.send_request(request);
        let answer = answered.recv_timeout(LATE_ANSWER_WAIT);
        self.shared.asked.forget(id);
        answer == Ok(Answer::Allowed)
    }

    /// Never answers by itself; only a cancel ends the turn.
    fn stall(&self) -> &'static str {
        while !self.is_cancelled() {
            std::thread::sleep(POLL);
        }
        "cancelled"
    }
}

fn crash() -> ! {
    eprintln!("sakura-fake-agent: simulated crash in the middle of a turn");
    std::process::exit(EXIT_CRASH_IN_TURN);
}
