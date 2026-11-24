//! What a lane's agents are called and what state they are in, in words: the model of a run as the person
//! knows it, the state of a run, and the mark that goes with it. Pure, so the list and the chat say the same.

use super::kit::StateGlyph;
use super::thread::engine_tool_words;
use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;
use sakura_core::engine::agents::RunState;

/// The provider of a run that reads its model as a Codex model.
const CODEX: &str = "codex";

/// The model of a run as the person says it: "Claude Sonnet", "Codex GPT-5.5". The engine names a Claude
/// model by its tier and a Codex model by its id; a name this build does not know is shown as it came.
pub fn model_words(provider: &str, model: &str) -> SharedString {
    let model = model.trim();
    if provider == CODEX {
        return tr_with(
            Key("agents.model.codex"),
            &[("name", &model_id_words(model))],
        );
    }
    match model {
        "opus" | "sonnet" | "haiku" => {
            tr_with(Key("agents.model.claude"), &[("name", &capitalised(model))])
        }
        other => SharedString::from(super::thread::shown(other)),
    }
}

/// A Codex model id written as its maker writes it: `gpt-5.5` is "GPT-5.5".
pub fn model_id_words(id: &str) -> String {
    match id.strip_prefix("gpt-") {
        Some(version) => format!("GPT-{version}"),
        None => id.to_string(),
    }
}

pub fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// The state of a run in a word: a run in a state this build does not know is shown as stopped, never as at work.
pub fn state_words(state: RunState) -> SharedString {
    let key = match state {
        RunState::Starting => Key("agents.state.starting"),
        RunState::Running => Key("agents.state.running"),
        RunState::Finished => Key("agents.state.finished"),
        RunState::Aborted | RunState::Orphaned | RunState::Unknown => Key("agents.state.stopped"),
    };
    SharedString::from(tr(key))
}

/// The mark beside the state word.
pub fn state_glyph(state: RunState) -> StateGlyph {
    match state {
        RunState::Starting | RunState::Running => StateGlyph::Running,
        RunState::Finished => StateGlyph::Finished,
        RunState::Aborted | RunState::Orphaned | RunState::Unknown => StateGlyph::Stopped,
    }
}

/// What a tool call is, in a plain line: the sentence of an engine tool or of the tool search, the verb of a
/// common tool of the agents, and "Used a tool" for any other, so a raw tool name never reaches the person.
/// The verbs ("Read", "Edited") are for a call that names its target and stands before it.
pub fn tool_label(name: &str) -> SharedString {
    tool_words(name, true)
}

/// The words of a tool call: its verb when `has_target` (something follows it), and the tool's name alone when
/// the call named nothing ("Read"), never a sentence that promises a file it does not name.
pub fn tool_words(name: &str, has_target: bool) -> SharedString {
    let name = name.trim();
    let bare = |with: Key, without: Key| if has_target { with } else { without };
    let key = engine_tool_words(name).unwrap_or(match name.to_ascii_lowercase().as_str() {
        "read" => bare(Key("lane.feed.tool.read"), Key("lane.feed.tool.read_bare")),
        "edit" | "multiedit" | "notebookedit" => {
            bare(Key("lane.feed.tool.edit"), Key("lane.feed.tool.edit_bare"))
        }
        "write" => bare(
            Key("lane.feed.tool.write"),
            Key("lane.feed.tool.write_bare"),
        ),
        "bash" => Key("lane.feed.tool.command"),
        "grep" | "glob" => bare(
            Key("lane.feed.tool.search"),
            Key("lane.feed.tool.search_bare"),
        ),
        "webfetch" | "websearch" => Key("lane.feed.tool.web"),
        _ => Key("lane.feed.tool.other"),
    });
    SharedString::from(tr(key))
}

/// The first line of what an agent wrote, trimmed: the row of a list has room for one.
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_row_says_what_the_tool_did_in_words() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let said = |name: &str| tool_label(name).to_string();
        assert_eq!(said("ToolSearch"), "Looked for a tool");
        assert_eq!(
            said("mcp__sakura__board_post"),
            "Posted on a project's board"
        );
        assert_eq!(said("Read"), "Read");
        assert_eq!(said("Bash"), "Ran a command");
    }

    #[test]
    fn a_tool_with_no_words_never_shows_its_raw_name() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        for name in [
            "mcp__github__create_issue",
            "mcp__sakura__something_new",
            "mcp__x__0a1b2c3d4e5f",
            "TodoWrite",
            "",
        ] {
            let shown = tool_label(name).to_string();
            assert!(!shown.contains("mcp__") && !shown.is_empty());
            assert_ne!(shown, name);
        }
        assert_eq!(tool_label("mcp__github__create_issue"), "Used a tool");
    }

    #[test]
    fn the_tool_words_exist_in_italian_too() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(tool_label("Edit"), "Ha modificato");
        assert_eq!(tool_label("mcp__github__x"), "Ha usato uno strumento");
    }

    #[test]
    fn a_tool_with_no_target_says_the_tool_s_name_alone_and_never_a_vague_sentence() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let said = |name: &str, has_target: bool| tool_words(name, has_target).to_string();
        assert_eq!(said("Read", true), "Read", "a target follows the verb");
        assert_eq!(said("Read", false), "Read");
        assert_eq!(said("Edit", false), "Edit");
        assert_eq!(said("Write", false), "Write");
        assert_eq!(said("Grep", false), "Search");
        assert_eq!(said("Bash", false), "Ran a command", "already a sentence");
        assert_eq!(said("WebFetch", false), "Looked something up online");
    }

    #[test]
    fn the_targetless_tool_words_exist_in_italian_too() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(tool_words("Read", false), "Lettura");
        assert_eq!(tool_words("Read", true), "Ha letto");
    }
}
