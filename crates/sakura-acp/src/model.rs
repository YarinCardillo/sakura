//! The conversation's own types. The protocol's types stay behind `convert` and `connection`, so a new
//! version of the SDK changes two files and nothing the reducer, the policy or the window reads.

use sakura_core::patch::FilePatch;
use serde_json::Value;

/// The most text one tool call content block keeps; the rest is dropped and the block says so.
pub const MAX_TOOL_TEXT_BYTES: usize = 512 * 1024;

/// What a tool call does, as the protocol names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    Other,
}

/// Where a tool call stands. The first four are the protocol's; the others are the client's own knowledge
/// (it asked the person, the person refused, the turn was cancelled), in Zed's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
    /// The agent requested permission and the person has not answered.
    WaitingForConfirmation,
    /// The person refused.
    Rejected,
    /// The turn ended or was cancelled before the call finished.
    Canceled,
}

impl ToolStatus {
    /// Whether the call will not change by itself any more.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            ToolStatus::Completed
                | ToolStatus::Failed
                | ToolStatus::Rejected
                | ToolStatus::Canceled
        )
    }
}

/// A file a tool call touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: Option<u32>,
}

/// What a tool call shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContent {
    /// Text: a file's lines, a command's output, an error.
    Text { text: String, is_truncated: bool },
    /// A change to one file.
    Diff(DiffContent),
    /// A terminal the client would own. Sakura declares no terminal capability, so an agent should not send it.
    Terminal { id: String },
}

impl ToolContent {
    /// Text content, cut at [`MAX_TOOL_TEXT_BYTES`] on a character boundary.
    pub fn text(text: &str) -> Self {
        let mut end = text.len().min(MAX_TOOL_TEXT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        ToolContent::Text {
            text: text[..end].to_string(),
            is_truncated: end < text.len(),
        }
    }
}

/// A change to one file: the path as the agent gave it, and the change as hunks when it could be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffContent {
    pub path: String,
    /// None when the texts were too large to compare; the figures of `patch` are then missing too.
    pub patch: Option<FilePatch>,
}

/// The fields of a `tool_call` or a `tool_call_update`; absent means "unchanged".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCallFields {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub content: Option<Vec<ToolContent>>,
    pub locations: Option<Vec<Location>>,
    pub raw_input: Option<Value>,
}

impl ToolCallFields {
    /// Fields for the call `id` with nothing set.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
}

/// One step of the agent's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// What an option of a permission request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

impl OptionKind {
    pub fn is_allow(self) -> bool {
        matches!(self, OptionKind::AllowOnce | OptionKind::AllowAlways)
    }
}

/// One choice the agent offers with a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    /// The agent's label. Shown to the person, never used to decide.
    pub name: String,
    pub kind: OptionKind,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

/// One `session/update`, reduced to what the conversation shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    UserText(String),
    AgentText(String),
    Thought(String),
    /// A `tool_call`: the call exists, with these fields.
    ToolCall(ToolCallFields),
    /// A `tool_call_update`: a patch to a call, which may arrive before its `tool_call`.
    ToolCallUpdate(ToolCallFields),
    /// The whole plan, replacing the one before.
    Plan(Vec<PlanEntry>),
    /// Something the conversation does not show yet (commands, modes, usage).
    Ignored,
}
