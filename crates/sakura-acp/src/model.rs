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
    /// The name of the agent's tool (`Grep`, `Glob`, `Read`), as ACP's `name` gives it: what the raw
    /// input's fields mean depends on it.
    pub name: Option<String>,
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

/// What the agent's prompts take besides text, as `initialize` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PromptSupport {
    /// `image` content blocks, and images embedded as resources.
    pub has_images: bool,
    /// `resource` content blocks: a file's text or bytes sent inside the prompt.
    pub has_embedded_context: bool,
}

/// A slash command the agent announces: typed as `/name` at the start of a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    /// What the command takes after its name, when it takes something (`[file]`).
    pub hint: Option<String>,
}

/// One value a selector offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// A model: the reasoning efforts it accepts, when the agent lists them with the model (Codex does; Claude
    /// has a selector of its own for the effort of the session).
    pub efforts: Vec<Choice>,
    /// A model: the effort it runs at when none is chosen.
    pub default_effort: Option<String>,
    /// A model: the service tiers it offers besides the standard one (`priority` is "fast").
    pub tiers: Vec<Choice>,
    /// The agent keeps this value out of its own default list (a legacy model).
    pub is_hidden: bool,
}

/// A choice the session offers (its model, its mode): what can be picked and what is picked now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selector {
    /// What the agent calls the option: "Thought level", "Fast mode".
    pub name: String,
    /// A switch the agent offers as an on/off option: its choices are `false` and `true`.
    pub is_toggle: bool,
    /// The id of the session config option this is, when it is one: it is set with
    /// `session/set_config_option`. A mode of the session's own mode list has none and is set with
    /// `session/set_mode`.
    pub config_id: Option<String>,
    pub current: String,
    pub choices: Vec<Choice>,
}

impl Selector {
    /// Whether there is anything to choose between: a selector with one value is not shown.
    pub fn is_choosable(&self) -> bool {
        self.choices.len() > 1
    }

    /// The choice that `wanted` names: the one whose id is it, else the first whose id or name holds it as a
    /// word of its own (`haiku` for `claude-haiku-4-5` or "Haiku 4.5"); the case of the letters does not matter.
    pub fn matching(&self, wanted: &str) -> Option<&Choice> {
        let wanted = wanted.trim().to_lowercase();
        if wanted.is_empty() {
            return None;
        }
        let holds = |text: &str| {
            let text = text.to_lowercase();
            text.split(|c: char| !c.is_alphanumeric())
                .any(|word| word == wanted)
        };
        let exact = self
            .choices
            .iter()
            .find(|choice| choice.id.to_lowercase() == wanted);
        exact.or_else(|| {
            let found = self.choices.iter();
            found
                .into_iter()
                .find(|choice| holds(&choice.id) || holds(&choice.name))
        })
    }

    /// The name of the value picked now, or its id when the list does not hold it.
    pub fn current_name(&self) -> &str {
        let current = self.choices.iter().find(|choice| choice.id == self.current);
        current.map_or(self.current.as_str(), |choice| choice.name.as_str())
    }
}

/// What a session lets the person pick, as the agent last said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionChoices {
    pub modes: Option<Selector>,
    pub models: Option<Selector>,
    /// The reasoning effort of the session (the option of the thought-level category).
    pub efforts: Option<Selector>,
    /// Whether the launch left the effort to the session.
    pub uses_session_effort: Option<bool>,
    /// Every other select or switch the agent offers, by option id.
    pub others: Vec<Selector>,
}

impl SessionChoices {
    /// Takes in what the agent said now: a part it did not mention stays as it was.
    pub fn merge(&mut self, news: SessionChoices) {
        if news.modes.is_some() {
            self.modes = news.modes;
        }
        if news.models.is_some() {
            self.models = news.models;
        }
        if news.efforts.is_some() {
            self.efforts = news.efforts;
        }
        self.uses_session_effort = news.uses_session_effort.or(self.uses_session_effort);
        for other in news.others {
            match self
                .others
                .iter_mut()
                .find(|held| held.config_id == other.config_id)
            {
                Some(held) => *held = other,
                None => self.others.push(other),
            }
        }
    }

    /// The session is in the mode `id` now.
    pub fn set_mode(&mut self, id: &str) {
        if let Some(modes) = &mut self.modes {
            modes.current = id.to_string();
        }
    }
}

#[cfg(test)]
mod choices_tests {
    use super::*;

    fn selector(current: &str, ids: &[&str]) -> Selector {
        let choice = |id: &&str| Choice {
            id: id.to_string(),
            name: id.to_uppercase(),
            ..Choice::default()
        };
        Selector {
            config_id: None,
            current: current.to_string(),
            choices: ids.iter().map(choice).collect(),
            ..Selector::default()
        }
    }

    #[test]
    fn choices_merge_replaces_what_the_agent_said_and_keeps_what_it_did_not() {
        let mut held = SessionChoices {
            modes: Some(selector("a", &["a", "b"])),
            ..SessionChoices::default()
        };
        held.merge(SessionChoices {
            models: Some(selector("x", &["x", "y"])),
            ..SessionChoices::default()
        });
        assert_eq!(held.modes.as_ref().map(|m| m.current.as_str()), Some("a"));
        assert_eq!(held.models.as_ref().map(|m| m.current.as_str()), Some("x"));
        held.set_mode("b");
        assert_eq!(held.modes.as_ref().map(Selector::current_name), Some("B"));
    }

    fn named(pairs: &[(&str, &str)]) -> Selector {
        let choice = |(id, name): &(&str, &str)| Choice {
            id: id.to_string(),
            name: name.to_string(),
            ..Choice::default()
        };
        Selector {
            config_id: Some("model".to_string()),
            current: "default".to_string(),
            choices: pairs.iter().map(choice).collect(),
            ..Selector::default()
        }
    }

    #[test]
    fn a_model_is_found_by_its_id_first_and_then_by_a_word_of_its_id_or_name() {
        let models = named(&[
            ("default", "Default (recommended)"),
            ("opus[1m]", "Opus 5.5 1M"),
            ("opus", "Opus 5.5"),
            ("claude-haiku-4-5", "Haiku 4.5"),
        ]);
        let id = |wanted: &str| models.matching(wanted).map(|choice| choice.id.as_str());
        assert_eq!(
            id("opus"),
            Some("opus"),
            "the exact id wins over an earlier row"
        );
        assert_eq!(id("Haiku"), Some("claude-haiku-4-5"));
        assert_eq!(id("default"), Some("default"));
        assert_eq!(id("sonnet"), None);
        assert_eq!(id("hai"), None, "a part of a word is not the word");
        assert_eq!(id(""), None);
    }

    #[test]
    fn a_selector_with_one_value_is_not_choosable() {
        assert!(!selector("a", &["a"]).is_choosable());
        assert!(selector("a", &["a", "b"]).is_choosable());
    }
}
