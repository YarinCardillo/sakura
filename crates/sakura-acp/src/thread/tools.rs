//! Tool calls and the permissions requested them. A call is created by whichever message names its id first
//! (an update may arrive before its `tool_call`), and every later message patches it in place.

use super::{EntryKind, Permission, PermissionOutcome, Thread, ToolCall};
use crate::model::{PermissionOption, ToolCallFields, ToolContent, ToolKind, ToolStatus};
use crate::policy::{Reason, Subject};
use serde_json::Value;

impl Thread {
    /// Creates or patches the call `fields.id` and returns its entry index. A declaration (`tool_call`)
    /// that arrives after updates fills what is missing without emptying what an update already brought.
    pub(super) fn upsert_tool(&mut self, fields: ToolCallFields, is_declaration: bool) -> usize {
        let Some(&index) = self.tool_index.get(&fields.id) else {
            return self.create_tool(fields);
        };
        if let EntryKind::Tool(call) = &mut self.entries[index].kind {
            let before = held_bytes(call);
            patch(call, fields, is_declaration);
            let after = held_bytes(call);
            self.held_bytes = (self.held_bytes + after).saturating_sub(before);
        }
        self.touch(index);
        index
    }

    fn create_tool(&mut self, fields: ToolCallFields) -> usize {
        self.settle_streaming();
        let call = ToolCall {
            id: fields.id.clone(),
            title: fields.title.unwrap_or_default(),
            kind: fields.kind.unwrap_or_default(),
            status: fields.status.unwrap_or_default(),
            content: fields.content.unwrap_or_default(),
            locations: fields.locations.unwrap_or_default(),
            raw_input: fields.raw_input,
        };
        self.held_bytes += held_bytes(&call);
        let index = self.push(EntryKind::Tool(call));
        self.tool_index.insert(fields.id, index);
        index
    }

    /// The agent asks the person: the call waits, and the question is its own entry after it.
    pub(super) fn permission_asked(
        &mut self,
        request: u64,
        tool: ToolCallFields,
        (reason, subject): (Reason, Subject),
        options: Vec<PermissionOption>,
    ) {
        let tool_call = tool.id.clone();
        let index = self.upsert_tool(tool, false);
        self.set_tool_status(index, |status| match status.is_finished() {
            true => status,
            false => ToolStatus::WaitingForConfirmation,
        });
        self.push(EntryKind::Permission(Permission {
            request,
            tool_call,
            reason,
            subject,
            options,
            outcome: PermissionOutcome::Waiting,
        }));
    }

    /// The policy allowed the call once: it goes on without a question. A full conversation does not
    /// show it: the agent's calls are its output like any other.
    pub(super) fn permission_allowed(&mut self, tool: ToolCallFields) {
        if self.is_full {
            return;
        }
        let index = self.upsert_tool(tool, false);
        self.set_tool_status(index, started);
    }

    /// The answer to a question: the first one counts, and the call follows it.
    pub(super) fn permission_answered(&mut self, request: u64, outcome: PermissionOutcome) {
        let asked = self.entries.iter().rposition(|entry| {
            matches!(&entry.kind, EntryKind::Permission(permission) if permission.request == request)
        });
        let Some(index) = asked else { return };
        let EntryKind::Permission(permission) = &mut self.entries[index].kind else {
            return;
        };
        if permission.outcome != PermissionOutcome::Waiting {
            return;
        }
        permission.outcome = outcome;
        let tool_call = permission.tool_call.clone();
        self.touch(index);
        if let Some(&tool) = self.tool_index.get(&tool_call) {
            self.set_tool_status(tool, |status| after_answer(status, outcome));
        }
    }

    /// Every call that has not finished is cancelled, and every open question with it.
    pub(super) fn stop_unfinished(&mut self) {
        for index in 0..self.entries.len() {
            let is_changed = match &mut self.entries[index].kind {
                EntryKind::Tool(call) if !call.status.is_finished() => {
                    call.status = ToolStatus::Canceled;
                    true
                }
                EntryKind::Permission(permission)
                    if permission.outcome == PermissionOutcome::Waiting =>
                {
                    permission.outcome = PermissionOutcome::Cancelled;
                    true
                }
                _ => false,
            };
            if is_changed {
                self.touch(index);
            }
        }
    }

    fn set_tool_status(&mut self, index: usize, next: impl Fn(ToolStatus) -> ToolStatus) {
        let EntryKind::Tool(call) = &mut self.entries[index].kind else {
            return;
        };
        let status = next(call.status);
        if status != call.status {
            call.status = status;
            self.touch(index);
        }
    }
}

/// The bytes of text a call holds: its title, what it shows, the files it names and its raw input.
fn held_bytes(call: &ToolCall) -> usize {
    let content: usize = call.content.iter().map(content_bytes).sum();
    let locations: usize = call.locations.iter().map(|at| at.path.len()).sum();
    let input = call.raw_input.as_ref().map_or(0, value_bytes);
    call.title.len() + content + locations + input
}

fn content_bytes(content: &ToolContent) -> usize {
    match content {
        ToolContent::Text { text, .. } => text.len(),
        ToolContent::Terminal { id } => id.len(),
        ToolContent::Diff(diff) => {
            let hunks = diff.patch.iter().flat_map(|patch| &patch.hunks);
            let lines = hunks.map(|hunk| {
                hunk.header.len() + hunk.lines.iter().map(|line| line.text.len()).sum::<usize>()
            });
            diff.path.len() + lines.sum::<usize>()
        }
    }
}

/// The bytes of the strings and keys of a JSON value, with a word for each number, boolean and null.
fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(items) => items.iter().map(value_bytes).sum(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| key.len() + value_bytes(value))
            .sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) => std::mem::size_of::<u64>(),
    }
}

fn patch(call: &mut ToolCall, fields: ToolCallFields, is_declaration: bool) {
    if let Some(title) = fields.title.filter(|title| !title.is_empty()) {
        call.title = title;
    }
    if let Some(kind) = fields
        .kind
        .filter(|kind| !(is_declaration && *kind == ToolKind::Other))
    {
        call.kind = kind;
    }
    if let Some(status) = fields.status {
        call.status = merged_status(call.status, status);
    }
    if let Some(content) = fields
        .content
        .filter(|content| !(is_declaration && content.is_empty()))
    {
        call.content = content;
    }
    if let Some(locations) = fields
        .locations
        .filter(|all| !(is_declaration && all.is_empty()))
    {
        call.locations = locations;
    }
    if fields.raw_input.is_some() {
        call.raw_input = fields.raw_input;
    }
}

/// The status after a message says `incoming`. A message that would take the call backwards is stale: a
/// finished call does not start again, and what the client knows by itself (the person refused, the turn was
/// cancelled, the agent waits for an answer) yields only to a real end.
fn merged_status(current: ToolStatus, incoming: ToolStatus) -> ToolStatus {
    use ToolStatus::{Failed, InProgress, Pending, Rejected, WaitingForConfirmation};
    let is_backwards = match incoming {
        Pending => current != Pending,
        InProgress => current.is_finished() || current == WaitingForConfirmation,
        // The agent reports a call the person refused as failed; "refused" says more.
        Failed => current == Rejected,
        _ => false,
    };
    if is_backwards {
        current
    } else {
        incoming
    }
}

/// A call that was about to run is running.
fn started(status: ToolStatus) -> ToolStatus {
    match status {
        ToolStatus::Pending | ToolStatus::WaitingForConfirmation => ToolStatus::InProgress,
        other => other,
    }
}

fn after_answer(status: ToolStatus, outcome: PermissionOutcome) -> ToolStatus {
    if status != ToolStatus::WaitingForConfirmation {
        return status;
    }
    match outcome {
        PermissionOutcome::Selected(kind) if kind.is_allow() => ToolStatus::InProgress,
        PermissionOutcome::Selected(_) => ToolStatus::Rejected,
        PermissionOutcome::Cancelled => ToolStatus::Canceled,
        PermissionOutcome::Waiting => status,
    }
}
