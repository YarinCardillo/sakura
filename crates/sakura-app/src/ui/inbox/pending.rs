//! Answers the person pressed that wait a few seconds before they are sent, so that Undo can take one back.
//! An answer is held for [`HOLD`], then sent through the window's answers. An answer that asks to be confirmed
//! first shows the engine's question in the hold, and runs only with the token of that question. When the
//! window closes or the app quits, what may be sent is sent at once.

use crate::data::engine_facts::Pending;
use crate::data::source::Source;
use crate::theme::inbox::CONFIRM_BOUND;
use gpui::SharedString;
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::engine::snapshot::Action;
use std::collections::{BTreeMap, HashMap};

/// One answer waiting to be sent: the engine's action, what it applies to and what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldAct {
    /// What the answer is kept under in the window's answers.
    pub key: String,
    pub action: Action,
    pub target: String,
    pub inputs: BTreeMap<String, String>,
}

impl HeldAct {
    pub(super) fn request(&self) -> Act {
        Act {
            action: self.action.clone(),
            target: self.target.clone(),
            inputs: self.inputs.clone(),
            confirm: None,
        }
    }
}

/// The engine summary and token bound to the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub token: String,
    pub summary: String,
}

/// An answer taken out of the hold: what was pressed and, for an answer that asks to be confirmed, what the
/// engine asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Released {
    pub act: HeldAct,
    pub asked: Option<Asked>,
    /// The toast that offered Undo for it, when one did.
    pub toast: Option<u64>,
    pub success: SharedString,
}

struct Held {
    ticket: u64,
    act: HeldAct,
    words: SharedString,
    success: SharedString,
    asked: Option<Asked>,
    toast: Option<u64>,
}

/// The answers held now, by the ask each is about. Holding again an ask already held replaces its answer.
/// Each is kept with the words the Undo toast says of it, which a card of the ask can say too.
pub struct HeldActs {
    source: Source,
    held: HashMap<String, Held>,
    next_ticket: u64,
}

impl HeldActs {
    /// Nothing held; answers that must go at once are sent through `source`.
    pub fn new(source: Source) -> Self {
        Self {
            source,
            held: HashMap::new(),
            next_ticket: 0,
        }
    }

    /// Holds `act` as the answer to `ask`, said as `words`, and says which hold it is: only that hold
    /// releases it.
    pub fn hold(
        &mut self,
        ask: &str,
        act: HeldAct,
        words: SharedString,
        success: SharedString,
    ) -> u64 {
        self.next_ticket += 1;
        let held = Held {
            ticket: self.next_ticket,
            act,
            words,
            success,
            asked: None,
            toast: None,
        };
        self.held.insert(ask.to_string(), held);
        self.next_ticket
    }

    /// The engine's first call of an answer that asks to be confirmed: it returns the question and a token and
    /// runs nothing.
    pub fn ask(&self, act: Act) -> Pending<ActResult> {
        self.source.act(act)
    }

    /// Keeps what the engine asked of the answer held to `ask` under `ticket`, now said as `words`. False
    /// when that hold is gone (undone or replaced meanwhile), and the question is dropped with its token.
    pub fn set_asked(&mut self, ask: &str, ticket: u64, asked: Asked, words: SharedString) -> bool {
        match self.held.get_mut(ask) {
            Some(held) if held.ticket == ticket => {
                held.asked = Some(asked);
                held.words = words;
                true
            }
            _ => false,
        }
    }

    /// Notes the toast that offers Undo for the answer held to `ask` under `ticket`.
    pub fn set_toast(&mut self, ask: &str, ticket: u64, toast: u64) {
        if let Some(held) = self.held.get_mut(ask).filter(|held| held.ticket == ticket) {
            held.toast = Some(toast);
        }
    }

    /// Whether `ticket` is still the hold of the answer to `ask`.
    pub fn is_hold(&self, ask: &str, ticket: u64) -> bool {
        self.held.get(ask).is_some_and(|held| held.ticket == ticket)
    }

    /// Takes back the answer to `ask` when `ticket` is still its hold: the hold's time is up.
    pub fn release(&mut self, ask: &str, ticket: u64) -> Option<Released> {
        match self.held.get(ask) {
            Some(held) if held.ticket == ticket => self.take_released(ask),
            _ => None,
        }
    }

    /// Takes back the answer to `ask`, whichever hold it is: Undo. A question it was asked is dropped with
    /// its token; nothing ran.
    pub fn take(&mut self, ask: &str) -> Option<HeldAct> {
        self.take_released(ask).map(|released| released.act)
    }

    /// What is held as the answer to `ask`, when one is.
    pub fn act_of(&self, ask: &str) -> Option<HeldAct> {
        self.held.get(ask).map(|held| held.act.clone())
    }

    /// What the answer held to `ask` says, when one is held.
    pub fn words_of(&self, ask: &str) -> Option<SharedString> {
        self.held.get(ask).map(|held| held.words.clone())
    }

    /// Whether an answer to `ask` is held.
    pub fn is_held(&self, ask: &str) -> bool {
        self.held.contains_key(ask)
    }

    /// The asks whose answers are held now.
    pub fn asks(&self) -> Vec<String> {
        self.held.keys().cloned().collect()
    }

    /// Sends every held answer that may go now, and holds none. An answer that does not ask to be confirmed
    /// goes as one call. One that does goes only when its question was already shown with its token, and
    /// then only if the engine still asks the same question (within [`CONFIRM_BOUND`]); any other stays
    /// unanswered and open in the Inbox. The calls start at once and go on when the results are dropped; the
    /// results are for whoever has to wait for them.
    pub fn flush(&mut self) -> Vec<Pending<ActResult>> {
        let held = std::mem::take(&mut self.held);
        held.into_values()
            .filter_map(|held| self.send_now(held))
            .collect()
    }

    fn send_now(&self, held: Held) -> Option<Pending<ActResult>> {
        let HeldAct {
            action,
            target,
            inputs,
            ..
        } = held.act;
        let mut act = Act {
            action,
            target,
            inputs,
            confirm: None,
        };
        match (act.action.confirm, held.asked) {
            (false, _) => Some(self.source.act_within(act, CONFIRM_BOUND)),
            (true, Some(asked)) => {
                act.confirm = Some(asked.token);
                Some(self.source.act_confirmed(act, asked.summary, CONFIRM_BOUND))
            }
            (true, None) => None,
        }
    }

    fn take_released(&mut self, ask: &str) -> Option<Released> {
        self.held.remove(ask).map(|held| Released {
            act: held.act,
            asked: held.asked,
            toast: held.toast,
            success: held.success,
        })
    }
}
