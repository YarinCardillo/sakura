//! The engine's master, as the app starts it: the launch spec `sakura master spec --json` prints and the
//! answer `sakura master decide --json` gives to one permission request. The engine writes both from its own
//! files and policy; the app reads them, passes the spec on to the adapter and acts on the answer, and decides
//! nothing itself.

use super::accounts::Readiness;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What a starter needs to run the master through the Agent Client Protocol.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MasterSpec {
    /// The adapter's command; empty when the host names none (`SAKURA_ACP_ADAPTER`).
    #[serde(default)]
    pub argv: Vec<String>,
    /// Environment variables the adapter is started with, on top of the app's own.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The master's working folder.
    #[serde(default)]
    pub cwd: PathBuf,
    /// The session options of the Claude adapter (`_meta.claudeCode.options`).
    #[serde(default)]
    pub options: serde_json::Value,
    /// The MCP servers of the session, in the protocol's shape.
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
    /// The mode set with `session/set_mode` before the first prompt.
    #[serde(default)]
    pub start_mode: String,
    /// Whether every session is closed with `session/close` when it is no longer needed.
    #[serde(default)]
    pub close_sessions: bool,
    /// Variables of the app's environment the adapter must not inherit.
    #[serde(default)]
    pub unset_env: Vec<String>,
    /// The absolute path of the interpreter the adapter runs under (`node`); empty when none was found.
    #[serde(default)]
    pub node: String,
    /// The adapter's real path.
    #[serde(default)]
    pub adapter: String,
    /// Why the engine found no adapter to start, in its words and with what to set; empty when it found one.
    #[serde(default)]
    pub adapter_problem: String,
    /// The PATH the adapter and the Claude it starts get: absolute folders that exist. An app opened from
    /// Finder has none of its own.
    #[serde(default)]
    pub path: String,
    /// The only variables of the app's environment the adapter inherits; it starts from nothing else.
    #[serde(default)]
    pub env_allow: Vec<String>,
    /// The engine's generated instructions for the master, which the adapter appends to its system prompt.
    #[serde(default)]
    pub system_prompt_append: String,
    /// The Leeway account the master runs on, its program (`claude` or `codex`) and its login folder; empty
    /// without one.
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub account_provider: String,
    #[serde(default)]
    pub account_folder: String,
    /// For a Codex account: the folder to start Codex with as its `CODEX_HOME`.
    #[serde(default)]
    pub codex_home: String,
    pub acp: Option<MasterAcp>,
}

/// The selected account and choices for one Goose Master slot.
#[derive(Debug, Clone)]
pub struct GooseMasterRequest {
    pub account: String,
    pub slot: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub is_fresh: bool,
}

/// The engine-owned operator descriptor for an ACP Master tab.
///
/// The engine also emits a binding-only descriptor (no `harness`, `slot` or `argv`) next to the legacy
/// launch fields while the unified launch is unavailable; those fields default to empty so the legacy
/// launch still parses, and the Goose starter refuses the empty values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MasterAcp {
    pub protocol_version: u32,
    #[serde(default)]
    pub harness: String,
    #[serde(default)]
    pub slot: String,
    pub readiness: Readiness,
    pub reason: Option<String>,
    pub launch_id: Option<String>,
    #[serde(default)]
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_allow: Vec<String>,
    pub binding: Option<MasterBinding>,
    pub start_choices: Option<MasterStartChoices>,
    pub conversation: Option<MasterConversation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MasterBinding {
    pub harness: String,
    pub account: String,
    pub context: String,
    pub storage_namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MasterStartChoices {
    pub model: String,
    pub effort: String,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MasterConversation {
    pub conversation_id: String,
    pub session_id: String,
    pub title: String,
    pub binding: MasterBinding,
}

impl MasterSpec {
    /// The model the engine's spec runs the master on (`options.model`, a name such as `haiku`); none when
    /// the spec names none.
    pub fn model(&self) -> Option<&str> {
        self.options
            .get("model")?
            .as_str()
            .filter(|name| !name.is_empty())
    }

    /// The default reasoning effort, when the engine includes it in the adapter options.
    pub fn effort(&self) -> Option<&str> {
        self.options
            .get("effort")?
            .as_str()
            .filter(|effort| !effort.is_empty())
    }

    /// The `_meta` of `session/new` and `session/load`: the options under the adapter's own name, and the
    /// generated instructions as the system prompt's append.
    pub fn session_meta(&self) -> serde_json::Value {
        serde_json::json!({
            "claudeCode": { "options": self.options },
            "systemPrompt": { "append": self.system_prompt_append },
        })
    }
}

/// What the engine decided for one permission request of the master.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// A rule of the master's settings allows it: it is answered without the person.
    Allow,
    /// A deny rule or the cage forbids it: it never reaches the person.
    Deny,
    /// No rule covers it: it is put to the person.
    Ask,
}

/// The engine's answer to a permission request: its decision, and why in plain words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Decided {
    pub decision: Decision,
    #[serde(default)]
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::MasterSpec;

    fn real_spec() -> MasterSpec {
        let text = include_str!("../../tests/fixtures/engine-real-master-spec.json");
        serde_json::from_str(text).expect("the engine's own spec")
    }

    #[test]
    fn an_explicit_goose_spec_can_omit_every_legacy_launch_field() {
        let spec: MasterSpec = serde_json::from_value(serde_json::json!({
            "acp": {
                "protocol_version": 1, "harness": "goose", "slot": "tab-1",
                "readiness": "unavailable",
                "reason": "goose_account_connection_check_required", "argv": []
            }
        }))
        .unwrap();
        assert!(spec.argv.is_empty());
    }

    #[test]
    fn a_claude_spec_with_the_engine_s_binding_only_acp_descriptor_still_parses() {
        let text = include_str!("../../tests/fixtures/engine-real-master-spec.json");
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        value["acp"] = serde_json::json!({
            "protocol_version": 1, "readiness": "unavailable", "reason": "release_proof_required",
            "binding": {"harness": "claude", "account": "claude-1", "context": "master",
                        "storage_namespace": "master:claude:0a26"}
        });
        let spec: MasterSpec =
            serde_json::from_value(value).expect("the engine's 0.4.0 spec shape");
        assert_eq!(spec.model(), Some("opus"));
        let acp = spec.acp.expect("the descriptor is kept");
        assert!(acp.harness.is_empty() && acp.slot.is_empty() && acp.argv.is_empty());
    }

    #[test]
    fn the_spec_s_model_is_the_one_its_options_name() {
        assert_eq!(real_spec().model(), Some("opus"));
    }

    #[test]
    fn a_spec_whose_options_name_no_model_has_none() {
        let mut spec = real_spec();
        spec.options["model"] = "".into();
        assert_eq!(spec.model(), None);
        spec.options = serde_json::json!({});
        assert_eq!(spec.model(), None);
    }
}
