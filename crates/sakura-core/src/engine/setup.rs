use super::accounts::{AccountRef, ProviderConfig, Readiness};
use super::cli::Act;
use super::snapshot::Action;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The supported programs and login folders reported by setup detection.
#[derive(Debug, Deserialize)]
pub struct SetupDetection {
    pub harnesses: Vec<DetectedHarness>,
    pub logins: Vec<DetectedLogin>,
}

/// One program checked by the engine.
#[derive(Debug, Deserialize)]
pub struct DetectedHarness {
    pub id: String,
    pub program_path: Option<String>,
    pub version: Option<String>,
    pub found: bool,
    pub label: Option<String>,
    pub adapter_path: Option<String>,
    pub adapter_version: Option<String>,
    pub readiness: Option<Readiness>,
    pub reason: Option<String>,
}

/// One login folder checked by the engine.
#[derive(Debug, Deserialize)]
pub struct DetectedLogin {
    pub harness: String,
    pub folder: String,
    #[serde(default, deserialize_with = "crate::models::null_as_false")]
    pub logged_in: bool,
    pub probe_error: Option<String>,
    pub in_book_as: Option<String>,
    pub proposed_name: String,
    pub readiness: Option<Readiness>,
    pub reason: Option<String>,
    pub registered_account: Option<AccountRef>,
}

/// The setup choices sent to the engine.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SetupChoices {
    pub accounts: Vec<SetupAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_master",
        skip_serializing_if = "Option::is_none"
    )]
    pub master_account: Option<Option<SetupMaster>>,
}

fn present_master<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> Result<Option<Option<SetupMaster>>, D::Error> {
    Option::deserialize(reader).map(Some)
}

/// A registered account or a folder checked in the same operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SetupMaster {
    Registered(AccountRef),
    Selected { harness: String, folder: String },
}

/// A login to register using the chosen name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SetupAccount {
    pub harness: String,
    pub folder: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_config: Option<ProviderConfig>,
}

impl SetupChoices {
    /// The idempotent cockpit action for the approved choices.
    pub fn act(&self, op: &str) -> Act {
        Act {
            action: Action {
                key: "setupapply".into(),
                label: String::new(),
                needs: vec!["op".into(), "choices".into()],
                optional: Vec::new(),
                confirm: false,
            },
            target: r#"["host","engine"]"#.into(),
            inputs: BTreeMap::from([
                ("op".into(), op.into()),
                (
                    "choices".into(),
                    serde_json::to_string(self).expect("setup choices are strings"),
                ),
            ]),
            confirm: None,
        }
    }
}

/// A book account returned for a chosen login folder.
#[derive(Debug, Deserialize)]
pub struct AppliedAccount {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub folder: String,
}

/// The accounts the engine found or registered while applying setup.
#[derive(Debug, Deserialize)]
pub struct SetupApplied {
    pub accounts: Vec<AppliedAccount>,
    pub master_account: Option<AccountRef>,
}

#[cfg(test)]
mod tests {
    use super::{DetectedLogin, SetupChoices};

    #[test]
    fn setup_retry_preserves_the_selected_goose_provider_and_master() {
        let saved = serde_json::json!({
            "accounts": [{
                "harness": "goose", "folder": "/fixture/.goose-local", "name": "Local models",
                "provider_config": {"provider": "ollama"}
            }],
            "master_account": {"harness": "goose", "folder": "/fixture/.goose-local"}
        });
        let choices: SetupChoices = serde_json::from_value(saved.clone()).unwrap();
        let retry = choices.act("same-operation");
        assert_eq!(retry.inputs["op"], "same-operation");
        let sent: serde_json::Value = serde_json::from_str(&retry.inputs["choices"]).unwrap();
        assert_eq!(sent, saved);
    }

    #[test]
    fn setup_distinguishes_clearing_the_default_from_leaving_it_in_place() {
        for saved in [
            serde_json::json!({"accounts": []}),
            serde_json::json!({"accounts": [], "master_account": null}),
        ] {
            let choices: SetupChoices = serde_json::from_value(saved.clone()).unwrap();
            assert_eq!(serde_json::to_value(choices).unwrap(), saved);
        }
    }

    #[test]
    fn a_failed_login_probe_preserves_the_engine_reason() {
        let login: DetectedLogin = serde_json::from_str(
            r#"{
                "harness": "claude",
                "folder": "/fixture/.claude-uncheckable",
                "logged_in": null,
                "probe_error": "The program could not run because its interpreter is missing.",
                "in_book_as": null,
                "proposed_name": "Claude Uncheckable"
            }"#,
        )
        .unwrap();
        assert!(!login.logged_in);
        assert_eq!(
            login.probe_error.as_deref(),
            Some("The program could not run because its interpreter is missing.")
        );
    }
}
