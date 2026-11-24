use super::snapshot::Action;
use super::unreadable_answer;
use crate::error::BridgeResult;
use crate::said::Said;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const COLLECTION: &str = r#"["tracks"]"#;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Tracks {
    #[default]
    Unsupported,
    Unavailable(String),
    Available {
        rows: Vec<Track>,
        actions: Vec<Action>,
    },
}

impl Tracks {
    pub(super) fn parse(document: &Value) -> BridgeResult<Self> {
        let decode_error = |_| unreadable_answer();
        let actions = match document.get("track_actions") {
            Some(value) => Vec::deserialize(value).map_err(decode_error)?,
            None => Vec::new(),
        };
        let problem = document
            .get("track_problem")
            .map(|value| String::deserialize(value).map_err(decode_error))
            .transpose()?;
        match document.get("tracks") {
            None => Ok(Self::Unsupported),
            Some(Value::Null) => problem.map(Self::Unavailable).ok_or_else(unreadable_answer),
            Some(value) => Ok(Self::Available {
                rows: Vec::deserialize(value).map_err(decode_error)?,
                actions,
            }),
        }
    }

    pub fn rows(&self) -> &[Track] {
        match self {
            Self::Available { rows, .. } => rows,
            _ => &[],
        }
    }

    pub fn action(&self, key: &str) -> Option<&Action> {
        match self {
            Self::Available { actions, .. } => actions.iter().find(|action| action.key == key),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub summary: String,
    pub status: TrackStatus,
    pub updated_at: String,
    pub projects: Vec<TrackProject>,
    pub lanes: Vec<TrackLane>,
    pub conversations: Vec<TrackConversation>,
    pub decisions: Vec<TrackDecision>,
    pub actions: Vec<Action>,
}

impl Track {
    pub fn target(&self) -> String {
        serde_json::json!(["track", self.id]).to_string()
    }

    pub fn action(&self, key: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.key == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TrackRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackStatus {
    Open,
    Paused,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrackProject {
    pub id: String,
    pub folder: String,
    pub name: String,
    pub folder_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrackLane {
    pub project_id: String,
    pub task: String,
    pub key: Option<[String; 3]>,
    pub title: String,
    pub state: TrackLaneState,
    pub folder_missing: bool,
    pub gone: bool,
    pub did_not_start: bool,
    pub startop: Option<String>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackLaneState {
    Live,
    Finished,
    Unreadable,
    Gone,
    Pending,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrackConversation {
    pub key: ConversationKey,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConversationKey {
    pub provider: String,
    pub account: String,
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TrackDecision {
    pub id: u64,
    pub text: String,
    pub state: TrackDecisionState,
    pub answer: Option<String>,
    pub settled_at: Option<String>,
    pub wait_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackDecisionState {
    Open,
    Settled,
}

pub fn refusal(code: Option<&str>, args: Option<&BTreeMap<String, Value>>, reason: &str) -> Said {
    let key = match code {
        Some("invalid_input") => "track.reason.invalid_input",
        Some("forbidden") => "track.reason.forbidden",
        Some("operation_conflict") => "track.reason.operation_conflict",
        Some("not_found") => "track.reason.not_found",
        Some("name_in_use") => "track.reason.name_in_use",
        Some("closed") => "track.reason.closed",
        Some("open_lanes") => return open_lanes_refusal(args),
        Some("decision_settled") => "track.reason.decision_settled",
        Some("lane_unavailable") => "track.reason.lane_unavailable",
        _ if reason.trim().is_empty() => "track.reason.refused",
        _ => return Said::new("track.reason.engine").text("reason", reason),
    };
    Said::new(key)
}

fn open_lanes_refusal(args: Option<&BTreeMap<String, Value>>) -> Said {
    let labels: Vec<_> = args
        .and_then(|args| args.get("labels"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|label| !label.trim().is_empty())
        .collect();
    if labels.is_empty() {
        Said::new("track.reason.open_lanes")
    } else {
        Said::new("track.reason.named_open_lanes").text("lanes", labels.join(", "))
    }
}
