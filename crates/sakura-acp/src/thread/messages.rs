//! Text that arrives in chunks: what the person wrote (only when a session is replayed), what the agent
//! answers and what it thinks. Chunks of the same kind that follow each other are one entry.

use super::{EntryKind, Message, Problem, ProblemKind, Thread};
use crate::model::PlanEntry;

/// What an agent writes, as a message of the person, in place of a turn the person stopped.
const STOPPED_WORDS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

impl Thread {
    /// A chunk of the person's message. Outside a replay it is the agent echoing the prompt the thread
    /// already shows, so it is dropped.
    pub(super) fn user_text(&mut self, text: String) {
        if !self.is_replaying {
            return;
        }
        // An agent replays a turn the person stopped as a message of the person, in the adapter's own
        // words for it: it is the line of a stop again, never something the person wrote.
        if STOPPED_WORDS.contains(&text.trim()) {
            let kind = ProblemKind::Stopped;
            let detail = String::new();
            self.push(EntryKind::Problem(Problem { kind, detail }));
            return;
        }
        let text = self.take_room(text);
        if text.is_empty() {
            return;
        }
        if let Some(index) = self.last_index() {
            if let EntryKind::User(existing) = &mut self.entries[index].kind {
                existing.push_str(&text);
                return self.touch(index);
            }
        }
        self.push(EntryKind::User(text));
    }

    /// A chunk of the agent's answer, or of its thinking.
    pub(super) fn agent_text(&mut self, text: String, is_thought: bool) {
        let text = self.take_room(text);
        if text.is_empty() {
            return;
        }
        if let Some(index) = self.last_index() {
            let existing = match &mut self.entries[index].kind {
                EntryKind::Assistant(message) if !is_thought => Some(message),
                EntryKind::Thought(message) if is_thought => Some(message),
                _ => None,
            };
            if let Some(message) = existing {
                message.text.push_str(&text);
                return self.touch(index);
            }
        }
        self.settle_streaming();
        let message = Message {
            text,
            is_streaming: !self.is_replaying,
        };
        self.push(match is_thought {
            true => EntryKind::Thought(message),
            false => EntryKind::Assistant(message),
        });
    }

    /// The whole plan. Within one turn it replaces the plan already shown; after a new prompt it is a new
    /// entry, and the older plan stays where it was said.
    pub(super) fn plan(&mut self, entries: Vec<PlanEntry>) {
        let last_prompt = self.last_position(|kind| matches!(kind, EntryKind::User(_)));
        let last_plan = self.last_position(|kind| matches!(kind, EntryKind::Plan(_)));
        self.held_bytes += plan_bytes(&entries);
        match last_plan {
            Some(index) if last_prompt.is_none_or(|prompt| index > prompt) => {
                let replaced =
                    std::mem::replace(&mut self.entries[index].kind, EntryKind::Plan(entries));
                if let EntryKind::Plan(replaced) = replaced {
                    self.held_bytes = self.held_bytes.saturating_sub(plan_bytes(&replaced));
                }
                self.touch(index);
            }
            _ => {
                self.settle_streaming();
                self.push(EntryKind::Plan(entries));
            }
        }
    }

    /// The last message stops streaming: something else follows it, or the turn is over.
    pub(super) fn settle_streaming(&mut self) {
        let Some(index) = self.last_index() else {
            return;
        };
        let message = match &mut self.entries[index].kind {
            EntryKind::Assistant(message) | EntryKind::Thought(message) => message,
            _ => return,
        };
        if message.is_streaming {
            message.is_streaming = false;
            self.touch(index);
        }
    }

    fn last_index(&self) -> Option<usize> {
        self.entries.len().checked_sub(1)
    }

    fn last_position(&self, is_wanted: impl Fn(&EntryKind) -> bool) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|entry| is_wanted(&entry.kind))
    }
}

/// The bytes of text a plan holds.
fn plan_bytes(entries: &[PlanEntry]) -> usize {
    entries.iter().map(|entry| entry.content.len()).sum()
}
