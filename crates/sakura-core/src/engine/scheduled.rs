use super::snapshot::Action;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Scheduled definitions and actions reported by this engine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Scheduled {
    #[serde(default)]
    pub tasks: Vec<ScheduledTask>,
    #[serde(default)]
    pub templates: Vec<Template>,
    pub target: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub revision: String,
    #[serde(flatten)]
    pub definition: Definition,
    pub state: String,
    pub reason: Option<String>,
    pub connector_label: Option<String>,
    pub next_at: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// The complete definition sent on create and update, independent of snapshot metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Definition {
    pub title: String,
    pub prompt: String,
    pub connector_ids: Vec<String>,
    pub when: Trigger,
    pub project: Option<String>,
    pub preset: Option<String>,
    pub account: Option<String>,
    #[serde(default)]
    pub may_start_lanes: bool,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    Daily {
        time: String,
        timezone: String,
    },
    Interval {
        minutes: u16,
    },
    ConnectorEvent {
        connector_id: String,
        event_kind: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Template {
    pub id: String,
    pub labels: BTreeMap<String, String>,
    pub definitions: BTreeMap<String, Definition>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct HistoryPage {
    #[serde(default)]
    pub runs: Vec<Run>,
    pub before: Option<u64>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Run {
    pub id: String,
    pub task_id: Option<String>,
    pub sequence: Option<u64>,
    pub title: Option<String>,
    pub trigger: Option<RunTrigger>,
    pub state: String,
    pub reason: Option<String>,
    pub queued_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunTrigger {
    pub kind: String,
    pub at: Option<String>,
    pub connector_id: Option<String>,
    pub revision: Option<String>,
    pub event_kind: Option<String>,
    pub occurrence: Option<String>,
    pub item_id: Option<String>,
    pub occurred_at: Option<String>,
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ConversationPage {
    #[serde(default)]
    pub entries: Vec<ConversationEntry>,
    pub after: Option<u64>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConversationEntry {
    pub sequence: u64,
    pub kind: String,
    pub text: String,
}
