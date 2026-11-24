//! The items of a Codex thread as the conversation shows them: what the person wrote, what the agent
//! answered and thought, and each tool call as one line with its result as text. The items are read as
//! plain JSON, field by field, so a field a later Codex adds or drops changes one line here and nothing
//! the reducer or the window reads. A tool's output is the tool's own text: it is kept as text and never
//! read for instructions.

use crate::model::{Location, ToolCallFields, ToolContent, ToolKind, ToolStatus, Update};
use serde_json::{json, Value};

/// The title of a call of one of the master's own tools, as the Claude adapter names it, so that the
/// window knows the engine's tools by the same words whichever agent called them.
pub(super) fn engine_tool_title(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// The text of `value` at `field`, when there is one.
fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

/// What the person wrote in a `userMessage`: its text parts, one after the other.
fn user_text(item: &Value) -> String {
    let parts = item.get("content").and_then(Value::as_array);
    let texts = parts
        .into_iter()
        .flatten()
        .filter_map(|part| text(part, "text"));
    texts.collect::<Vec<_>>().join("\n")
}

/// What the agent thought, as its summary paragraphs.
pub(super) fn reasoning_text(item: &Value) -> String {
    let parts = item.get("summary").and_then(Value::as_array);
    let paragraphs = parts.into_iter().flatten().filter_map(Value::as_str);
    paragraphs.collect::<Vec<_>>().join("\n\n")
}

/// Where a call stands, from the item's `status`.
fn status(item: &Value) -> ToolStatus {
    match text(item, "status") {
        Some("completed") => match item.get("exitCode").and_then(Value::as_i64) {
            Some(code) if code != 0 => ToolStatus::Failed,
            _ => ToolStatus::Completed,
        },
        Some("failed") => ToolStatus::Failed,
        Some("declined") => ToolStatus::Rejected,
        _ => ToolStatus::InProgress,
    }
}

/// The text a finished MCP call returned: the text parts of its content, or its error's message.
fn mcp_output(item: &Value) -> Option<String> {
    if let Some(message) = item.get("error").and_then(|error| text(error, "message")) {
        return Some(message.to_string());
    }
    let parts = item.get("result")?.get("content")?.as_array()?;
    let texts: Vec<&str> = parts.iter().filter_map(|part| text(part, "text")).collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

fn text_content(output: Option<String>) -> Option<Vec<ToolContent>> {
    let output = output.filter(|output| !output.is_empty())?;
    Some(vec![ToolContent::text(&output)])
}

fn command_call(item: &Value, id: &str) -> ToolCallFields {
    let command = text(item, "command").unwrap_or_default();
    let output = text(item, "aggregatedOutput").map(str::to_string);
    ToolCallFields {
        title: Some(command.to_string()),
        kind: Some(ToolKind::Execute),
        status: Some(status(item)),
        content: text_content(output),
        raw_input: Some(json!({ "command": command })),
        ..ToolCallFields::new(id)
    }
}

pub(super) fn changed_paths(item: &Value) -> Vec<String> {
    let changes = item.get("changes").and_then(Value::as_array);
    let paths = changes
        .into_iter()
        .flatten()
        .filter_map(|change| text(change, "path"));
    paths.map(str::to_string).collect()
}

/// One change of a patch: the file, what is done to it (`add`, `delete`, `update`) and where it moves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Change {
    pub path: String,
    pub kind: &'static str,
    pub move_to: Option<String>,
}

/// The changes of a `fileChange` item; none when one has no path or a kind this client does not know, so
/// that a patch it cannot read whole is never judged by the part it can.
pub(super) fn changes(item: &Value) -> Option<Vec<Change>> {
    let changes = item.get("changes")?.as_array()?;
    changes.iter().map(change_of).collect()
}

fn change_of(change: &Value) -> Option<Change> {
    let path = text(change, "path")?.to_string();
    let kind = change.get("kind")?;
    let kind_name = match text(kind, "type")? {
        "add" => "add",
        "delete" => "delete",
        "update" => "update",
        _ => return None,
    };
    let move_to = match kind.get("move_path") {
        None | Some(Value::Null) => None,
        Some(Value::String(to)) => Some(to.clone()),
        Some(_) => return None,
    };
    Some(Change {
        path,
        kind: kind_name,
        move_to,
    })
}

fn file_change_call(item: &Value, id: &str) -> ToolCallFields {
    let paths = changed_paths(item);
    let locations = paths
        .iter()
        .map(|path| Location {
            path: path.clone(),
            line: None,
        })
        .collect();
    ToolCallFields {
        title: Some(format!("Edit {}", paths.join(", "))),
        kind: Some(ToolKind::Edit),
        status: Some(status(item)),
        locations: Some(locations),
        ..ToolCallFields::new(id)
    }
}

fn mcp_call(item: &Value, id: &str) -> ToolCallFields {
    let (server, tool) = (text(item, "server"), text(item, "tool"));
    let (server, tool) = (server.unwrap_or_default(), tool.unwrap_or_default());
    ToolCallFields {
        title: Some(engine_tool_title(server, tool)),
        kind: Some(ToolKind::Other),
        name: Some(tool.to_string()),
        status: Some(status(item)),
        content: text_content(mcp_output(item)),
        raw_input: item.get("arguments").cloned(),
        ..ToolCallFields::new(id)
    }
}

fn search_call(item: &Value, id: &str) -> ToolCallFields {
    let query = text(item, "query").filter(|query| !query.is_empty());
    ToolCallFields {
        title: Some(query.unwrap_or("Web search").to_string()),
        kind: Some(ToolKind::Search),
        status: Some(status(item)),
        ..ToolCallFields::new(id)
    }
}

/// The tool call `item` is, as far as it is known now; none for an item that is not a tool call.
pub(super) fn tool_call(item: &Value) -> Option<ToolCallFields> {
    let id = text(item, "id")?;
    match text(item, "type")? {
        "commandExecution" => Some(command_call(item, id)),
        "fileChange" => Some(file_change_call(item, id)),
        "mcpToolCall" => Some(mcp_call(item, id)),
        "webSearch" => Some(search_call(item, id)),
        _ => None,
    }
}

/// The updates a stored item replays as: the person's words, the agent's, its thoughts and its calls as
/// they ended.
pub(super) fn replayed(item: &Value) -> Vec<Update> {
    let kind = text(item, "type").unwrap_or_default();
    match kind {
        "userMessage" => vec![Update::UserText(user_text(item))],
        "agentMessage" => vec![Update::AgentText(
            text(item, "text").unwrap_or_default().into(),
        )],
        "reasoning" => match reasoning_text(item) {
            thought if thought.is_empty() => Vec::new(),
            thought => vec![Update::Thought(thought)],
        },
        _ => tool_call(item).map(Update::ToolCall).into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_one_execute_line_with_its_output_as_text() {
        let item = json!({ "type": "commandExecution", "id": "c1", "command": "ls -la",
            "status": "completed", "exitCode": 0, "aggregatedOutput": "a\nb" });
        let call = tool_call(&item).expect("a call");
        assert_eq!(call.title.as_deref(), Some("ls -la"));
        assert_eq!(call.kind, Some(ToolKind::Execute));
        assert_eq!(call.status, Some(ToolStatus::Completed));
        assert_eq!(call.raw_input, Some(json!({ "command": "ls -la" })));
        assert_eq!(call.content, Some(vec![ToolContent::text("a\nb")]));
    }

    #[test]
    fn a_command_that_exited_with_an_error_or_was_declined_says_so() {
        let failed = json!({ "type": "commandExecution", "id": "c", "command": "false",
            "status": "completed", "exitCode": 1 });
        assert_eq!(
            tool_call(&failed).and_then(|call| call.status),
            Some(ToolStatus::Failed)
        );
        let declined = json!({ "type": "commandExecution", "id": "c", "command": "rm x",
            "status": "declined" });
        assert_eq!(
            tool_call(&declined).and_then(|call| call.status),
            Some(ToolStatus::Rejected)
        );
    }

    #[test]
    fn an_engine_tool_is_titled_as_the_adapter_titles_it_and_its_answer_is_text() {
        let item = json!({ "type": "mcpToolCall", "id": "m1", "server": "sakura-master",
            "tool": "lane_open", "status": "completed", "arguments": { "project": "demo" },
            "result": { "content": [{ "type": "text", "text": "{\"lane\":\"demo/x\"}" }] } });
        let call = tool_call(&item).expect("a call");
        assert_eq!(call.title.as_deref(), Some("mcp__sakura-master__lane_open"));
        assert_eq!(call.name.as_deref(), Some("lane_open"));
        assert_eq!(
            call.content,
            Some(vec![ToolContent::text("{\"lane\":\"demo/x\"}")])
        );
        assert_eq!(call.raw_input, Some(json!({ "project": "demo" })));
    }

    #[test]
    fn a_tool_call_that_failed_shows_its_error_message() {
        let item = json!({ "type": "mcpToolCall", "id": "m2", "server": "s", "tool": "t",
            "status": "failed", "arguments": {}, "error": { "message": "refused" } });
        let call = tool_call(&item).expect("a call");
        assert_eq!(call.status, Some(ToolStatus::Failed));
        assert_eq!(call.content, Some(vec![ToolContent::text("refused")]));
    }

    #[test]
    fn a_file_change_names_the_files_and_shows_no_diff() {
        let item = json!({ "type": "fileChange", "id": "f1", "status": "inProgress",
            "changes": [{ "path": "/w/a.rs", "kind": { "type": "add" }, "diff": "+x" },
                        { "path": "/w/b.rs", "kind": { "type": "delete" }, "diff": "" }] });
        let call = tool_call(&item).expect("a call");
        assert_eq!(call.title.as_deref(), Some("Edit /w/a.rs, /w/b.rs"));
        assert_eq!(call.kind, Some(ToolKind::Edit));
        assert_eq!(call.locations.map(|places| places.len()), Some(2));
        assert_eq!(call.content, None);
    }

    #[test]
    fn a_patch_is_read_whole_with_its_moves_or_not_at_all() {
        let item = json!({ "changes": [
            { "path": "/w/a", "kind": { "type": "add" } },
            { "path": "/w/b", "kind": { "type": "update", "move_path": "/w/c" } }] });
        let read = changes(&item).expect("readable");
        assert_eq!(
            read[0],
            Change {
                path: "/w/a".into(),
                kind: "add",
                move_to: None
            }
        );
        assert_eq!(read[1].move_to.as_deref(), Some("/w/c"));
        for unreadable in [
            json!({ "changes": [{ "path": "/w/a", "kind": { "type": "chmod" } }] }),
            json!({ "changes": [{ "kind": { "type": "add" } }] }),
            json!({ "changes": [{ "path": "/w/a", "kind": { "type": "update", "move_path": 3 } }] }),
            json!({}),
        ] {
            assert_eq!(changes(&unreadable), None, "{unreadable}");
        }
    }

    #[test]
    fn items_that_are_not_tool_calls_are_not_calls() {
        for kind in [
            "agentMessage",
            "reasoning",
            "userMessage",
            "plan",
            "contextCompaction",
        ] {
            assert_eq!(
                tool_call(&json!({ "type": kind, "id": "x" })),
                None,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_stored_turn_replays_its_words_thoughts_and_calls_in_order() {
        let items = [
            json!({ "type": "userMessage", "id": "u", "content": [{ "type": "text", "text": "hi", "text_elements": [] }] }),
            json!({ "type": "reasoning", "id": "r", "summary": ["one", "two"], "content": [] }),
            json!({ "type": "commandExecution", "id": "c", "command": "ls", "status": "completed", "exitCode": 0 }),
            json!({ "type": "agentMessage", "id": "a", "text": "done" }),
        ];
        let updates: Vec<Update> = items.iter().flat_map(replayed).collect();
        assert_eq!(updates.len(), 4);
        assert_eq!(updates[0], Update::UserText("hi".into()));
        assert_eq!(updates[1], Update::Thought("one\n\ntwo".into()));
        assert!(matches!(updates[2], Update::ToolCall(_)));
        assert_eq!(updates[3], Update::AgentText("done".into()));
    }
}
