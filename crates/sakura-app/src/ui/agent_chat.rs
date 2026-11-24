//! One agent of a lane, one to one: its conversation read from the engine as the run writes it, the text it is
//! writing now at the end with the master's stream fade, and the box the person writes to that agent alone in.
//! The page shows what the engine reports and sends the person's words through `flow steer --run`; it decides
//! nothing about who hears them.

pub(in crate::ui) mod draw;
pub mod rows;

use super::lane_agents::{AgentCard, LaneAgents, Round};
use super::lane_composer::{ComposerEvent, LaneComposer};
use super::lane_side::Location;
use super::lanes::role_words;
use super::thread::Answer;
use crate::data::source::Source;
use crate::l10n::{told, tr_with, Key};
use crate::theme::{lane_page as sizes, Theme};
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, SharedString,
    Subscription, Window,
};
use gpui_component::message_scroller::MessageScrollerState;
use gpui_component::text::TextViewState;
use rows::{join_parts, latest_place, parts_of, sort_by_id, Row, ToolLocation};
use sakura_core::engine::agents::{AgentRun, Listening, Outcome, RunState, WriteOutcome};
use sakura_core::engine::feed::{AgentFeedPage, FeedEntry};

const HISTORY_PAGE_ROWS: usize = 500;
const OLDER_ROWS_THRESHOLD: usize = 8;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// What the box says before anything is typed: who the message goes to, by how it reaches the agent. None
/// when nothing can read it: the page then says why instead of showing a box.
pub fn composer_words(listening: &Listening, role: &str) -> Option<SharedString> {
    let key = match listening {
        Listening::Live => Key("agent_chat.write.live"),
        Listening::Resume => Key("agent_chat.write.resume"),
        Listening::Queue => Key("agent_chat.write.queue"),
        Listening::No(_) => return None,
    };
    let role = role_words(role).to_lowercase();
    Some(tr_with(key, &[("role", &role)]))
}

/// What the conversation asks of its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChatEvent {
    /// The person pressed the way back to the lane's overview.
    Back,
    /// What the box holds, unsent, changed: the window keeps it.
    DraftChanged,
}

/// The text the agent is writing now, and the rows that draw it.
struct Live {
    text: String,
    answer: Answer,
}

/// The conversation of the agent whose first run is `origin`.
pub struct AgentChat {
    source: Source,
    repo: PathBuf,
    task: String,
    origin: String,
    /// The first run of each round of the agent, in order: the conversation is read from each in turn, and a
    /// quiet divider stands where a round after the first starts.
    rounds: Vec<Round>,
    /// The round being read; the ones before it are read to their end.
    reading: usize,
    agents: Entity<LaneAgents>,
    rows: Vec<Row>,
    /// The id of the entry each row came from, in the rows' order.
    ids: HashMap<String, usize>,
    first_shown: usize,
    /// Whether the last row's text continues in a part not read yet.
    awaits_part: bool,
    /// The ids of every text already shown, parts included: a text sent again is not added twice.
    seen_texts: HashSet<String>,
    /// Where the next page starts; none before the first.
    cursor: Option<String>,
    live: Option<Live>,
    /// How many times the text being written grew in place: a capture waits for the next.
    growths: usize,
    state: RunState,
    is_reading: bool,
    /// A read was requested while one was under way: one more follows it.
    is_again: bool,
    problem: Option<String>,
    scroller: Entity<MessageScrollerState>,
    /// Scrolls the conversation while a drag-selection is held at its edge.
    drag_scroll: super::drag_scroll::DragScroll,
    /// Where the focus stands when the person presses in the conversation: the box lets go of it then, and
    /// with nothing focused the Mac's Copy has no window to reach the selection through.
    focus: gpui::FocusHandle,
    /// The messages whose "Show all" was pressed.
    expanded: HashSet<usize>,
    /// The markdown of each message, read once, whole or folded.
    texts: HashMap<(usize, bool), (u64, Entity<TextViewState>)>,
    text_access: u64,
    composer: Entity<LaneComposer>,
    /// A message is on its way: the box waits for the outcome.
    is_sending: bool,
    /// The message on its way, and where it stands in the rows once the engine took it, until the engine's own
    /// line of it comes with the feed and takes its place.
    sending_text: Option<String>,
    echoed_at: Option<usize>,
    /// What the engine said to the last message when it was not a plain delivery.
    outcome: Option<String>,
    /// The run those words are about: they go when it ends.
    outcome_run: Option<String>,
    _observers: Vec<Subscription>,
    /// Whether the conversation is shown: only then it reads.
    is_active: bool,
    _tick: Option<gpui::Task<()>>,
    _clock: Option<gpui::Task<()>>,
    location: Option<(ToolLocation, Option<Location>)>,
    location_task: Option<gpui::Task<()>>,
}

impl EventEmitter<AgentChatEvent> for AgentChat {}

/// The state the header says: the record's, except that a record that says running with no process behind it (the
/// moment between a resumed answer ending and the record saying so) is what the feed says of the run when that is
/// finished or running: never a "Stopped" that is gone a moment later.
pub fn shown_state(listed: RunState, read: RunState) -> RunState {
    match (listed, read) {
        (RunState::Orphaned, RunState::Finished | RunState::Running) => read,
        _ => listed,
    }
}

impl AgentChat {
    /// The conversation of `origin` in the lane `task` of `repo`, read at once and again while it is shown.
    pub fn new(
        (source, repo, task): (Source, PathBuf, String),
        origin: String,
        agents: Entity<LaneAgents>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| LaneComposer::new(window, cx));
        let observers = vec![
            cx.observe(&agents, |chat, _, cx| {
                chat.drop_words_of_ended_run(cx);
                cx.notify();
            }),
            cx.subscribe(&composer, |chat, _, ComposerEvent::Send(text), cx| {
                chat.send(text, cx)
            }),
            cx.observe(&composer, |_, _, cx| cx.emit(AgentChatEvent::DraftChanged)),
        ];
        Self {
            source,
            repo,
            task,
            rounds: vec![Round {
                origin: origin.clone(),
                number: 1,
            }],
            reading: 0,
            origin,
            agents,
            rows: Vec::new(),
            ids: HashMap::new(),
            first_shown: 0,
            awaits_part: false,
            seen_texts: HashSet::new(),
            cursor: None,
            live: None,
            growths: 0,
            state: RunState::Starting,
            is_reading: false,
            is_again: false,
            problem: None,
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            drag_scroll: super::drag_scroll::DragScroll::default(),
            focus: cx.focus_handle(),
            expanded: HashSet::new(),
            texts: HashMap::new(),
            text_access: 0,
            composer,
            is_sending: false,
            sending_text: None,
            echoed_at: None,
            outcome: None,
            outcome_run: None,
            is_active: false,
            _observers: observers,
            _tick: None,
            _clock: None,
            location: None,
            location_task: None,
        }
    }

    /// Starts or stops the reading of the conversation and the clock of its running tools. A conversation the
    /// person does not see (the overview shows, another agent is open, the lane's tab is not shown) reads
    /// nothing; it keeps its rows, its cursor and what the last message came to, and reads at once when it is
    /// shown again.
    pub fn set_active(&mut self, is_active: bool, cx: &mut Context<Self>) {
        if self.is_active == is_active {
            return;
        }
        self.is_active = is_active;
        match is_active {
            true => self.start_timers(cx),
            false => {
                self._tick = None;
                self._clock = None;
                if self.location_task.take().is_some() {
                    self.location = None;
                }
            }
        }
    }

    fn start_timers(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| loop {
            let wait = this.update(cx, |chat, cx| {
                chat.refresh(cx);
                chat.next_tick()
            });
            let Ok(wait) = wait else { return };
            cx.background_executor().timer(wait).await;
        }));
        self._clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(sizes::AGENTS_CLOCK_TICK)
                .await;
            if this.update(cx, |chat, cx| chat.tick_clock(cx)).is_err() {
                return;
            }
        }));
    }

    /// What the box holds, unsent.
    pub fn draft(&self, cx: &gpui::App) -> String {
        self.composer.read(cx).draft(cx)
    }

    pub(super) fn is_sending(&self) -> bool {
        self.is_sending
    }

    /// Puts back the words a last launch left unsent.
    pub fn restore(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.restore(text, window, cx));
    }

    pub fn place(&mut self, worktree: Option<&str>, cx: &mut Context<Self>) -> Option<Location> {
        let request = latest_place(&self.rows, worktree)?;
        if let Some((_, resolved)) = self.location.as_ref().filter(|(key, _)| *key == request) {
            return resolved.clone();
        }
        self.location = Some((request.clone(), None));
        let lookup = cx
            .background_executor()
            .spawn(async move { request.resolve() });
        self.location_task = Some(cx.spawn(async move |chat, cx| {
            let resolved = lookup.await;
            drop(chat.update(cx, |chat, cx| {
                chat.location_task = None;
                if let Some((_, location)) = chat.location.as_mut() {
                    *location = Some(resolved);
                    cx.notify();
                }
            }));
        }));
        None
    }

    /// How many times the text being written grew in place.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn growths(&self) -> usize {
        self.growths
    }

    /// Which markdown state draws the first part of the text being written: the same while it grows.
    #[cfg(test)]
    pub fn live_part_id(&self) -> Option<gpui::EntityId> {
        let live = self.live.as_ref()?;
        live.answer.parts().first().map(|part| part.entity_id())
    }

    /// The field of the box.
    #[cfg(test)]
    pub fn composer_field(&self, cx: &gpui::App) -> Entity<gpui_component::input::TextareaState> {
        self.composer.read(cx).field()
    }

    /// The agent as the list has it now: the run that works, how a message reaches it.
    fn card(&self, cx: &gpui::App) -> Option<AgentCard> {
        let cards = super::lane_agents::cards_of(self.agents.read(cx).runs());
        let is_ours =
            |card: &AgentCard| card.rounds.iter().any(|round| round.origin == self.origin);
        cards.into_iter().find(is_ours)
    }

    /// The run the conversation is read from now: the round being read.
    fn reading_origin(&self) -> String {
        let round = self.rounds.get(self.reading);
        round.map_or(&self.origin, |round| &round.origin).clone()
    }

    /// Takes the rounds the list has now: a round may have started since the last read.
    fn learn_rounds(&mut self, cx: &gpui::App) {
        if let Some(card) = self.card(cx) {
            self.rounds = card.rounds;
        }
    }

    /// How long until the conversation is read again: often while the agent works, seldom after.
    fn next_tick(&self) -> Duration {
        match self.state.is_live() {
            true => sizes::AGENT_CHAT_LIVE_TICK,
            false => sizes::AGENT_CHAT_IDLE_TICK,
        }
    }

    /// Draws the time of a tool that runs again.
    fn tick_clock(&mut self, cx: &mut Context<Self>) {
        let is_timing = self
            .rows
            .iter()
            .any(|row| matches!(row, Row::Tool(tool) if tool.is_running()));
        if is_timing {
            cx.notify();
        }
    }

    /// Reads what the conversation gained since the cursor; a read requested during one follows it.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_active {
            return;
        }
        if self.is_reading {
            self.is_again = true;
            return;
        }
        self.is_reading = true;
        self.learn_rounds(cx);
        let (repo, task, origin) = (self.repo.clone(), self.task.clone(), self.reading_origin());
        let pending = self
            .source
            .agent_feed(repo, task, origin, self.cursor.clone());
        cx.spawn(async move |this, cx| {
            let read = pending.await;
            drop(this.update(cx, |chat, cx| chat.read(read, cx)));
        })
        .detach();
    }

    fn read(
        &mut self,
        read: sakura_core::error::BridgeResult<AgentFeedPage>,
        cx: &mut Context<Self>,
    ) {
        self.is_reading = false;
        match read {
            Ok(page) => {
                self.problem = None;
                self.take(page, cx);
            }
            Err(error) => self.problem = Some(told(&error)),
        }
        cx.notify();
        if std::mem::take(&mut self.is_again) {
            self.refresh(cx);
        }
    }

    /// Adds what a page holds. An entry already shown (a tool sent again when it ends) takes its row's place.
    /// The text being written grows in place while nothing new arrived; when a message completed, its row
    /// takes the place of the live text, and a new live text starts after it.
    fn take(&mut self, page: AgentFeedPage, cx: &mut Context<Self>) {
        self.cursor = Some(page.cursor.clone());
        self.state = page.state;
        let is_round_over = page.entries.is_empty() && page.live.is_none() && !page.state.is_live();
        let joined = join_parts(self.awaits_part, self.new_parts(&page.entries));
        self.awaits_part = joined.awaits_part;
        if let Some(tail) = joined.tail {
            self.extend_last(&tail, cx);
        }
        let (mut replaced, mut added) = sort_by_id(&self.ids, joined.rows);
        self.take_over_echo(&mut replaced, &mut added);
        self.replace_rows(replaced, cx);
        let keeps_live = added.is_empty()
            && match (&self.live, &page.live) {
                (Some(live), Some(text)) => text.starts_with(&live.text),
                _ => false,
            };
        if keeps_live {
            self.grow_live(page.live.as_deref().unwrap_or_default(), cx);
            return;
        }
        let was_read_live = self.live.is_some();
        self.drop_live(cx);
        let first_words = added
            .iter()
            .position(|(_, row)| matches!(row, Row::Words { .. }));
        let at = self.rows.len();
        self.add_rows(added, cx);
        if let Some(words) = first_words.filter(|_| was_read_live) {
            // The message that was streaming is the one being read: it stays whole when it completes.
            self.expanded.insert(at + words);
        }
        if let Some(text) = page.live {
            self.start_live(&text, cx);
        }
        let has_next = self.reading + 1 < self.rounds.len();
        match (is_round_over, has_next && !self.state.is_live()) {
            (true, _) => self.start_next_round(cx),
            // A round that ended is read again at once, until a read brings nothing: its end.
            (false, true) => self.is_again = true,
            _ => {}
        }
    }

    /// The engine's own line of the message that was echoed takes the echo's place, so it is not shown twice.
    fn take_over_echo(&mut self, replaced: &mut Vec<(usize, Row)>, added: &mut Vec<(String, Row)>) {
        let Some(at) = self.echoed_at else {
            return;
        };
        let Some(Row::You(echoed)) = self.rows.get(at) else {
            self.echoed_at = None;
            return;
        };
        let theirs = added
            .iter()
            .position(|(_, row)| matches!(row, Row::You(text) if text == echoed));
        if let Some(found) = theirs {
            let (id, row) = added.remove(found);
            if !id.is_empty() {
                self.ids.entry(id).or_insert(at);
            }
            replaced.push((at, row));
            self.echoed_at = None;
        }
    }

    /// Moves on to the next round, when the agent has one: a divider, and the read starts from its beginning.
    fn start_next_round(&mut self, cx: &mut Context<Self>) {
        if self.reading + 1 >= self.rounds.len() {
            return;
        }
        self.reading += 1;
        self.cursor = None;
        self.awaits_part = false;
        let number = self.rounds[self.reading].number;
        self.add_rows(vec![(String::new(), Row::Round(number))], cx);
        self.is_again = true;
    }

    /// Whether the message of row `index` is the agent's last: no message after it, written or being written.
    pub(super) fn is_last_message(&self, index: usize) -> bool {
        let is_later = |row: &Row| matches!(row, Row::Words { .. });
        self.live.is_none() && !self.rows.iter().skip(index + 1).any(is_later)
    }

    fn drop_live(&mut self, cx: &mut Context<Self>) {
        if self.live.take().is_some() {
            let at = self.rows.len() - self.first_shown;
            self.scroller
                .update(cx, |scroller, cx| scroller.splice(at..at + 1, 0, cx));
        }
    }

    /// The page's rows without the texts already shown, which the engine sends again after it read a
    /// replaced log from its start; tools stay, so an ended tool replaces its row by id.
    fn new_parts(&mut self, entries: &[FeedEntry]) -> Vec<(String, Row, bool)> {
        let round = self.reading_origin();
        let mut parts = parts_of(entries);
        for (id, _, _) in &mut parts {
            // The ids are the run's own: two rounds may use the same.
            if !id.is_empty() {
                *id = format!("{round}|{id}");
            }
        }
        parts.retain(|(id, row, _)| {
            let is_text = matches!(row, Row::Words { .. });
            !is_text || id.is_empty() || self.seen_texts.insert(id.clone())
        });
        parts
    }

    /// Adds `tail` to the text of the last row: the next part of a long text.
    fn extend_last(&mut self, tail: &str, cx: &mut Context<Self>) {
        let Some(Row::Words { text, .. }) = self.rows.last_mut() else {
            return;
        };
        text.push_str(tail);
        let at = self.rows.len() - 1;
        self.forget_text(at);
        self.remeasure(at, cx);
    }

    /// Drops the drawn text of row `at`, so the row draws its new words.
    fn forget_text(&mut self, at: usize) {
        self.texts.remove(&(at, true));
        self.texts.remove(&(at, false));
    }

    fn replace_rows(&mut self, replaced: Vec<(usize, Row)>, cx: &mut Context<Self>) {
        for (at, row) in replaced {
            self.rows[at] = row;
            self.forget_text(at);
            self.remeasure(at, cx);
        }
    }

    fn remeasure(&self, row: usize, cx: &mut Context<Self>) {
        if let Some(index) = row.checked_sub(self.first_shown) {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(index..index + 1, cx)
            });
        }
    }

    fn add_rows(&mut self, added: Vec<(String, Row)>, cx: &mut Context<Self>) {
        let at = self.rows.len();
        if at == 0 {
            self.first_shown = added.len().saturating_sub(HISTORY_PAGE_ROWS);
        }
        let visible_at = at.saturating_sub(self.first_shown);
        let count = added.len() - self.first_shown.saturating_sub(at);
        for (id, row) in added {
            if !id.is_empty() {
                self.ids.entry(id).or_insert(self.rows.len());
            }
            self.rows.push(row);
        }
        self.scroller.update(cx, |scroller, cx| {
            scroller.splice(visible_at..visible_at, count, cx)
        });
    }

    fn page_older_rows(&mut self, cx: &mut Context<Self>) {
        let list = self.scroller.read(cx);
        if self.first_shown == 0
            || !list.is_scrolled_up()
            || list.logical_scroll_top().item_ix > OLDER_ROWS_THRESHOLD
        {
            return;
        }
        let count = self.first_shown.min(HISTORY_PAGE_ROWS);
        self.first_shown -= count;
        self.scroller
            .update(cx, |list, cx| list.splice(0..0, count, cx));
    }

    fn start_live(&mut self, text: &str, cx: &mut Context<Self>) {
        let answer = Answer::new(text, true, false, cx);
        self.live = Some(Live {
            text: text.to_string(),
            answer,
        });
        let at = self.rows.len() - self.first_shown;
        self.scroller
            .update(cx, |scroller, cx| scroller.splice(at..at, 1, cx));
    }

    fn grow_live(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.text.len() == text.len() {
            return;
        }
        live.text = text.to_string();
        live.answer.grow(text, true, cx);
        self.growths += 1;
        self.remeasure(self.rows.len(), cx);
    }

    /// Sends `text` to the agent: to the run that works now, which is the agent. One message at a time: the
    /// box waits for the outcome, whatever it is pressed or typed meanwhile.
    fn send(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(card) = self.card(cx) else { return };
        if std::mem::replace(&mut self.is_sending, true) {
            return;
        }
        self.outcome = None;
        self.outcome_run = None;
        self.sending_text = Some(text.to_string());
        self.composer
            .update(cx, |composer, cx| composer.set_busy(true, cx));
        let (repo, task) = (self.repo.clone(), self.task.clone());
        let pending = self
            .source
            .write_to_agent(repo, task, card.tail.name, text.to_string());
        cx.spawn(async move |this, cx| {
            let outcome = pending.await;
            drop(this.update(cx, |chat, cx| chat.sent(outcome, cx)));
        })
        .detach();
    }

    /// What became of the message: the box empties when the engine took it, and keeps it when it did not.
    fn sent(
        &mut self,
        outcome: sakura_core::error::BridgeResult<WriteOutcome>,
        cx: &mut Context<Self>,
    ) {
        self.is_sending = false;
        self.composer
            .update(cx, |composer, cx| composer.set_busy(false, cx));
        match outcome {
            Ok(done) if done.outcome == Outcome::Refused => self.outcome = Some(done.words),
            Ok(done) => {
                self.composer.update(cx, |composer, cx| composer.clear(cx));
                self.echo_sent(cx);
                let is_plain = done.outcome == Outcome::Delivered;
                self.outcome_run = (!is_plain).then_some(done.run);
                self.outcome = (!is_plain).then_some(done.words);
                self.agents.update(cx, |agents, cx| agents.refresh(cx));
                self.refresh(cx);
            }
            Err(error) => self.outcome = Some(told(&error)),
        }
        cx.notify();
    }

    /// The message the engine took shows as the person's at once, in the frame the box empties; the engine's own
    /// line of it takes its place when the feed brings it.
    fn echo_sent(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.sending_text.take() else {
            return;
        };
        self.echoed_at = Some(self.rows.len());
        self.add_rows(vec![(String::new(), Row::You(text))], cx);
    }

    /// Drops what the engine said of a message once the run it names has ended: "back at it" is no longer true.
    fn drop_words_of_ended_run(&mut self, cx: &gpui::App) {
        let Some(named) = self.outcome_run.as_deref() else {
            return;
        };
        let runs = self.agents.read(cx).runs();
        let has_ended = runs
            .iter()
            .find(|run| run.name == named)
            .is_some_and(|run| !run.state.is_live());
        if has_ended {
            self.outcome = None;
            self.outcome_run = None;
        }
    }

    /// The run that works now, for the header's state and for the listening of the box.
    fn run_of(&self, cx: &gpui::App) -> Option<AgentRun> {
        self.card(cx).map(|card| card.tail)
    }
}

impl Render for AgentChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        self.chat(&palette, window, cx)
    }
}
