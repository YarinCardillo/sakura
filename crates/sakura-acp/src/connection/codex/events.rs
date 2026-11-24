//! What the server says: the notifications of a turn become what the conversation shows, and the requests
//! of the server (approvals, mostly) are answered by the engine's decider, by the person, or by a refusal.

use super::approvals::{self, ACCEPT};
use super::items;
use super::wire::{self, Incoming};
use super::{decider_of, Serve, Waiting, MAX_KNOWN_ITEMS, MAX_WAITING_APPROVALS};
use crate::connection::AgentEvent;
use crate::model::{StopReason, ToolCallFields, Update};
use crate::policy::decider::{Decider, Verdict};
use crate::policy::{Reason, Subject};
use crate::thread::{PermissionOutcome, Problem, ProblemKind, ThreadEvent};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tokio::time::Instant;

/// The items started, by id, so that an approval can name what it is about.
#[derive(Default)]
pub(super) struct Known {
    items: HashMap<String, Value>,
    order: VecDeque<String>,
}

impl Known {
    fn note(&mut self, item: &Value) {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        if self.items.insert(id.to_string(), item.clone()).is_none() {
            self.order.push_back(id.to_string());
        }
        if self.order.len() > MAX_KNOWN_ITEMS {
            if let Some(oldest) = self.order.pop_front() {
                self.items.remove(&oldest);
            }
        }
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

impl Serve {
    /// One message of the server.
    pub(super) async fn handle(&mut self, message: Incoming) {
        match message {
            Incoming::Notification { method, params } => self.notification(&method, &params).await,
            Incoming::Request { id, method, params } => self.request(id, &method, &params).await,
            Incoming::Failure { message, .. } => {
                eprintln!("sakura: the codex server refused a request: {message}")
            }
            Incoming::Response { .. } => {}
        }
    }

    /// Whether `params` are about the conversation's own thread: a thread of a sub-agent is not shown.
    fn is_ours(&self, params: &Value) -> bool {
        let thread = text(params, "threadId");
        thread.is_some() && thread == self.thread_id.as_deref()
    }

    async fn notification(&mut self, method: &str, params: &Value) {
        if !self.is_ours(params) {
            return;
        }
        self.last_heard = Instant::now();
        match method {
            "turn/started" => self.turn_started(params),
            "turn/completed" => self.turn_completed(params).await,
            "item/started" => self.item_started(params).await,
            "item/completed" => self.item_completed(params).await,
            "item/agentMessage/delta" => self.stream(params, false).await,
            "item/reasoning/summaryPartAdded" => self.part_added(params),
            "item/reasoning/summaryTextDelta" => self.stream(params, true).await,
            _ => {}
        }
    }

    fn turn_started(&mut self, params: &Value) {
        let id = params.get("turn").and_then(|turn| text(turn, "id"));
        if let (Some(turn), Some(id)) = (self.turn.as_mut(), id) {
            turn.id = Some(id.to_string());
        }
    }

    async fn turn_completed(&mut self, params: &Value) {
        let turn = params.get("turn").cloned().unwrap_or(Value::Null);
        self.turn = None;
        self.streamed.clear();
        self.parts.clear();
        self.cancel_waiting().await;
        let event = match text(&turn, "status") {
            Some("interrupted") => ThreadEvent::TurnEnded(StopReason::Cancelled),
            Some("failed") => ThreadEvent::Failed(failure_of(&turn)),
            _ => ThreadEvent::TurnEnded(StopReason::EndTurn),
        };
        self.emit_thread(event).await;
    }

    async fn item_started(&mut self, params: &Value) {
        let Some(item) = params.get("item") else {
            return;
        };
        self.known.note(item);
        if let Some(call) = items::tool_call(item) {
            self.emit_thread(ThreadEvent::Update(Update::ToolCall(call)))
                .await;
        }
    }

    async fn item_completed(&mut self, params: &Value) {
        let Some(item) = params.get("item") else {
            return;
        };
        self.known.note(item);
        let id = text(item, "id").unwrap_or_default();
        let update = match text(item, "type") {
            Some("agentMessage") if !self.streamed.contains(id) => Some(Update::AgentText(
                text(item, "text").unwrap_or_default().to_string(),
            )),
            Some("reasoning") if !self.streamed.contains(id) => Some(items::reasoning_text(item))
                .filter(|thought| !thought.is_empty())
                .map(Update::Thought),
            _ => items::tool_call(item).map(Update::ToolCallUpdate),
        };
        if let Some(update) = update {
            self.emit_thread(ThreadEvent::Update(update)).await;
        }
    }

    /// A delta of the agent's words, or of its thoughts.
    async fn stream(&mut self, params: &Value, is_thought: bool) {
        let (Some(id), Some(delta)) = (text(params, "itemId"), text(params, "delta")) else {
            return;
        };
        let mut shown = String::new();
        // A new summary paragraph of the same thought starts on a line of its own.
        if let Some(part) = params.get("summaryIndex").and_then(Value::as_i64) {
            let seen = self.parts.insert(id.to_string(), part);
            if seen.is_some_and(|seen| seen != part) {
                shown.push_str("\n\n");
            }
        }
        shown.push_str(delta);
        self.streamed.insert(id.to_string());
        let update = if is_thought {
            Update::Thought(shown)
        } else {
            Update::AgentText(shown)
        };
        self.emit_thread(ThreadEvent::Update(update)).await;
    }

    fn part_added(&mut self, params: &Value) {
        let (id, part) = (
            text(params, "itemId"),
            params.get("summaryIndex").and_then(Value::as_i64),
        );
        if let (Some(id), Some(part)) = (id, part) {
            self.parts.entry(id.to_string()).or_insert(part);
        }
    }

    /// A request of the server, answered at once or parked for the person.
    async fn request(&mut self, id: Value, method: &str, params: &Value) {
        self.last_heard = Instant::now();
        let answer = match method {
            "item/commandExecution/requestApproval" => self.command_approval(&id, params).await,
            "item/fileChange/requestApproval" => self.file_change_approval(&id, params).await,
            _ => Some(match approvals::refusal_of(method) {
                Some(refusal) => wire::response(&id, refusal),
                None => wire::failure(&id, "the master's client does not answer this request"),
            }),
        };
        if let Some(answer) = answer {
            if self.send(answer).await.is_err() {
                eprintln!("sakura: an answer to {method} was not sent: the server is gone");
            }
        }
    }

    /// The tool call an approval is about: what is known of its item, else what the request says.
    fn tool_of(&self, params: &Value) -> ToolCallFields {
        let id = text(params, "itemId").unwrap_or_default();
        match self.known.items.get(id).and_then(items::tool_call) {
            Some(call) => call,
            None => {
                let command = text(params, "command").unwrap_or_default();
                items::tool_call(
                    &json!({ "type": "commandExecution", "id": id, "command": command }),
                )
                .unwrap_or_else(|| ToolCallFields::new(id))
            }
        }
    }

    async fn command_approval(&mut self, id: &Value, params: &Value) -> Option<String> {
        let tool = self.tool_of(params);
        // A request that cannot be judged whole (no command, a foreign folder, a field not understood) is
        // judged by nobody and declined.
        let request = approvals::command_request(params, &self.config.agent.workspace);
        let decider = decider_of(&self.config).cloned();
        let verdict = verdict(decider, request.into_iter().collect()).await;
        self.settle(id, tool, verdict, approvals::command_decision)
            .await
    }

    async fn file_change_approval(&mut self, id: &Value, params: &Value) -> Option<String> {
        let tool = self.tool_of(params);
        let item = self
            .known
            .items
            .get(text(params, "itemId").unwrap_or_default());
        let changes = item
            .and_then(items::changes)
            .filter(|_| approvals::patch_is_plain(params));
        let requests = changes.map(|changes| approvals::edit_requests(&changes));
        let decider = decider_of(&self.config).cloned();
        let verdict = verdict(decider, requests.unwrap_or_default()).await;
        self.settle(id, tool, verdict, approvals::file_change_decision)
            .await
    }

    /// Acts on a verdict: a refusal and an allowance are answered at once, a question is shown to the
    /// person and answered when they answer. Only while a turn runs; otherwise the request is refused.
    async fn settle(
        &mut self,
        id: &Value,
        tool: ToolCallFields,
        verdict: Verdict,
        decision: fn(bool) -> Value,
    ) -> Option<String> {
        let is_open = self
            .turn
            .as_ref()
            .is_some_and(|turn| turn.cancel_limit.is_none());
        match verdict {
            Verdict::Allow if is_open => {
                self.emit_thread(ThreadEvent::PermissionAllowed { tool })
                    .await;
                Some(wire::response(id, decision(true)))
            }
            Verdict::Ask if is_open && self.waiting.len() < MAX_WAITING_APPROVALS => {
                self.park(id, tool, decision).await;
                None
            }
            _ => Some(wire::response(id, decision(false))),
        }
    }

    async fn park(&mut self, id: &Value, tool: ToolCallFields, decision: fn(bool) -> Value) {
        let request = self.next_request;
        self.next_request += 1;
        let waiting = Waiting {
            server_id: id.clone(),
            decision,
        };
        self.waiting.insert(request, waiting);
        let subject = Subject::of(&tool);
        let (reason, options) = (Reason::EngineAsks, approvals::options());
        let asked = ThreadEvent::PermissionAsked {
            request,
            tool,
            reason,
            subject,
            options,
        };
        self.emit_thread(asked).await;
    }

    /// The person's answer to the approval `request`. An option it never offered is not sent.
    pub(super) async fn answer(&mut self, request: u64, option_id: String) {
        self.last_heard = Instant::now();
        let kind = approvals::options()
            .into_iter()
            .find(|option| option.id == option_id);
        let (Some(kind), Some(waiting)) = (kind, self.waiting.remove(&request)) else {
            return eprintln!("sakura: no waiting approval {request} offers that option");
        };
        let is_accepted = option_id == ACCEPT;
        let answer = wire::response(&waiting.server_id, (waiting.decision)(is_accepted));
        if self.send(answer).await.is_err() {
            eprintln!("sakura: an answer to an approval was not sent: the server is gone");
        }
        let outcome = PermissionOutcome::Selected(kind.kind);
        self.emit_thread(ThreadEvent::PermissionAnswered { request, outcome })
            .await;
    }

    /// Every approval that waits is answered "cancel": the turn it was for is over.
    pub(super) async fn cancel_waiting(&mut self) {
        let waiting: Vec<(u64, Waiting)> = self.waiting.drain().collect();
        for (request, waiting) in waiting {
            let answer = wire::response(&waiting.server_id, json!({ "decision": "cancel" }));
            self.send(answer).await.ok();
            let outcome = PermissionOutcome::Cancelled;
            self.emit_thread(ThreadEvent::PermissionAnswered { request, outcome })
                .await;
        }
    }

    /// Tells the window what a choice of the session is now.
    pub(super) async fn emit_choices(&mut self) {
        let choices = self.choices();
        self.emit(AgentEvent::Choices(choices)).await;
    }
}

/// The engine's verdict on every request of one approval; a refusal when there is no decider.
async fn verdict(decider: Option<Decider>, requests: Vec<Value>) -> Verdict {
    let Some(decider) = decider else {
        return Verdict::Deny;
    };
    let mut verdicts = Vec::new();
    for request in requests {
        verdicts.push(decider.decide(request).await);
    }
    approvals::combine(&verdicts)
}

/// Why a turn failed, in the server's own words.
fn failure_of(turn: &Value) -> Problem {
    let message = turn.get("error").and_then(|error| text(error, "message"));
    let detail = message.unwrap_or("the turn failed").to_string();
    Problem {
        kind: ProblemKind::RequestFailed,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_that_failed_says_why_in_the_servers_words() {
        let turn = json!({ "status": "failed", "error": { "message": "usage limit reached" } });
        assert_eq!(failure_of(&turn).detail, "usage limit reached");
        assert_eq!(failure_of(&json!({})).detail, "the turn failed");
    }

    #[test]
    fn the_oldest_known_items_are_forgotten_first() {
        let mut known = Known::default();
        for number in 0..=MAX_KNOWN_ITEMS {
            known.note(&json!({ "id": format!("i{number}") }));
        }
        assert!(!known.items.contains_key("i0"));
        assert!(known.items.contains_key(&format!("i{MAX_KNOWN_ITEMS}")));
        assert_eq!(known.items.len(), MAX_KNOWN_ITEMS);
    }
}
