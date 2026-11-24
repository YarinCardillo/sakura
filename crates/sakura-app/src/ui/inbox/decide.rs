//! Deciding an ask: pressing one of its verbs holds the answer for a few seconds under an Undo toast, then
//! sends it through the window's answers. A decline or a reply first opens the note field, the one place the
//! person writes anything on this page.

use super::model::{self, Answer, Say, Verb, Verbs};
use super::pending::{Asked, HeldAct, Released};
use super::{InboxPage, Shown};
use crate::l10n::{tr, tr_count, tr_with, Key};
use crate::theme::inbox::HOLD;
use crate::theme::kit::TOAST_TICK;
use crate::ui::answers::Step;
use crate::ui::confirm_words::{confirm_statement, plain_summary, summary_adds_words};
use crate::ui::kit::Toast;
use gpui::{App, AppContext as _, Context, Entity, SharedString, WeakEntity, Window};
use gpui_component::input::{InputState, TextareaState};
use sakura_core::engine::cli::{Act, ActResult};
use sakura_core::engine::resume_note;
use sakura_core::engine::snapshot::{Action, Snapshot, Wait};
use sakura_core::error::BridgeResult;
use std::collections::BTreeMap;

const INPUT_NOTE: &str = "note";
const INPUT_TEXT: &str = "text";
const INPUT_CHOICE: &str = "choice";

/// What the note field is being written for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NoteFor {
    /// A decline that takes a note: the note goes with it.
    Decline(Verb),
    Resume,
    /// A reply in words: the words are the answer.
    Reply,
    Track,
}

/// The note being written, and the ask it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Note {
    pub ask: String,
    pub purpose: NoteFor,
    pub resume_key: Option<ResumeKey>,
}

pub(super) type ResumeKey = (String, String);

pub(super) struct ResumeDraft {
    field: Entity<TextareaState>,
    answer_key: Option<String>,
    operation: crate::ui::track_page::Operation,
    _observer: gpui::Subscription,
}

pub(super) struct ReadMark {
    is_read: bool,
    was_read: Option<bool>,
}

pub(super) struct SentAnswer {
    pub(super) key: String,
    revision: String,
    raised_at: Option<i64>,
    sent_at: i64,
    lane_state: Option<String>,
    notice: Option<(String, SharedString)>,
}

impl SentAnswer {
    fn new(key: String, wait: &Wait, snapshot: &Snapshot) -> Self {
        Self {
            key,
            revision: wait.key_revision().to_string(),
            raised_at: model::raised(wait),
            sent_at: crate::ui::unix_now(),
            lane_state: model::lane_of(wait, snapshot).map(|lane| lane.agent_state.clone()),
            notice: None,
        }
    }

    fn still_waiting(&self, ask: &str, snapshot: &Snapshot) -> bool {
        snapshot
            .waits
            .iter()
            .find(|wait| wait.id == ask)
            .is_some_and(|wait| {
                let lane_state =
                    model::lane_of(wait, snapshot).map(|lane| lane.agent_state.as_str());
                let raised = model::raised(wait);
                let raised_again = raised > self.raised_at && raised >= Some(self.sent_at);
                self.revision == wait.key_revision()
                    && self.lane_state.as_deref() == lane_state
                    && !raised_again
            })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Notice {
    Success,
    Confirmation,
    Failure,
}

impl InboxPage {
    /// Counts the listed asks whose answers are neither held for Undo nor pending engine acknowledgement.
    pub fn open_asks(&self, snapshot: &Snapshot, cx: &gpui::App) -> usize {
        let now = super::list::now();
        snapshot
            .waits
            .iter()
            .filter(|wait| {
                !self.is_away(&wait.id, cx)
                    && !wait.is_snoozed_at(now)
                    && self.kept_snooze(wait).is_none()
            })
            .count()
    }

    /// Whether the answer to `ask` is held for Undo or has been sent and not yet told.
    pub(super) fn is_away(&self, ask: &str, cx: &gpui::App) -> bool {
        self.held.is_held(ask) || self.is_sending(ask, cx)
    }

    /// Whether the sent answer still hides the ask it was for.
    pub(super) fn is_sending(&self, ask: &str, cx: &gpui::App) -> bool {
        let answers = self.answers.read(cx);
        self.sending.get(ask).is_some_and(|sent| {
            matches!(
                answers.step(&sent.key),
                Some(
                    Step::Running
                        | Step::Told {
                            is_problem: false,
                            ..
                        }
                )
            )
        })
    }

    pub(super) fn is_hidden(&self, ask: &str, cx: &App) -> bool {
        self.is_sending(ask, cx) && !self.has_resume_note(ask)
    }

    fn has_resume_note(&self, ask: &str) -> bool {
        self.note.as_ref().is_some_and(|note| {
            note.ask == ask && matches!(note.purpose, NoteFor::Resume | NoteFor::Track)
        })
    }

    pub(super) fn resume_note_pending(&self, note: &Note, cx: &App) -> bool {
        self.is_answered(&note.ask, cx)
            || self.resume_answer_key(note).is_some_and(|key| {
                matches!(
                    self.answers.read(cx).step(key),
                    Some(Step::Running | Step::Confirming { .. })
                )
            })
    }

    /// Whether `ask` has been answered, held or sent: a second press on it is ignored, so a double click, a
    /// held key or a press on a button under a moved list never answers twice.
    pub(super) fn is_answered(&self, ask: &str, cx: &gpui::App) -> bool {
        self.is_away(ask, cx)
    }

    /// The ask selected in the Open list, when it is one.
    pub(super) fn selected_ask<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a Wait> {
        let id = self
            .selected
            .as_deref()
            .filter(|_| self.shown == Shown::Open)?;
        snapshot.waits.iter().find(|wait| wait.id == id)
    }

    /// What the engine lets the person do on `wait`.
    pub(super) fn verbs_of(wait: &Wait, snapshot: &Snapshot) -> Verbs {
        model::verbs(wait, model::lane_of(wait, snapshot))
    }

    pub(super) fn open_row(&mut self, id: String, cx: &mut Context<Self>) {
        self.select(id, cx);
        self.mark_opened(cx);
    }

    pub(super) fn mark_opened(&mut self, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.snapshot(cx) {
            if self
                .selected_ask(&snapshot)
                .is_some_and(|wait| !self.is_read(wait))
            {
                self.attention("read", None, cx);
            }
        }
    }

    pub(super) fn toggle_read(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(wait) = self.selected_ask(&snapshot) else {
            return;
        };
        self.attention(if self.is_read(wait) { "unread" } else { "read" }, None, cx);
    }

    pub(super) fn attention(&mut self, key: &str, until: Option<&str>, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(wait) = self.selected_ask(&snapshot) else {
            return;
        };
        let Some(action) = wait.actions.iter().find(|action| action.key == key) else {
            return;
        };
        let result_key = crate::ui::answers::lane_key(&wait.id, "attention");
        if self.filter.snoozed != (key == super::snooze::UNSNOOZE)
            || self.is_answered(&wait.id, cx)
            || self.answers.read(cx).is_running(&result_key)
        {
            return;
        }
        if matches!(key, "read" | "unread") {
            self.read_marks.insert(
                wait.id.clone(),
                ReadMark {
                    is_read: key == "read",
                    was_read: Some(self.is_read(wait)),
                },
            );
        }
        let mut inputs = BTreeMap::new();
        if let Some(until) = until {
            let Some(deadline) = super::snooze::deadline(until, chrono::Local::now()) else {
                return;
            };
            let mut wait = wait.clone();
            wait.is_snoozed = Some(true);
            wait.snoozed_until = Some(deadline.clone());
            self.snoozes.insert(
                wait.id.clone(),
                super::snooze::Snooze {
                    wait,
                    is_pending: true,
                },
            );
            inputs.insert("until".into(), deadline);
        }
        self.answers.update(cx, |answers, cx| {
            answers.press(result_key, action.clone(), wait.id.clone(), inputs, cx);
        });
        cx.notify();
    }

    pub(super) fn is_read(&self, wait: &Wait) -> bool {
        self.read_marks
            .get(&wait.id)
            .map_or(wait.is_read, |mark| mark.was_read.unwrap_or(mark.is_read))
    }

    pub(super) fn settle_read_marks(&mut self, cx: &App) {
        let answers = self.answers.read(cx);
        for (id, mark) in &mut self.read_marks {
            let key = crate::ui::answers::lane_key(id, "attention");
            let Some(previous) = mark.was_read else {
                continue;
            };
            match answers.step(&key) {
                Some(Step::Running) => continue,
                Some(Step::Told {
                    is_problem: false, ..
                }) => {}
                _ => mark.is_read = previous,
            }
            mark.was_read = None;
        }
        self.reconcile_read_marks(cx);
    }

    pub(super) fn reconcile_read_marks(&mut self, cx: &App) {
        if let Some(snapshot) = self.snapshot(cx) {
            self.read_marks.retain(|id, mark| {
                snapshot.waits.iter().any(|wait| {
                    wait.id == *id && (mark.was_read.is_some() || wait.is_read != mark.is_read)
                })
            });
        }
    }

    /// The primary answer of the ask selected: what A does.
    pub(super) fn press_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let verb = self.selected_verbs(cx).and_then(|verbs| verbs.primary);
        if let Some(verb) = verb {
            self.press(verb, window, cx);
        } else {
            self.press_reply(window, cx);
        }
    }

    /// The decline of the ask selected: what D does.
    pub(super) fn press_decline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let verbs = self.selected_verbs(cx);
        let decline = verbs.and_then(|verbs| {
            verbs
                .secondary
                .into_iter()
                .find(|v| matches!(v.say, Say::Decline | Say::Dismiss))
        });
        if let Some(verb) = decline {
            self.press(verb, window, cx);
        }
    }

    /// The reply to the ask selected: what R does.
    pub(super) fn press_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reply = self.selected_verbs(cx).and_then(|verbs| verbs.reply);
        if let Some(action) = reply {
            let purpose = if action.key == "tracksettle" {
                NoteFor::Track
            } else {
                NoteFor::Reply
            };
            self.open_note(purpose, window, cx);
        }
    }

    fn selected_verbs(&self, cx: &gpui::App) -> Option<Verbs> {
        let snapshot = self.snapshot(cx)?;
        let wait = self.selected_ask(&snapshot)?;
        Some(Self::verbs_of(wait, &snapshot))
    }

    /// Presses `verb` on the ask selected: a decline that takes a note asks for it first.
    pub(super) fn press(&mut self, verb: Verb, window: &mut Window, cx: &mut Context<Self>) {
        if verb.say == Say::Start {
            return self.open_proposal(verb, window, cx);
        }
        if self.snapshot(cx).is_some_and(|snapshot| {
            self.selected_ask(&snapshot)
                .is_some_and(|wait| folder_blocks(&snapshot, wait, &verb.action.key))
        }) {
            return;
        }
        if verb.say == Say::Decline && verb.takes_note {
            return self.open_note(NoteFor::Decline(verb), window, cx);
        }
        let Some(ask) = self
            .selected
            .clone()
            .filter(|ask| !self.is_answered(ask, cx))
        else {
            return;
        };
        if verb.say == Say::Resume {
            return self.answer_ask(&ask, Answer::Verb(Say::Resume), window, cx);
        }
        let words = verb.done_words();
        let act = HeldAct {
            key: verb.key,
            action: verb.action,
            target: verb.target,
            inputs: verb.inputs,
        };
        self.decide(ask, act, words, window, cx);
    }

    pub(super) fn start_proposal(
        &mut self,
        verb: Verb,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ask = verb.target.clone();
        let Some(snapshot) = self.snapshot(cx) else {
            return false;
        };
        if !snapshot.waits.iter().any(|wait| wait.id == ask) || self.is_answered(&ask, cx) {
            return false;
        }
        if proposal_folder_missing(&snapshot, verb.inputs.get("project")) {
            Toast::show_error(
                window,
                cx,
                tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)),
            );
            return false;
        }
        let words = verb.done_words();
        let act = HeldAct {
            key: verb.key,
            action: verb.action,
            target: verb.target,
            inputs: verb.inputs,
        };
        self.decide(ask, act, words, window, cx);
        true
    }

    /// Answers the question selected with one of the choices it offers.
    pub(super) fn press_choice(
        &mut self,
        choice: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ask) = self.selected.clone() {
            self.answer_ask(&ask, Answer::Choice(choice), window, cx);
        }
    }

    fn open_note(&mut self, purpose: NoteFor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self
            .selected
            .clone()
            .filter(|ask| !self.is_answered(ask, cx))
        else {
            return;
        };
        self.open_note_for(ask, purpose, window, cx);
    }

    fn open_note_for(
        &mut self,
        ask: String,
        purpose: NoteFor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match purpose {
            NoteFor::Decline(_) => tr(Key("inbox.note.decline")),
            NoteFor::Reply => tr(Key("inbox.note.reply")),
            NoteFor::Track => tr(Key("inbox.note.track_answer")),
            NoteFor::Resume => tr(Key("inbox.note.resume")),
        };
        let resume_key = if matches!(purpose, NoteFor::Resume | NoteFor::Track) {
            let Some(key) = self.resume_key(&ask, cx) else {
                return;
            };
            self.resume_drafts.entry(key.clone()).or_insert_with(|| {
                let field = cx.new(|cx| TextareaState::new(window, cx).rows(3));
                field.update(cx, |field, cx| {
                    field.set_placeholder(placeholder, window, cx)
                });
                let observer = cx.observe(&field, |_, _, cx| cx.notify());
                ResumeDraft {
                    field,
                    answer_key: None,
                    operation: Default::default(),
                    _observer: observer,
                }
            });
            Some(key)
        } else {
            self.field_for(window, cx).update(cx, |field, cx| {
                field.set_value("", window, cx);
                field.set_placeholder(placeholder, window, cx);
            });
            None
        };
        self.note = Some(Note {
            ask,
            purpose,
            resume_key,
        });
        cx.notify();
    }

    fn resume_key(&self, ask: &str, cx: &App) -> Option<ResumeKey> {
        let snapshot = self.snapshot(cx)?;
        let wait = snapshot.waits.iter().find(|wait| wait.id == ask)?;
        Some((wait.id.clone(), wait.key_revision().to_string()))
    }

    pub(super) fn resume_field(&self) -> Option<&Entity<TextareaState>> {
        let key = self.note.as_ref()?.resume_key.as_ref()?;
        Some(&self.resume_drafts.get(key)?.field)
    }

    pub(super) fn resume_answer_key(&self, note: &Note) -> Option<&str> {
        let key = note.resume_key.as_ref()?;
        self.resume_drafts.get(key)?.answer_key.as_deref()
    }

    pub(super) fn retire_resume_drafts(&mut self, snapshot: &Snapshot) {
        self.resume_drafts.retain(|(id, revision), _| {
            snapshot
                .waits
                .iter()
                .any(|wait| &wait.id == id && wait.key_revision() == revision)
        });
        if self
            .note
            .as_ref()
            .and_then(|note| note.resume_key.as_ref())
            .is_some_and(|key| !self.resume_drafts.contains_key(key))
        {
            self.note = None;
        }
    }

    /// The one field the page writes notes in, made when first requested.
    fn field_for(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(field) = &self.field {
            return field.clone();
        }
        let field = cx.new(|cx| InputState::new(window, cx));
        self._observers.push(Self::watch_field(&field, window, cx));
        self.field = Some(field.clone());
        field
    }

    /// Sends what is written in the note field with the answer it was opened for. A reply with no words
    /// sends nothing; a decline with no note is a plain decline.
    pub(super) fn send_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.note.clone() else {
            return;
        };
        let value = match note.purpose {
            NoteFor::Resume | NoteFor::Track => {
                self.resume_field().map(|field| field.read(cx).value())
            }
            _ => self.field.as_ref().map(|field| field.read(cx).value()),
        };
        let Some(value) = value else { return };
        let said = value.trim().to_string();
        match note.purpose {
            NoteFor::Decline(verb) => {
                let words = verb.done_words();
                let mut act = HeldAct {
                    key: verb.key,
                    action: verb.action,
                    target: verb.target,
                    inputs: verb.inputs,
                };
                if !said.is_empty() {
                    act.inputs.insert(INPUT_NOTE.to_string(), said);
                }
                self.decide(note.ask, act, words, window, cx);
            }
            NoteFor::Resume if resume_note::problem(&value).is_none() => {
                let words = (!said.is_empty()).then_some(said);
                self.answer_ask(&note.ask, Answer::Resume(words), window, cx);
            }
            NoteFor::Reply if !said.is_empty() => {
                self.answer_ask(&note.ask, Answer::Reply(said), window, cx);
            }
            NoteFor::Track if !said.is_empty() => {
                self.send_track_answer(&note, value.to_string(), window, cx)
            }
            NoteFor::Resume | NoteFor::Reply | NoteFor::Track => {}
        }
    }

    fn send_track_answer(
        &mut self,
        note: &Note,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.resume_note_pending(note, cx) {
            return;
        }
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(wait) = self
            .selected_ask(&snapshot)
            .filter(|wait| wait.id == note.ask)
        else {
            return;
        };
        let Some(action) = Self::verbs_of(wait, &snapshot)
            .reply
            .filter(|action| action.key == "tracksettle")
        else {
            return;
        };
        let Some(draft) = note
            .resume_key
            .as_ref()
            .and_then(|key| self.resume_drafts.get_mut(key))
        else {
            return;
        };
        let act = draft.operation.prepare(crate::ui::track_page::request(
            &action,
            wait.id.clone(),
            [("answer".into(), value)],
        ));
        let held = HeldAct {
            key: wait.id.clone(),
            target: act.target,
            action,
            inputs: act.inputs,
        };
        self.decide(
            note.ask.clone(),
            held,
            tr(Key("track.answered")).into(),
            window,
            cx,
        );
    }

    pub(super) fn resume_without_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(note) = self
            .note
            .clone()
            .filter(|note| note.purpose == NoteFor::Resume)
        {
            self.answer_ask(&note.ask, Answer::Resume(None), window, cx);
        }
    }

    /// Closes the note field without sending, and gives the keyboard back to the list.
    pub(super) fn leave_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(note) = self.note.take() {
            if let Some(key) = note.resume_key {
                self.resume_drafts.remove(&key);
            }
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Holds `act` as the answer to `ask`, says so in a toast with Undo and, when `ask` was the one selected,
    /// moves on to the next ask.
    fn decide(
        &mut self,
        ask: String,
        act: HeldAct,
        success: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_answered(&ask, cx) {
            return;
        }
        let snapshot = self.snapshot(cx);
        if snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.waits.iter().any(|wait| {
                wait.id == ask
                    && match act.action.key.as_str() {
                        "start" => proposal_folder_missing(snapshot, act.inputs.get("project")),
                        action => folder_blocks(snapshot, wait, action),
                    }
            })
        }) {
            return;
        }
        let words = match (
            act.action.key.as_str(),
            act.inputs.get("verdict").map(String::as_str),
        ) {
            ("k", Some("ok")) => snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.waits.iter().find(|wait| wait.id == ask))
                .map_or_else(
                    || success.clone(),
                    |wait| tr_with(Key("inbox.toast.marked_done"), &[("title", &wait.title)]),
                ),
            ("k", _) => success.clone(),
            _ => confirm_statement(snapshot.as_deref(), &act.request(), ""),
        };
        self.keep_resume_note(&ask, &act, cx);
        let ticket = self.held.hold(&ask, act, words.clone(), success);
        self.window = Some(window.window_handle());
        if self.selected.as_deref() == Some(ask.as_str()) && !self.has_resume_note(&ask) {
            window.focus(&self.focus, cx);
        }
        let held = self.held.asks().len();
        let toast = Self::show_hold(&cx.weak_entity(), &ask, (words, None), held, window, cx);
        self.held.set_toast(&ask, ticket, toast);
        self.begin_hold(ask, ticket, cx);
        cx.notify();
    }

    fn keep_resume_note(&mut self, ask: &str, act: &HeldAct, cx: &App) {
        if matches!(act.action.key.as_str(), "R" | "tracksettle") {
            if let Some(draft) = self
                .resume_key(ask, cx)
                .and_then(|key| self.resume_drafts.get_mut(&key))
            {
                draft.answer_key = Some(act.key.clone());
            }
        } else if self.note.as_ref().is_some_and(|note| note.ask == ask) {
            self.note = None;
        }
    }

    /// The toast of a hold: what the answer says, and Undo. While several answers are held at once the one
    /// toast says how many, and its Undo takes them all back, so answers do not stack as toasts.
    fn show_hold(
        page: &WeakEntity<Self>,
        ask: &str,
        (words, detail): (SharedString, Option<SharedString>),
        held: usize,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> u64 {
        let (page, undo_ask) = (page.clone(), ask.to_string());
        let is_batch = held > 1;
        let undo = Box::new(move |_: &mut Window, cx: &mut gpui::App| {
            drop(page.update(cx, |page, cx| match is_batch {
                true => page.undo_all(cx),
                false => page.undo(&undo_ask, cx),
            }));
        });
        match is_batch {
            true => {
                let said = tr_count(Key("inbox.answer.batch"), held as u64, &[]);
                Toast::show_with_detail(window, cx, said, detail, Some(undo))
            }
            false => Toast::show_with_detail(window, cx, words, detail, Some(undo)),
        }
    }

    /// Starts the hold of the answer to `ask`. One that asks to be confirmed first asks the engine, which
    /// only returns its question and a token, and the hold's time runs while that question is on screen; any
    /// other is simply held.
    fn begin_hold(&mut self, ask: String, ticket: u64, cx: &mut Context<Self>) {
        let Some(act) = self.held.act_of(&ask).filter(|act| act.action.confirm) else {
            return self.release_after_hold(ask, ticket, cx);
        };
        let pending = self.held.ask(act.request());
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |page, cx| page.asked(&ask, ticket, result, cx)));
        })
        .detach();
    }

    /// The engine's answer to the call that asks. Its question is said in the toast, with Undo, and the
    /// hold's time starts. When the engine asks nothing, its answer is the answer: a refusal comes back to
    /// the person with its reason.
    fn asked(
        &mut self,
        ask: &str,
        ticket: u64,
        result: BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        let Some(act) = self.held.act_of(ask) else {
            return;
        };
        match result {
            Ok(ActResult::Confirm { token, summary, .. }) => {
                self.show_question(ask, ticket, &act, (token, summary), cx)
            }
            other => {
                let next = self.neighbour_of(ask, cx);
                if let Some(released) = self.held.release(ask, ticket) {
                    self.move_on_from(ask, next);
                    self.take_undo_off(released.toast, cx);
                    self.deliver(ask, released, other, cx);
                }
            }
        }
    }

    fn show_question(
        &mut self,
        ask: &str,
        ticket: u64,
        act: &HeldAct,
        (token, summary): (String, String),
        cx: &mut Context<Self>,
    ) {
        let shown = act.request();
        let snapshot = self.snapshot(cx);
        let said = confirm_statement(snapshot.as_deref(), &shown, &summary);
        let detail = plain_summary(&summary)
            .filter(|line| *line != said && summary_adds_words(snapshot.as_deref(), &shown));
        let asked = Asked { token, summary };
        if !self.held.set_asked(ask, ticket, asked, said.clone()) {
            return;
        }
        if let Some(window) = self.window {
            let (page, held) = (cx.weak_entity(), self.held.asks().len());
            let detail = if held > 1 { Some(said.clone()) } else { detail };
            let toast = window.update(cx, |_, window, cx| {
                Self::show_hold(&page, ask, (said, detail), held, window, cx)
            });
            match toast {
                Ok(toast) => self.held.set_toast(ask, ticket, toast),
                Err(_) => {
                    eprintln!("sakura: the window is gone, the engine's question was not shown")
                }
            }
        }
        self.release_after_hold(ask.to_string(), ticket, cx);
        cx.notify();
    }

    /// The engine's answer to a call the window did not make through its answers: kept as theirs, and the
    /// ask comes back when it needs the person.
    fn deliver(
        &mut self,
        ask: &str,
        released: Released,
        result: BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        self.mark_sending(ask, &released, cx);
        let sent = released.act;
        let act = Act {
            action: sent.action.clone(),
            target: sent.target.clone(),
            inputs: sent.inputs.clone(),
            confirm: None,
        };
        self.answers
            .update(cx, |answers, cx| answers.deliver(sent.key, act, result, cx));
        cx.notify();
    }

    /// Answers the open ask `wait` with `answer`: held under the Undo toast, then sent, whichever page the
    /// person answers on. The page's selection and focus move on only when `wait` was the ask selected.
    /// Anything the ask does not offer is told on stderr and changes nothing.
    pub fn answer_ask(
        &mut self,
        wait: &str,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(ask) = snapshot.waits.iter().find(|listed| listed.id == wait) else {
            return eprintln!("sakura: the ask {wait:?} is no longer open");
        };
        let verbs = Self::verbs_of(ask, &snapshot);
        if answer == Answer::Verb(Say::Resume) && folder_blocks(&snapshot, ask, "R") {
            return;
        }
        if answer == Answer::Verb(Say::Start) {
            if let Some(verb) = verbs.primary.filter(|verb| verb.say == Say::Start) {
                self.open_proposal(verb, window, cx);
            }
            return;
        }
        if answer == Answer::Verb(Say::Resume)
            && verbs
                .primary
                .as_ref()
                .is_some_and(|verb| verb.say == Say::Resume && verb.takes_note)
            && !self.is_answered(wait, cx)
        {
            if !self.has_resume_note(wait) {
                self.open_note_for(wait.to_string(), NoteFor::Resume, window, cx);
            }
            return;
        }
        if matches!(&answer, Answer::Resume(_))
            && self
                .note
                .as_ref()
                .is_some_and(|note| note.ask == wait && self.resume_note_pending(note, cx))
        {
            return;
        }
        let Some((act, words)) = Self::held_answer(wait, &answer, verbs) else {
            return eprintln!("sakura: the ask {wait:?} does not offer {answer:?}");
        };
        self.decide(wait.to_string(), act, words, window, cx);
    }

    /// The call that carries `answer` to the ask `wait` and what it says while held, when `verbs` offer it.
    fn held_answer(wait: &str, answer: &Answer, verbs: Verbs) -> Option<(HeldAct, SharedString)> {
        let in_words = |action: Action, input: &str, said: &str, words: SharedString| {
            let act = HeldAct {
                key: wait.to_string(),
                action,
                target: wait.to_string(),
                inputs: BTreeMap::from([(input.to_string(), said.to_string())]),
            };
            Some((act, words))
        };
        match answer {
            Answer::Verb(_) | Answer::Resume(_) => {
                let say = match answer {
                    Answer::Verb(say) => *say,
                    _ => Say::Resume,
                };
                let mut all = verbs.primary.into_iter().chain(verbs.secondary);
                let mut verb = all.find(|verb| verb.say == say)?;
                if let Answer::Resume(Some(note)) = answer {
                    if !verb.takes_note || resume_note::problem(note).is_some() {
                        return None;
                    }
                    verb.inputs.insert(INPUT_NOTE.into(), note.clone());
                }
                let words = verb.done_words();
                let act = HeldAct {
                    key: verb.key,
                    action: verb.action,
                    target: verb.target,
                    inputs: verb.inputs,
                };
                Some((act, words))
            }
            Answer::Choice(choice) if verbs.choices.contains(choice) => {
                let words = tr_with(Key("inbox.answer.choice"), &[("choice", choice)]);
                in_words(verbs.reply?, INPUT_CHOICE, choice, words)
            }
            Answer::Reply(text) if !text.trim().is_empty() => {
                let words = SharedString::from(tr(Key("inbox.answer.replied")));
                in_words(
                    verbs.reply.filter(|action| action.key != "tracksettle")?,
                    INPUT_TEXT,
                    text,
                    words,
                )
            }
            Answer::Choice(_) | Answer::Reply(_) => None,
        }
    }

    /// What the answer held to `ask` says, while it is held for Undo.
    pub fn held_words(&self, ask: &str) -> Option<SharedString> {
        self.held.words_of(ask)
    }

    /// The hold of `ask` is over and the ask leaves the list: when it was the one selected, `next` is.
    fn move_on_from(&mut self, ask: &str, next: Option<String>) {
        if self.selected.as_deref() == Some(ask) && !self.has_resume_note(ask) {
            self.selected = next;
        }
    }

    /// Moves the selection off the ask just answered to the next one still to answer, for the keys: the
    /// answered ask stays in its place, muted, until its hold is over.
    pub(super) fn move_past_answered(&mut self, cx: &mut Context<Self>) {
        let (Some(snapshot), Some(selected)) = (self.snapshot(cx), self.selected.clone()) else {
            return;
        };
        if !self.is_answered(&selected, cx) {
            return;
        }
        let ids = self.ids(&snapshot, cx);
        let at = ids.iter().position(|id| *id == selected).unwrap_or(0);
        let later = ids.iter().skip(at + 1);
        let earlier = ids.iter().take(at).rev();
        let open = later.chain(earlier).find(|id| !self.is_answered(id, cx));
        if let Some(id) = open.cloned() {
            self.select(id, cx);
            self.reveal_selected_row();
        }
    }

    /// The ask that takes the place of `ask` in the list once it is gone.
    fn neighbour_of(&mut self, ask: &str, cx: &gpui::App) -> Option<String> {
        let snapshot = self.snapshot(cx)?;
        let ids = self.ids(&snapshot, cx);
        let at = ids.iter().position(|id| id == ask)?;
        ids.get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|before| ids.get(before)))
            .cloned()
    }

    /// The hold's time: [`HOLD`], and while the pointer is on the toast after it, as long as it stays there,
    /// so Undo is never under the pointer when the answer has already gone.
    fn release_after_hold(&self, ask: String, ticket: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOLD).await;
            while this
                .update(cx, |page, cx| {
                    page.is_waiting_under_pointer(&ask, ticket, cx)
                })
                .unwrap_or(false)
            {
                cx.background_executor().timer(TOAST_TICK).await;
            }
            drop(this.update(cx, |page, cx| page.release(&ask, ticket, cx)));
        })
        .detach();
    }

    /// Whether the hold of `ask` is still on and the pointer is on the toast of the window.
    fn is_waiting_under_pointer(&self, ask: &str, ticket: u64, cx: &mut Context<Self>) -> bool {
        let Some(window) = self.window.filter(|_| self.held.is_hold(ask, ticket)) else {
            return false;
        };
        window
            .update(cx, |_, window, cx| Toast::is_hovered(window, cx))
            .unwrap_or(false)
    }

    /// The hold is over: the answer is sent. One whose question was shown is checked once more, and runs
    /// with the token that was shown only if the engine still asks the same.
    fn release(&mut self, ask: &str, ticket: u64, cx: &mut Context<Self>) {
        let next = self.neighbour_of(ask, cx);
        let Some(released) = self.held.release(ask, ticket) else {
            return;
        };
        if released.act.action.key == "tracksettle"
            && self
                .snapshot(cx)
                .is_none_or(|snapshot| !snapshot.waits.iter().any(|wait| wait.id == ask))
        {
            self.take_undo_off(released.toast, cx);
            cx.notify();
            return;
        }
        if self.held_destination_missing(&released.act, cx) {
            self.take_undo_off(released.toast, cx);
            if let Some(window) = self.window {
                let _ = window.update(cx, |_, window, cx| {
                    Toast::show_error(
                        window,
                        cx,
                        tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)),
                    );
                });
            }
            cx.notify();
            return;
        }
        self.mark_sending(ask, &released, cx);
        let held = released.act;
        self.move_on_from(ask, next);
        self.take_undo_off(released.toast, cx);
        match released.asked {
            Some(asked) => {
                let act = Act {
                    action: held.action,
                    target: held.target,
                    inputs: held.inputs,
                    confirm: Some(asked.token),
                };
                self.answers.update(cx, |answers, cx| {
                    answers.press_after_question(held.key, act, asked.summary, cx)
                });
            }
            None => self.answers.update(cx, |answers, cx| {
                answers.press(held.key, held.action, held.target, held.inputs, cx)
            }),
        }
        cx.notify();
    }

    pub(super) fn held_destination_missing(&self, act: &HeldAct, cx: &App) -> bool {
        act.action.key == "start"
            && self.snapshot(cx).is_some_and(|snapshot| {
                proposal_folder_missing(&snapshot, act.inputs.get("project"))
            })
    }

    fn mark_sending(&mut self, ask: &str, released: &Released, cx: &gpui::App) {
        if let Some(snapshot) = self.snapshot(cx) {
            if let Some(wait) = snapshot.waits.iter().find(|wait| wait.id == ask) {
                let mut sent = SentAnswer::new(released.act.key.clone(), wait, &snapshot);
                sent.notice = Some((released.act.action.label.clone(), released.success.clone()));
                self.sending.insert(ask.to_string(), sent);
            }
        }
    }

    pub(super) fn forget_acted_on_answers(&mut self, cx: &gpui::App) {
        if let Some(snapshot) = self.snapshot(cx) {
            self.sending
                .retain(|ask, sent| sent.notice.is_some() || sent.still_waiting(ask, &snapshot));
        }
    }

    /// The answer is sent: the toast that offered Undo for it, if still on screen, offers it no more.
    fn take_undo_off(&self, toast: Option<u64>, cx: &mut Context<Self>) {
        if !self.held.asks().is_empty() {
            return;
        }
        if let (Some(window), Some(toast)) = (self.window, toast) {
            drop(window.update(cx, |_, window, cx| Toast::drop_undo(window, cx, toast)));
        }
    }

    /// Restores the ask when the action failed or returned a confirmation.
    pub(super) fn bring_back_answered(&mut self, cx: &mut Context<Self>) {
        self.show_outcomes(cx);
        self.close_sent_resume_note(cx);
        let answers = self.answers.read(cx);
        let mut needed = Vec::new();
        self.sending
            .retain(|ask, sent| match answers.step(&sent.key) {
                None => false,
                Some(Step::Running) => true,
                Some(Step::Told {
                    is_problem: false, ..
                }) => true,
                Some(_) => {
                    needed.push(ask.clone());
                    false
                }
            });
        if let Some(ask) = needed.pop().filter(|_| {
            !self
                .note
                .as_ref()
                .is_some_and(|note| matches!(note.purpose, NoteFor::Resume | NoteFor::Track))
        }) {
            self.select(ask, cx);
        }
        cx.notify();
    }

    fn close_sent_resume_note(&mut self, cx: &mut Context<Self>) {
        let answers = self.answers.read(cx);
        self.resume_drafts.retain(|_, draft| {
            !draft.answer_key.as_deref().is_some_and(|key| {
                matches!(
                    answers.step(key),
                    Some(Step::Told {
                        is_problem: false,
                        ..
                    })
                )
            })
        });
        let closed = self
            .note
            .as_ref()
            .and_then(|note| note.resume_key.as_ref())
            .filter(|key| !self.resume_drafts.contains_key(*key));
        if let Some((ask, _)) = closed {
            let ask = ask.clone();
            let next = self.neighbour_of(&ask, cx);
            self.note = None;
            self.move_on_from(&ask, next);
        }
    }

    fn show_outcomes(&mut self, cx: &mut Context<Self>) {
        let answers = self.answers.read(cx);
        let mut outcomes = Vec::new();
        for sent in self.sending.values_mut() {
            let step = answers.step(&sent.key);
            if !matches!(step, Some(Step::Told { .. } | Step::Confirming { .. })) {
                continue;
            }
            if let Some((action, success)) = sent.notice.take() {
                let (priority, words) = match step {
                    Some(Step::Confirming { words, .. }) => {
                        (Notice::Confirmation, words.clone().into())
                    }
                    Some(Step::Told {
                        text,
                        is_problem: true,
                    }) => (Notice::Failure, failure_words(&action, text)),
                    _ => (Notice::Success, success),
                };
                outcomes.push((priority, words));
            }
        }
        if let Some(window) = self.window {
            outcomes.sort_by_key(|(priority, _)| *priority);
            for (priority, words) in outcomes {
                drop(window.update(cx, |_, window, cx| {
                    if priority != Notice::Success
                        && Toast::add_undo_detail(window, cx, words.clone())
                    {
                        return;
                    }
                    if priority == Notice::Failure {
                        Toast::show_error(window, cx, words);
                    } else if self.held.asks().is_empty() {
                        Toast::show(window, cx, words, None);
                    }
                }));
            }
        }
    }

    /// Undo of a toast that spoke for several answers: every held answer is taken back.
    fn undo_all(&mut self, cx: &mut Context<Self>) {
        for ask in self.held.asks() {
            self.undo(&ask, cx);
        }
    }

    /// Undo: the held answer is taken back, and its ask is in front of the person again.
    pub(super) fn undo(&mut self, ask: &str, cx: &mut Context<Self>) {
        if self.held.take(ask).is_some() {
            self.sending.remove(ask);
            self.selected = Some(ask.to_string());
            cx.notify();
        }
    }
}

fn proposal_folder_missing(snapshot: &Snapshot, destination: Option<&String>) -> bool {
    destination.is_some_and(|folder| snapshot.is_project_folder_missing(folder))
}

fn folder_blocks(snapshot: &Snapshot, wait: &Wait, action: &str) -> bool {
    model::is_folder_action(wait, action) && snapshot.is_project_folder_missing(&wait.repo)
}

fn failure_words(action: &str, message: &str) -> SharedString {
    let missing = tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING));
    if [
        missing,
        tr(Key("inbox.note.lane_changed")),
        tr(Key("inbox.note.resume_failed")),
    ]
    .contains(&message)
    {
        return message.to_string().into();
    }
    tr_with(Key("inbox.answer.failed"), &[("action", action)])
}
