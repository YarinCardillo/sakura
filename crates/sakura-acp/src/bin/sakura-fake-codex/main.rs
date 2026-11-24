//! A fake Codex app-server for tests and for looking at the master's conversation without a real Codex. It
//! speaks the app-server's JSON-RPC over stdio as plain JSON (one message per line, no `jsonrpc` member),
//! written from the protocol's generated schema and not with the client's own code, so the two sides do
//! not share a bug. It is a development tool and is never bundled with the app.
//!
//! The first word of a turn's text picks what the turn does: `plain` (a streamed answer), `think` (two
//! thought paragraphs, then an answer), `tool` (a call of the master's `lane_open` that answers with a
//! lane), `master` (thinks, calls `lane_open` and says so), `command` (asks to run `git status`, and says
//! what it was answered), `patch` (asks to change `/work/a.txt`, and says what it was answered), `grant`
//! (asks for a permission grant and says what it was answered), `nameless` (asks to run a command it does not name), `foreigncwd`, `unknownfield` and `amendment` (a command request that is not the plain one), `moveout`, `grantroot` and `patchunknown` (a patch request that is not the plain one), `model` (answers with the model of its
//! turn), `slow` (streams a word and waits to be interrupted), `silent` (never says anything more), `deaf`
//! (streams a word and ignores a stop), `fail` (the turn fails). Any other text is answered as `plain`.
//!
//! Environment: `SAKURA_FAKE_WORKSPACE=<folder>` is where commands say they run (default `/engine/master`).
//! `SAKURA_FAKE_LOG=<file>` appends one JSON line for every message the client sends.
//! `SAKURA_FAKE_GONE=<id>` makes `thread/resume` of that thread answer an error.
//! `SAKURA_FAKE_INHERITED=<a,b>` names MCP servers the person's own Codex home configures: `config/read`
//! lists them, and they stay enabled in every thread unless the thread's config says `enabled: false`.
//! The turn `servers` answers with the servers and tools the thread ends up with.
//! `SAKURA_FAKE_REFUSE=<method>` makes that method answer an error.
//! `SAKURA_FAKE_LOGIN=<how>` is what a ChatGPT login does after `account/login/start` answers with an address:
//! `success` (the default) writes `auth.json` in `CODEX_HOME` and tells it completed, `fail` tells it
//! completed without success and with a reason, `hold` tells nothing until `account/login/cancel`.
//! `SAKURA_FAKE_LOGIN_URL=<address>` is the address `account/login/start` answers with.
//! `SAKURA_FAKE_HANG=<method>` makes the fake stop answering at that method and ignore the end of its input,
//! so only a kill ends it; its process id is written to `<SAKURA_FAKE_LOG>.pid`.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// The thread a start answers with.
const STARTED_THREAD: &str = "thr-1";
/// The thread the conversation is in now: the one started, or the one resumed.
static CURRENT_THREAD: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn thread() -> String {
    let current = CURRENT_THREAD.lock().expect("the thread");
    if current.is_empty() {
        STARTED_THREAD.to_string()
    } else {
        current.clone()
    }
}
/// The models the fake offers; the first is the default.
const MODELS: [(&str, &str); 2] = [("gpt-fast", "Fast"), ("gpt-deep", "Deep")];
/// JSON-RPC's own code for a request that cannot be served.
const INVALID_REQUEST: i64 = -32600;

struct Fake {
    lines: std::io::Lines<std::io::StdinLock<'static>>,
    log: Option<std::fs::File>,
    turns: u64,
    asked: u64,
    /// The MCP servers of the current thread with their tools; a server's tools are `*` when the
    /// person's own configuration set it up.
    servers: std::collections::BTreeMap<String, String>,
}

fn main() {
    let log = std::env::var("SAKURA_FAKE_LOG").ok().and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let mut fake = Fake {
        lines: std::io::stdin().lock().lines(),
        log,
        turns: 0,
        asked: 0,
        servers: Default::default(),
    };
    while let Some(message) = fake.next_message() {
        fake.serve(&message);
    }
}

fn send(message: &Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{message}")
        .and_then(|()| out.flush())
        .expect("stdout");
}

fn notify(method: &str, params: Value) {
    send(&json!({ "method": method, "params": params }));
}

fn answer(id: &Value, result: Value) {
    send(&json!({ "id": id, "result": result }));
}

impl Fake {
    /// The next message the client sends, logged.
    fn next_message(&mut self) -> Option<Value> {
        loop {
            let line = self.lines.next()?.ok()?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(log) = self.log.as_mut() {
                writeln!(log, "{message}").ok();
            }
            return Some(message);
        }
    }

    fn serve(&mut self, message: &Value) {
        let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id")) else {
            return;
        };
        let params = &message["params"];
        if std::env::var("SAKURA_FAKE_REFUSE").is_ok_and(|refused| refused == method) {
            let error = json!({ "code": INVALID_REQUEST, "message": format!("{method} refused") });
            return send(&json!({ "id": id, "error": error }));
        }
        match method {
            "initialize" => answer(
                id,
                json!({ "userAgent": "sakura-desk/9.9.9 (fake)", "codexHome": "/fake", "platformFamily": "unix", "platformOs": "macos" }),
            ),
            "account/login/start" => self.login_start(id, params),
            "account/login/cancel" => answer(id, json!({ "status": "canceled" })),
            "model/list" => answer(id, json!({ "data": models(), "nextCursor": null })),
            "config/read" => answer(id, json!({ "config": { "mcp_servers": inherited() } })),
            "thread/start" => {
                self.configure(params);
                self.start(id)
            }
            "thread/resume" => {
                self.hang_at(method);
                self.configure(params);
                self.resume(id, params)
            }
            "turn/start" => self.turn(id, params),
            "turn/interrupt" => answer(id, json!({})),
            "skills/list" => answer(
                id,
                json!({ "data": [{ "cwd": workspace(), "skills": [
                    { "name": "pdf", "description": "Read and write PDF files", "path": "/skills/pdf/SKILL.md", "enabled": true }
                ], "errors": [] }] }),
            ),
            "thread/fork" => answer(
                id,
                json!({ "thread": { "id": "thr-forked", "turns": [] }, "model": MODELS[0].0 }),
            ),
            "thread/name/set" | "thread/goal/set" | "thread/goal/clear" => answer(id, json!({})),
            "thread/goal/get" => answer(
                id,
                json!({ "goal": { "objective": "ship it", "status": "active" } }),
            ),
            "mcpServerStatus/list" => answer(
                id,
                json!({ "data": [{ "name": "sakura-master", "tools": { "lanes": {}, "lane_open": {} }, "authStatus": "unsupported" }], "nextCursor": null }),
            ),
            "account/rateLimits/read" => answer(
                id,
                json!({ "rateLimits": { "primary": { "usedPercent": 12, "windowDurationMins": 300 } } }),
            ),
            "app/list" | "plugin/list" | "hooks/list" | "experimentalFeature/list" => {
                answer(id, json!({ "data": [], "nextCursor": null }))
            }
            "thread/compact/start" | "review/start" => self.side_turn(id, method),
            _ => send(
                &json!({ "id": id, "error": { "code": INVALID_REQUEST, "message": format!("no {method}") } }),
            ),
        }
    }

    /// A ChatGPT login: the address to open, then how it ends as `SAKURA_FAKE_LOGIN` says.
    fn login_start(&mut self, id: &Value, params: &Value) {
        if params["type"] != "chatgpt" {
            let error =
                json!({ "code": INVALID_REQUEST, "message": "only the ChatGPT login is fake" });
            return send(&json!({ "id": id, "error": error }));
        }
        let url = std::env::var("SAKURA_FAKE_LOGIN_URL")
            .unwrap_or_else(|_| "https://auth.example.test/login?state=abc".to_string());
        answer(
            id,
            json!({ "type": "chatgpt", "authUrl": url, "loginId": "login-1" }),
        );
        match std::env::var("SAKURA_FAKE_LOGIN")
            .as_deref()
            .unwrap_or("success")
        {
            "hold" => {}
            kind @ ("fail" | "late-fail") => {
                if kind == "late-fail" {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                notify(
                    "account/login/completed",
                    json!({ "loginId": "login-1", "success": false, "error": "the browser window was closed" }),
                );
            }

            _ => {
                if let Ok(home) = std::env::var("CODEX_HOME") {
                    std::fs::write(std::path::Path::new(&home).join("auth.json"), "{}")
                        .expect("the login");
                }
                notify(
                    "account/login/completed",
                    json!({ "loginId": "login-1", "success": true, "error": null }),
                );
            }
        }
    }

    /// The MCP servers a thread ends up with: the inherited ones, enabled, then the thread's own config
    /// on top as Codex merges it.
    fn configure(&mut self, params: &Value) {
        self.servers = names_inherited()
            .into_iter()
            .map(|name| (name, "*".to_string()))
            .collect();
        let overrides = params["config"]["mcp_servers"].as_object();
        for (name, entry) in overrides.into_iter().flatten() {
            if entry["enabled"] == false {
                self.servers.remove(name);
                continue;
            }
            let tools = entry["enabled_tools"].as_array().map(|tools| {
                let tools = tools.iter().filter_map(Value::as_str);
                tools.collect::<Vec<_>>().join("+")
            });
            self.servers
                .insert(name.clone(), tools.unwrap_or_else(|| "*".to_string()));
        }
    }

    /// Stops answering for good when `SAKURA_FAKE_HANG` names `method`.
    fn hang_at(&mut self, method: &str) {
        if std::env::var("SAKURA_FAKE_HANG").is_ok_and(|hang| hang == method) {
            if let Ok(path) = std::env::var("SAKURA_FAKE_LOG") {
                std::fs::write(format!("{path}.pid"), std::process::id().to_string()).ok();
            }
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }

    fn start(&mut self, id: &Value) {
        answer(
            id,
            json!({ "thread": { "id": STARTED_THREAD, "turns": [] }, "model": MODELS[0].0 }),
        );
        notify(
            "thread/started",
            json!({ "thread": { "id": STARTED_THREAD } }),
        );
    }

    fn resume(&mut self, id: &Value, params: &Value) {
        let thread = params["threadId"].as_str().unwrap_or_default();
        if std::env::var("SAKURA_FAKE_GONE").is_ok_and(|gone| gone == thread) {
            let error = json!({ "code": INVALID_REQUEST, "message": "no rollout found" });
            return send(&json!({ "id": id, "error": error }));
        }
        if std::env::var_os("SAKURA_FAKE_RESUME_FAILURE").is_some() {
            let error =
                json!({ "code": INVALID_REQUEST, "message": "storage temporarily unavailable" });
            return send(&json!({ "id": id, "error": error }));
        }
        *CURRENT_THREAD.lock().expect("the thread") = thread.to_string();
        let items = json!([
            { "type": "userMessage", "id": "u0", "clientId": null, "content": [{ "type": "text", "text": "earlier question", "text_elements": [] }] },
            { "type": "reasoning", "id": "r0", "summary": ["thought earlier"], "content": [] },
            { "type": "agentMessage", "id": "a0", "text": "earlier answer", "phase": null },
        ]);
        let thread = json!({ "id": thread, "turns": [{ "id": "t0", "items": items, "status": "completed" }] });
        answer(id, json!({ "thread": thread, "model": MODELS[0].0 }));
    }

    /// A request that makes a turn of its own (a compaction, a review): the answer, then the turn's events.
    fn side_turn(&mut self, id: &Value, method: &str) {
        self.turns += 1;
        let turn = format!("turn-{}", self.turns);
        answer(
            id,
            json!({ "turn": { "id": turn, "status": "inProgress", "items": [] } }),
        );
        notify(
            "turn/started",
            json!({ "threadId": thread(), "turn": { "id": turn, "status": "inProgress" } }),
        );
        say("a0", &format!("{method} done."));
        self.end(&turn, "completed", None);
    }

    fn turn(&mut self, id: &Value, params: &Value) {
        // The real app-server answers a turn that names a model it does not list with an error, on a ChatGPT
        // account (`SAKURA_FAKE_STRICT_MODELS`).
        let asked = params["model"].as_str();
        let is_unlisted =
            asked.is_some_and(|model| !MODELS.iter().any(|(listed, _)| *listed == model));
        if std::env::var("SAKURA_FAKE_STRICT_MODELS").is_ok() && is_unlisted {
            let message = format!(
                "The '{}' model is not supported when using Codex with a ChatGPT account.",
                asked.unwrap_or_default()
            );
            return send(
                &json!({ "id": id, "error": { "code": INVALID_REQUEST, "message": message } }),
            );
        }
        self.turns += 1;
        let turn = format!("turn-{}", self.turns);
        answer(
            id,
            json!({ "turn": { "id": turn, "status": "inProgress", "items": [] } }),
        );
        notify(
            "turn/started",
            json!({ "threadId": thread(), "turn": { "id": turn, "status": "inProgress" } }),
        );
        let first = params["input"][0]["text"].as_str().unwrap_or_default();
        match first.split_whitespace().next().unwrap_or("plain") {
            "think" => self.think(),
            "tool" => self.tool(),
            "master" => self.master(),
            "command" => self.command(),
            "patch" => self.patch(),
            "nameless" => self.nameless(),
            "foreigncwd" => self.command_with("/elsewhere", json!({})),
            "unknownfield" => self.command_with(&workspace(), json!({ "surprise": true })),
            "amendment" => self.command_with(&workspace(), json!({ "proposedExecpolicyAmendment": ["git"] })),
            "moveout" => self.patch_with(json!({ "path": "/work/a.txt", "kind": { "type": "update", "move_path": "/outside/b.txt" }, "diff": "" }), json!({})),
            "grantroot" => self.patch_with(json!({ "path": "/work/a.txt", "kind": { "type": "add" }, "diff": "+x" }), json!({ "grantRoot": "/" })),
            "patchunknown" => self.patch_with(json!({ "path": "/work/a.txt", "kind": { "type": "add" }, "diff": "+x" }), json!({ "surprise": 1 })),
            "grant" => self.grant(),
            "servers" => say("a0", &format!("servers {}", self.inventory())),
            "settings" => say(
                "a0",
                &format!(
                    "settings model={} effort={} tier={} mode={}",
                    params["model"].as_str().unwrap_or("none"),
                    params["effort"].as_str().unwrap_or("none"),
                    params["serviceTier"].as_str().unwrap_or("none"),
                    params["collaborationMode"]["mode"].as_str().unwrap_or("none"),
                ),
            ),
            "model" => say(
                "a0",
                &format!("model {}", params["model"].as_str().unwrap_or("none")),
            ),
            "slow" => return self.slow(&turn),
            "silent" => return self.ignore_everything(),
            "deaf" => {
                notify(
                    "item/agentMessage/delta",
                    json!({ "threadId": thread(), "turnId": turn, "itemId": "a0", "delta": "Working" }),
                );
                return self.ignore_everything();
            }
            "fail" => return self.end(&turn, "failed", Some("usage limit reached")),
            _ => say("a0", "Hello there."),
        }
        self.end(&turn, "completed", None);
    }

    /// The servers and tools of the thread, as `name=tool+tool` separated by spaces.
    fn inventory(&self) -> String {
        let servers = self.servers.iter();
        let servers = servers.map(|(name, tools)| format!("{name}={tools}"));
        servers.collect::<Vec<_>>().join(" ")
    }

    fn end(&self, turn: &str, status: &str, error: Option<&str>) {
        let error = error.map(|message| json!({ "message": message }));
        notify(
            "turn/completed",
            json!({ "threadId": thread(), "turn": { "id": turn, "status": status, "error": error, "items": [] } }),
        );
    }

    fn think(&mut self) {
        self.thought();
        say("a0", "Thought it through.");
    }

    /// Two paragraphs of thought, streamed.
    fn thought(&mut self) {
        notify(
            "item/started",
            item_event(json!({ "type": "reasoning", "id": "r1", "summary": [], "content": [] })),
        );
        for (part, text) in ["First, ", "then second."].iter().enumerate() {
            notify(
                "item/reasoning/summaryPartAdded",
                json!({ "threadId": thread(), "turnId": "t", "itemId": "r1", "summaryIndex": part }),
            );
            notify(
                "item/reasoning/summaryTextDelta",
                json!({ "threadId": thread(), "turnId": "t", "itemId": "r1", "delta": text, "summaryIndex": part }),
            );
        }
    }

    fn tool(&mut self) {
        self.tool_call();
        say("a0", "Opened it.");
    }

    /// A call of the master's `lane_open`, which answers with a lane the engine's fixtures list.
    fn tool_call(&mut self) {
        let call = |status: &str, result: Value| json!({ "type": "mcpToolCall", "id": "m1", "server": "sakura-master", "tool": "lane_open", "status": status, "arguments": { "project": "demo" }, "result": result, "error": null });
        notify("item/started", item_event(call("inProgress", Value::Null)));
        let result = json!({ "content": [{ "type": "text", "text": "{\"lane\": \"demo\"}" }], "structuredContent": null, "_meta": null });
        notify("item/completed", item_event(call("completed", result)));
    }

    /// What the engine's master does in a turn: thinks, starts a lane, says so.
    fn master(&mut self) {
        self.thought();
        self.tool_call();
        say(
            "a0",
            "Started a lane for it. The reviewer gates every step.",
        );
    }

    fn command(&mut self) {
        self.command_with(&workspace(), json!({}));
    }

    /// A request to run `git status` in `cwd`, with `extra` fields added to the request.
    fn command_with(&mut self, cwd: &str, extra: Value) {
        let item = |status: &str| json!({ "type": "commandExecution", "id": "c1", "command": "git status", "cwd": cwd, "status": status, "commandActions": [], "aggregatedOutput": null, "exitCode": null });
        notify("item/started", item_event(item("inProgress")));
        let mut params = json!({ "threadId": thread(), "turnId": "t", "itemId": "c1", "command": "git status", "cwd": cwd, "startedAtMs": 0 });
        merge(&mut params, extra);
        let decision = self.ask("item/commandExecution/requestApproval", params);
        let status = if decision == "accept" {
            "completed"
        } else {
            "declined"
        };
        notify("item/completed", item_event(item(status)));
        say("a0", &format!("command {decision}"));
    }

    /// An approval of a command that names none.
    fn nameless(&mut self) {
        let params = json!({ "threadId": thread(), "turnId": "t", "itemId": "n1", "startedAtMs": 0, "reason": "needs the network" });
        let decision = self.ask("item/commandExecution/requestApproval", params);
        say("a0", &format!("nameless {decision}"));
    }

    fn patch(&mut self) {
        let change = json!({ "path": "/work/a.txt", "kind": { "type": "add" }, "diff": "+x" });
        self.patch_with(change, json!({}));
    }

    /// A request to apply the change `change`, with `extra` fields added to the request.
    fn patch_with(&mut self, change: Value, extra: Value) {
        let item = |status: &str| json!({ "type": "fileChange", "id": "f1", "changes": [change], "status": status });
        notify("item/started", item_event(item("inProgress")));
        let mut params =
            json!({ "threadId": thread(), "turnId": "t", "itemId": "f1", "startedAtMs": 0 });
        merge(&mut params, extra);
        let decision = self.ask("item/fileChange/requestApproval", params);
        let status = if decision == "accept" {
            "completed"
        } else {
            "declined"
        };
        notify("item/completed", item_event(item(status)));
        say("a0", &format!("patch {decision}"));
    }

    fn grant(&mut self) {
        let params = json!({ "threadId": thread(), "turnId": "t", "itemId": "g1", "startedAtMs": 0, "permissions": {}, "cwd": "/work" });
        let granted = self.ask_raw("item/permissions/requestApproval", params);
        say("a0", &format!("grant {}", granted["permissions"]));
    }

    /// Streams a word, then waits for the client to interrupt the turn.
    fn slow(&mut self, turn: &str) {
        notify(
            "item/agentMessage/delta",
            json!({ "threadId": thread(), "turnId": turn, "itemId": "a0", "delta": "Working" }),
        );
        while let Some(message) = self.next_message() {
            if message["method"] == "turn/interrupt" {
                answer(&message["id"], json!({}));
                return self.end(turn, "interrupted", None);
            }
        }
    }

    /// Reads what the client sends and answers nothing: a turn that never ends and ignores a stop.
    fn ignore_everything(&mut self) {
        while self.next_message().is_some() {}
    }

    /// Asks the client and gives what it decided, `accept` or `decline` or `cancel`.
    fn ask(&mut self, method: &str, params: Value) -> String {
        let answered = self.ask_raw(method, params);
        answered["decision"].as_str().unwrap_or("none").to_string()
    }

    /// Asks the client and gives its whole answer; an interrupt that comes first is `cancel`.
    fn ask_raw(&mut self, method: &str, params: Value) -> Value {
        self.asked += 1;
        let id = format!("req-{}", self.asked);
        send(&json!({ "id": id, "method": method, "params": params }));
        while let Some(message) = self.next_message() {
            if message["id"] == id && message.get("method").is_none() {
                return message["result"].clone();
            }
            if message["method"] == "turn/interrupt" {
                answer(&message["id"], json!({}));
            }
        }
        json!({ "decision": "cancel" })
    }
}

/// The names in `SAKURA_FAKE_INHERITED`.
fn names_inherited() -> Vec<String> {
    let names = std::env::var("SAKURA_FAKE_INHERITED").unwrap_or_default();
    names
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The servers of the person's own configuration as `config/read` lists them.
fn inherited() -> Value {
    let servers = names_inherited().into_iter();
    Value::Object(
        servers
            .map(|name| (name, json!({ "command": "/bin/true", "enabled": true })))
            .collect(),
    )
}

/// The folder the master works in as the fake reports it for commands: `SAKURA_FAKE_WORKSPACE`.
fn workspace() -> String {
    std::env::var("SAKURA_FAKE_WORKSPACE").unwrap_or_else(|_| "/engine/master".to_string())
}

/// Adds the members of the object `extra` to the object `target`.
fn merge(target: &mut Value, extra: Value) {
    if let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) {
        target.extend(extra);
    }
}

fn models() -> Value {
    // The first model accepts four efforts and a fast tier; the second three efforts and no tier; both are
    // listed with a hidden legacy one after them, as the real server lists with `includeHidden`.
    let model = |(id, name): &(&str, &str)| {
        let is_first = *id == MODELS[0].0;
        let efforts: &[&str] = if is_first {
            &["low", "medium", "high", "xhigh"]
        } else {
            &["low", "medium", "high"]
        };
        let tiers = if is_first {
            json!([{ "id": "priority", "name": "Fast", "description": "faster" }])
        } else {
            json!([])
        };
        json!({ "id": id, "model": id, "displayName": name, "description": format!("{name} model"), "hidden": false,
                "isDefault": is_first, "defaultReasoningEffort": if is_first { "medium" } else { "low" },
                "supportedReasoningEfforts": efforts.iter().map(|effort| json!({ "reasoningEffort": effort, "description": effort })).collect::<Vec<_>>(),
                "serviceTiers": tiers })
    };
    Value::Array(MODELS.iter().map(model).collect())
}

fn item_event(item: Value) -> Value {
    json!({ "threadId": thread(), "turnId": "t", "item": item, "startedAtMs": 0, "completedAtMs": 0 })
}

/// A streamed answer of two deltas, then the completed item.
fn say(id: &str, text: &str) {
    notify(
        "item/started",
        item_event(json!({ "type": "agentMessage", "id": id, "text": "" })),
    );
    let (head, tail) = text.split_at(text.len() / 2);
    for delta in [head, tail] {
        notify(
            "item/agentMessage/delta",
            json!({ "threadId": thread(), "turnId": "t", "itemId": id, "delta": delta }),
        );
    }
    notify(
        "item/completed",
        item_event(json!({ "type": "agentMessage", "id": id, "text": text })),
    );
}
