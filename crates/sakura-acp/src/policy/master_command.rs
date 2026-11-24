use super::{command, lexer};
use crate::model::ToolKind;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub(crate) fn acp_request(
    mut wire: Value,
    workspace: &Path,
    kind: Option<ToolKind>,
) -> Option<Value> {
    let has_request_metadata = !is_empty_metadata(wire.pointer("/params/_meta"));
    let call = wire.pointer_mut("/params/toolCall")?;
    let name = call.get("name").and_then(Value::as_str);
    let adapter_name = call
        .pointer("/_meta/claudeCode/toolName")
        .and_then(Value::as_str);
    let input = &call["rawInput"];
    let is_bash = name == Some("Bash") || adapter_name == Some("Bash");
    if !is_bash && kind != Some(ToolKind::Execute) && input.get("command").is_none() {
        return Some(wire);
    }
    let has_conflicting_name = name
        .zip(adapter_name)
        .is_some_and(|(name, adapter)| name != adapter);
    let metadata = call.get("_meta");
    let is_known_metadata = is_empty_metadata(metadata)
        || metadata == Some(&json!({ "claudeCode": { "toolName": "Bash" } }));
    if !is_bash
        || has_conflicting_name
        || has_request_metadata
        || !is_known_metadata
        || !is_acp_input(input)
    {
        return None;
    }
    call["rawInput"] = approval_input(input, workspace)?;
    Some(wire)
}

fn is_empty_metadata(metadata: Option<&Value>) -> bool {
    metadata.is_none_or(|value| {
        value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty)
    })
}

fn is_acp_input(input: &Value) -> bool {
    input.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "command" | "cwd" | "commandActions" => true,
            "description" => value.is_string(),
            "timeout" => value.as_u64().is_some(),
            _ => false,
        })
    })
}

pub(crate) fn approval_input(input: &Value, workspace: &Path) -> Option<Value> {
    let actions = input.get("commandActions");
    if actions.is_some_and(|value| !value.is_null() && !value.as_array().is_some_and(Vec::is_empty))
    {
        return None;
    }
    let cwd = Path::new(input.get("cwd")?.as_str()?);
    let cwd = approval_cwd(cwd, workspace)?;
    let text = input.get("command")?.as_str()?;
    let command = plain_command(text, &cwd)?;
    let mut validated = input.clone();
    validated["command"] = json!(command);
    validated["cwd"] = json!(cwd.to_str()?);
    Some(validated)
}

fn approval_cwd(cwd: &Path, workspace: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() || cwd.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    let relative = cwd.strip_prefix(workspace).ok()?;
    let root = workspace.canonicalize().ok()?;
    let actual = cwd.canonicalize().ok()?;
    (actual == root.join(relative) && actual.is_dir()).then_some(actual)
}

fn plain_command(text: &str, cwd: &Path) -> Option<String> {
    // The engine matches literal word prefixes, so the forwarded text must have one spelling.
    let words = lexer::words(text)?;
    let is_plain = |c: char| c.is_ascii_alphanumeric() || "_-./:,+@%=".contains(c);
    if words
        .iter()
        .any(|word| word.is_empty() || !word.chars().all(is_plain))
    {
        return None;
    }
    let normalized = words.join(" ");
    command::reason_to_ask(&normalized, cwd)
        .is_none()
        .then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_command_requires_an_explicit_cwd_and_understood_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap();
        let request = |input, metadata| {
            let wire =
                json!({ "params": { "toolCall": { "rawInput": input, "_meta": metadata } } });
            acp_request(wire, &workspace, Some(ToolKind::Execute))
        };
        let bash = json!({ "claudeCode": { "toolName": "Bash" } });
        assert!(request(json!({"command": "git status"}), bash.clone()).is_none());
        let input = json!({ "command": "git status", "cwd": workspace });
        let accepted = request(input.clone(), bash).unwrap();
        assert_eq!(accepted.pointer("/params/toolCall/rawInput"), Some(&input));
        let unknown = json!({ "claudeCode": { "toolName": "Bash", "unknown": true } });
        assert!(request(input, unknown).is_none());
    }
}
