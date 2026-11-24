//! The agent conversation: a transcript that follows its end while the agent works, and a composer under
//! it. The view owns the conversation's reducer (`sakura_acp::thread::Thread`) and only draws its entries;
//! everything the agent says reaches it as events, from a connection (`session`) or from a test.
//!
//! What moves: streamed words fade in, a label that waits shimmers in one direction, and the jump button
//! fades. Nothing else, and none of it under reduced motion or for a replayed session.

// What the master's conversation adds is built only by the engine's window: the boxes build, which has
// none, compiles it and uses none of it.
#![cfg_attr(feature = "boxes", allow(dead_code, unused_imports))]

pub(crate) mod agent_menu;
mod answer;
mod attach;
mod breaks;
mod composer;
mod copy;
mod dictation;
mod diff;
mod glyphs;
mod local_commands;
mod master;
mod master_cards;
mod master_choices;
mod master_composer;
mod permission;
mod rows;
#[cfg(not(feature = "boxes"))]
pub(super) use rows::saved_message;
mod segments;
mod session;
mod slash;
mod states;
mod tool;

#[cfg(test)]
mod view_tests;

#[cfg(feature = "snapshot")]
pub use attach::pick_next_files;
pub use attach::refusal_words;
pub use composer::{send_face, send_look};
pub use copy::tool_words as engine_tool_words;
#[cfg(not(feature = "boxes"))]
pub use copy::{elapsed, shown, JUMP_TO_END};
pub use glyphs::send as send_glyph;
pub use master::{
    AgentMenu, LaneAsk, LaneCard, MasterKit, MasterWorld, MenuEntry, MenuGroup, MenuPick, Proposal,
    ProposalAsk, SharedStart, StartChoice, LOGIN_OPTION, PLAN_MODE,
};
pub(crate) use rows::prompt;
pub use session::{
    Ask, ChooseAccount, Host, Launched, Login, Readiness, StoreSession, StoredSession,
};
pub use states::{button, Face};

use crate::l10n::tr;
pub(crate) use answer::Answer;
use answer::Growth;

use crate::theme::thread as tokens;
use crate::theme::Theme;
use crate::ui::brand::{BrandMark, Rest};
use gpui::{
    div, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Task, Window,
};
use gpui_component::input::{InputEvent, TextareaState};
use gpui_component::message_scroller::MessageScrollerState;
use sakura_acp::attachment::{Attachment, Refusal};
use sakura_acp::connection::Agent;
use sakura_acp::model::{PromptSupport, SessionChoices, SlashCommand};
#[cfg(test)]
use sakura_acp::thread::ThreadEvent;
use sakura_acp::thread::{EntryKind, Problem, Thread, Turn};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Where the conversation stands with its agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// No agent runs for this view.
    Idle,
    /// The task is being made: the person's first message is shown and waits for it.
    Preparing,
    /// The agent is starting or a session is opening.
    Connecting,
    /// A stored session is being replayed; it is shown when it is whole.
    Loading,
    /// The account must sign in before a session can open.
    SignIn,
    /// No account is named for the task's agent: the person chooses one. A message that was sent
    /// waits, and goes to the agent that starts with the account chosen.
    NoAccount,
    /// A session is open.
    Ready,
    /// The agent ended with a problem; the transcript stays, with "Restart".
    Ended(Problem),
    /// This `box` cannot hold a conversation (it has no `acp`): the terminal is what there is.
    NoConversation,
}

/// What the view tells whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadViewEvent {
    /// Whether the agent waits for the person changed: the task's header shows it.
    WaitsForPerson(bool),
    /// What the conversation says of its agent changed: the task's state shows it.
    Live(sakura_core::model::Live),
    /// The person typed `/clear` in the master's composer: its holder starts a fresh conversation.
    StartFresh,
    /// The person chose the project the master's messages are about; the holder reads it.
    ProjectChosen,
    /// The person chose to log in to Codex for Sakura: the holder opens the program's own login.
    CodexLoginRequested,
    /// The agent can no longer load the stored conversation.
    KeptSessionLost(String),
}

/// What the view keeps for one entry, by the entry's id.
enum RowState {
    /// Nothing to keep.
    Plain,
    /// The agent's answer, as the parts that draw it.
    Markdown(Box<Answer>),
    /// A row that opens: the person's choice, or none while the default holds; and whether a long body is
    /// shown whole.
    Foldable {
        is_open: Option<bool>,
        is_whole: bool,
    },
}

/// The conversation of one task.
pub struct ThreadView {
    thread: Thread,
    rows: HashMap<u64, RowState>,
    /// How many rows of the list each entry takes (an answer takes one per part), and the first row of
    /// each entry.
    spans: Vec<usize>,
    starts: Vec<usize>,
    /// The answer the person pressed on, kept as one row of the list while they select in it: a row the list
    /// drops from its screen takes its text's selection away, and a drag held at the edge scrolls far.
    held: Option<u64>,
    /// The wait that says a conversation did not load, while one is loading.
    load_watch: Option<gpui::Task<()>>,
    scroller: Entity<MessageScrollerState>,
    /// Scrolls the transcript while a drag-selection is held at its edge.
    drag_scroll: crate::ui::drag_scroll::DragScroll,
    composer: Entity<TextareaState>,
    phase: Phase,
    host: Host,
    agent: Option<Agent>,
    has_opened_session: bool,
    /// A prompt written before the session was open, sent when it opens.
    waiting_prompt: Option<String>,
    /// A message sent while the conversation was loading: it goes when the session opens, after what was loaded.
    held_message: Option<String>,
    /// A held message that did not go (the agent ended first): put back in the composer when it is next drawn.
    given_back: Option<String>,
    /// Whether the session that is opening was requested as the one kept from before.
    asked_for_kept: bool,
    is_kept_lost: bool,
    /// Whether the conversation kept from before was already requested by being shown.
    tried_kept: bool,
    turn_started: Option<Instant>,
    /// What the master's conversation adds; none for any other.
    master: Option<MasterKit>,
    /// The files attached to the message being written, what the agent said its prompts take (not known
    /// until it connects) and why the last file was refused.
    attachments: Vec<Attachment>,
    prompt_support: Option<PromptSupport>,
    /// The models and modes the session offers, as the agent last said.
    choices: SessionChoices,
    /// The slash commands the agent announced, and which one of the menu is highlighted.
    commands: Vec<SlashCommand>,
    slash_selected: usize,
    /// The menu's scroll: it holds more commands than it has room for, and the highlighted one stays in view.
    slash_scroll: gpui::ScrollHandle,
    /// `/help` was run: the list of every command is shown over the composer until something is typed.
    is_help_open: bool,
    /// The proposal cards of the master (by their call's id) the person opened to read every line.
    expanded_notes: HashSet<String>,
    /// The folded rows of tool calls (by the id of the first call of each) the person opened.
    opened_groups: HashSet<u64>,
    /// Where each chip of the composer was last drawn, so a command can open its menu.
    chip_frames: master_choices::ChipFrames,
    refusal: Option<Refusal>,
    /// The system did not start dictation when the microphone was pressed.
    is_dictation_unavailable: bool,
    waits_for_person: bool,
    /// What the desk was last told of the agent, and whether a turn ever ran here.
    told_live: sakura_core::model::Live,
    had_turn: bool,
    /// The brand's sign of an agent connecting: it loops while the agent is prepared, starts or loads a
    /// kept conversation, in the conversation's own area, and takes no room otherwise.
    sign: Entity<BrandMark>,
    is_connecting: bool,
    /// Whether what the service wrote about a creation that failed is shown.
    shows_details: bool,
    _pump: Option<Task<()>>,
    _clock: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadViewEvent> for ThreadView {}

impl ThreadView {
    /// A view with no agent yet. Nothing takes the focus.
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let drag_scroll = crate::ui::drag_scroll::DragScroll::default();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(tokens::COMPOSER_MIN_ROWS, tokens::COMPOSER_MAX_ROWS)
                .placeholder(tr(copy::PLACEHOLDER))
                .submit_on_enter(true)
        });
        let on_key = cx.subscribe_in(
            &composer,
            window,
            |view, _, event: &InputEvent, window, cx| {
                match event {
                    // Return picks a command while the menu of commands is open, and sends otherwise.
                    InputEvent::PressEnter { shift: false, .. } => view.on_return(window, cx),
                    InputEvent::Change => {
                        view.slash_selected = 0;
                        view.close_help_when_typing(cx);
                        cx.notify();
                    }
                    _ => {}
                }
            },
        );
        let on_language = cx
            .observe_global_in::<crate::l10n::Spoken>(window, |view, window, cx| {
                view.set_placeholder(window, cx)
            });
        // Every change of this view (a phase, a turn) passes here: the desk hears what the agent does.
        let me = cx.entity();
        let on_change = cx.observe(&me, |view, _, cx| view.tell_attention(cx));
        Self {
            thread: Thread::new(),
            rows: HashMap::new(),
            spans: Vec::new(),
            starts: Vec::new(),
            held: None,
            load_watch: None,
            scroller,
            drag_scroll,
            composer,
            phase: Phase::Idle,
            host,
            agent: None,
            has_opened_session: false,
            waiting_prompt: None,
            held_message: None,
            given_back: None,
            asked_for_kept: false,
            is_kept_lost: false,
            tried_kept: false,
            turn_started: None,
            master: None,
            attachments: Vec::new(),
            prompt_support: None,
            choices: SessionChoices::default(),
            commands: Vec::new(),
            slash_selected: 0,
            slash_scroll: gpui::ScrollHandle::new(),
            is_help_open: false,
            expanded_notes: HashSet::new(),
            opened_groups: HashSet::new(),
            chip_frames: master_choices::ChipFrames::default(),
            refusal: None,
            is_dictation_unavailable: false,
            waits_for_person: false,
            told_live: sakura_core::model::Live::Unknown,
            had_turn: false,
            sign: cx.new(|_| BrandMark::connecting(Rest::Nothing)),
            is_connecting: false,
            shows_details: false,
            _pump: None,
            _clock: None,
            _subscriptions: vec![on_key, on_language, on_change],
        }
    }

    /// Makes this the master's conversation: its cards, its project chip, its words.
    pub fn with_master(&mut self, kit: MasterKit, window: &mut Window, cx: &mut Context<Self>) {
        self.master = Some(kit);
        self.set_placeholder(window, cx);
    }

    /// The composer's focus, for the tests.
    #[cfg(test)]
    pub fn composer_handle(&self, cx: &App) -> gpui::FocusHandle {
        gpui::Focusable::focus_handle(self.composer.read(cx), cx)
    }

    /// What the empty composer says, in the conversation's own words.
    fn set_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let words = self.placeholder_words();
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(words, window, cx)
        });
    }

    fn placeholder_words(&self) -> &'static str {
        match self.master {
            Some(_) => tr(copy::MASTER_PLACEHOLDER),
            None => tr(copy::PLACEHOLDER),
        }
    }

    #[cfg(any(test, not(feature = "boxes")))]
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The person signed in: an agent that ended because it had no login starts again; any other state stays as
    /// it is.
    pub fn restart_after_sign_in(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::SignIn | Phase::Ended(_) => self.retry(cx),
            _ => cx.notify(),
        }
    }

    /// What the empty composer says.
    #[cfg(test)]
    pub fn placeholder(&self, cx: &App) -> String {
        let composer = self.composer.read(cx).presentation();
        composer.placeholder().to_string()
    }

    /// Whether the conversation waits for the person to sign in.
    pub fn needs_sign_in(&self) -> bool {
        self.phase == Phase::SignIn
    }

    /// Whether the conversation waits for an account to start its agent with.
    pub fn needs_account(&self) -> bool {
        self.phase == Phase::NoAccount
    }

    /// Whose login a sign-in would be about, as the host says now.
    #[cfg(test)]
    pub fn login(&self, cx: &App) -> Login {
        (self.host.login)(cx)
    }

    /// Whether a permission request waits for the person.
    #[cfg(test)]
    pub fn waits_for_person(&self) -> bool {
        self.waits_for_person
    }

    /// What is written in the composer and not sent yet.
    pub fn draft(&self, cx: &App) -> String {
        composed(&self.composer, cx)
    }

    /// Where the conversation stands, in a line, for the scripted checks.
    #[cfg(all(feature = "snapshot", not(feature = "boxes")))]
    pub fn summary(&self) -> String {
        let kinds: Vec<&str> = self
            .thread
            .entries()
            .iter()
            .map(|entry| match entry.kind {
                EntryKind::User(_) => "user",
                EntryKind::Assistant(_) => "assistant",
                EntryKind::Thought(_) => "thought",
                EntryKind::Tool(_) => "tool",
                EntryKind::Plan(_) => "plan",
                EntryKind::Permission(_) => "permission",
                EntryKind::Problem(_) => "problem",
            })
            .collect();
        let waits = self.waits_for_person;
        format!(
            "phase={:?} waits_for_person={waits} entries={kinds:?}",
            self.phase
        )
    }

    /// How many characters of its own words the agent has said in this conversation so far.
    #[cfg(feature = "snapshot")]
    pub fn assistant_text_len(&self) -> usize {
        let length = |entry: &sakura_acp::thread::Entry| match &entry.kind {
            EntryKind::Assistant(message) => message.text.len(),
            _ => 0,
        };
        self.thread.entries().iter().map(length).sum()
    }

    /// The words of the agent's last message, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn last_assistant_text(&self) -> String {
        let words = |entry: &sakura_acp::thread::Entry| match &entry.kind {
            EntryKind::Assistant(message) => Some(message.text.clone()),
            _ => None,
        };
        let last = self.thread.entries().iter().rev().find_map(words);
        last.unwrap_or_default()
    }

    /// The conversation as it stands.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Applies events to the conversation and draws what changed.
    #[cfg(test)]
    pub fn apply(&mut self, events: impl IntoIterator<Item = ThreadEvent>, cx: &mut Context<Self>) {
        for event in events {
            self.thread.apply(event);
        }
        self.sync(cx);
    }

    /// Brings the rows, the list and the clock in line with the thread after it changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let changes = self.thread.take_changes();
        let count = self.thread.entries().len();
        if changes.is_reset {
            self.rows.clear();
            self.spans.clear();
            self.held = None;
            (0..count).for_each(|index| self.add_row(index, cx));
            self.place_rows();
            let rows = self.row_count();
            self.scroller
                .update(cx, |scroller, cx| scroller.reset(rows, cx));
        } else {
            for &index in &changes.updated {
                self.refresh_row(index, cx);
            }
            if let Some(first) = changes.first_new {
                let before = self.row_count();
                (first..count).for_each(|index| self.add_row(index, cx));
                self.place_rows();
                // A call that joined a run changes the row of the run's first call.
                let owners: HashSet<usize> = (first..count)
                    .map(|index| self.row_owner(index))
                    .filter(|owner| *owner < first)
                    .collect();
                owners
                    .into_iter()
                    .for_each(|owner| self.remeasure(owner, cx));
                let added = self.row_count() - before;
                self.scroller
                    .update(cx, |scroller, cx| scroller.append(added, cx));
            }
        }
        self.follow_turn(cx);
        self.tell_attention(cx);
        cx.notify();
    }

    /// The rows of the list: the sum of what every entry takes.
    fn row_count(&self) -> usize {
        self.spans.iter().sum()
    }

    /// Notes the first row of every entry, after the rows an entry takes changed.
    fn place_rows(&mut self) {
        let mut next = 0;
        self.starts = self
            .spans
            .iter()
            .map(|span| {
                let first = next;
                next += span;
                first
            })
            .collect();
    }

    /// The entry a row of the list belongs to, and which of the entry's rows it is.
    fn locate(&self, row: usize) -> Option<(usize, usize)> {
        let entry = self
            .starts
            .partition_point(|start| *start <= row)
            .checked_sub(1)?;
        let part = row - self.starts[entry];
        (part < self.spans[entry]).then_some((entry, part))
    }

    fn add_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let entry = &self.thread.entries()[index];
        let state = match &entry.kind {
            EntryKind::Assistant(message) => RowState::Markdown(Box::new(Answer::new(
                &message.text,
                message.is_streaming,
                entry.is_replayed,
                cx,
            ))),
            EntryKind::Thought(_)
            | EntryKind::Tool(_)
            | EntryKind::Plan(_)
            | EntryKind::Problem(_) => RowState::Foldable {
                is_open: None,
                is_whole: false,
            },
            _ => RowState::Plain,
        };
        let span = match &state {
            RowState::Markdown(answer) => answer.parts().len(),
            // A call that follows another is part of the row of the first one.
            _ if index > 0 && self.is_grouped(index) && self.is_grouped(index - 1) => 0,
            _ => 1,
        };
        self.spans.push(span);
        self.rows.insert(entry.id, state);
    }

    /// Whether the entry at `index` is a tool call of the master that is folded with the calls beside it: the
    /// calls drawn as cards (a lane started, a note proposed) stand alone.
    fn is_grouped(&self, index: usize) -> bool {
        let Some(entry) = self.thread.entries().get(index) else {
            return false;
        };
        let is_call =
            matches!(&entry.kind, EntryKind::Tool(call) if master::CardTool::of(call).is_none());
        self.master.is_some() && is_call
    }

    /// The entry whose row draws the entry at `index`: the first call of the run it is in, or itself.
    fn row_owner(&self, index: usize) -> usize {
        let mut owner = index;
        while owner > 0 && self.is_grouped(owner) && self.is_grouped(owner - 1) {
            owner -= 1;
        }
        owner
    }

    /// An entry changed: its answer takes in the new text, and the list measures again only the rows
    /// that changed (for an answer, its last part and the parts it gained).
    fn refresh_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let index = self.row_owner(index);
        let Some(entry) = self.thread.entries().get(index) else {
            return;
        };
        let grown = match (self.rows.get_mut(&entry.id), &entry.kind) {
            (Some(RowState::Markdown(answer)), EntryKind::Assistant(message)) => {
                answer.grow(&message.text, message.is_streaming, cx)
            }
            _ => Growth::Same,
        };
        let (first, span) = (self.starts[index], self.spans[index]);
        let end = first + span;
        if self.held == Some(entry.id) {
            // Kept as one row: its parts grow inside it.
            return self.remeasure_rows(first..end, cx);
        }
        match grown {
            Growth::Same => self.remeasure_rows(end - 1..end, cx),
            Growth::Added(added) => {
                self.spans[index] += added;
                self.place_rows();
                self.scroller
                    .update(cx, |scroller, cx| scroller.splice(end..end, added, cx));
                self.remeasure_rows(end - 1..end + added, cx);
            }
            Growth::Joined { from } => {
                self.spans[index] = 1;
                self.place_rows();
                self.scroller.update(cx, |scroller, cx| {
                    scroller.splice(first + 1..first + from, 0, cx)
                });
                self.remeasure_rows(first..first + 1, cx);
            }
        }
    }

    /// Keeps the answer `id`, which the person pressed on, as one row; the answer held before goes back to a row
    /// per part. The parts stay as they were: only the rows of the list change.
    pub(super) fn hold_answer(&mut self, id: u64, cx: &mut Context<Self>) {
        let has_parts = match self.rows.get(&id) {
            Some(RowState::Markdown(answer)) => answer.parts().len() > 1,
            _ => false,
        };
        if self.held == Some(id) || !has_parts {
            return;
        }
        if let Some(before) = self.held.take() {
            self.set_answer_span(before, None, cx);
        }
        self.held = Some(id);
        self.set_answer_span(id, Some(1), cx);
    }

    /// Gives the answer `id` `span` rows, or a row per part when none is given.
    fn set_answer_span(&mut self, id: u64, span: Option<usize>, cx: &mut Context<Self>) {
        let Some(index) = self
            .thread
            .entries()
            .iter()
            .position(|entry| entry.id == id)
        else {
            return;
        };
        let Some(RowState::Markdown(answer)) = self.rows.get(&id) else {
            return;
        };
        let wanted = span.unwrap_or_else(|| answer.parts().len());
        let (first, had) = (self.starts[index], self.spans[index]);
        if wanted == had {
            return;
        }
        self.spans[index] = wanted;
        self.place_rows();
        self.scroller.update(cx, |scroller, cx| match wanted > had {
            true => drop(scroller.splice(first + had..first + had, wanted - had, cx)),
            false => drop(scroller.splice(first + wanted..first + had, 0, cx)),
        });
        self.remeasure_rows(first..first + wanted, cx);
        cx.notify();
    }

    /// Measures again every row of the entry at `index`.
    fn remeasure(&mut self, index: usize, cx: &mut Context<Self>) {
        let index = self.row_owner(index);
        let first = self.starts[index];
        self.remeasure_rows(first..first + self.spans[index], cx);
    }

    fn remeasure_rows(&mut self, rows: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(rows, cx);
        });
    }

    /// Keeps the turn's start and the one-second clock that draws its elapsed time.
    fn follow_turn(&mut self, cx: &mut Context<Self>) {
        let is_running = self.thread.turn() != Turn::Idle;
        if self.turn_started.is_some() == is_running {
            return;
        }
        if is_running {
            self.turn_started = Some(Instant::now());
            self._clock = Some(cx.spawn(async move |view, cx| loop {
                cx.background_executor().timer(tokens::ELAPSED_TICK).await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }));
        } else {
            self.turn_started = None;
            self._clock = None;
        }
    }

    fn tell_attention(&mut self, cx: &mut Context<Self>) {
        self.follow_connection(cx);
        let waits = self.thread.waits_for_person();
        if waits != self.waits_for_person {
            self.waits_for_person = waits;
            cx.emit(ThreadViewEvent::WaitsForPerson(waits));
        }
        use sakura_core::model::Live;
        let works = self.thread.turn() != sakura_acp::thread::Turn::Idle;
        self.had_turn |= works;
        let live = match &self.phase {
            Phase::Preparing => Live::Starting,
            // The agent starts for a message that waits: the task is at work on it, and the line over
            // the composer says the step. Starting it to show a conversation kept from before is no
            // work on the task: its state stays what it was.
            Phase::Connecting | Phase::Loading if works => Live::Working,
            Phase::Ended(_) => Live::Stopped,
            Phase::Ready if works => Live::Working,
            Phase::Ready if self.had_turn => Live::Answered,
            _ => Live::Unknown,
        };
        if live != self.told_live {
            self.told_live = live;
            cx.emit(ThreadViewEvent::Live(live));
        }
    }

    /// Starts the connecting sign when the agent begins to connect and stops it when it has, once per
    /// change; the transcript's last row, under which the sign stands, is measured again.
    fn follow_connection(&mut self, cx: &mut Context<Self>) {
        let connects = matches!(
            self.phase,
            Phase::Preparing | Phase::Connecting | Phase::Loading
        );
        if connects == self.is_connecting {
            return;
        }
        self.is_connecting = connects;
        self.sign.update(cx, |sign, cx| match connects {
            true => sign.start(cx),
            false => sign.stop(cx),
        });
        let rows = self.row_count();
        if rows > 0 {
            self.remeasure_rows(rows - 1..rows, cx);
        }
    }

    /// Whether the connecting sign is shown and loops.
    #[cfg(test)]
    pub fn is_connecting_shown(&self, cx: &App) -> bool {
        self.is_connecting && self.sign.read(cx).is_ticking()
    }

    /// The person's choice for a row that opens, and the list measures it again.
    fn toggle(&mut self, id: u64, is_open_now: bool, cx: &mut Context<Self>) {
        if let Some(RowState::Foldable { is_open, .. }) = self.rows.get_mut(&id) {
            *is_open = Some(!is_open_now);
        }
        self.row_changed(id, cx);
    }

    /// Shows a long body whole.
    fn unfold(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(RowState::Foldable { is_whole, .. }) = self.rows.get_mut(&id) {
            *is_whole = true;
        }
        self.row_changed(id, cx);
    }

    /// Opens or folds the row of tool calls that begins with the call `id`.
    fn toggle_group(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.opened_groups.remove(&id) {
            self.opened_groups.insert(id);
        }
        self.row_changed(id, cx);
    }

    fn row_changed(&mut self, id: u64, cx: &mut Context<Self>) {
        let index = self
            .thread
            .entries()
            .iter()
            .position(|entry| entry.id == id);
        if let Some(index) = index {
            self.remeasure(index, cx);
        }
        cx.notify();
    }
}

impl Render for ThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        if let Some(text) = self.given_back.take() {
            self.composer
                .update(cx, |composer, cx| composer.set_value(text, window, cx));
        }
        let dragged_over = palette.fill_hover;
        div()
            .id("thread")
            .debug_selector(|| "thread".to_string())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            // Files dropped on the conversation are attached to the master's next message.
            .on_drop(cx.listener(|view, paths: &gpui::ExternalPaths, _, cx| {
                view.attach_paths(paths.paths(), cx)
            }))
            .drag_over::<gpui::ExternalPaths>(move |style, _, _, _| style.bg(dragged_over))
            // No fill of its own: the panel under it is the surface, and a square fill here covered
            // the panel's rounded bottom corner.
            .text_color(palette.text)
            .child(self.body(&palette, window, cx))
            .child(self.footer(&palette, window, cx))
    }
}

/// The column every part of the conversation sits in: centred, no wider than the reading width.
fn column() -> gpui::Div {
    div()
        .w_full()
        .max_w(tokens::COLUMN_MAX_WIDTH)
        .mx_auto()
        .px(tokens::COLUMN_PADDING_X)
}

/// The text of the composer, trimmed; empty when there is nothing to send.
fn composed(composer: &Entity<TextareaState>, cx: &App) -> String {
    composer.read(cx).value().trim().to_string()
}
