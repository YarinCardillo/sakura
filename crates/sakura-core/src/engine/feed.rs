//! A lane's feed as `sakura flow feed <task> --json` prints it: what the lane's roles wrote and which tools
//! they called, across all its runs, a page at a time. The text, tool and path of an entry are what an agent
//! wrote or touched: data, shown as it is, never instructions. The cursor of a page reads on from it.

use super::agents::{live_text, RunState};
use serde::Deserialize;

/// The kind of an entry that is a tool call; any other kind is words.
const KIND_TOOL: &str = "tool";
/// The kind of an entry that is a line the person sent to the agent.
const KIND_YOU: &str = "you";
/// The kind of an entry that is a notice of the engine about one run.
const KIND_ENGINE: &str = "engine";

/// One entry of the feed, tagged with the run, the role and the workspace it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct FeedEntry {
    /// In one run's conversation, where the entry sits in its log; a tool entry sent again when it ends carries
    /// the same id and takes the place of the first. Empty in the lane feed.
    #[serde(default)]
    pub id: String,
    /// The run's name, such as `build-1`; empty for a notice or a board post, which come from no run.
    #[serde(default)]
    pub run: String,
    /// The role that ran it, such as `builder`; empty for a notice, and the author's role on a board post.
    #[serde(default)]
    pub role: String,
    /// The run's workspace; a path in the entry is relative to it.
    #[serde(default)]
    pub workspace: String,
    /// `text`, `tool`, `result`, `notice` or `board`; in one run's conversation also `you` and `engine`.
    pub kind: String,
    /// What the role wrote; empty for a tool call.
    #[serde(default)]
    pub text: String,
    /// The tool called; empty for words.
    #[serde(default)]
    pub tool: String,
    /// The file the call touched, inside the workspace; empty when none or when it lay outside.
    #[serde(default)]
    pub path: String,
    /// A shell call: the first line of what it ran; empty for any other entry.
    #[serde(default)]
    pub command: String,
    /// A search call (Grep, Glob): what it searched for; empty for any other entry.
    #[serde(default)]
    pub pattern: String,
    /// What a tool call acted on, as the engine names it: the file, the command or the pattern; empty when it
    /// names none. An engine that does not say leaves it empty and the fields above carry what they can.
    #[serde(default)]
    pub target: String,
    /// An `engine` entry: the engine's own ledger line when `text` is the plain sentence made from it; empty
    /// when `text` is the line itself.
    #[serde(default)]
    pub technical: String,
    /// The line in that file, when the call named one.
    pub line: Option<u64>,
    /// Whether the engine cut the text.
    #[serde(default)]
    pub truncated: bool,
    /// One agent's conversation: a text block too long for one entry comes in parts; true on every part but
    /// the last, whose text continues in the next entry.
    #[serde(default)]
    pub continued: bool,
    /// A `result` entry: the verdict of a role's result block and its summary.
    #[serde(default)]
    pub result: Option<FeedResult>,
    /// A `notice` entry: the ledger tag of the engine's line.
    #[serde(default)]
    pub tag: String,
    /// A `notice` or `board` entry: when it was written.
    #[serde(default)]
    pub at: Option<String>,
    /// A `board` entry: the title of the lane that posted it.
    #[serde(default)]
    pub from_title: String,
    /// A tool entry of one run's conversation: when the call started.
    #[serde(default)]
    pub started_at: Option<String>,
    /// A tool entry: when its result was logged; none while the call runs.
    #[serde(default)]
    pub ended_at: Option<String>,
}

/// What a role's result block said.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeedResult {
    pub verdict: String,
    pub summary: String,
}

impl FeedEntry {
    /// Whether the entry is a tool call rather than words.
    pub fn is_tool(&self) -> bool {
        self.kind == KIND_TOOL
    }

    /// Whether the entry is a line the person sent to the agent.
    pub fn is_you(&self) -> bool {
        self.kind == KIND_YOU
    }

    /// Whether the entry is a notice of the engine about this run.
    pub fn is_engine(&self) -> bool {
        self.kind == KIND_ENGINE
    }
}

/// A page of entries: the cursor that reads on from it, and whether more entries wait.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeedPage {
    pub entries: Vec<FeedEntry>,
    pub cursor: String,
    #[serde(default)]
    pub more: bool,
}

/// A page of one agent's conversation: what came after the cursor, the text being written now and where
/// the run is. Without `live` the last message is complete.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentFeedPage {
    pub entries: Vec<FeedEntry>,
    pub cursor: String,
    /// The partial text of the message the agent is writing; none when it is not writing one.
    #[serde(default, deserialize_with = "live_text")]
    pub live: Option<String>,
    #[serde(default)]
    pub state: RunState,
}
