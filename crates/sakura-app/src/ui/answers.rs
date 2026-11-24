//! The person's answers to the engine, from any page: an action sent through `sakura cockpit act`, what
//! it asks in words before a confirmed action runs (never the engine's command line), and what the engine
//! said back, kept under what it was about while that is still listed. The engine decides; this keeps
//! only the conversation of one answer: sent, asked to confirm, told.

use super::chrome::{control, secondary_variant, text};
use super::confirm_words::confirm_words;
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::inbox::CONFIRM_BOUND;
use crate::theme::{self, Palette};
use gpui::{
    div, AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Styled, Subscription,
};
use gpui_component::button::{Button, ButtonVariants as _};
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::engine::resume_note;
use sakura_core::engine::snapshot::{Action, Snapshot};
use sakura_core::error::BridgeResult;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where an answer of the person stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The Stop token was requested. The engine has yet to return the question.
    PreparingStop,
    /// Sent; the engine has not answered.
    Running,
    /// The engine requested a confirmation: the question in words, and the call that runs it.
    Confirming { act: Act, words: String },
    /// The engine's last word on it.
    Told { text: String, is_problem: bool },
}

/// The answers of a window, by what they are about: a wait's id, or a lane's id and the action.
pub struct Answers {
    facts: Entity<EngineFacts>,
    source: Source,
    steps: HashMap<String, Step>,
    /// The texts whose "Details" are open (an error's raw text), by the key the text is shown under.
    open_details: std::collections::HashSet<String>,
    _observer: Subscription,
}

impl Answers {
    /// Over `facts`, answering through `source`.
    pub fn new(facts: Entity<EngineFacts>, source: Source, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&facts, |answers, facts, cx| {
            answers.forget_resolved(&facts, cx)
        });
        Self {
            facts,
            source,
            steps: HashMap::new(),
            open_details: std::collections::HashSet::new(),
            _observer: observer,
        }
    }

    /// Whether the "Details" shown under `key` are open.
    pub fn is_details_open(&self, key: &str) -> bool {
        self.open_details.contains(key)
    }

    /// Opens the "Details" shown under `key`, or closes them.
    pub fn toggle_details(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.open_details.remove(key) {
            self.open_details.insert(key.to_string());
        }
        cx.notify();
    }

    /// Where the answer kept under `key` stands, when there is one.
    pub fn step(&self, key: &str) -> Option<&Step> {
        self.steps.get(key)
    }

    /// Whether the answer under `key` is on its way.
    pub fn is_running(&self, key: &str) -> bool {
        matches!(
            self.steps.get(key),
            Some(Step::PreparingStop | Step::Running)
        )
    }

    /// What the engine told of the answer under `key`.
    #[cfg(test)]
    pub fn told(&self, key: &str) -> Option<&str> {
        match self.steps.get(key) {
            Some(Step::Told { text, .. }) => Some(text),
            _ => None,
        }
    }

    /// What the answer under `key` asks the person to confirm, in words.
    #[cfg(test)]
    pub fn summary(&self, key: &str) -> Option<&str> {
        match self.steps.get(key) {
            Some(Step::Confirming { words, .. }) => Some(words),
            _ => None,
        }
    }

    /// Sends the first call of `action` on `target`; an action that asks answers with a question.
    pub fn press(
        &mut self,
        key: String,
        action: Action,
        target: String,
        inputs: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let act = Act {
            action,
            target,
            inputs,
            confirm: None,
        };
        if !is_lane_stop(&key, &act) || !self.is_running(&key) {
            self.send(key, act, cx);
        }
    }

    /// An answer whose question the person was shown with the token in `act.confirm`: the question is asked
    /// again and the answer runs only if it is the same (`summary`); a changed question comes back as a
    /// question to confirm, with its own token, and nothing ran.
    pub fn press_after_question(
        &mut self,
        key: String,
        act: Act,
        summary: String,
        cx: &mut Context<Self>,
    ) {
        self.steps.insert(key.clone(), Step::Running);
        let pending = self
            .source
            .act_confirmed(act.clone(), summary, CONFIRM_BOUND);
        let asked = Act {
            confirm: None,
            ..act
        };
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |answers, cx| answers.answered(key, asked, result, cx)));
        })
        .detach();
        cx.notify();
    }

    /// The engine's answer to what was sent for `key` without the window's own call: kept as if the answer
    /// had come back to this window's call.
    pub fn deliver(
        &mut self,
        key: String,
        act: Act,
        result: BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        self.answered(key, act, result, cx);
    }

    /// The person confirmed what the engine summarised: the same call, with its token.
    pub fn confirm(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(Step::Confirming { act, .. }) = self.steps.remove(key) {
            self.send(key.to_string(), act, cx);
        }
    }

    /// The person left what the engine summarised: nothing runs.
    pub fn cancel(&mut self, key: &str, cx: &mut Context<Self>) {
        self.steps.remove(key);
        cx.notify();
    }

    fn send(&mut self, key: String, act: Act, cx: &mut Context<Self>) {
        let step = if act.confirm.is_none() && act.action.confirm && is_lane_stop(&key, &act) {
            Step::PreparingStop
        } else {
            Step::Running
        };
        self.steps.insert(key.clone(), step);
        let pending = self.source.act(act.clone());
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |answers, cx| answers.answered(key, act, result, cx)));
        })
        .detach();
        cx.notify();
    }

    fn answered(
        &mut self,
        key: String,
        act: Act,
        result: BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        let is_resume = act.action.key == "R";
        let step = match result {
            Ok(ActResult::Confirm { token, summary, .. }) => {
                let snapshot = self.facts.read(cx).snapshot();
                let words = confirm_words(snapshot, &act, &summary).to_string();
                Step::Confirming {
                    act: Act {
                        confirm: Some(token),
                        ..act
                    },
                    words,
                }
            }
            Ok(ActResult::Done {
                exit_code,
                output,
                audit,
                ..
            }) => {
                self.facts.update(cx, |facts, cx| facts.read(cx));
                let outcome = if exit_code == 0 {
                    self.lane_outcome(&key, &act, cx)
                } else {
                    None
                };
                done(
                    exit_code,
                    outcome.as_deref().unwrap_or(&output),
                    audit.as_deref(),
                )
            }
            Ok(ActResult::Refused {
                reason,
                reason_code,
                reason_args,
            }) => {
                let text = if act.is_track() {
                    let said = sakura_core::engine::tracks::refusal(
                        reason_code.as_deref(),
                        reason_args.as_ref(),
                        &reason,
                    );
                    crate::l10n::say(&said).to_string()
                } else {
                    reason
                };
                Step::Told {
                    text,
                    is_problem: true,
                }
            }
            Err(error) => Step::Told {
                text: crate::l10n::told(&error),
                is_problem: true,
            },
        };
        self.steps.insert(
            key,
            if is_resume {
                plain_resume_step(step)
            } else {
                step
            },
        );
        cx.notify();
    }

    fn lane_outcome(&self, key: &str, act: &Act, cx: &App) -> Option<SharedString> {
        let is_stop = is_lane_stop(key, act);
        let is_close = super::lane_page::is_close_action(&act.action.key)
            && key == lane_key(&act.target, &act.action.key);
        if !is_stop && !is_close {
            return None;
        }
        let item = self
            .facts
            .read(cx)
            .snapshot()
            .and_then(|snapshot| snapshot.items.iter().find(|item| item.id == act.target));
        Some(match item {
            Some(item) => {
                let name = super::lanes::lane_words(item, false);
                let message = if is_stop {
                    "lane.result.stop_requested"
                } else if item.is_digest() {
                    "lane.result.archived"
                } else {
                    "lane.result.closed"
                };
                tr_with(Key(message), &[("lane", &name)])
            }
            None => tr(Key("waiting.done")).into(),
        })
    }

    /// An answer is kept while what it is about is listed: once the engine has moved on, so has it.
    fn forget_resolved(&mut self, facts: &Entity<EngineFacts>, cx: &mut Context<Self>) {
        let snapshot = facts.read(cx).snapshot();
        let ids: HashSet<&str> = snapshot.into_iter().flat_map(listed_ids).collect();
        let before = self.steps.len();
        let is_listed = |key: &str| {
            ids.iter()
                .any(|id| key == *id || key.starts_with(&format!("{id}#")))
        };
        self.steps.retain(|key, step| {
            matches!(step, Step::PreparingStop | Step::Running) || is_listed(key)
        });
        if self.steps.len() != before {
            cx.notify();
        }
    }
}

fn is_lane_stop(key: &str, act: &Act) -> bool {
    act.action.key == "s" && key == lane_key(&act.target, &act.action.key)
}

fn plain_resume_step(step: Step) -> Step {
    let Step::Told {
        text,
        is_problem: true,
    } = step
    else {
        return step;
    };
    let missing = tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING));
    let reason = text
        .split_once(resume_note::REFUSAL_PREFIX)
        .map(|(_, reason)| reason.trim());
    let text = match reason {
        _ if text.starts_with("no longer waiting:") => {
            tr(Key("inbox.note.lane_changed")).to_string()
        }
        Some(resume_note::HIDDEN_CHARACTER_REASON) => {
            tr(Key("inbox.note.hidden_character")).to_string()
        }
        Some(reason) if !reason.is_empty() => {
            tr_with(Key("inbox.note.refused"), &[("reason", reason)]).to_string()
        }
        _ if text == missing => text,
        _ => tr(Key("inbox.note.resume_failed")).to_string(),
    };
    Step::Told {
        text,
        is_problem: true,
    }
}

/// The ids of what an answer can be about: the waits and the lanes.
fn listed_ids(snapshot: &Snapshot) -> impl Iterator<Item = &str> {
    let waits = snapshot.waits.iter().map(|wait| wait.id.as_str());
    waits.chain(snapshot.items.iter().map(|item| item.id.as_str()))
}

/// The key under which the answer to an action of a lane is kept.
pub fn lane_key(item: &str, action: &str) -> String {
    format!("{item}#{action}")
}

/// The engine's word that a command ran: its output's first line, or that it did; a command that failed
/// says its code. A note that the log could not be written follows, since the command ran all the same.
fn done(exit_code: i64, output: &str, audit: Option<&str>) -> Step {
    let line = output.lines().map(str::trim).find(|line| !line.is_empty());
    let mut text = match (exit_code, line) {
        (0, Some(line)) => line.to_string(),
        (0, None) => tr(Key("waiting.done")).to_string(),
        (code, Some(line)) => format!("{} {line}", failed_with(code)),
        (code, None) => failed_with(code).to_string(),
    };
    if let Some(audit) = audit {
        text = format!("{text} ({audit})");
    }
    Step::Told {
        text,
        is_problem: exit_code != 0,
    }
}

fn failed_with(code: i64) -> SharedString {
    tr_with(Key("waiting.failed"), &[("code", &code.to_string())])
}

/// What the engine said to the person's last answer under `key`: nothing sent yet, the question to
/// confirm, or its word.
pub fn step_view(
    answers: &Entity<Answers>,
    key: &str,
    step: &Step,
    palette: &Palette,
    cx: &App,
) -> AnyElement {
    match step {
        Step::PreparingStop => div().into_any_element(),
        Step::Running => text(theme::TEXT_SMALL, palette.text_tertiary)
            .debug_selector({
                let selector = format!("sending-{key}");
                move || selector
            })
            .child(tr(Key("waiting.sending")))
            .into_any_element(),
        Step::Confirming { words, .. } => confirmation(answers, key, words, palette, cx),
        Step::Told {
            text: said,
            is_problem,
        } => {
            let colour = if *is_problem {
                palette.danger
            } else {
                palette.text_secondary
            };
            let selector = format!("told-{key}");
            let plain = match is_problem {
                true => crate::ui::plain_error::plain_of(said, ""),
                false => crate::ui::plain_error::Plain {
                    words: said.clone(),
                    raw: None,
                },
            };
            let words = text(theme::TEXT_SMALL, colour)
                .debug_selector(move || selector)
                .child(plain.words.clone());
            let details = plain.raw.as_deref().map(|raw| {
                crate::ui::plain_error::details_view("told-details", key, raw, answers, palette, cx)
            });
            div()
                .flex()
                .flex_col()
                .gap(theme::GAP_TIGHT)
                .child(words)
                .children(details)
                .into_any_element()
        }
    }
}

/// The question in words, and the two ways out.
fn confirmation(
    answers: &Entity<Answers>,
    key: &str,
    words: &str,
    palette: &Palette,
    cx: &App,
) -> AnyElement {
    let (confirm_id, cancel_id) = (format!("confirm-{key}"), format!("cancel-{key}"));
    let (confirm_selector, cancel_selector) = (confirm_id.clone(), cancel_id.clone());
    let (to_confirm, to_cancel, kept) = (answers.clone(), answers.clone(), key.to_string());
    let (cancel_key, summary_selector) = (key.to_string(), format!("summary-{key}"));
    let confirm = Button::new(SharedString::from(confirm_id))
        .debug_selector(move || confirm_selector)
        .custom(secondary_variant(cx))
        .bg(crate::theme::Theme::palette(cx).fill);
    let confirm = control(
        confirm,
        tr(Key("waiting.confirm")),
        &[tr(Key("waiting.confirm"))],
    )
    .on_click(move |_, _, cx| to_confirm.update(cx, |answers, cx| answers.confirm(&kept, cx)));
    let cancel = Button::new(SharedString::from(cancel_id))
        .debug_selector(move || cancel_selector)
        .custom(secondary_variant(cx))
        .bg(crate::theme::Theme::palette(cx).fill);
    let cancel = control(
        cancel,
        tr(Key("common.cancel")),
        &[tr(Key("common.cancel"))],
    )
    .on_click(move |_, _, cx| to_cancel.update(cx, |answers, cx| answers.cancel(&cancel_key, cx)));
    div()
        .flex()
        .flex_col()
        .gap(theme::GAP_SMALL)
        .child(
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(move || summary_selector)
                .child(words.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme::GAP)
                .child(confirm)
                .child(cancel),
        )
        .into_any_element()
}
