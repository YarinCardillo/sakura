//! What a confirmation says to the person: the lane and the action in the interface's words. The engine's
//! own summary of an action is a command line with a host path, which says nothing a person approving in
//! a window can use, so it is never shown.

use crate::l10n::{tr, tr_with, Key};
use gpui::SharedString;
use sakura_core::engine::cli::Act;
use sakura_core::engine::snapshot::{Item, Snapshot, Wait};

const KIND_APPROVAL: &str = "approval";
const GATE_SPEC: &str = "spec";
const GATE_CHECKS: &str = "checks";
const ACTION_APPROVE: &str = "a";
const ACTION_RESUME: &str = "R";
const ACTION_STOP: &str = "s";
const ACTION_ACCEPT: &str = "y";
const ACTION_REJECT: &str = "N";

/// The sentences of a confirmation, one for each thing it can be about, in one grammar: a question, or a
/// statement of what is being done.
struct Sentences {
    spec: Key,
    /// The sentence of a gate this build knows (`overlap`, `checks`, `budget`), in the form of the grammar.
    gates: [(&'static str, Key); 3],
    approve: Key,
    stop: Key,
    resume: Key,
    other: Key,
    bare: Key,
}

const QUESTION: Sentences = Sentences {
    spec: Key("waiting.confirm_spec"),
    gates: [
        ("overlap", Key("waiting.confirm_overlap")),
        (GATE_CHECKS, Key("waiting.confirm_checks")),
        ("budget", Key("waiting.confirm_budget")),
    ],
    approve: Key("waiting.confirm_approve"),
    stop: Key("waiting.confirm_stop"),
    resume: Key("waiting.confirm_resume"),
    other: Key("waiting.confirm_other"),
    bare: Key("waiting.confirm_bare"),
};

const STATEMENT: Sentences = Sentences {
    spec: Key("waiting.doing_spec"),
    gates: [
        ("overlap", Key("waiting.doing_overlap")),
        (GATE_CHECKS, Key("waiting.doing_checks")),
        ("budget", Key("waiting.doing_budget")),
    ],
    approve: Key("waiting.doing_approve"),
    stop: Key("waiting.doing_stop"),
    resume: Key("waiting.doing_resume"),
    other: Key("waiting.doing_other"),
    bare: Key("waiting.doing_bare"),
};

/// What the person is asked to confirm for `act`, as a question. A target the snapshot no longer lists is
/// asked about by the action alone. A proposal's own sentence (`summary`, which binds the text shown on its
/// wait) is the engine's plain words, not a command, and stands as it is.
pub fn confirm_words(snapshot: Option<&Snapshot>, act: &Act, summary: &str) -> SharedString {
    words(snapshot, act, summary, &QUESTION)
}

/// What is being done for `act` once the person pressed it, as a statement ("Approving the spec of demo"),
/// for the toast that holds the answer. A proposal's own sentence stands as it is.
pub fn confirm_statement(snapshot: Option<&Snapshot>, act: &Act, summary: &str) -> SharedString {
    words(snapshot, act, summary, &STATEMENT)
}

/// Whether the engine's summary of `act` says something the sentences of this window leave out, so that it
/// is worth a second line under a toast, including the engine's details for spec and checks approvals.
pub fn summary_adds_words(snapshot: Option<&Snapshot>, act: &Act) -> bool {
    let lane = lane_name(snapshot, &act.target);
    let gate = gate_of(snapshot, &act.target);
    match (act.action.key.as_str(), lane.as_str(), gate.as_str()) {
        (ACTION_APPROVE, lane, GATE_SPEC | GATE_CHECKS) if !lane.is_empty() => true,
        (ACTION_APPROVE, lane, gate) if !lane.is_empty() && QUESTION.gate(gate).is_some() => false,
        (ACTION_APPROVE | ACTION_STOP | ACTION_RESUME, lane, _) if !lane.is_empty() => false,
        _ => true,
    }
}

impl Sentences {
    /// The sentence of the gate `name`, when this build knows it.
    fn gate(&self, name: &str) -> Option<Key> {
        self.gates
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, key)| *key)
    }
}

/// The engine's summary of an action as a line a person can read: its parts (the engine separates them
/// with a colon) up to the first that names a folder or starts the command it runs; nothing when none is
/// left.
pub fn plain_summary(summary: &str) -> Option<SharedString> {
    let is_technical = |part: &&str| part.contains('/') || part.trim_start().starts_with("sakura");
    let parts = summary.split(": ").take_while(|part| !is_technical(part));
    let line = parts.collect::<Vec<_>>().join(": ");
    let line = line.trim();
    (!line.is_empty()).then(|| SharedString::from(line.to_string()))
}

fn words(snapshot: Option<&Snapshot>, act: &Act, summary: &str, said: &Sentences) -> SharedString {
    if !summary.is_empty() && matches!(act.action.key.as_str(), ACTION_ACCEPT | ACTION_REJECT) {
        return SharedString::from(summary.to_string());
    }
    let lane = lane_name(snapshot, &act.target);
    let gate = gate_of(snapshot, &act.target);
    match (act.action.key.as_str(), lane.as_str(), gate.as_str()) {
        (ACTION_APPROVE, lane, GATE_SPEC) if !lane.is_empty() => {
            tr_with(said.spec, &[("lane", lane)])
        }
        (ACTION_APPROVE, lane, gate) if !lane.is_empty() && said.gate(gate).is_some() => {
            tr_with(said.gate(gate).unwrap_or(said.approve), &[("lane", lane)])
        }
        (ACTION_APPROVE, lane, _) if !lane.is_empty() => tr_with(said.approve, &[("lane", lane)]),
        (ACTION_STOP, lane, _) if !lane.is_empty() => tr_with(said.stop, &[("lane", lane)]),
        (ACTION_RESUME, lane, _) if !lane.is_empty() => tr_with(said.resume, &[("lane", lane)]),
        (key, lane, _) => other(&action_words(key, &act.action.label), lane, said),
    }
}

/// An action with no sentence of its own: its words, and the lane when there is one.
fn other(action: &str, lane: &str, said: &Sentences) -> SharedString {
    match lane.is_empty() {
        true => tr_with(said.bare, &[("action", action)]),
        false => tr_with(said.other, &[("action", action), ("lane", lane)]),
    }
}

/// The interface's own word for an action it knows, else the engine's label.
fn action_words(key: &str, label: &str) -> String {
    let known = match key {
        ACTION_APPROVE => Some(Key("waiting.approve")),
        ACTION_RESUME => Some(Key("waiting.resume")),
        ACTION_STOP => Some(Key("waiting.stop")),
        _ => None,
    };
    known.map_or_else(|| label.to_string(), |key| tr(key).to_string())
}

/// The lane a target is about: a wait's task, or a lane's own name; empty when the snapshot lists neither.
fn lane_name(snapshot: Option<&Snapshot>, target: &str) -> String {
    let Some(snapshot) = snapshot else {
        return String::new();
    };
    if let Some(wait) = snapshot.waits.iter().find(|wait| wait.id == target) {
        return lane_of(snapshot, wait).map_or_else(
            || wait.lane_name().to_string(),
            |item| item.name().to_string(),
        );
    }
    let item = snapshot.items.iter().find(|item| item.id == target);
    item.map_or_else(String::new, |item| item.name().to_string())
}

/// The gate an approval target is about: the gate of the wait itself, or of the approval its lane waits at.
fn gate_of(snapshot: Option<&Snapshot>, target: &str) -> String {
    let Some(snapshot) = snapshot else {
        return String::new();
    };
    let approval = |wait: &&Wait| wait.kind == KIND_APPROVAL;
    if let Some(wait) = snapshot.waits.iter().find(|wait| wait.id == target) {
        return wait.key_name().to_string();
    }
    let Some(item) = snapshot.items.iter().find(|item| item.id == target) else {
        return String::new();
    };
    let lane_key = item.lane_key();
    let waits = snapshot.waits.iter().filter(approval);
    let found = waits.into_iter().find(|wait| wait.lane_key() == lane_key);
    found.map_or_else(String::new, |wait| wait.key_name().to_string())
}

fn lane_of<'a>(snapshot: &'a Snapshot, wait: &Wait) -> Option<&'a Item> {
    let key = wait.lane_key();
    snapshot.items.iter().find(|item| item.lane_key() == key)
}
