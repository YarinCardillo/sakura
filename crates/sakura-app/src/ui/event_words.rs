//! The lines the engine recorded of a lane, as a person reads them. The engine writes its ledger for its own
//! use: a tag, the actor as `cli:<login>`, a program's host path, the command to run next. The lane page says
//! each as a sentence, in the same rules as the Waiting page: who in words, what in words, no host path and
//! no command line.

use super::event_changes::change_words;
use super::event_kinds::{kind_words, phase_name};
use super::event_notify::notify_words;
use crate::l10n::{tr, tr_count, tr_with, Key};
use crate::ui::lanes::capitalised;
use gpui::SharedString;

const ACTOR_MARK: &str = " actor=";
const COMMAND_MARK: &str = " sakura ";
const PRESET_MARK: &str = "preset ";
const ERROR_MARK: &str = "the engine stopped on an error";
const PROGRAM_MARK: &str = " program ";
const DENIED_MARK: &str = "cage denied";
const STEER: &str = "steer";
const STEER_TO: &str = "to";

/// `raw`, one line of the engine's record of a lane, as a sentence. A line this has no words for is "an engine event"
/// and nothing of its text: a line the engine wrote for itself can hold a command, a host path or what an agent
/// said, and none of it is shown unless it is a kind that is known not to.
pub fn event_words(raw: &str) -> SharedString {
    let (tag, rest) = split_tag(raw.trim());
    let (what, actor) = split_actor(rest);
    let by = actor.map(by_words);
    known(tag, what, by).unwrap_or_else(|| unknown(tag))
}

/// The sentence of a kind of line this knows.
fn known(tag: &str, what: &str, by: Option<SharedString>) -> Option<SharedString> {
    if let Some(words) = change_words(tag, what, by.as_deref()) {
        return Some(words);
    }
    let phase = phase_name(tag);
    match (tag, what) {
        ("operator", "start requested") => Some(by_sentence("events.start", by)),
        ("operator", "resume requested") => Some(by_sentence("events.resume", by)),
        ("operator", "stop requested") => Some(by_sentence("events.stop", by)),
        ("operator", "discard requested") => Some(by_sentence("events.discard", by)),
        ("operator", what) => operator_words(what, by.clone()).or_else(|| approval(what, by)),
        ("engine", what) if what.starts_with(PRESET_MARK) => Some(preset(what)),
        ("engine", what) if what.starts_with(ERROR_MARK) => {
            Some(tr(Key("events.engine_error")).into())
        }
        ("notify", what) => notify_words(what),
        (_, what) if what.starts_with(DENIED_MARK) => Some(tr(Key("events.cage_denied")).into()),
        (_, what) if steer_words(what).is_some() => steer_words(what),
        ("workspace", what) => setup_step(what),
        ("", what) => outside(what),
        (_, "done") => Some(tr_with(Key("events.phase_done"), &[("phase", &phase)])),
        (tag, what) if !tag.is_empty() && is_skip(what) => {
            Some(tr_with(Key("events.phase_skipped"), &[("phase", &phase)]))
        }
        (tag, what) if kind_words(tag, what).is_some() => kind_words(tag, what),
        (tag, what) if !tag.is_empty() && what.contains(PROGRAM_MARK) => {
            Some(tr_with(Key("events.agent_found"), &[("phase", &phase)]))
        }
        _ => None,
    }
}

/// `steer to <run> <outcome>`: what became of a message the person wrote to an agent, said by the outcome.
fn steer_words(what: &str) -> Option<SharedString> {
    let mut words = what.split_whitespace();
    let is_steer = words.next() == Some(STEER) && words.next() == Some(STEER_TO);
    let outcome = words.nth(1).filter(|_| is_steer)?;
    let key = match outcome {
        "delivered" => "events.steer_delivered",
        "queued" => "events.steer_queued",
        "resumed" => "events.steer_resumed",
        "refused" => "events.steer_refused",
        _ => return None,
    };
    Some(tr(Key(key)).into())
}

/// A line this has no words for: its phase when it has one, and that it is an engine event.
fn unknown(tag: &str) -> SharedString {
    match tag.is_empty() {
        true => tr(Key("events.unknown")).into(),
        false => tr_with(Key("events.unknown_in"), &[("phase", &phase_name(tag))]),
    }
}

/// A phase the engine skipped: `skipped`, with or without its reason.
fn is_skip(what: &str) -> bool {
    what == "skipped" || what.starts_with("skipped:")
}

/// What the person decided on a gate besides the plain `<gate> approved`: contract commands approved or kept for
/// later, local sockets, a delivery despite an overlap, a budget wait lifted.
fn operator_words(what: &str, by: Option<SharedString>) -> Option<SharedString> {
    let count = leading_number(what);
    if let Some(n) = count.filter(|_| what.contains(" contract commands approved")) {
        let said = tr_count(
            Key("events.contract_approved"),
            n.parse().unwrap_or_default(),
            &[("by", &by.unwrap_or_default())],
        );
        return Some(said.trim_end().to_string().into());
    }
    if let Some(n) = count.filter(|_| what.contains(" contract commands remembered")) {
        let count = n.parse().unwrap_or_default();
        return Some(tr_count(Key("events.contract_remembered"), count, &[]));
    }
    let key = match what {
        w if w.starts_with("local sockets approved") => "events.local_sockets",
        w if w.starts_with("delivery approved despite an overlap") => "events.overlap_approved",
        w if w.starts_with("budget wait released") => "events.budget_wait_released",
        _ => return None,
    };
    Some(with_actor(tr(Key(key)).to_string(), by))
}

/// The number `text` starts with, when it starts with one.
fn leading_number(text: &str) -> Option<&str> {
    let first = text.split(' ').next()?;
    first.chars().all(|c| c.is_ascii_digit()).then_some(first)
}

/// `<what> approved`, with who approved, when `what` is one word (a gate).
fn approval(what: &str, by: Option<SharedString>) -> Option<SharedString> {
    let gate = what.strip_suffix(" approved")?;
    let name = gate.split(':').next().unwrap_or(gate);
    let is_word = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    is_word.then(|| {
        let said = tr_with(Key("events.approved"), &[("gate", &capitalised(name))]);
        with_actor(said.to_string(), by)
    })
}

/// `<n> install: exit <code>, log <path>`: which step, and whether it finished.
fn setup_step(what: &str) -> Option<SharedString> {
    let (step, rest) = what.split_once(" install: exit ")?;
    let code = rest.split([',', ' ']).next()?;
    let is_step = !step.is_empty() && step.chars().all(|c| c.is_ascii_digit());
    let key = match (is_step, code) {
        (false, _) => return None,
        (true, "0") => "events.setup_finished",
        (true, _) => "events.setup_failed",
    };
    Some(tr_with(Key(key), &[("step", step)]))
}

/// What an outside agent's lane recorded (`opened <title> by <agent>`, `row ...`, `asked ...`): the kind, and the
/// agent's words with no path and no command.
fn outside(what: &str) -> Option<SharedString> {
    let known = ["opened", "row", "asked", "answered", "closed"];
    let (kind, _) = what.split_once(' ').unwrap_or((what, ""));
    known
        .contains(&kind)
        .then(|| capitalised(&plain(what)).into())
}

/// `[tag] rest` as (`tag`, `rest`); a line with no tag has an empty one.
fn split_tag(line: &str) -> (&str, &str) {
    let Some(inner) = line.strip_prefix('[') else {
        return ("", line);
    };
    match inner.split_once("] ") {
        Some((tag, rest)) => (tag, rest),
        None => ("", line),
    }
}

/// The text and the actor the line ends with (`... actor=cli:me`), when it has one.
fn split_actor(text: &str) -> (&str, Option<&str>) {
    match text.rsplit_once(ACTOR_MARK) {
        Some((what, actor)) if !actor.contains(' ') => (what, Some(actor)),
        _ => (text, None),
    }
}

/// Who an actor (`<source>:<login>`) is, in words, after "by".
fn by_words(actor: &str) -> SharedString {
    let (source, name) = actor.split_once(':').unwrap_or((actor, ""));
    match source {
        "cli" => tr(Key("events.by_cli")).into(),
        "app" => tr(Key("events.by_app")).into(),
        "master" => tr(Key("events.by_master")).into(),
        "overlay" => tr(Key("events.by_remote")).into(),
        "outside" => tr_with(Key("events.by_outside"), &[("name", name)]),
        _ => tr(Key("events.by_unknown")).into(),
    }
}

fn by_sentence(key: &'static str, by: Option<SharedString>) -> SharedString {
    let key = Key(key);
    match by {
        Some(by) => tr_with(key, &[("by", &by)]),
        None => tr_with(key, &[("by", "")]).trim_end().to_string().into(),
    }
}

fn with_actor(what: String, by: Option<SharedString>) -> SharedString {
    match by {
        Some(by) => format!("{what} {by}").into(),
        None => what.into(),
    }
}

fn preset(what: &str) -> SharedString {
    let name = what[PRESET_MARK.len()..]
        .split([':', ' '])
        .next()
        .unwrap_or_default();
    tr_with(Key("events.preset"), &[("name", name)])
}

/// `text` with no host path and no command: a path is its last name, an actor in the text is its name, and
/// whatever follows a `sakura` command is cut.
fn plain(text: &str) -> String {
    let before_command = text.split(COMMAND_MARK).next().unwrap_or(text);
    let words: Vec<&str> = before_command.split(' ').map(plain_word).collect();
    words.join(" ")
}

/// One word: an absolute path (also in parentheses or before a comma) is its last name, `<source>:<name>` is the name.
fn plain_word(word: &str) -> &str {
    let bare = word.trim_matches(|c| matches!(c, '(' | ')' | ','));
    if bare.starts_with('/') && bare.matches('/').count() > 1 {
        return bare.rsplit('/').next().unwrap_or(bare);
    }
    match bare.split_once(':') {
        Some(("outside" | "cli" | "app" | "master" | "overlay", name)) => name,
        _ => word,
    }
}
