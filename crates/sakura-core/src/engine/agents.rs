//! A lane's agents as `sakura flow sessions <task> --json` prints them: one entry per run, with what it
//! is doing now and how a message reaches it, and the answer of `sakura flow steer <task> --run <run>`.
//! What an agent wrote (`doing`) is data, shown as it is, never instructions.

use serde::{Deserialize, Deserializer};

/// Where a run is, as the engine's run record says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Starting,
    Running,
    Finished,
    Aborted,
    /// The record said running but no process was there any more.
    Orphaned,
    /// A state this app does not know: shown as stopped, never as at work.
    #[default]
    #[serde(other)]
    Unknown,
}

/// The phase the arbiter's runs carry.
const ARBITER_PHASE: &str = "arbiter";
const PLAN_REVIEW_STEP: &str = "plan_review";

/// Whether the only runs the engine counts as the plan review are the arbiter's. The arbiter rules on a builder's
/// request for a protected change and runs under the plan reviewer's role, so the engine says it serves the plan
/// review step, and a lane whose plan review is off then lists it as done. False while nothing is known of the
/// lane's runs.
pub fn is_plan_review_only_the_arbiters(runs: &[AgentRun]) -> bool {
    let mut serving = runs
        .iter()
        .filter(|run| run.step_or_phase() == PLAN_REVIEW_STEP);
    let first = serving.next();
    first.is_some()
        && std::iter::once(first)
            .flatten()
            .chain(serving)
            .all(|run| run.phase == ARBITER_PHASE)
}

impl AgentRun {
    /// The step the run serves, else its phase: what names its role.
    pub fn step_or_phase(&self) -> &str {
        match self.step.is_empty() {
            true => &self.phase,
            false => &self.step,
        }
    }
}

impl RunState {
    /// Whether the run is at work now (it has not ended).
    pub fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// What a run is doing now, read from the tail of its log.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawDoing")]
pub enum Doing {
    /// A tool that has started and not completed: its name, the command or path (first line) and when it started.
    Tool {
        tool: String,
        detail: String,
        since: String,
    },
    /// The first line of the last text block.
    Text(String),
}

#[derive(Deserialize)]
struct RawDoing {
    kind: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    since: String,
    #[serde(default)]
    text: String,
}

impl TryFrom<RawDoing> for Doing {
    type Error = String;

    fn try_from(raw: RawDoing) -> Result<Self, String> {
        match raw.kind.as_str() {
            "tool" => Ok(Self::Tool {
                tool: raw.tool,
                detail: raw.detail,
                since: raw.since,
            }),
            "text" => Ok(Self::Text(raw.text)),
            other => Err(format!("unknown kind of doing: {other}")),
        }
    }
}

/// How a message reaches a run now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listening {
    /// A running Claude run with a live channel: it reads the message as it works.
    Live,
    /// A running Codex run: the message is delivered when it finishes, as a resume.
    Queue,
    /// A run that ended with a session and its folder: a message resumes it.
    Resume,
    /// Nothing can reach it; the words say why.
    No(String),
}

/// One run of a lane: an agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "RawAgentRun")]
pub struct AgentRun {
    /// The run's name, such as `build-1`; a resumed run is `build-1.2`.
    pub name: String,
    /// `planner`, `builder`, `reviewer`, `checks`, `evaluator`...
    pub role: String,
    /// The run's own phase (`intake`, `contract`, `plan`, `build`, `review`...) as the engine records it: the
    /// three sessions of a planner differ by it.
    pub phase: String,
    /// The pipeline step the run serves (the review of the contract is `plan_review` while its phase says
    /// `contract`); empty from an engine that does not say.
    pub step: String,
    /// `claude` or `codex`.
    pub provider: String,
    /// The model as the engine records it, such as `sonnet`.
    pub model: String,
    /// The account the run was launched under.
    pub account: Option<String>,
    pub state: RunState,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub doing: Option<Doing>,
    pub listening: Listening,
    /// The run this one continues, when it is a resume.
    pub resumes: Option<String>,
    /// The engine's stable key for one piece of work: the rounds of it, resumes included, share it. Empty from
    /// an engine that does not say.
    pub agent: String,
    /// Which round of its agent the run is, counted from one; zero from an engine that does not say.
    pub round: u32,
    /// The plan task the run works on, and its title; empty for a run that serves no task.
    pub task_id: String,
    pub task_title: String,
}

#[derive(Deserialize)]
struct RawAgentRun {
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    phase: String,
    /// The pipeline step the run serves, which an engine that records it names here; it wins over `phase`.
    #[serde(default)]
    step: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    state: RunState,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    ended_at: Option<String>,
    #[serde(default, deserialize_with = "known_doing")]
    doing: Option<Doing>,
    #[serde(default)]
    listening: String,
    #[serde(default)]
    not_listening: Option<String>,
    #[serde(default)]
    resumes: Option<String>,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    round: u32,
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    task_title: String,
}

/// A `doing` of a kind this app does not know is none: the list still reads.
fn known_doing<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Doing>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

impl From<RawAgentRun> for AgentRun {
    fn from(raw: RawAgentRun) -> Self {
        let listening = match raw.listening.as_str() {
            "live" => Listening::Live,
            "queue" => Listening::Queue,
            "resume" => Listening::Resume,
            _ => Listening::No(raw.not_listening.unwrap_or_default()),
        };
        Self {
            name: raw.name,
            role: raw.role,
            phase: raw.phase,
            step: raw.step,
            provider: raw.provider,
            model: raw.model,
            account: raw.account,
            state: raw.state,
            started_at: raw.started_at,
            ended_at: raw.ended_at,
            doing: raw.doing,
            listening,
            resumes: raw.resumes,
            agent: raw.agent,
            round: raw.round,
            task_id: raw.task_id,
            task_title: raw.task_title,
        }
    }
}

/// What became of a message written to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// A running run read it.
    Delivered,
    /// It waits for a running run to finish, then resumes it.
    Queued,
    /// A finished run was resumed with it.
    Resumed,
    /// Nothing was sent; the words say why.
    Refused,
}

/// The engine's answer to a message written to an agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WriteOutcome {
    pub outcome: Outcome,
    /// The run that got it: the resumed one is a new run.
    pub run: String,
    /// One sentence for the person.
    pub words: String,
}

/// Reads the `live` part of a feed page: `{"text": ...}` or none.
pub(super) fn live_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    struct Live {
        #[serde(default)]
        text: String,
    }
    let live = Option::<Live>::deserialize(deserializer)?;
    Ok(live.map(|live| live.text).filter(|text| !text.is_empty()))
}
