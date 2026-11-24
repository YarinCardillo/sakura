//! The notice lines of a lane (`notify <event>: <message>` and the engine's lines about a phone page), as sentences.
//! The message of a notice can be written by an agent, so none of it is shown: only the event it announces.

use crate::l10n::{tr, Key};
use gpui::SharedString;

const NOTICE_MARK: &str = "notify ";
const DEFERRED_MARK: &str = "deferred (night):";
const DEFER_FAILED_MARK: &str = "defer failed:";
const FAILED_MARK: &str = "failed:";

/// The events a lane sends (`gates.PAGED_EVENTS` and the budget ask of the engine) and the sentence of each.
const EVENTS: [(&str, Key); 7] = [
    ("awaiting_approval", Key("events.awaiting")),
    ("escalation", Key("events.notify.escalation")),
    ("failed", Key("events.notify.failed")),
    ("ready_for_review", Key("events.notify.ready_for_review")),
    ("gate_red", Key("events.notify.gate_red")),
    ("budget_warning", Key("events.notify.budget_warning")),
    ("budget_ask", Key("events.notify.budget_ask")),
];

/// The sentence of a `notify ...` line, or `None` when `what` is not one. A notice of an event this has no
/// sentence for is still a notice of the lane, said plainly.
pub fn notify_words(what: &str) -> Option<SharedString> {
    let rest = what.strip_prefix(NOTICE_MARK)?;
    let key = if rest.starts_with(DEFERRED_MARK) {
        Key("events.notify.deferred")
    } else if rest.starts_with(DEFER_FAILED_MARK) {
        Key("events.notify.defer_failed")
    } else if let Some(page) = rest.strip_prefix(FAILED_MARK) {
        failed_key(page.trim())
    } else {
        event_key(rest.split(':').next().unwrap_or_default())
    };
    Some(tr(key).into())
}

/// `notify failed: <x>` is a phone page that was not sent when `<x>` is exactly one event name, and the lane
/// that failed (with its reason, which is not shown) otherwise.
fn failed_key(rest: &str) -> Key {
    match EVENTS.iter().any(|(event, _)| *event == rest) {
        true => Key("events.notify.page_failed"),
        false => Key("events.notify.failed"),
    }
}

fn event_key(event: &str) -> Key {
    EVENTS
        .iter()
        .find(|(name, _)| *name == event)
        .map_or(Key("events.notify.other"), |(_, key)| *key)
}
