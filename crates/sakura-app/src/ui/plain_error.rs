//! What an error says to the person. The engine's own words when it gives them; else, for a raw text that
//! shows a command line or a path of this Mac, a plain sentence naming what failed; the raw text is kept for a
//! "Details" disclosure, never shown first.

use super::answers::Answers;
use super::chrome::{mono, DETAILS};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use gpui::{
    div, AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// The head of a raw error ("pull request lookup failed") that names what failed ends with one of these.
const FAILURE_ENDINGS: [&str; 4] = ["failed", "refused", "stopped", "unavailable"];
/// The programs whose command line an error may print, and a word that asks the person to run one.
const COMMANDS: [&str; 14] = [
    "gh", "git", "python", "python3", "npm", "pip", "cargo", "sakura", "claude", "codex", "curl",
    "ssh", "bash", "make",
];
/// How many words after a program a flag still shows that it is a command line.
const FLAG_REACH: usize = 3;
/// The longest head of a raw error that can name what failed.
const MOST_WORDS_OF_A_NAME: usize = 8;

/// What to show of an error: the words, and the raw text behind them when there is some to hide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plain {
    pub words: String,
    pub raw: Option<String>,
}

/// What the engine's text says with the command line it points at taken out: "…wait for the person's approval;
/// see sakura flow status" is "…wait for the person's approval". No command line of the engine's is ever shown,
/// not even behind "Details".
pub fn without_command_lines(text: &str) -> String {
    const POINTERS: [&str; 3] = ["see sakura ", "run sakura ", "try sakura "];
    let lower = text.to_lowercase();
    let at = POINTERS
        .iter()
        .filter_map(|pointer| lower.find(pointer))
        .min();
    let Some(at) = at else {
        return text.to_string();
    };
    let head = text[..at].trim_end_matches(|c: char| " ;,:-".contains(c));
    // What follows the command line, when a sentence goes on after it, is kept.
    let tail = text[at..].split_once(". ").map_or("", |(_, rest)| rest);
    match (head.is_empty(), tail.is_empty()) {
        (_, true) => head.to_string(),
        (true, false) => without_command_lines(tail),
        (false, false) => format!("{head}. {}", without_command_lines(tail)),
    }
}

/// The words for the error `raw`, with the engine's own `engine_words` when it gave any.
pub fn plain_of(raw: &str, engine_words: &str) -> Plain {
    let (raw, engine_words) = (
        without_command_lines(raw),
        without_command_lines(engine_words),
    );
    let (raw, engine_words) = (raw.as_str(), engine_words.as_str());
    let raw = raw.trim();
    let raw_kept = (!raw.is_empty()).then(|| raw.to_string());
    if !engine_words.trim().is_empty() {
        let is_same = raw == engine_words.trim();
        return Plain {
            words: engine_words.trim().to_string(),
            raw: raw_kept.filter(|_| !is_same),
        };
    }
    if !is_unreadable(raw) {
        return Plain {
            words: raw.to_string(),
            raw: None,
        };
    }
    Plain {
        words: named_sentence(raw).unwrap_or_else(|| tr(Key("error.plain.generic")).to_string()),
        raw: raw_kept,
    }
}

/// "Pull request lookup failed." from "pull request lookup failed: To get started ...": the head, when it
/// names a failure in a few words, said as a sentence.
fn named_sentence(raw: &str) -> Option<String> {
    let head = raw.split_once(':')?.0.trim();
    let is_short = head.split_whitespace().count() <= MOST_WORDS_OF_A_NAME;
    let names_failure = FAILURE_ENDINGS.iter().any(|ending| head.ends_with(ending));
    if !is_short || !names_failure || shows_command_or_path(head) {
        return None;
    }
    let mut letters = head.chars();
    let first = letters.next()?.to_uppercase();
    Some(format!(
        "{}{}.",
        first.chain(letters).collect::<String>(),
        ""
    ))
}

/// Whether `text` shows a command line or a path of this Mac: an absolute path of two parts or more, a path
/// under `~`, a prompt, a request to run something, or a known program followed by its arguments.
pub fn shows_command_or_path(text: &str) -> bool {
    let lower = text.to_lowercase();
    let asks_to_run =
        lower.contains("please run") || lower.contains(" run:") || lower.starts_with("run:");
    let words: Vec<&str> = text.split_whitespace().collect();
    let has_path = words
        .iter()
        .any(|word| is_path(word.trim_matches(|c: char| ",;:'\"()".contains(c))));
    let has_prompt = text.trim_start().starts_with("$ ");
    asks_to_run || has_path || has_prompt || has_command(&words)
}

/// Whether a known program starts the text or is followed, within a few words, by a flag: "git push --force"
/// shows a command, "a README can make R12 pass" is English.
fn has_command(words: &[&str]) -> bool {
    let is_flag = |word: &&str| word.starts_with('-') && word.len() > 1;
    words.iter().enumerate().any(|(at, word)| {
        let program = word.trim_matches(|c: char| ",;:'\"()".contains(c));
        COMMANDS.contains(&program)
            && (at == 0 || words[at + 1..].iter().take(FLAG_REACH).any(is_flag))
    })
}

/// Whether `text` is no words for a person: it shows a command or a path, a stack trace, or the name of a run.
fn is_unreadable(text: &str) -> bool {
    shows_command_or_path(text) || has_stack_trace(text) || has_run_name(text)
}

fn has_stack_trace(text: &str) -> bool {
    text.contains("Traceback (most recent call last)")
        || text.lines().any(|line| line.starts_with("  File \""))
        || text.contains("panicked at")
}

/// A run's name (`build-1`, `review-2`, `build-1.2`): a word, a dash and a number.
fn has_run_name(text: &str) -> bool {
    text.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| ",;:'\"().".contains(c));
        let Some((name, number)) = word.rsplit_once('-') else {
            return false;
        };
        let digits = number
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_') && digits
    })
}

fn is_path(word: &str) -> bool {
    let absolute = word.starts_with('/') && word[1..].contains('/');
    absolute || word.starts_with("~/")
}

/// The "Details" of an error: a row that opens the raw text under it, and closes it. `stem` and `key` name the
/// row (`{stem}-{key}`) and the raw text (`{stem}-text-{key}`) in the window.
pub fn details_view(
    stem: &'static str,
    key: &str,
    raw: &str,
    answers: &Entity<Answers>,
    palette: &Palette,
    cx: &App,
) -> AnyElement {
    let is_open = answers.read(cx).is_details_open(&format!("{stem}-{key}"));
    let (row, words) = (format!("{stem}-{key}"), format!("{stem}-text-{key}"));
    let (toggled, toggled_key) = (answers.clone(), row.clone());
    let chevron = match is_open {
        true => IconName::ChevronUp,
        false => IconName::ChevronDown,
    };
    let header = div()
        .id(SharedString::from(row.clone()))
        .debug_selector(move || row.clone())
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(tr(DETAILS))
        .child(Icon::new(chevron).with_size(theme::HAND_MARK_SIZE))
        .on_click(move |_, _, cx| {
            toggled.update(cx, |answers, cx| answers.toggle_details(&toggled_key, cx));
        });
    let raw = is_open.then(|| {
        mono()
            .debug_selector(move || words.clone())
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(super::thread::shown(raw))
    });
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .child(header)
        .children(raw)
        .into_any_element()
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    const RAW: &str =
        "11 contract commands changed and wait for the person's approval; see sakura flow status";

    #[test]
    fn the_command_line_the_engine_points_at_is_taken_out_and_the_sentence_before_it_stays() {
        assert_eq!(
            without_command_lines(RAW),
            "11 contract commands changed and wait for the person's approval"
        );
        assert_eq!(
            without_command_lines("Run sakura flow resume gated. Then wait."),
            "Then wait."
        );
        assert_eq!(
            without_command_lines("The lane is stuck."),
            "The lane is stuck."
        );
    }

    #[test]
    fn no_command_line_of_the_engine_s_shows_in_the_words_or_behind_details() {
        let plain = plain_of(
            RAW,
            "The commands of the checks changed and need your approval.",
        );
        assert!(!plain.words.contains("sakura"), "{}", plain.words);
        assert!(!plain.raw.unwrap_or_default().contains("sakura"));
        let alone = plain_of(RAW, "");
        assert!(
            !alone.words.contains("sakura") && !alone.raw.unwrap_or_default().contains("sakura")
        );
    }
}
