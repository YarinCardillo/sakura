//! From the protocol's types to the conversation's own. With `connection`, the only code that names the
//! SDK: when its version changes, this is where the compiler says what moved. The protocol's enums are open
//! (`non_exhaustive`), so every match has an arm for what a newer agent may send.

use crate::diff;
use crate::model::{
    DiffContent, Location, OptionKind, PermissionOption, PlanEntry, PlanStatus, StopReason,
    ToolCallFields, ToolContent, ToolKind, ToolStatus, Update,
};
use agent_client_protocol::v1::schema;

/// What stands for content the conversation cannot show as text.
const IMAGE_PLACEHOLDER: &str = "[image]";
const AUDIO_PLACEHOLDER: &str = "[audio]";
const RESOURCE_PLACEHOLDER: &str = "[resource]";

/// A session update as the conversation reads it.
pub fn update(update: schema::SessionUpdate) -> Update {
    match update {
        schema::SessionUpdate::UserMessageChunk(chunk) => Update::UserText(text(chunk.content)),
        schema::SessionUpdate::AgentMessageChunk(chunk) => Update::AgentText(text(chunk.content)),
        schema::SessionUpdate::AgentThoughtChunk(chunk) => Update::Thought(text(chunk.content)),
        schema::SessionUpdate::ToolCall(call) => Update::ToolCall(tool_call(call)),
        schema::SessionUpdate::ToolCallUpdate(update) => {
            Update::ToolCallUpdate(tool_call_update(update))
        }
        schema::SessionUpdate::Plan(plan) => Update::Plan(plan_entries(plan)),
        _ => Update::Ignored,
    }
}

/// The fields of a `tool_call`: all of them are stated, even when empty.
pub fn tool_call(call: schema::ToolCall) -> ToolCallFields {
    ToolCallFields {
        id: call.tool_call_id.0.to_string(),
        title: Some(call.title),
        kind: Some(tool_kind(call.kind)),
        status: Some(tool_status(call.status)),
        content: Some(contents(call.content)),
        locations: Some(locations(call.locations)),
        raw_input: call.raw_input,
    }
}

/// The fields of a `tool_call_update`: only the ones it carries.
pub fn tool_call_update(update: schema::ToolCallUpdate) -> ToolCallFields {
    let fields = update.fields;
    ToolCallFields {
        id: update.tool_call_id.0.to_string(),
        title: fields.title,
        kind: fields.kind.map(tool_kind),
        status: fields.status.map(tool_status),
        content: fields.content.map(contents),
        locations: fields.locations.map(locations),
        raw_input: fields.raw_input,
    }
}

/// The options of a permission request; one of a kind this client does not know is left out, so it can
/// never be chosen.
pub fn options(options: Vec<schema::PermissionOption>) -> Vec<PermissionOption> {
    let known = options.into_iter().filter_map(|option| {
        Some(PermissionOption {
            kind: option_kind(option.kind)?,
            id: option.option_id.0.to_string(),
            name: option.name,
        })
    });
    known.collect()
}

/// Why a turn ended; a reason this client does not know is an ordinary end.
pub fn stop_reason(reason: schema::StopReason) -> StopReason {
    match reason {
        schema::StopReason::MaxTokens => StopReason::MaxTokens,
        schema::StopReason::MaxTurnRequests => StopReason::MaxTurnRequests,
        schema::StopReason::Refusal => StopReason::Refusal,
        schema::StopReason::Cancelled => StopReason::Cancelled,
        _ => StopReason::EndTurn,
    }
}

fn text(block: schema::ContentBlock) -> String {
    match block {
        schema::ContentBlock::Text(text) => text.text,
        schema::ContentBlock::Image(_) => IMAGE_PLACEHOLDER.to_string(),
        schema::ContentBlock::Audio(_) => AUDIO_PLACEHOLDER.to_string(),
        schema::ContentBlock::ResourceLink(link) => format!("[{}]({})", link.name, link.uri),
        _ => RESOURCE_PLACEHOLDER.to_string(),
    }
}

fn contents(contents: Vec<schema::ToolCallContent>) -> Vec<ToolContent> {
    contents.into_iter().map(content).collect()
}

fn content(content: schema::ToolCallContent) -> ToolContent {
    match content {
        schema::ToolCallContent::Content(content) => ToolContent::text(&text(content.content)),
        schema::ToolCallContent::Diff(diff) => {
            let path = diff.path.to_string_lossy().into_owned();
            let patch = diff::file_patch(&path, diff.old_text.as_deref(), &diff.new_text);
            ToolContent::Diff(DiffContent { path, patch })
        }
        schema::ToolCallContent::Terminal(terminal) => ToolContent::Terminal {
            id: terminal.terminal_id.0.to_string(),
        },
        _ => ToolContent::text(RESOURCE_PLACEHOLDER),
    }
}

fn locations(locations: Vec<schema::ToolCallLocation>) -> Vec<Location> {
    let location = |location: schema::ToolCallLocation| Location {
        path: location.path.to_string_lossy().into_owned(),
        line: location.line,
    };
    locations.into_iter().map(location).collect()
}

fn plan_entries(plan: schema::Plan) -> Vec<PlanEntry> {
    let entry = |entry: schema::PlanEntry| PlanEntry {
        content: entry.content,
        status: match entry.status {
            schema::PlanEntryStatus::InProgress => PlanStatus::InProgress,
            schema::PlanEntryStatus::Completed => PlanStatus::Completed,
            _ => PlanStatus::Pending,
        },
    };
    plan.entries.into_iter().map(entry).collect()
}

fn tool_kind(kind: schema::ToolKind) -> ToolKind {
    match kind {
        schema::ToolKind::Read => ToolKind::Read,
        schema::ToolKind::Edit => ToolKind::Edit,
        schema::ToolKind::Delete => ToolKind::Delete,
        schema::ToolKind::Move => ToolKind::Move,
        schema::ToolKind::Search => ToolKind::Search,
        schema::ToolKind::Execute => ToolKind::Execute,
        schema::ToolKind::Think => ToolKind::Think,
        schema::ToolKind::Fetch => ToolKind::Fetch,
        schema::ToolKind::SwitchMode => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

fn tool_status(status: schema::ToolCallStatus) -> ToolStatus {
    match status {
        schema::ToolCallStatus::InProgress => ToolStatus::InProgress,
        schema::ToolCallStatus::Completed => ToolStatus::Completed,
        schema::ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Pending,
    }
}

fn option_kind(kind: schema::PermissionOptionKind) -> Option<OptionKind> {
    match kind {
        schema::PermissionOptionKind::AllowOnce => Some(OptionKind::AllowOnce),
        schema::PermissionOptionKind::AllowAlways => Some(OptionKind::AllowAlways),
        schema::PermissionOptionKind::RejectOnce => Some(OptionKind::RejectOnce),
        schema::PermissionOptionKind::RejectAlways => Some(OptionKind::RejectAlways),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A `session/update` payload as it comes off the wire.
    fn wire(update: serde_json::Value) -> Update {
        let parsed: schema::SessionUpdate =
            serde_json::from_value(update).expect("a session update");
        super::update(parsed)
    }

    #[test]
    fn update_message_chunks_carry_their_text_by_kind() {
        let chunk = |kind: &str| json!({ "sessionUpdate": kind, "content": { "type": "text", "text": "wave" } });
        assert_eq!(
            wire(chunk("user_message_chunk")),
            Update::UserText("wave".into())
        );
        assert_eq!(
            wire(chunk("agent_message_chunk")),
            Update::AgentText("wave".into())
        );
        assert_eq!(
            wire(chunk("agent_thought_chunk")),
            Update::Thought("wave".into())
        );
    }

    #[test]
    fn update_tool_call_states_every_field_and_an_update_only_what_it_carries() {
        let declared = wire(json!({
            "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read", "kind": "read",
            "locations": [{ "path": "/work/demo/a.rs", "line": 3 }],
        }));
        let Update::ToolCall(fields) = declared else {
            panic!("a tool call")
        };
        assert_eq!(fields.kind, Some(ToolKind::Read));
        assert_eq!(fields.status, Some(ToolStatus::Pending));
        assert_eq!(fields.content, Some(Vec::new()));
        let location = Location {
            path: "/work/demo/a.rs".into(),
            line: Some(3),
        };
        assert_eq!(fields.locations, Some(vec![location]));
        let patched = wire(
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }),
        );
        let Update::ToolCallUpdate(fields) = patched else {
            panic!("a tool call update")
        };
        assert_eq!(fields.status, Some(ToolStatus::Completed));
        assert_eq!(
            (fields.title, fields.kind, fields.content, fields.locations),
            (None, None, None, None)
        );
    }

    #[test]
    fn update_diff_content_becomes_hunks() {
        let update = wire(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "content": [{ "type": "diff", "path": "/work/demo/a.rs", "oldText": "a\n", "newText": "b\n" }],
        }));
        let Update::ToolCallUpdate(fields) = update else {
            panic!("a tool call update")
        };
        let Some(ToolContent::Diff(diff)) = fields.content.and_then(|mut content| content.pop())
        else {
            panic!("a diff");
        };
        let patch = diff.patch.expect("a patch");
        assert_eq!(
            (diff.path.as_str(), patch.added, patch.removed),
            ("/work/demo/a.rs", 1, 1)
        );
    }

    #[test]
    fn update_plan_carries_every_entry_with_its_status() {
        let entry = |status: &str| json!({ "content": "x", "priority": "low", "status": status });
        let plan = wire(json!({
            "sessionUpdate": "plan",
            "entries": [entry("pending"), entry("in_progress"), entry("completed")],
        }));
        let Update::Plan(entries) = plan else {
            panic!("a plan")
        };
        let statuses: Vec<PlanStatus> = entries.iter().map(|entry| entry.status).collect();
        assert_eq!(
            statuses,
            [
                PlanStatus::Pending,
                PlanStatus::InProgress,
                PlanStatus::Completed
            ]
        );
    }

    #[test]
    fn update_kinds_the_conversation_does_not_show_are_ignored() {
        let commands =
            json!({ "sessionUpdate": "available_commands_update", "availableCommands": [] });
        assert_eq!(wire(commands), Update::Ignored);
        let mode = json!({ "sessionUpdate": "current_mode_update", "currentModeId": "plan" });
        assert_eq!(wire(mode), Update::Ignored);
    }

    #[test]
    fn update_unknown_tool_kind_from_a_newer_agent_is_other() {
        let call = wire(
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t", "title": "x", "kind": "teleport" }),
        );
        let Update::ToolCall(fields) = call else {
            panic!("a tool call")
        };
        assert_eq!(fields.kind, Some(ToolKind::Other));
    }

    #[test]
    fn options_keep_the_agent_order_and_kinds() {
        let parsed: Vec<schema::PermissionOption> = serde_json::from_value(json!([
            { "optionId": "a", "name": "Allow", "kind": "allow_once" },
            { "optionId": "b", "name": "Never", "kind": "reject_always" },
        ]))
        .expect("options");
        let options = options(parsed);
        assert_eq!(options[0].kind, OptionKind::AllowOnce);
        assert_eq!(
            (options[1].id.as_str(), options[1].kind),
            ("b", OptionKind::RejectAlways)
        );
    }
}
