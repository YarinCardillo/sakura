//! What the master's conversation adds to the shared thread view: cards for what the master did (the lane
//! it started, the note it proposes), the project the person's messages are about, and the history of
//! earlier conversations. The view knows the engine only through [`MasterWorld`], so the boxes build, which
//! has no engine pages, still compiles this module and never builds the kit.

use crate::ui::kit::StateGlyph;
use gpui::{AnyElement, App, SharedString, Window};
use sakura_acp::model::{SessionChoices, ToolContent};
use sakura_acp::thread::ToolCall;
use serde_json::Value;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

/// The tool calls the master makes that are drawn as cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTool {
    LaneOpen,
    KnowledgePropose,
}

impl CardTool {
    /// The card tool a call is, by the title the adapter gives it.
    pub fn of(call: &ToolCall) -> Option<Self> {
        match super::copy::engine_tool_name(&call.title)? {
            "lane_open" => Some(Self::LaneOpen),
            "knowledge_propose" => Some(Self::KnowledgePropose),
            _ => None,
        }
    }
}

/// What a `lane_open` call answered. The answer is the agent's own text: it only ever names a lane, and
/// only an exact name the engine lists binds the card to it. Nothing else the call said is shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaneAsk {
    pub result: String,
}

/// What a `knowledge_propose` call answered; see [`LaneAsk`]. The note's path and text are never read from
/// the call: the card shows what the engine's own wait says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProposalAsk {
    pub result: String,
}

impl LaneAsk {
    /// Reads the call's answer.
    pub fn of(call: &ToolCall) -> Self {
        Self {
            result: answer(call),
        }
    }
}

impl ProposalAsk {
    /// Reads the call's answer.
    pub fn of(call: &ToolCall) -> Self {
        Self {
            result: answer(call),
        }
    }
}

/// What the tool answered: its text blocks, one after the other.
fn answer(call: &ToolCall) -> String {
    let texts = call.content.iter().filter_map(|content| match content {
        ToolContent::Text { text, .. } => Some(text.as_str()),
        _ => None,
    });
    texts.collect::<Vec<_>>().join("\n")
}

/// A lane as the card shows it, from the engine's own facts.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneCard {
    /// The lane's key, when the answer names exactly one lane the engine lists: a click opens it.
    pub key: Option<String>,
    pub title: String,
    pub project: String,
    /// The step the lane is at, in the engine's words.
    pub phase: String,
    pub progress: Option<(u32, u32)>,
    pub glyph: StateGlyph,
}

/// What a card knows of the note it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// The answer names exactly one proposal the engine waits on: its id, and what the engine says of it.
    Waiting {
        wait: String,
        title: String,
        detail: String,
    },
    /// No exact, single match: the card carries no answer to give, only a way to the Inbox.
    Unbound,
}

/// What the master's conversation asks of the window around it.
pub trait MasterWorld {
    /// The lane a `lane_open` call started.
    fn lane(&self, ask: &LaneAsk, cx: &App) -> LaneCard;
    /// Shows the lane `key` in the window.
    fn open_lane(&self, key: &str, window: &mut Window, cx: &mut App);
    /// Where the note a `knowledge_propose` call proposed stands.
    fn proposal(&self, ask: &ProposalAsk, cx: &App) -> Proposal;
    /// Shows the Inbox.
    fn open_inbox(&self, window: &mut Window, cx: &mut App);
    /// Approves or declines the note waiting under `wait`: the answer is held for a few seconds under an Undo
    /// toast, as in the Inbox, and then sent as the engine's own action, with its confirmation.
    fn decide(&self, wait: &str, approve: bool, window: &mut Window, cx: &mut App);
    /// What the answer under `wait` says while it is held for Undo, then what the engine said of it (its
    /// question to confirm, or its word).
    fn decision(&self, wait: &str, cx: &App) -> Option<AnyElement>;
    fn accounts(
        &self,
        _: &App,
    ) -> Option<(
        SharedString,
        super::agent_menu::ChipGroup,
        super::agent_menu::MenuChoice,
    )> {
        None
    }
    /// The names of the projects the person can speak about.
    fn projects(&self, cx: &App) -> Vec<String>;
}

/// The model, mode and effort a conversation starts with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartChoice {
    pub model: Option<String>,
    pub mode: Option<String>,
    pub effort: Option<String>,
    pub uses_session_effort: bool,
}

/// The choice shared by the composer and session launch.
#[derive(Debug, Clone, Default)]
pub struct SharedStart(Arc<Mutex<StartChoice>>);

impl SharedStart {
    pub fn get(&self) -> StartChoice {
        self.held().clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut StartChoice)) {
        change(&mut self.held());
    }

    fn held(&self) -> std::sync::MutexGuard<'_, StartChoice> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The master's additions to a conversation.
pub struct MasterKit {
    pub(super) world: Rc<dyn MasterWorld>,
    /// Asks the system to dictate into the focused field; says whether it started.
    pub(super) dictate: Rc<dyn Fn() -> bool>,
    /// The project the person's next messages are about, none for any.
    pub(super) project: Option<String>,
    /// The model and the mode chosen before the first message.
    pub(super) start: SharedStart,
    /// What the agent chip says and offers, given what the session lets the person pick (nothing before it
    /// has started).
    pub(super) menu: Rc<dyn Fn(&SessionChoices) -> AgentMenu>,
}

/// What the agent chip of an empty conversation says and offers: the real name of what will run, and the
/// groups of its menu (Claude's models, Codex's models, the mode).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentMenu {
    /// The chip's words: "Claude Opus 5.5".
    pub name: SharedString,
    pub groups: Vec<MenuGroup>,
}

/// One group of the menu under its title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuGroup {
    /// The words over the group; none for the models, which stand first.
    pub title: Option<SharedString>,
    pub entries: Vec<MenuEntry>,
}

/// One entry of the menu: what it says, whether it is the one in force and what choosing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    pub name: SharedString,
    pub is_on: bool,
    pub pick: MenuPick,
}

/// The `config_id` of the menu entry that opens the Codex login for Sakura.
pub const LOGIN_OPTION: &str = "login";

/// The id of the mode in which the agent only reads and proposes, the same for both agents.
pub const PLAN_MODE: &str = "plan";

/// What choosing an entry sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuPick {
    /// The model `id` of the conversation's agent; empty asks the agent for its own.
    Model(String),
    /// The reasoning effort `id`.
    Effort(String),
    /// The value `value` of the option `config_id` the agent sends (a switch is `true` or `false`).
    Option { config_id: String, value: String },
    /// The mode `id` of the session.
    Mode(String),
}

impl MasterKit {
    /// A kit over `world`, with no project chosen and nothing to pick before the agent has started.
    pub fn new(world: Rc<dyn MasterWorld>) -> Self {
        Self {
            world,
            dictate: Rc::new(super::dictation::start_system_dictation),
            project: None,
            start: SharedStart::default(),
            menu: Rc::new(|_| AgentMenu::default()),
        }
    }

    /// The same kit offering `menu` before the agent has started, and keeping what is picked in `start`.
    pub fn with_start(
        mut self,
        start: SharedStart,
        menu: Rc<dyn Fn(&SessionChoices) -> AgentMenu>,
    ) -> Self {
        self.start = start;
        self.menu = menu;
        self
    }

    /// The same kit with another way to start dictation: a test's.
    #[cfg(test)]
    pub fn with_dictation(mut self, dictate: Rc<dyn Fn() -> bool>) -> Self {
        self.dictate = dictate;
        self
    }

    /// `text` as the master is sent it: with the project the person chose, on a line of its own, first. A
    /// slash command stays the start of the message, where the agent announced it is read, and the project
    /// follows on its own line.
    pub fn outgoing(&self, text: &str) -> String {
        let Some(name) = &self.project else {
            return text.to_string();
        };
        let line = crate::l10n::tr_with(super::copy::PROJECT_LINE, &[("name", name)]);
        match is_slash_command(text) {
            true => format!("{text}\n{line}"),
            false => format!("{line}\n{text}"),
        }
    }
}

/// Whether `text` starts with a command: a slash and a word.
fn is_slash_command(text: &str) -> bool {
    text.strip_prefix('/')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|first| !first.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::thread::copy::PROJECT_LINE;
    use sakura_acp::model::{ToolKind, ToolStatus};

    fn call(title: &str, input: Value, answer: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            title: title.into(),
            kind: ToolKind::Other,
            status: ToolStatus::Completed,
            content: vec![ToolContent::text(answer)],
            locations: Vec::new(),
            raw_input: Some(input),
        }
    }

    #[test]
    fn card_tools_are_the_engine_tools_under_either_prefix_and_no_others() {
        let titled = |title: &str| CardTool::of(&call(title, Value::Null, ""));
        assert_eq!(
            titled("mcp__sakura-master__lane_open"),
            Some(CardTool::LaneOpen)
        );
        assert_eq!(
            titled("mcp__sakura__knowledge_propose"),
            Some(CardTool::KnowledgePropose)
        );
        assert_eq!(titled("mcp__sakura__lanes"), None);
        assert_eq!(titled("mcp__other__lane_open"), None);
        assert_eq!(titled("Read src/main.rs"), None);
    }

    #[test]
    fn an_ask_reads_only_the_text_the_call_answered() {
        let input =
            serde_json::json!({ "project": "repo", "text": "Make uploads retry", "path": "n.md" });
        let lane = LaneAsk::of(&call(
            "mcp__sakura__lane_open",
            input.clone(),
            "opened task-1",
        ));
        assert_eq!(lane.result, "opened task-1");
        let note = ProposalAsk::of(&call("mcp__sakura__knowledge_propose", input, "ok"));
        assert_eq!(note.result, "ok");
    }

    struct NoWorld;
    impl MasterWorld for NoWorld {
        fn lane(&self, _: &LaneAsk, _: &App) -> LaneCard {
            unreachable!()
        }
        fn open_lane(&self, _: &str, _: &mut Window, _: &mut App) {}
        fn proposal(&self, _: &ProposalAsk, _: &App) -> Proposal {
            Proposal::Unbound
        }
        fn open_inbox(&self, _: &mut Window, _: &mut App) {}
        fn decide(&self, _: &str, _: bool, _: &mut Window, _: &mut App) {}
        fn decision(&self, _: &str, _: &App) -> Option<AnyElement> {
            None
        }
        fn projects(&self, _: &App) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn a_project_chosen_is_the_first_line_of_what_is_sent_and_none_leaves_it_alone() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut kit = MasterKit::new(Rc::new(NoWorld));
        assert_eq!(kit.outgoing("Hello"), "Hello");
        kit.project = Some("work".to_string());
        let line = crate::l10n::tr_with(PROJECT_LINE, &[("name", "work")]);
        assert_eq!(kit.outgoing("Hello"), format!("{line}\nHello"));
        assert_eq!(line.as_ref(), "Project: work.");
    }

    #[test]
    fn a_slash_command_stays_the_start_of_the_message_with_a_project_chosen() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut kit = MasterKit::new(Rc::new(NoWorld));
        kit.project = Some("work".to_string());
        let line = crate::l10n::tr_with(PROJECT_LINE, &[("name", "work")]);
        assert_eq!(kit.outgoing("/compact"), format!("/compact\n{line}"));
        assert!(kit.outgoing("/context now").starts_with("/context now\n"));
        assert_eq!(
            kit.outgoing("/ not a command"),
            format!("{line}\n/ not a command")
        );
    }
}
