//! What the command palette offers on the selected ask, and what choosing one of those verbs does: the
//! same as pressing the verb's button, so the answer is held for a few seconds under the Undo toast and
//! only then sent.

use super::model::{Say, Verb};
use super::InboxPage;
use crate::l10n::{tr, Key};
use crate::ui::palette::engine::{AskVerb, SelectedAsk};
use gpui::{App, Context, Window};

/// The name of the verb that replies in words, in the palette's asks.
const REPLY: &str = "reply";

impl InboxPage {
    /// The ask selected in the Open list, with the verbs the palette lists for it. None while the Done list
    /// is shown or nothing is selected.
    pub fn selected_ask_for_palette(&self, cx: &App) -> Option<SelectedAsk> {
        if self.filter.snoozed {
            return None;
        }
        let snapshot = self.snapshot(cx)?;
        let wait = self.selected_ask(&snapshot)?;
        let verbs = Self::verbs_of(wait, &snapshot);
        let mut offered: Vec<AskVerb> = verbs
            .primary
            .iter()
            .chain(&verbs.secondary)
            .map(offer)
            .collect();
        if verbs.reply.is_some() {
            let label = tr(Key(if wait.original_kind() == "track_decision" {
                "track.answer"
            } else {
                "inbox.reply"
            }))
            .to_string();
            offered.push(AskVerb {
                verb: REPLY.into(),
                label,
            });
        }
        Some(SelectedAsk {
            wait: wait.id.clone(),
            verbs: offered,
        })
    }

    /// Answers the ask `wait` with the verb the palette named, when it is still the ask selected and still
    /// offers that verb. Anything else is told on stderr and changes nothing.
    pub fn answer_with_verb(
        &mut self,
        wait: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let found = self
            .selected_ask(&snapshot)
            .filter(|selected| selected.id == wait)
            .map(|selected| Self::verbs_of(selected, &snapshot));
        let Some(verbs) = found else {
            return eprintln!("sakura: the ask {wait:?} is no longer the one selected");
        };
        if name == REPLY {
            return self.press_reply(window, cx);
        }
        let chosen = verbs
            .primary
            .into_iter()
            .chain(verbs.secondary)
            .find(|verb| verb_name(verb.say) == name);
        match chosen {
            Some(verb) => self.press(verb, window, cx),
            None => eprintln!("sakura: the ask {wait:?} does not offer {name:?}"),
        }
    }
}

fn offer(verb: &Verb) -> AskVerb {
    AskVerb {
        verb: verb_name(verb.say).into(),
        label: verb.label().to_string(),
    }
}

/// The palette's name for what a verb says.
fn verb_name(say: Say) -> &'static str {
    match say {
        Say::Approve => "approve",
        Say::AlwaysApprove => "always-approve",
        Say::Decline => "decline",
        Say::Resume => "resume",
        Say::Stop => "stop",
        Say::Close => "close",
        Say::Restore => "restore",
        Say::Review => "markreviewed",
        Say::Start => "start",
        Say::Dismiss => "dismiss",
    }
}
