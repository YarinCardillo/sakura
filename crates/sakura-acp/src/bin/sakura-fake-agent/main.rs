//! A fake agent that speaks ACP v1 over stdio, for tests and for looking at the conversation view without a
//! real agent. It is written against the protocol's wire format with plain JSON, not with the SDK the client
//! uses, so the two sides do not share a bug. A response of the client is matched to the permission request
//! it answers by that request's id (`asked.rs`). It is a development tool and is never bundled with the app.
//!
//! The first word of a prompt picks the scripted session (see `scripts.rs`). Flags:
//! `--delay-ms N` (pause between streamed steps, default 12), `--auth-required` (sessions answer
//! `auth_required`), `--auth-on-prompt` (sessions open, every prompt answers `auth_required`, as the Claude
//! adapter does without a login), `--crash-at-start` (exit before reading), `--no-list`, `--no-load` (capabilities off),
//! `--hang-load` (a `session/load` is never answered), `--empty` (no stored session to list or load), `--refused-history` (the stored session is one prompt
//! the model refused), `--mute` (reads and never answers), `--noise` (a line that
//! is not JSON before every answer), `--read-lines N` (stops reading its input after N lines and stays
//! alive), `--exit-after-ms N` (exits with the crash code N ms after it started, whatever it is doing),
//! `--login-allows-all` (the login's own settings let every tool run: a permission is asked only for a
//! tool the client's ask rules name in the session's opening, by its exact name; a pattern such as `*` is
//! not read, as no test can pin how the real agent reads one), `--fail-set-mode` (`session/set_mode` answers an
//! error), `--commands` (announces slash commands once a session is open), `--choices` (sessions offer
//! three modes and two models; `--single-choice` offers one of each), `--no-attachments` (the prompt
//! capabilities name no image and no embedded context). A prompt that starts with `/` is a slash command:
//! the agent says it ran it.
//!
//! Environment: `SAKURA_FAKE_LOG=<file>` appends one JSON line for every request and notification the agent
//! receives (`{"method": ..., "params": ...}`), after a first line `{"method": "started", "environment":
//! {...}}` that says, for each name listed in `SAKURA_FAKE_ENV_NAMES` (comma separated), whether the agent
//! has it. A test reads the file to see what the client sent and in which order.

mod asked;
mod scripts;
mod wire;

use asked::{Answer, Asked};
use scripts::Step;
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{BufRead, Write};
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
const INTERNAL_ERROR: i64 = -32603;
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
    is_history_refused: bool,
    is_mute: bool,
    is_load_hanging: bool,
    is_load_failing: bool,
    has_more_session_pages: bool,
    is_login_allowing_all: bool,
    is_set_mode_failing: bool,
    announces_commands: bool,
    offers_choices: bool,
    reported_effort: Option<String>,
    is_choice_single: bool,
    takes_attachments: bool,
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
            is_history_refused: has("--refused-history"),
            is_mute: has("--mute"),
            is_load_hanging: has("--hang-load"),
            is_load_failing: has("--fail-load"),
            has_more_session_pages: has("--more-session-pages"),
            is_login_allowing_all: has("--login-allows-all"),
            is_set_mode_failing: has("--fail-set-mode"),
            announces_commands: has("--commands"),
            offers_choices: has("--choices"),
            reported_effort: args
                .iter()
                .position(|arg| arg == "--reported-effort")
                .and_then(|at| args.get(at + 1))
                .cloned(),
            is_choice_single: has("--single-choice"),
            takes_attachments: !has("--no-attachments"),
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
    /// The client's ask rules by session: the tools they name ask, whatever the login allows.
    asking: Mutex<HashMap<String, Vec<String>>>,
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
        next_session: Mutex::new(u32::from(
            !args.iter().any(|arg| arg == "--first-is-stored"),
        )),
        asking: Mutex::new(HashMap::new()),
    });
    if let Some(after) = Options::number_after(&args, "--exit-after-ms") {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(after));
            eprintln!("sakura-fake-agent: simulated death while idle");
            std::process::exit(EXIT_CRASH_IN_TURN);
        });
    }
    log_start();
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
        log_received(&message);
        receive(&shared, message);
    }
}

/// Environment variable that names the file the agent logs what it receives to.
const LOG_VARIABLE: &str = "SAKURA_FAKE_LOG";
/// Environment variable that lists the variables whose presence the first log line reports.
const ENV_NAMES_VARIABLE: &str = "SAKURA_FAKE_ENV_NAMES";

/// Appends one line to the log file, when there is one.
fn log_line(line: &Value) {
    let Some(path) = std::env::var_os(LOG_VARIABLE) else {
        return;
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut file) = file {
        writeln!(file, "{line}").ok();
    }
}

/// The first log line: which of the listed environment variables the agent was started with.
fn log_start() {
    let listed = std::env::var(ENV_NAMES_VARIABLE).unwrap_or_default();
    let environment: serde_json::Map<String, Value> = listed
        .split(',')
        .filter(|name| !name.is_empty())
        .map(|name| (name.to_string(), json!(std::env::var_os(name).is_some())))
        .collect();
    log_line(&json!({ "method": "started", "environment": environment }));
}

/// Logs a request or notification of the client; its answers are not logged.
fn log_received(message: &Value) {
    if let Some(method) = message.get("method") {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        log_line(&json!({ "method": method, "params": params }));
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
        "session/set_mode" if options.is_set_mode_failing => {
            shared.wire.error(&id, INTERNAL_ERROR, "Invalid Mode")
        }
        "session/set_mode" | "session/close" => shared.wire.result(&id, json!({})),
        "session/set_config_option" => shared.wire.result(&id, set_option_result(options, &params)),
        "session/new" | "session/load" if options.is_auth_required => {
            shared
                .wire
                .error(&id, AUTH_REQUIRED, "Authentication required")
        }
        "session/new" => {
            let session = new_session(shared);
            note_asking(shared, &session, &params);
            let mut result = json!({ "sessionId": session });
            add_choices(options, &mut result, None);
            shared.wire.result(&id, result);
            announce_commands(shared, &session);
        }
        "session/list" if options.has_list => {
            shared.wire.result(&id, list_result(options, &params))
        }
        // The load starts and never answers: what a stuck agent does.
        "session/load" if options.is_load_hanging => {}
        "session/load" if options.is_load_failing => {
            shared.wire.error(&id, INTERNAL_ERROR, "Temporary failure");
        }
        "session/load" if options.has_load => {
            let session = params.get("sessionId").and_then(Value::as_str);
            note_asking(shared, session.unwrap_or(""), &params);
            load(shared, &id, &params);
            announce_commands(shared, session.unwrap_or(""));
        }
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

/// Notes the ask rules for the Claude agent that the opening of `session` carries.
fn note_asking(shared: &Shared, session: &str, params: &Value) {
    let rules = params.pointer("/_meta/claudeCode/options/settings/permissions/ask");
    let rules: Vec<String> = rules
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| Some(rule.as_str()?.to_string()))
        .collect();
    if !rules.is_empty() {
        let mut asking = shared.asking.lock().expect("the asking sessions");
        asking.insert(session.to_string(), rules);
    }
}

/// Whether the ask rule `rule` names the tool `name`: by its exact name only.
fn rule_covers(rule: &str, name: &str) -> bool {
    rule == name
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
            "promptCapabilities": {
                "image": options.takes_attachments,
                "embeddedContext": options.takes_attachments,
            },
            "sessionCapabilities": { "list": list, "close": {} },
        },
        "agentInfo": { "name": "sakura-fake-agent", "version": env!("CARGO_PKG_VERSION") },
        "authMethods": [{ "id": "fake-login", "name": "Fake login" }],
    })
}

/// The slash commands a session announces: one that takes nothing, one that takes an argument, one of
/// an MCP server and one whose output is local to the agent.
fn announce_commands(shared: &Shared, session: &str) {
    if !shared.options.announces_commands {
        return;
    }
    let commands = json!([
        { "name": "compact", "description": "Summarise the conversation so far", "input": null },
        { "name": "review", "description": "Review a pull request", "input": { "hint": "[pull request]" } },
        { "name": "mcp:docs:search", "description": "Search the docs (MCP)", "input": { "hint": "[words]" } },
        { "name": "context", "description": "Show what fills the context", "input": null },
    ]);
    let update =
        json!({ "sessionUpdate": "available_commands_update", "availableCommands": commands });
    shared.wire.update(session, update);
}

/// Ids and names of what a session offers.
type Named<'a> = &'a [(&'a str, &'a str)];

/// The modes and models a session offers, when the agent was started to offer any. `model` names the
/// model that is current.
fn add_choices(options: &Options, result: &mut Value, model: Option<&str>) {
    if !options.offers_choices {
        return;
    }
    let (modes, models): (Named, Named) = match options.is_choice_single {
        true => (&[("default", "Default")], &[("haiku", "Haiku")]),
        false => (
            &[
                ("default", "Default"),
                ("acceptEdits", "Accept edits"),
                ("plan", "Plan"),
            ],
            &[("haiku", "Haiku"), ("sonnet", "Sonnet")],
        ),
    };
    let named = |list: Named, key: &str| {
        list.iter()
            .map(|(id, name)| json!({ key: id, "name": name }))
            .collect::<Vec<_>>()
    };
    result["modes"] = json!({ "currentModeId": "default", "availableModes": named(modes, "id") });
    result["configOptions"] = json!([{
        "id": "model", "name": "Model", "category": "model", "type": "select",
        "currentValue": model.unwrap_or("haiku"), "options": named(models, "value"),
    }]);
    if let (Some(effort), Some(config)) = (
        &options.reported_effort,
        result["configOptions"].as_array_mut(),
    ) {
        config.push(json!({
            "id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
            "currentValue": effort, "options": named(&[("default", "Default"), ("high", "High")], "value"),
        }));
    }
}

/// The answer to `session/set_config_option`: every option as it stands, with the model set.
fn set_option_result(options: &Options, params: &Value) -> Value {
    let value = params.get("value").and_then(Value::as_str);
    let mut result = json!({});
    add_choices(options, &mut result, value);
    json!({ "configOptions": result["configOptions"].clone() })
}

fn new_session(shared: &Shared) -> String {
    let mut next = shared.next_session.lock().expect("the session counter");
    *next += 1;
    format!("fake-session-{}", *next)
}

fn list_result(options: &Options, params: &Value) -> Value {
    if options.has_more_session_pages {
        return json!({ "sessions": [], "nextCursor": "next" });
    }
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
    let history = match shared.options.is_history_refused {
        true => scripts::refused_history(),
        false => scripts::history(),
    };
    for update in history {
        shared.wire.update(session, update);
    }
    // A loaded session says what it lets the person pick, as the adapter's does.
    let mut result = json!({});
    add_choices(&shared.options, &mut result, None);
    shared.wire.result(id, result);
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
                    request, allowed, ..
                } if self.runs_unasked(&request) => {
                    self.shared.wire.update(self.session, allowed);
                }
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

    /// Whether the login lets the tool of `request` run without asking in this session: it allows
    /// everything and no ask rule of the session names the tool. A request that names no tool asks in a
    /// session with ask rules.
    fn runs_unasked(&self, request: &Value) -> bool {
        let asking = self.shared.asking.lock().expect("the asking sessions");
        let Some(rules) = asking.get(self.session) else {
            return self.shared.options.is_login_allowing_all;
        };
        let name = request.pointer("/toolCall/_meta/claudeCode/toolName");
        let is_named = |name: &str| rules.iter().any(|rule| rule_covers(rule, name));
        self.shared.options.is_login_allowing_all
            && name
                .and_then(Value::as_str)
                .is_some_and(|name| !is_named(name))
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
