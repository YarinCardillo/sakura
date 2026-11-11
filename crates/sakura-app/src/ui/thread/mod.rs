//! The agent conversation: a transcript that follows its end while the agent works, and a composer under
//! it. The view owns the conversation's reducer (`sakura_acp::thread::Thread`) and only draws its entries;
//! everything the agent says reaches it as events, from a connection (`session`) or from a test.
//!
//! What moves: streamed words fade in, a label that waits shimmers in one direction, and the jump button
//! fades. Nothing else, and none of it under reduced motion or for a replayed session.

mod answer;
mod breaks;
mod composer;
mod copy;
mod diff;
mod glyphs;
mod permission;
mod rows;
mod segments;
mod session;
mod states;
mod tool;

#[cfg(test)]
mod view_tests;

pub use composer::send_face;
pub use glyphs::send as send_glyph;
pub use session::{
    Ask, ChooseAccount, Host, Launched, Login, Readiness, StoreSession, StoredSession,
};
pub use states::{button, Face};

use crate::l10n::tr;
use answer::{Answer, Growth};

use crate::theme::thread as tokens;
use crate::theme::Theme;
use crate::ui::brand::{BrandMark, Rest};
use gpui::{
    div, App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    Styled, Subscription, Task, Window,
};
use gpui_component::input::{InputEvent, TextareaState};
use gpui_component::message_scroller::MessageScrollerState;
use sakura_acp::connection::Agent;
#[cfg(test)]
use sakura_acp::thread::ThreadEvent;
use sakura_acp::thread::{EntryKind, Problem, Thread, Turn};
use std::collections::HashMap;
use std::time::Instant;

/// Where the conversation stands with its agent.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// No agent runs for this view.
    Idle,
    /// The lavoro is being made: the person's first message is shown and waits for it.
    Preparing,
    /// The agent is starting or a session is opening.
    Connecting,
    /// A stored session is being replayed; it is shown when it is whole.
    Loading,
    /// The account must sign in before a session can open.
    SignIn,
    /// No account is named for the lavoro's agent: the person chooses one. A message that was sent
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadViewEvent {
    /// Whether the agent waits for the person changed: the task's header shows it.
    WaitsForPerson(bool),
    /// What the conversation says of its agent changed: the task's state shows it.
    Live(sakura_core::model::Live),
}

/// What the view keeps for one entry, by the entry's id.
enum RowState {
    /// Nothing to keep.
    Plain,
    /// The agent's answer, as the parts that draw it.
    Markdown(Answer),
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
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    phase: Phase,
    host: Host,
    agent: Option<Agent>,
    /// A prompt written before the session was open, sent when it opens.
    waiting_prompt: Option<String>,
    /// Whether the session that is opening was requested as the one kept from before, and whether the
    /// agent then opened a new one because it no longer has it.
    asked_for_kept: bool,
    is_kept_lost: bool,
    /// Whether the conversation kept from before was already requested by being shown.
    tried_kept: bool,
    turn_started: Option<Instant>,
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
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    view.send(window, cx);
                }
            },
        );
        let on_language =
            cx.observe_global_in::<crate::l10n::Spoken>(window, |view, window, cx| {
                let composer = view.composer.clone();
                composer.update(cx, |composer, cx| {
                    composer.set_placeholder(tr(copy::PLACEHOLDER), window, cx)
                });
            });
        // Every change of this view (a phase, a turn) passes here: the desk hears what the agent does.
        let me = cx.entity();
        let on_change = cx.observe(&me, |view, _, cx| view.tell_attention(cx));
        Self {
            thread: Thread::new(),
            rows: HashMap::new(),
            spans: Vec::new(),
            starts: Vec::new(),
            scroller,
            composer,
            phase: Phase::Idle,
            host,
            agent: None,
            waiting_prompt: None,
            asked_for_kept: false,
            is_kept_lost: false,
            tried_kept: false,
            turn_started: None,
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

    #[cfg(test)]
    pub fn phase(&self) -> &Phase {
        &self.phase
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
    #[cfg(test)]
    pub fn draft(&self, cx: &App) -> String {
        composed(&self.composer, cx)
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
            EntryKind::Assistant(message) => RowState::Markdown(Answer::new(
                &message.text,
                message.is_streaming,
                entry.is_replayed,
                cx,
            )),
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
            _ => 1,
        };
        self.spans.push(span);
        self.rows.insert(entry.id, state);
    }

    /// An entry changed: its answer takes in the new text, and the list measures again only the rows
    /// that changed (for an answer, its last part and the parts it gained).
    fn refresh_row(&mut self, index: usize, cx: &mut Context<Self>) {
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

    /// Measures again every row of the entry at `index`.
    fn remeasure(&mut self, index: usize, cx: &mut Context<Self>) {
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
        if !is_running {
            self.turn_started = None;
            self._clock = None;
        } else if self.turn_started.is_none() {
            self.turn_started = Some(Instant::now());
            self._clock = Some(cx.spawn(async move |view, cx| loop {
                cx.background_executor().timer(tokens::ELAPSED_TICK).await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }));
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
            // The agent starts for a message that waits: the lavoro is at work on it, and the line over
            // the composer says the step. Starting it to show a conversation kept from before is no
            // work on the lavoro: its state stays what it was.
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
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
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
