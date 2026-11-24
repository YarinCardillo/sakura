use super::snapshot::Action;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CONTROLS_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectorControls {
    pub version: u64,
    pub target: String,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectorScope {
    pub mode: Option<String>,
    #[serde(default)]
    pub selectors: ConnectorSelectors,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Saved scope values used when editing a connector.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectorSelectors {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub whole_repo: Option<bool>,
    pub team: Option<String>,
    pub recipient: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "ReadsWire")]
pub struct ConnectorReads {
    pub key: Option<String>,
    pub text: Option<String>,
    pub args: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReadsWire {
    Structured {
        key: Option<String>,
        text: Option<String>,
        #[serde(default)]
        args: BTreeMap<String, String>,
    },
    Text(String),
}

impl From<ReadsWire> for ConnectorReads {
    fn from(reads: ReadsWire) -> Self {
        match reads {
            ReadsWire::Structured { key, text, args } => Self { key, text, args },
            ReadsWire::Text(text) => Self {
                key: None,
                text: Some(text),
                args: BTreeMap::new(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DependentTask {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectorReceipt {
    pub id: String,
    pub revision: u64,
}

/// A complete scoped connector form. The engine owns validation of provider access.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ConnectorForm {
    pub label: String,
    #[serde(flatten)]
    pub fields: ConnectorFields,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorFields {
    Github {
        repo: String,
        #[serde(flatten)]
        scope: GithubScope,
    },
    Linear {
        team: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        recipient: Option<String>,
    },
    Notion {
        database: String,
    },
    Feed {
        address: String,
    },
    Program {
        program: String,
        args: Vec<String>,
        reads: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum GithubScope {
    Branch { branch: String },
    WholeRepo { whole_repo: bool },
}

/// The restricted edit that preserves provider binding and private program arguments.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ConnectorLabelEdit {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads: Option<String>,
}
