//! The life of a turn: a prompt starts it, the agent or a failure ends it, a cancel marks at once what was
//! still running; and the replay of a stored session.

use super::{EntryKind, Problem, ProblemKind, Thread, Turn};
use crate::model::StopReason;

impl Thread {
    pub(super) fn prompted(&mut self, text: String) {
        self.settle_streaming();
        self.push(EntryKind::User(text));
        self.turn = Turn::Running;
    }

    /// The person asked to stop: unfinished calls and open questions are cancelled at once, as the protocol
    /// asks, without waiting for the agent. Updates the agent still sends before it confirms are applied.
    pub(super) fn cancel_requested(&mut self) {
        if self.turn == Turn::Running {
            self.turn = Turn::Cancelling;
        }
        self.settle_streaming();
        self.stop_unfinished();
    }

    pub(super) fn turn_ended(&mut self, reason: StopReason) {
        self.close_turn();
        let kind = match reason {
            StopReason::EndTurn | StopReason::Cancelled => return,
            StopReason::Refusal => ProblemKind::Refusal,
            StopReason::MaxTokens => ProblemKind::MaxTokens,
            StopReason::MaxTurnRequests => ProblemKind::MaxTurnRequests,
        };
        let detail = match kind {
            ProblemKind::Refusal => self.take_refused_words(),
            _ => String::new(),
        };
        self.push(EntryKind::Problem(Problem { kind, detail }));
    }

    /// The provider says why it refused in a message that ends the turn: those words are no answer of
    /// the agent, so they leave the conversation and are kept as the refusal's detail.
    fn take_refused_words(&mut self) -> String {
        if !matches!(
            self.entries.last().map(|entry| &entry.kind),
            Some(EntryKind::Assistant(_))
        ) {
            return String::new();
        }
        let Some(EntryKind::Assistant(message)) = self.entries.pop().map(|entry| entry.kind) else {
            return String::new();
        };
        self.changes.is_reset = true;
        message.text
    }

    pub(super) fn failed(&mut self, problem: Problem) {
        self.close_turn();
        self.push(EntryKind::Problem(problem));
    }

    pub(super) fn replay_started(&mut self) {
        *self = Thread {
            is_replaying: true,
            next_id: self.next_id,
            limits: self.limits,
            ..Thread::default()
        };
        self.changes.is_reset = true;
    }

    /// Nothing of a stored session is still running.
    pub(super) fn replay_finished(&mut self) {
        self.is_replaying = false;
        self.close_turn();
        self.changes.is_reset = true;
    }

    fn close_turn(&mut self) {
        self.turn = Turn::Idle;
        self.settle_streaming();
        self.stop_unfinished();
    }
}
