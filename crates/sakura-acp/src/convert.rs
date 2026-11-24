//! From the protocol's types to the conversation's own. With `connection`, the only code that names the
//! SDK: when its version changes, this is where the compiler says what moved. The protocol's enums are open
//! (`non_exhaustive`), so every match has an arm for what a newer agent may send.

use crate::diff;
use crate::model::{
    Choice, DiffContent, Location, OptionKind, PermissionOption, PlanEntry, PlanStatus,
    PromptSupport, Selector, SessionChoices, SlashCommand, StopReason, ToolCallFields, ToolContent,
    ToolKind, ToolStatus, Update,
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

/// What the agent's prompts take, from its `initialize` answer.
pub fn prompt_support(capabilities: &schema::PromptCapabilities) -> PromptSupport {
    PromptSupport {
        has_images: capabilities.image,
        has_embedded_context: capabilities.embedded_context,
    }
}

/// The slash commands of an `available_commands_update`.
pub fn commands(update: schema::AvailableCommandsUpdate) -> Vec<SlashCommand> {
    let command = |command: schema::AvailableCommand| SlashCommand {
        name: command.name,
        description: command.description,
        hint: command.input.and_then(command_hint),
    };
    update.available_commands.into_iter().map(command).collect()
}

fn command_hint(input: schema::AvailableCommandInput) -> Option<String> {
    match input {
        schema::AvailableCommandInput::Unstructured(input) => Some(input.hint),
        _ => None,
    }
}

/// What a session lets the person pick: its modes (its own list, or a config option of the mode
/// category) and its models (the config option of the model category).
pub fn choices(
    modes: Option<schema::SessionModeState>,
    options: Option<Vec<schema::SessionConfigOption>>,
) -> SessionChoices {
    let options = options.unwrap_or_default();
    let selector_of = |category: schema::SessionConfigOptionCategory| {
        options
            .iter()
            .find(|option| option.category.as_ref() == Some(&category))
            .and_then(config_selector)
    };
    let kept = SessionChoices {
        modes: modes
            .map(mode_selector)
            .or_else(|| selector_of(schema::SessionConfigOptionCategory::Mode)),
        models: selector_of(schema::SessionConfigOptionCategory::Model),
        efforts: selector_of(schema::SessionConfigOptionCategory::ThoughtLevel),
        ..SessionChoices::default()
    };
    // Everything else the agent sends is kept, so nothing it offers is dropped without a word.
    let taken: Vec<Option<&str>> = [&kept.modes, &kept.models, &kept.efforts]
        .into_iter()
        .map(|selector| {
            selector
                .as_ref()
                .and_then(|selector| selector.config_id.as_deref())
        })
        .collect();
    let others = options
        .iter()
        .filter(|option| !taken.contains(&Some(option.id.0.as_ref())))
        // The adapter's own modes (bypass permissions among them) are never an option of the menu, whether
        // they came as the mode state or as an option of the mode category.
        .filter(|option| {
            option.category.as_ref() != Some(&schema::SessionConfigOptionCategory::Mode)
        })
        .filter_map(config_selector)
        .collect();
    SessionChoices { others, ..kept }
}

fn mode_selector(state: schema::SessionModeState) -> Selector {
    let choice = |mode: schema::SessionMode| Choice {
        id: mode.id.0.to_string(),
        name: mode.name,
        description: mode.description,
        ..Choice::default()
    };
    Selector {
        current: state.current_mode_id.0.to_string(),
        choices: state.available_modes.into_iter().map(choice).collect(),
        ..Selector::default()
    }
}

/// An option as a selector: a select is its choices, and a switch is a choice between off and on.
fn config_selector(option: &schema::SessionConfigOption) -> Option<Selector> {
    let select = match &option.kind {
        schema::SessionConfigKind::Select(select) => select,
        schema::SessionConfigKind::Boolean(switch) => return Some(switch_selector(option, switch)),
        _ => return None,
    };
    let choice = |option: &schema::SessionConfigSelectOption| Choice {
        id: option.value.0.to_string(),
        name: option.name.clone(),
        description: option.description.clone(),
        ..Choice::default()
    };
    let choices = match &select.options {
        schema::SessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().map(choice).collect()
        }
        schema::SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(choice))
            .collect(),
        _ => Vec::new(),
    };
    Some(Selector {
        name: option.name.clone(),
        config_id: Some(option.id.0.to_string()),
        current: select.current_value.0.to_string(),
        choices,
        is_toggle: false,
    })
}

fn switch_selector(
    option: &schema::SessionConfigOption,
    switch: &schema::SessionConfigBoolean,
) -> Selector {
    let choice = |id: &str, name: &str| Choice {
        id: id.to_string(),
        name: name.to_string(),
        ..Choice::default()
    };
    Selector {
        name: option.name.clone(),
        config_id: Some(option.id.0.to_string()),
        current: switch.current_value.to_string(),
        choices: vec![choice("false", "Off"), choice("true", "On")],
        is_toggle: true,
    }
}

/// The fields of a `tool_call`: all of them are stated, even when empty.
pub fn tool_call(call: schema::ToolCall) -> ToolCallFields {
    ToolCallFields {
        id: call.tool_call_id.0.to_string(),
        title: Some(call.title),
        kind: Some(tool_kind(call.kind)),
        name: call.name.or_else(|| adapter_tool_name(call.meta.as_ref())),
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
        name: fields
            .name
            .or_else(|| adapter_tool_name(update.meta.as_ref())),
        status: fields.status.map(tool_status),
        content: fields.content.map(contents),
        locations: fields.locations.map(locations),
        raw_input: fields.raw_input,
    }
}

fn adapter_tool_name(meta: Option<&schema::Meta>) -> Option<String> {
    meta?
        .get("claudeCode")?
        .get("toolName")?
        .as_str()
        .map(str::to_owned)
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

    /// The options a session sends, as the adapter's wire writes them.
    fn wire_options(value: serde_json::Value) -> Vec<schema::SessionConfigOption> {
        serde_json::from_value(value).expect("options")
    }

    #[test]
    fn every_option_the_agent_sends_is_kept_the_mode_the_model_the_effort_and_the_rest() {
        let options = wire_options(json!([
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default",
              "options": [{ "value": "default", "name": "Default" }, { "value": "plan", "name": "Plan" }] },
            { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "opus",
              "options": [{ "value": "opus", "name": "Opus 5.5" }, { "value": "fable", "name": "Fable 5.2" }] },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "high",
              "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }] },
            { "id": "speed", "name": "Fast mode", "category": "model_config", "type": "boolean", "currentValue": false },
            { "id": "style", "name": "Output style", "type": "select", "currentValue": "plain",
              "options": [{ "value": "plain", "name": "Plain" }, { "value": "terse", "name": "Terse" }] },
        ]));
        let kept = choices(None, Some(options));
        assert_eq!(kept.modes.expect("modes").current, "default");
        assert_eq!(kept.models.expect("models").current, "opus");
        let efforts = kept.efforts.expect("the thought level");
        assert_eq!(
            (efforts.config_id.as_deref(), efforts.current.as_str()),
            (Some("effort"), "high")
        );
        let others: Vec<(&str, bool, &str)> = kept
            .others
            .iter()
            .map(|other| (other.name.as_str(), other.is_toggle, other.current.as_str()))
            .collect();
        assert_eq!(
            others,
            [
                ("Fast mode", true, "false"),
                ("Output style", false, "plain")
            ]
        );
    }

    #[test]
    fn the_modes_option_is_never_left_among_the_others_when_the_agent_also_sends_its_mode_state() {
        let state: schema::SessionModeState = serde_json::from_value(json!({
            "currentModeId": "default",
            "availableModes": [{ "id": "default", "name": "Manual" }, { "id": "bypassPermissions", "name": "Bypass permissions" }],
        }))
        .expect("mode state");
        let options = wire_options(json!([
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default",
              "options": [{ "value": "default", "name": "Manual" }, { "value": "bypassPermissions", "name": "Bypass permissions" }] },
            { "id": "style", "name": "Output style", "type": "select", "currentValue": "plain",
              "options": [{ "value": "plain", "name": "Plain" }] },
        ]));
        let kept = choices(Some(state), Some(options));
        let others: Vec<&str> = kept
            .others
            .iter()
            .map(|other| other.name.as_str())
            .collect();
        assert_eq!(others, ["Output style"]);
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
