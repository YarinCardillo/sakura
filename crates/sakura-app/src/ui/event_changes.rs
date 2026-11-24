//! Two families of ledger lines that carry a value the engine chose: a change the person queued and the engine
//! applied (`pace changed to fast (cmd-1) actor=app:me`), and what pace decided about a step's account. Each is
//! said in a sentence that names the change, never the command's id, an account or a reason the line holds.

use super::lanes::role_words;
use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;

const COMMAND_OPEN: &str = " (";
const PACE_TAG: &str = "pace";

/// The sentence of `what`, a line of the engine's record, when it is one of these; `by` is who asked.
pub fn change_words(tag: &str, what: &str, by: Option<&str>) -> Option<SharedString> {
    let said = what.split(COMMAND_OPEN).next().unwrap_or(what);
    if let Some(words) = applied(said, by) {
        return Some(words);
    }
    if what.contains(" rejected (") {
        return rejected(said);
    }
    (tag == PACE_TAG).then(|| account_move(said)).flatten()
}

/// `<kind> changed to <value>`, `role <role> changed to <model>` and `<kind> accepted`.
fn applied(said: &str, by: Option<&str>) -> Option<SharedString> {
    let by = by.unwrap_or_default();
    if let Some(role) = said
        .strip_prefix("role ")
        .and_then(|rest| rest.split_once(" changed to "))
    {
        let role = role_words(role.0);
        return Some(sentence(
            Key("events.change.role"),
            &[("role", &role), ("by", by)],
        ));
    }
    if matches!(said, "steer accepted" | "reply accepted") {
        return Some(tr(Key("events.change.accepted")).into());
    }
    let (kind, value) = said.split_once(" changed to ")?;
    let key = match kind {
        "pace" => Key("events.change.pace"),
        "preset" => Key("events.change.preset"),
        "dial" => Key("events.change.dial"),
        _ => return None,
    };
    is_word(value).then(|| {
        tr_with(key, &[("value", value), ("by", by)])
            .trim_end()
            .into()
    })
}

/// `key` filled in, with no space left where there is no "by".
fn sentence(key: Key, values: &[(&str, &str)]) -> SharedString {
    tr_with(key, values).trim_end().to_string().into()
}

/// `<kind> rejected (<file>): <reason>`: that a change was refused, and none of why.
fn rejected(said: &str) -> Option<SharedString> {
    let kind = said.strip_suffix(" rejected")?;
    let what = match kind {
        "pace" => Key("events.change.of_pace"),
        "preset" => Key("events.change.of_preset"),
        "dial" => Key("events.change.of_dial"),
        _ => return Some(tr(Key("events.change.refused_other")).into()),
    };
    Some(tr_with(Key("events.change.refused"), &[("what", tr(what))]))
}

/// What pace decided: a step moved to another account, put back on its own, or the usage unreadable.
fn account_move(said: &str) -> Option<SharedString> {
    if said.ends_with("; steps run on the reserve rules alone") {
        return Some(tr(Key("events.pace.unreadable")).into());
    }
    let (step, key) = match said.split_once(" moves to the account ") {
        Some((step, _)) => (step, Key("events.pace.moved")),
        None => (
            said.split_once(" goes back to its own account")?.0,
            Key("events.pace.back"),
        ),
    };
    is_word(step).then(|| tr_with(key, &[("step", step)]))
}

/// Whether `value` is one plain word, as the engine's own names are: it is shown, anything else is not.
fn is_word(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
