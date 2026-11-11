//! The conversation as typed entries, built from events by a pure reducer: no I/O, no clock, no UI. The
//! window only reads [`Thread::entries`] and asks [`Thread::take_changes`] what to draw again.
//!
//! The shape follows Zed's `acp_thread` crate (https://github.com/zed-industries/zed, `crates/acp_thread`,
//! GPL-3.0-or-later, Copyright Zed Industries, Inc.): one thread model whose entries are updated in place by
//! session updates, tool calls found by their id and created by whichever message names them first, the
//! plan replaced whole, and Zed's tool call status vocabulary. It is written from that description; no code
//! of Zed's is copied.

mod messages;
mod tools;
mod turn;

#[cfg(test)]
mod tests;

use crate::model::{
    Location, OptionKind, PermissionOption, PlanEntry, StopReason, ToolCallFields, ToolContent,
    ToolKind, ToolStatus, Update,
};
use crate::policy::{Reason, Subject};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

/// One row of the conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable for the life of the thread, so the window can keep state per entry.
    pub id: u64,
    /// The entry came from a loaded session: it is shown at once, never animated.
    pub is_replayed: bool,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    /// What the person wrote.
    User(String),
    /// What the agent answers, as markdown.
    Assistant(Message),
    /// What the agent thinks before it answers.
    Thought(Message),
    Tool(ToolCall),
    /// The agent's plan, whole.
    Plan(Vec<PlanEntry>),
    /// A question only the person can answer.
    Permission(Permission),
    /// Why the conversation stopped.
    Problem(Problem),
}

/// Text that arrives in chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    /// More of it may still arrive.
    pub is_streaming: bool,
}

/// One tool call, as far as it is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    /// The agent's words for the call; empty until a message carries them.
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    pub locations: Vec<Location>,
    pub raw_input: Option<Value>,
}

impl ToolCall {
    /// The command of an `execute` call, when the agent gave it as `rawInput.command`.
    pub fn command(&self) -> Option<&str> {
        self.raw_input.as_ref()?.get("command")?.as_str()
    }
}

/// A permission request that reached the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permission {
    /// The connection's handle for the answer.
    pub request: u64,
    /// The id of the tool call it is about.
    pub tool_call: String,
    /// Why the confinement policy did not answer by itself.
    pub reason: Reason,
    /// What the request is about, as the policy read it when it arrived. The card shows this, not the
    /// call's entry, which later messages can change under the card.
    pub subject: Subject,
    pub options: Vec<PermissionOption>,
    pub outcome: PermissionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The agent waits for the person.
    Waiting,
    /// The person chose an option of this kind.
    Selected(OptionKind),
    /// The turn was cancelled before an answer.
    Cancelled,
}

/// Why the conversation stopped, in words the window turns into a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    /// The agent's or the system's own words, when there are any. Untrusted; shown, never interpreted.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// The agent's process could not be started.
    NotStarted,
    /// The agent's process ended or its connection broke.
    Died,
    /// The agent needs the account to sign in first.
    AuthRequired,
    /// The agent sent more than could be taken in, faster than the window read it.
    Overrun,
    /// The conversation holds as much as is kept: what the agent sends from here on is not shown.
    Full,
    /// The agent did not answer a request before its deadline.
    TimedOut,
    /// The agent answered a request with an error.
    RequestFailed,
    /// The model refused to go on.
    Refusal,
    /// The person stopped the turn: no failure, one quiet line where it ended.
    Stopped,
    /// The turn ran out of tokens.
    MaxTokens,
    /// The turn ran out of model requests.
    MaxTurnRequests,
}

/// What happens to a conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadEvent {
    /// The person sent a prompt: a turn starts.
    Prompted(String),
    /// The agent sent a session update.
    Update(Update),
    /// The agent asks for a permission that the person must give.
    PermissionAsked {
        request: u64,
        tool: ToolCallFields,
        reason: Reason,
        subject: Subject,
        options: Vec<PermissionOption>,
    },
    /// The agent requested a permission that the confinement policy gave once, without the person.
    PermissionAllowed { tool: ToolCallFields },
    /// A permission request was answered.
    PermissionAnswered {
        request: u64,
        outcome: PermissionOutcome,
    },
    /// The person asked to stop the turn.
    CancelRequested,
    /// The agent ended the turn.
    TurnEnded(StopReason),
    /// The turn or the connection failed.
    Failed(Problem),
    /// A stored session is about to be replayed: the thread starts again from nothing.
    ReplayStarted,
    /// The stored session has been replayed.
    ReplayFinished,
}

/// Whether the agent is working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Turn {
    #[default]
    Idle,
    Running,
    /// A stop was requested and the agent has not confirmed it yet.
    Cancelling,
}

/// What changed since the last [`Thread::take_changes`], by entry index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// Everything changed: draw the thread again from its entries.
    pub is_reset: bool,
    /// The first entry appended since the last take; entries from it on are new.
    pub first_new: Option<usize>,
    /// Entries before `first_new` whose content changed.
    pub updated: BTreeSet<usize>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        !self.is_reset && self.first_new.is_none() && self.updated.is_empty()
    }
}

/// How much of a conversation is kept. The agent's output is untrusted and a session can last days: past
/// either limit the conversation says once that it is full and takes in nothing more from the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Entries: messages, thoughts, tool calls, plans.
    pub entries: usize,
    /// Bytes of text held: messages, thoughts, and what tool calls show.
    pub bytes: usize,
}

/// The entries and bytes a conversation holds unless a test asks for less.
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_HELD_BYTES: usize = 64 * 1024 * 1024;

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            bytes: MAX_HELD_BYTES,
        }
    }
}

/// The conversation of one session.
#[derive(Debug, Default)]
pub struct Thread {
    entries: Vec<Entry>,
    limits: Limits,
    /// Bytes of text the entries hold.
    held_bytes: usize,
    /// The limits were reached: nothing more is taken in from the agent.
    is_full: bool,
    next_id: u64,
    /// Where each tool call's entry is, by the call's id.
    tool_index: HashMap<String, usize>,
    turn: Turn,
    is_replaying: bool,
    changes: Changes,
}

impl Thread {
    pub fn new() -> Self {
        Self::default()
    }

    /// A conversation that holds at most `limits`.
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Whether the limits were reached and the agent's output is no longer taken in.
    pub fn is_full(&self) -> bool {
        self.is_full
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn turn(&self) -> Turn {
        self.turn
    }

    /// Whether a permission request waits for the person: the task's header says so.
    pub fn waits_for_person(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(&entry.kind, EntryKind::Permission(permission)
                if permission.outcome == PermissionOutcome::Waiting)
        })
    }

    /// Applies one event.
    pub fn apply(&mut self, event: ThreadEvent) {
        match event {
            ThreadEvent::Prompted(text) => self.prompted(text),
            ThreadEvent::Update(update) => self.update(update),
            ThreadEvent::PermissionAsked {
                request,
                tool,
                reason,
                subject,
                options,
            } => self.permission_asked(request, tool, (reason, subject), options),
            ThreadEvent::PermissionAllowed { tool } => self.permission_allowed(tool),
            ThreadEvent::PermissionAnswered { request, outcome } => {
                self.permission_answered(request, outcome)
            }
            ThreadEvent::CancelRequested => self.cancel_requested(),
            ThreadEvent::TurnEnded(reason) => self.turn_ended(reason),
            ThreadEvent::Failed(problem) => self.failed(problem),
            ThreadEvent::ReplayStarted => self.replay_started(),
            ThreadEvent::ReplayFinished => self.replay_finished(),
        }
    }

    /// Replaces the thread with a stored session's updates, in one pass.
    pub fn replay(&mut self, updates: impl IntoIterator<Item = Update>) {
        self.replay_started();
        for update in updates {
            self.update(update);
        }
        self.replay_finished();
    }

    /// What changed since the last call.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    /// Takes in what the agent sent, unless the conversation is full; says so once when it becomes full.
    fn update(&mut self, update: Update) {
        self.close_if_full();
        if self.is_full {
            return;
        }
        self.take_in(update);
        self.close_if_full();
    }

    /// The conversation is full once it holds as many entries or bytes as it may: a note says so, and
    /// from then on only the person's prompts and the ends of turns are added.
    fn close_if_full(&mut self) {
        let is_reached =
            self.entries.len() >= self.limits.entries || self.held_bytes >= self.limits.bytes;
        if self.is_full || !is_reached {
            return;
        }
        self.is_full = true;
        self.settle_streaming();
        self.push(EntryKind::Problem(Problem {
            kind: ProblemKind::Full,
            detail: String::new(),
        }));
    }

    /// `text` cut to the room left under the byte limit, on a character boundary; that room is taken.
    fn take_room(&mut self, mut text: String) -> String {
        let room = self.limits.bytes.saturating_sub(self.held_bytes);
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        self.held_bytes += text.len();
        text
    }

    fn take_in(&mut self, update: Update) {
        match update {
            Update::UserText(text) => self.user_text(text),
            Update::AgentText(text) => self.agent_text(text, false),
            Update::Thought(text) => self.agent_text(text, true),
            Update::ToolCall(fields) => {
                self.upsert_tool(fields, true);
            }
            Update::ToolCallUpdate(fields) => {
                self.upsert_tool(fields, false);
            }
            Update::Plan(entries) => self.plan(entries),
            Update::Ignored => {}
        }
    }

    /// Appends an entry and returns its index.
    fn push(&mut self, kind: EntryKind) -> usize {
        let index = self.entries.len();
        self.entries.push(Entry {
            id: self.next_id,
            is_replayed: self.is_replaying,
            kind,
        });
        self.next_id += 1;
        self.changes.first_new.get_or_insert(index);
        index
    }

    /// Notes that the entry at `index` changed.
    fn touch(&mut self, index: usize) {
        let is_new = self.changes.first_new.is_some_and(|first| index >= first);
        if !is_new {
            self.changes.updated.insert(index);
        }
    }
}
