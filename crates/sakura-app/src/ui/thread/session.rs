//! The view's side of the connection: starting the agent through its host, reading its events once per
//! frame, and the person's three actions (send, stop, answer). What the person does shows in the same
//! frame, before the agent hears of it.

use super::{composed, Phase, ThreadView};
use crate::l10n::{tr, Key};
use crate::theme::thread as tokens;
use crate::ui::enter::is_composing;
use gpui::{App, Context, SharedString, Task, Window};
use sakura_acp::connection::{Agent, AgentEvent};
use sakura_acp::model::{SessionChoices, StopReason};
use sakura_acp::thread::{EntryKind, Problem, ProblemKind, ThreadEvent, Turn};
use tokio::sync::mpsc::Receiver;

/// What asking for the task's agent gave.
pub enum Launched {
    /// The agent's process was started: its handle and its events.
    Started(Agent, Receiver<AgentEvent>),
    /// No account is signed in for this task: nothing was started, the person signs in first.
    SignInFirst,
    /// No account is named for the task's agent, by its project or by being the only one: nothing
    /// was started, the person chooses one first.
    NoAccount,
    /// There is no `box` to start an agent with (the sample data).
    Unavailable,
}

/// Starts the task's agent.
pub type Launch = Box<dyn Fn(&App) -> Launched>;
/// Something the person asks of the window from the conversation: the sign-in, the terminal.
pub type Ask = Box<dyn Fn(&mut Window, &mut App)>;
/// Gives the task's project the account the person chose for its agent.
pub type ChooseAccount = Box<dyn Fn(&str, &mut App)>;
/// The accounts the task's agent could start with.
pub type Accounts = Box<dyn Fn(&App) -> Vec<String>>;

/// Reads the session stored for the task.
pub type StoredSession = Box<dyn Fn(&App) -> Option<String>>;
/// Stores the session that just opened.
pub type StoreSession = Box<dyn Fn(&str, &App)>;
/// The words of the refusals kept for a session. Reads memory only.
pub type Refusals = Box<dyn Fn(&str, &App) -> Vec<String>>;
/// Keeps the words of a refusal said in a session.
pub type KeepRefusal = Box<dyn Fn(&str, &str, &App)>;

/// Where the task of a first message stands with the service that makes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Readiness {
    /// The task is there: its agent can be started.
    Ready,
    /// The service is making it; its own progress line, when it gave one.
    Preparing(Option<String>),
    /// The service refused or failed: the sentence that says so, and what the service itself wrote.
    Failed {
        said: SharedString,
        details: Option<String>,
    },
}

/// Whose login a sign-in is about, and so what the conversation says and offers when one is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Login {
    /// An account of the boxes: the window can run its sign-in.
    Box,
    #[cfg(not(feature = "boxes"))]
    Codex { is_waiting: bool },
    /// The person's own login on this Mac, by its name: it expired, and is renewed outside Sakura.
    Mac { account: Option<String> },
}

/// What the view needs from whoever holds it. The desk gives the task's `box acp` command, its session
/// store, its sign-in and its terminal; a test gives a fake agent.
pub struct Host {
    pub launch: Launch,
    /// The session stored for the task, if any. Asked on every frame of an empty conversation: it must
    /// read memory only.
    pub stored_session: StoredSession,
    pub store_session: StoreSession,
    pub refusals: Refusals,
    pub keep_refusal: KeepRefusal,
    /// Whose login the agent needs, as the accounts read now say.
    pub login: Box<dyn Fn(&App) -> Login>,
    /// The accounts the task's agent could start with, when none is named for it. Read memory only.
    pub accounts: Accounts,
    pub choose_account: ChooseAccount,
    /// Opens the sign-in of the task's account (a task in a box).
    pub sign_in: Ask,
    /// Shows the task's terminal, when this `box` cannot hold a conversation.
    pub open_terminal: Ask,
    /// The agent's workspace as the agent names it (`/work/<task>` in a box, the worktree on the Mac):
    /// paths are shown from it. Asked when a row is drawn: it must read memory only.
    pub workspace: Box<dyn Fn(&App) -> String>,
    /// Where the task stands while its first message waits for it. Read memory only.
    pub readiness: Box<dyn Fn(&App) -> Readiness>,
    /// Asks for the task again after its creation failed.
    pub retry_creation: Ask,
}

impl ThreadView {
    /// Shows `text` as the person's first message of a task that was just requested. The agent is
    /// started, and the message sent to it, when the task is ready (see [`Self::proceed`]).
    pub fn begin(&mut self, text: String, cx: &mut Context<Self>) {
        self.thread.apply(ThreadEvent::Prompted(text.clone()));
        self.waiting_prompt = Some(text);
        self.phase = Phase::Preparing;
        self.proceed(cx);
    }

    /// Something was heard of the task a first message waits for: once it is ready the agent starts,
    /// and only once, whatever order the facts come in.
    pub fn proceed(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Preparing {
            return;
        }
        match (self.host.readiness)(cx) {
            Readiness::Ready => self.connect(cx),
            Readiness::Preparing(_) | Readiness::Failed { .. } => self.sync(cx),
        }
    }

    /// Whether the first message still waits for its task.
    pub fn is_preparing(&self) -> bool {
        self.phase == Phase::Preparing
    }

    /// Starts the agent, or starts it again after it ended; the stored session is opened when it answers.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if !self.has_opened_session {
            self.choices = SessionChoices::default();
        }
        match (self.host.launch)(cx) {
            Launched::Started(agent, events) => {
                self.agent = Some(agent);
                self.phase = Phase::Connecting;
                self._pump = Some(self.pump(events, cx));
            }
            Launched::SignInFirst => self.sign_in_needed(),
            Launched::NoAccount => self.phase = Phase::NoAccount,
            Launched::Unavailable => {
                let problem = Problem {
                    kind: ProblemKind::NotStarted,
                    detail: String::new(),
                };
                crate::agent_log::record(&problem, cx);
                self.ended(Some(problem));
            }
        }
        self.sync(cx);
    }

    /// Takes up the stored conversation: the agent starts and replays it. Requested when the task's
    /// conversation is the one shown, so that a task that is only listed starts nothing.
    /// It is tried once: an agent that ends without a word is not started again by being looked at.
    pub fn load_kept(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Idle && !self.tried_kept && self.shows_kept_next(cx) {
            self.tried_kept = true;
            self.connect(cx);
        }
    }

    /// Whether what this empty conversation shows next is the one stored for the task: it is about
    /// to be loaded, or its agent is starting to load it.
    pub(super) fn shows_kept_next(&self, cx: &App) -> bool {
        let is_before = match self.phase {
            Phase::Idle => !self.tried_kept,
            Phase::Connecting => true,
            _ => false,
        };
        is_before && self.thread.entries().is_empty() && (self.host.stored_session)(cx).is_some()
    }

    /// Reads the agent's events and applies them in batches, one batch per frame, so a fast stream costs
    /// one draw per frame and never holds the main thread.
    fn pump(&self, mut events: Receiver<AgentEvent>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |view, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while batch.len() < tokens::EVENT_BATCH {
                    match events.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                if view.update(cx, |view, cx| view.receive(batch, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(tokens::EVENT_FRAME).await;
            }
        })
    }

    pub(super) fn receive(&mut self, batch: Vec<AgentEvent>, cx: &mut Context<Self>) {
        for event in batch {
            self.receive_one(event, cx);
        }
        self.watch_loading(cx);
        self.sync(cx);
    }

    /// A conversation that is loading and is not loaded after the stall is said not to have loaded: never an
    /// endless "Loading".
    fn watch_loading(&mut self, cx: &mut Context<Self>) {
        match (self.phase == Phase::Loading, self.load_watch.is_some()) {
            (true, false) => {
                self.load_watch = Some(cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(tokens::LOAD_STALL).await;
                    drop(view.update(cx, |view, cx| view.load_stalled(cx)));
                }));
            }
            (false, true) => self.load_watch = None,
            _ => {}
        }
    }

    fn load_stalled(&mut self, cx: &mut Context<Self>) {
        self.load_watch = None;
        if self.phase != Phase::Loading {
            return;
        }
        self.drop_waiting_prompts();
        self.close_turn();
        let problem = Problem {
            kind: ProblemKind::TimedOut,
            detail: tr(Key("thread.load_stalled")).to_string(),
        };
        crate::agent_log::record(&problem, cx);
        self.phase = Phase::Ended(problem);
        self.sync(cx);
    }

    fn receive_one(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        match event {
            AgentEvent::Connected { .. } => self.open_session(cx),
            AgentEvent::SessionReady => self.ready_for_prompt(),
            AgentEvent::SessionOpened {
                session_id,
                is_loaded,
            } => {
                self.has_opened_session = true;
                self.is_kept_lost = self.asked_for_kept && !is_loaded;
                (self.host.store_session)(&session_id, cx);
                self.ready_for_prompt();
            }
            AgentEvent::SessionUnavailable { session_id } => {
                self.kept_session_unavailable(session_id, cx);
            }
            AgentEvent::AuthRequired => self.sign_in_needed(),
            AgentEvent::Supports(support) => self.prompt_support = Some(support),
            AgentEvent::Choices(choices) => self.choices.merge(choices),
            AgentEvent::ModeChanged(mode) => self.choices.set_mode(&mode),
            AgentEvent::Commands(commands) => self.commands = commands,
            AgentEvent::Thread(event) => {
                if event == ThreadEvent::ReplayStarted {
                    self.phase = Phase::Loading;
                }
                // The Claude adapter opens a session without a login and refuses the prompt instead.
                let is_sign_in = matches!(&event, ThreadEvent::Failed(problem)
                    if problem.kind == ProblemKind::AuthRequired);
                // A refusal while the session is still opening (its mode could not be set, say) ends the
                // wait: the message that waited is never sent to a session that was not opened.
                let refused = match &event {
                    ThreadEvent::Failed(problem)
                        if !is_sign_in
                            && matches!(self.phase, Phase::Connecting | Phase::Loading) =>
                    {
                        Some(problem.clone())
                    }
                    _ => None,
                };
                match is_sign_in {
                    true => self.sign_in_needed(),
                    false => self.apply_kept(event, cx),
                }
                if let Some(problem) = refused {
                    crate::agent_log::record(&problem, cx);
                    self.drop_waiting_prompts();
                    self.close_turn();
                    self.phase = Phase::Ended(problem);
                }
            }
            AgentEvent::Ended(Some(problem)) if problem.kind == ProblemKind::AuthRequired => {
                self.agent = None;
                self.sign_in_needed();
            }
            AgentEvent::Ended(problem) => {
                if let Some(problem) = &problem {
                    if matches!(self.phase, Phase::Connecting | Phase::Loading) {
                        crate::agent_log::record(problem, cx);
                    }
                }
                self.ended(problem);
            }
        }
    }

    fn ready_for_prompt(&mut self) {
        self.phase = Phase::Ready;
        self.waiting_prompt = self.held_message.take().or(self.waiting_prompt.take());
        self.send_waiting_prompt();
    }

    /// Applies an event of the thread. A refusal is kept with its session when it is said, and is the
    /// refusal again when that session is replayed: the agent replays it as a plain message.
    fn apply_kept(&mut self, event: ThreadEvent, cx: &App) {
        let is_refused = event == ThreadEvent::TurnEnded(StopReason::Refusal);
        let is_replayed = event == ThreadEvent::ReplayFinished;
        self.thread.apply(event);
        let Some(session) = (self.host.stored_session)(cx) else {
            return;
        };
        if is_replayed {
            let kept = (self.host.refusals)(&session, cx);
            self.thread.mark_refusals(&kept);
        }
        let last = self.thread.entries().last().map(|entry| &entry.kind);
        if let (true, Some(EntryKind::Problem(problem))) = (is_refused, last) {
            (self.host.keep_refusal)(&session, &problem.detail, cx);
        }
    }

    /// The agent cannot go on until the person signs in. The turn of a prompt that waited, or that the
    /// agent refused, is closed; why is said once, under the transcript, with what to do about it.
    fn sign_in_needed(&mut self) {
        self.phase = Phase::SignIn;
        self.drop_waiting_prompts();
        self.close_turn();
    }

    /// The account the person chose was not taken: the agent did not start, and what the service said
    /// is the problem's detail.
    pub fn account_refused(&mut self, details: String, cx: &mut Context<Self>) {
        self.ended(Some(Problem {
            kind: ProblemKind::NotStarted,
            detail: details,
        }));
        self.sync(cx);
    }

    /// Closes a turn the agent will never end, with nothing added to the transcript.
    fn close_turn(&mut self) {
        if self.thread.turn() != Turn::Idle {
            self.thread
                .apply(ThreadEvent::TurnEnded(StopReason::Cancelled));
        }
    }

    fn open_session(&mut self, cx: &App) {
        let stored = (self.host.stored_session)(cx);
        self.asked_for_kept = stored.is_some();
        match stored {
            Some(session) if self.master.is_some() => {
                self.tell_agent(|agent| agent.load_session(session));
            }
            stored => self.tell_agent(|agent| agent.open_session(stored)),
        }
    }

    fn kept_session_unavailable(&mut self, session_id: String, cx: &mut Context<Self>) {
        cx.emit(super::ThreadViewEvent::KeptSessionLost(session_id));
        self.is_kept_lost = true;
        self.has_opened_session = false;
        self.choices = SessionChoices::default();
        if self.waiting_prompt.is_some() || self.held_message.is_some() {
            self.phase = Phase::Connecting;
            self.tell_agent(|agent| agent.open_session(None));
        } else {
            self.agent = None;
            self.phase = Phase::Idle;
        }
    }

    /// Starts over after the person signed in: an agent that was started without the login is ended
    /// and a new one takes its place, so that it reads the login as it is now.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(agent) = self.agent.take() {
            if let Err(error) = agent.shutdown() {
                eprintln!("sakura: the agent did not take its end: {error:?}");
            }
        }
        self._pump = None;
        self.connect(cx);
    }

    fn ended(&mut self, problem: Option<Problem>) {
        self.agent = None;
        self.drop_waiting_prompts();
        self.phase = match problem {
            // A `box` that does not know `acp` prints its usage: no conversation can be held with it.
            // A prompt that waited is closed; why is said once, under the transcript.
            Some(problem) if sakura_core::process::is_usage(&problem.detail) => {
                self.close_turn();
                Phase::NoConversation
            }
            Some(problem) => {
                self.thread.apply(ThreadEvent::Failed(problem.clone()));
                Phase::Ended(problem)
            }
            None => Phase::Idle,
        };
    }

    /// The first message of the conversation, when there is one: what History names it by.
    pub fn first_message(&self) -> Option<String> {
        self.thread
            .entries()
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::User(text) => Some(text.clone()),
                _ => None,
            })
    }

    /// Leaves the conversation and starts an empty one: the agent is told to end, the transcript clears, and
    /// the next message, or [`Self::load_kept`] when a session was stored meanwhile, opens a session.
    pub fn start_over(&mut self, cx: &mut Context<Self>) {
        if let Some(agent) = self.agent.take() {
            if let Err(error) = agent.shutdown() {
                eprintln!("sakura: the agent did not take its end: {error:?}");
            }
        }
        self._pump = None;
        self.waiting_prompt = None;
        self.held_message = None;
        self.thread.apply(ThreadEvent::ReplayStarted);
        self.thread.apply(ThreadEvent::ReplayFinished);
        self.phase = Phase::Idle;
        self.asked_for_kept = false;
        self.is_kept_lost = false;
        self.tried_kept = false;
        self.had_turn = false;
        self.has_opened_session = false;
        self.choices = SessionChoices::default();
        if let Some(kit) = &self.master {
            kit.start.update(|start| *start = Default::default());
        }
        self.commands.clear();
        self.sync(cx);
    }

    /// Adds `text` to what is typed in the composer, on a line of its own, and sends nothing: lines of a
    /// file the person wants to ask about.
    pub fn quote(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.composer.read(cx).value().to_string();
        let whole = match typed.trim().is_empty() {
            true => text.to_string(),
            false => format!("{}\n{text}", typed.trim_end()),
        };
        self.composer
            .update(cx, |composer, cx| composer.set_value(whole, window, cx));
    }

    /// Puts `text` in the composer, as if the person had typed it.
    #[cfg(any(test, feature = "snapshot", not(feature = "boxes")))]
    pub fn write(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.composer
            .update(cx, |composer, cx| composer.set_value(text, window, cx));
        cx.notify();
    }

    /// Whether a message can be given to the agent now, whatever the composer holds: no turn runs and
    /// the conversation is not waiting for a sign-in, an account or its kept session.
    pub fn can_take_message(&self) -> bool {
        let is_idle = self.thread.turn() == Turn::Idle;
        let is_open = !matches!(
            self.phase,
            Phase::SignIn | Phase::NoAccount | Phase::Loading | Phase::NoConversation
        );
        is_idle && is_open
    }

    /// Whether a message can be kept for the conversation that is loading: one is, and none is kept yet.
    fn can_hold_message(&self) -> bool {
        self.phase == Phase::Loading
            && self.held_message.is_none()
            && self.thread.turn() == Turn::Idle
    }

    /// Whether the composer's text can be sent now: there is some, and a message can be taken, or kept until the
    /// conversation has loaded.
    pub fn can_send(&self, cx: &App) -> bool {
        let is_takable = self.can_take_message() || self.can_hold_message();
        is_takable && !composed(&self.composer, cx).is_empty()
    }

    /// Return in the composer: it picks the highlighted command while the menu of commands is open and
    /// otherwise sends, unless an input method holds marked text (Return is then the input method's).
    pub fn on_return(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if is_composing(&self.composer, window, cx) {
            return;
        }
        match self.slash_is_open(cx) {
            true => self.slash_accept(window, cx),
            false => self.send(window, cx),
        }
    }

    /// Sends the composer's text: it shows as the person's message at once. With no session yet, the agent
    /// is started and the prompt goes out when the session opens. While the conversation loads the text is kept
    /// and goes when it has loaded, so it follows what was loaded instead of being wiped by it.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(local) = self.local_command(cx) {
            return self.run_local(local, window, cx);
        }
        if !self.can_send(cx) {
            return;
        }
        let text = composed(&self.composer, cx);
        self.composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        if self.phase == Phase::Loading {
            self.held_message = Some(text);
            return self.sync(cx);
        }
        self.deliver(text, cx);
    }

    /// Sends `text` as a message of its own (a request the window makes for the person, such as
    /// "Publish") and leaves what is written in the composer exactly as it is. Says whether it went.
    pub fn say(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let text = text.trim();
        if text.is_empty() || !self.can_take_message() {
            return false;
        }
        self.deliver(text.to_string(), cx);
        true
    }

    /// `text` becomes the person's message, shown at once, and goes to the agent now or when its
    /// session opens.
    fn deliver(&mut self, text: String, cx: &mut Context<Self>) {
        let shown = self.with_attached_line(&text);
        self.thread.apply(ThreadEvent::Prompted(shown));
        self.is_kept_lost = false;
        match self.phase {
            Phase::Ready => self.hand_over(text),
            Phase::Idle | Phase::Ended(_) => {
                self.waiting_prompt = Some(text);
                self.connect(cx);
            }
            _ => self.waiting_prompt = Some(text),
        }
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
        self.sync(cx);
    }

    /// The session is open: the prompt that waited for it goes out. When a replay (of the stored
    /// session, or the empty one of a load that failed) cleared the conversation meanwhile, the prompt
    /// and its running turn are shown again first, after what was replayed.
    /// The prompts that waited for a session that will not come: a held message goes back to the composer, the
    /// one written before the session was open is closed with its turn, as the problem says.
    fn drop_waiting_prompts(&mut self) {
        self.waiting_prompt = None;
        self.given_back = self.held_message.take().or(self.given_back.take());
    }

    fn send_waiting_prompt(&mut self) {
        let Some(text) = self.waiting_prompt.take() else {
            return;
        };
        if self.thread.turn() == Turn::Idle {
            self.thread.apply(ThreadEvent::Prompted(text.clone()));
        }
        self.hand_over(text);
    }

    /// Gives the agent the message and the files attached to it.
    fn hand_over(&mut self, text: String) {
        let sent = self.outgoing(&text);
        let attachments = self.take_attachments();
        self.tell_agent(|agent| agent.prompt_with(sent, attachments));
    }

    /// `text` as the agent is sent it: the master's messages say which project they are about.
    fn outgoing(&self, text: &str) -> String {
        match &self.master {
            Some(kit) => kit.outgoing(text),
            None => text.to_string(),
        }
    }

    /// Whether a turn runs that the person can stop. While the task is being made, or its message
    /// waits for an account, there is no agent to stop.
    pub fn can_stop(&self) -> bool {
        let has_agent = !matches!(self.phase, Phase::Preparing | Phase::NoAccount);
        self.thread.turn() != Turn::Idle && has_agent
    }

    /// Stops the running turn: what was running shows as interrupted at once.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.thread.apply(ThreadEvent::CancelRequested);
        self.tell_agent(Agent::cancel);
        self.sync(cx);
    }

    /// Answers a permission request with one of its options.
    pub fn answer(&mut self, request: u64, option_id: String, cx: &mut Context<Self>) {
        self.tell_agent(|agent| agent.answer(request, option_id));
        cx.notify();
    }

    /// Hands a command to the agent. One that cannot be handed over is said on stderr: the connection's
    /// own end, which follows, is what the person sees.
    pub(super) fn tell_agent(
        &self,
        tell: impl FnOnce(&Agent) -> Result<(), sakura_acp::connection::SendError>,
    ) {
        let Some(agent) = self.agent.as_ref() else {
            return eprintln!("sakura: no agent to talk to");
        };
        if let Err(error) = tell(agent) {
            eprintln!("sakura: the agent did not take a command: {error:?}");
        }
    }
}
