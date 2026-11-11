//! The terminals of a lavoro's centre: one console of the agent (its terminal, or the login requested there)
//! and, beside it, one terminal of the lavoro's own (a shell). Each has its place: opening one never
//! replaces, hides for good or ends the other. They are kept while the app runs, so switching lavori or
//! views never attaches again and never loses scrollback. A console goes when its task is removed, when
//! facts read after its end say the agent is gone, when the person closes it, when its shell ends, or with
//! the app: every session is closed when it quits. A session is opened on the background executor,
//! never on the UI thread: the pane says it is opening meanwhile, and an opening that a newer one, a close or
//! a removal superseded is closed as soon as it answers, never shown.

pub mod login;
pub mod rules;
pub mod view;

use crate::data::actions::Accounts;
use crate::data::actions::Actions;
use crate::data::facts::{Facts, Phase};
use crate::data::source::{Source, FIXTURE_REFUSAL};
use crate::l10n::Said;
use crate::theme::{self, Theme};
use gpui::{AppContext as _, Context, Entity, EventEmitter, Subscription, Window};
use login::LoginFailure;
use rules::is_agent_running;
use rules::{ConsoleKind, Outcome, Place};
use sakura_core::box_words::{self, Failure};
use sakura_core::error::BridgeResult;
use sakura_core::links;
use sakura_core::models::TerminalRequest;
use sakura_core::terminal::Terminals;
use sakura_terminal::{CoreRouter, CoreSession, GridSize, TerminalChange, TerminalView};
use std::collections::HashMap;
use std::time::Instant;

/// Where a console is in its life.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleState {
    /// Its session is being opened off the UI thread.
    Opening,
    /// Its process runs.
    Live,
    /// Its process exited at `at`; what it printed stays on screen.
    Ended { code: Option<i32>, at: Instant },
    /// It could not start, or `box` refused at `at`: the reason replaces the terminal.
    Failed { reason: Failure, at: Instant },
    /// A login ended well at `at`: the account is being read again.
    Checking { at: Instant },
    /// A login failed at `at`; what it printed stays under the reason.
    LoginFailed { failure: LoginFailure, at: Instant },
}

/// Emitted when a login ended and its account reads as logged in: the task's pane is free again and the
/// account can start an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDone {
    pub account: String,
}

/// Emitted when the shell of a lavoro's own terminal ended (an `exit`, or anything else): the terminal
/// is gone, and only the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEnded {
    pub task: String,
    pub code: Option<i32>,
}

/// One console of a task.
pub struct Console {
    kind: ConsoleKind,
    /// Which opening this console is: an answer for another one is closed, not shown.
    generation: u64,
    state: ConsoleState,
    view: Option<Entity<TerminalView>>,
    /// The sign-in address a login printed, once the app opened it in the browser: at most one per
    /// session.
    sign_in: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Console {
    /// What it runs.
    pub fn kind(&self) -> &ConsoleKind {
        &self.kind
    }

    /// Where it is in its life.
    pub fn state(&self) -> &ConsoleState {
        &self.state
    }

    /// The sign-in address the app opened in the browser for this login, if it did.
    pub fn sign_in(&self) -> Option<&str> {
        self.sign_in.as_deref()
    }

    /// The terminal, while there is one to show.
    pub fn view(&self) -> Option<&Entity<TerminalView>> {
        self.view.as_ref()
    }

    fn ended_at(&self) -> Option<Instant> {
        match self.state {
            ConsoleState::Opening | ConsoleState::Live => None,
            ConsoleState::Ended { at, .. }
            | ConsoleState::Failed { at, .. }
            | ConsoleState::Checking { at }
            | ConsoleState::LoginFailed { at, .. } => Some(at),
        }
    }
}

/// A console's key: its task and its place there.
type At = (String, Place);

/// Every console of the window, by task name and place.
pub struct Consoles {
    terminals: Option<Terminals>,
    router: CoreRouter,
    facts: Entity<Facts>,
    actions: Entity<Actions>,
    consoles: HashMap<At, Console>,
    next_generation: u64,
    _subscriptions: [Subscription; 4],
}

impl EventEmitter<LoginDone> for Consoles {}
impl EventEmitter<TerminalEnded> for Consoles {}

impl Consoles {
    /// No console yet; sessions run through `source`'s `box`, none with the fixture.
    pub fn new(
        source: &Source,
        facts: Entity<Facts>,
        actions: Entity<Actions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let router = CoreRouter::new();
        let terminals = source.terminals(router.core_sink());
        Self::with_terminals(terminals, router, facts, actions, cx)
    }

    /// No console yet; sessions run on `terminals`, whose events reach `router`.
    pub fn with_terminals(
        terminals: Option<Terminals>,
        router: CoreRouter,
        facts: Entity<Facts>,
        actions: Entity<Actions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&facts, |consoles, _, cx| consoles.prune(cx)),
            cx.observe(&actions, |consoles, _, cx| consoles.check_logins(cx)),
            cx.observe_global::<Theme>(|consoles, cx| consoles.restyle(cx)),
            cx.on_app_quit(|consoles, _| {
                consoles.close_all();
                async {}
            }),
        ];
        Self {
            terminals,
            router,
            facts,
            actions,
            consoles: HashMap::new(),
            next_generation: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Whether consoles can open at all (the fixture has no `box` to run them with).
    pub fn can_open(&self) -> bool {
        self.terminals.is_some()
    }

    /// The sessions behind the consoles.
    #[cfg(test)]
    pub fn terminals(&self) -> Option<&Terminals> {
        self.terminals.as_ref()
    }

    /// The agent's console of `task` (its terminal, or a login), if one is open or ended.
    pub fn get(&self, task: &str) -> Option<&Console> {
        self.consoles.get(&(task.to_string(), Place::Agent))
    }

    /// The lavoro's own terminal, if one is open.
    pub fn terminal(&self, task: &str) -> Option<&Console> {
        self.consoles.get(&(task.to_string(), Place::Terminal))
    }

    /// Opens a console of `kind` in its place in `task`'s centre, closing the one it replaces there; the
    /// console in the other place is not touched. The pane shows it opening at once; the session is
    /// opened on the background executor.
    pub fn open(
        &mut self,
        task: &str,
        kind: ConsoleKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.next_generation += 1;
        let generation = self.next_generation;
        let at = (task.to_string(), kind.place());
        let state = match &self.terminals {
            Some(terminals) => {
                self.begin_opening(&at, &kind, generation, terminals.clone(), window, cx);
                ConsoleState::Opening
            }
            None => failed(FIXTURE_REFUSAL.into(), None),
        };
        let console = Console {
            kind,
            generation,
            state,
            view: None,
            sign_in: None,
            _subscriptions: Vec::new(),
        };
        self.consoles.insert(at, console);
        cx.notify();
    }

    /// Closes the agent's console of `task`; its process is hung up.
    pub fn close(&mut self, task: &str, cx: &mut Context<Self>) {
        self.close_at(&(task.to_string(), Place::Agent), cx);
    }

    /// Closes the lavoro's own terminal; its shell is hung up.
    pub fn close_terminal(&mut self, task: &str, cx: &mut Context<Self>) {
        self.close_at(&(task.to_string(), Place::Terminal), cx);
    }

    fn close_at(&mut self, at: &At, cx: &mut Context<Self>) {
        if self.consoles.remove(at).is_some() {
            self.tell_attached(cx);
            cx.notify();
        }
    }

    /// Tells the facts which agents have a live terminal here: its end says the agent stopped, so they
    /// are not asked about on a timer.
    fn tell_attached(&self, cx: &mut Context<Self>) {
        let is_attached = |console: &Console| {
            console.kind == ConsoleKind::Agent && console.state == ConsoleState::Live
        };
        let attached = self
            .consoles
            .iter()
            .filter(|(_, console)| is_attached(console));
        let names = attached.map(|((task, _), _)| task.clone()).collect();
        self.facts.update(cx, |facts, _| facts.set_attached(names));
    }

    /// Opens the session off the UI thread, then hands its answer to [`Consoles::opened`]; a session whose
    /// window went away meanwhile is closed.
    fn begin_opening(
        &self,
        at: &At,
        kind: &ConsoleKind,
        generation: u64,
        terminals: Terminals,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let request = request(&at.0, kind);
        let opener = terminals.clone();
        let opening = cx.background_spawn(async move { opener.open(&request) });
        let at = at.clone();
        cx.spawn_in(window, async move |this, cx| {
            let answer = opening.await;
            let id = answer.as_ref().ok().cloned();
            let is_taken = this
                .update_in(cx, |consoles, window, cx| {
                    consoles.opened(&at, generation, answer, window, cx)
                })
                .unwrap_or(false);
            if let (false, Some(id)) = (is_taken, id) {
                close_session(&terminals, &id);
            }
        })
        .detach();
    }

    /// Shows an opened session in its place, or its refusal; false when the opening was superseded, so the
    /// caller closes the session.
    fn opened(
        &mut self,
        at: &At,
        generation: u64,
        answer: BridgeResult<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_current = self.consoles.get(at).is_some_and(|console| {
            console.generation == generation && console.state == ConsoleState::Opening
        });
        let (true, Some(terminals)) = (is_current, self.terminals.clone()) else {
            return false;
        };
        let started = answer.map(|id| self.show(at, terminals, id, window, cx));
        if let Some(console) = self.consoles.get_mut(at) {
            match started {
                Ok((view, subscriptions)) => {
                    console.state = ConsoleState::Live;
                    console.view = Some(view);
                    console._subscriptions = subscriptions;
                }
                Err(error) => console.state = failed(error.said, error.details),
            }
        }
        self.tell_attached(cx);
        cx.notify();
        true
    }

    /// The view of an opened session, and the subscriptions to its exit and to its output (where a login
    /// prints its sign-in address).
    fn show(
        &self,
        at: &At,
        terminals: Terminals,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<TerminalView>, Vec<Subscription>) {
        let session = CoreSession::new(terminals, id, self.router.clone());
        let style = theme::terminal_style(Theme::palette(cx).appearance);
        let view = cx.new(|cx| TerminalView::for_source(session, style, window, cx));
        let terminal = view.read(cx).terminal().clone();
        let ended = at.clone();
        let exit = cx.subscribe(&terminal, move |consoles, terminal, change, cx| {
            if let TerminalChange::Exited(code) = change {
                let last_line = terminal.read(cx).last_line();
                consoles.exited(&ended, *code, last_line, cx);
            }
        });
        let name = at.0.clone();
        let output = cx.observe(&terminal, move |consoles, terminal, cx| {
            consoles.open_sign_in(&name, &terminal.read(cx).logical_lines(), cx);
        });
        (view, vec![exit, output])
    }

    /// Opens in the browser the sign-in address a running login printed, once: the login runs in a
    /// container without a display, so its own attempt to open the browser does nothing. What a terminal
    /// prints is not trusted: only `sakura-core`'s rule decides what a sign-in address is, only a login's
    /// console is looked at, and a second address in the same session is never opened.
    fn open_sign_in(&mut self, task: &str, lines: &[String], cx: &mut Context<Self>) {
        let at = (task.to_string(), Place::Agent);
        let Some(console) = self.consoles.get_mut(&at) else {
            return;
        };
        let is_login = matches!(console.kind, ConsoleKind::Login(_));
        if !is_login || console.state != ConsoleState::Live || console.sign_in.is_some() {
            return;
        }
        let Some(url) = lines.iter().find_map(|line| links::sign_in_url(line)) else {
            return;
        };
        cx.open_url(&url);
        console.sign_in = Some(url);
        cx.notify();
    }

    /// Opens again the sign-in address of `task`'s login, when the app opened one.
    pub fn reopen_sign_in(&self, task: &str, cx: &mut Context<Self>) {
        let url = self.get(task).and_then(Console::sign_in);
        if let Some(url) = url {
            cx.open_url(url);
        }
    }

    /// Applies the end of the console at `at`, then reads again what its end may have changed. A shell
    /// that ended takes its terminal away and nothing else: the lavoro is told, so it can say so.
    fn exited(
        &mut self,
        at: &At,
        code: Option<i32>,
        last_line: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(console) = self.consoles.get_mut(at) else {
            return;
        };
        let kind = console.kind.clone();
        let now = Instant::now();
        match rules::outcome(&kind, code, last_line.as_deref()) {
            Outcome::Close(code) => {
                self.consoles.remove(at);
                let task = at.0.clone();
                cx.emit(TerminalEnded { task, code });
            }
            Outcome::Ended(code) => console.state = ConsoleState::Ended { code, at: now },
            Outcome::LoginCheck => console.state = ConsoleState::Checking { at: now },
            Outcome::LoginFailed(failure) => {
                console.state = ConsoleState::LoginFailed { failure, at: now }
            }
            Outcome::Failed(reason) => {
                let reason = box_words::said(&reason);
                console.state = ConsoleState::Failed { reason, at: now };
                console.view = None;
            }
        }
        self.tell_attached(cx);
        let task = at.0.clone();
        match kind {
            ConsoleKind::Agent => self
                .facts
                .update(cx, |facts, cx| facts.refresh_task(task, cx)),
            ConsoleKind::Login(_) => self
                .actions
                .update(cx, |actions, cx| actions.reload_accounts(cx)),
            ConsoleKind::Shell => {}
        }
        cx.notify();
    }

    /// Ends the check of every login whose account was read again after it ended: logged in, the console
    /// goes and the login is done; otherwise it failed, and says so.
    fn check_logins(&mut self, cx: &mut Context<Self>) {
        let actions = self.actions.read(cx);
        let mut done = Vec::new();
        let mut is_changed = false;
        for (place, console) in &mut self.consoles {
            let (ConsoleKind::Login(account), ConsoleState::Checking { at }) =
                (&console.kind, &console.state)
            else {
                continue;
            };
            let Some(accounts) = actions.accounts_asked_after(*at) else {
                continue;
            };
            is_changed = true;
            match login_check(accounts, account) {
                Ok(()) => done.push((place.clone(), account.clone())),
                Err(failure) => {
                    let at = Instant::now();
                    console.state = ConsoleState::LoginFailed { failure, at };
                }
            }
        }
        for (place, account) in done {
            self.consoles.remove(&place);
            cx.emit(LoginDone { account });
        }
        if is_changed {
            cx.notify();
        }
    }

    /// Puts away consoles of removed tasks, and agent consoles whose agent fresh facts say is gone.
    fn prune(&mut self, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx);
        if facts.phase() != &Phase::Ready {
            return;
        }
        let before = self.consoles.len();
        self.consoles.retain(|(task, _), console| {
            facts.row(task).is_some_and(|row| {
                let is_running = is_agent_running(&row.task);
                !rules::is_agent_gone(
                    &console.kind,
                    console.ended_at(),
                    row.fetched_at,
                    is_running,
                )
            })
        });
        if self.consoles.len() != before {
            self.tell_attached(cx);
            cx.notify();
        }
    }

    /// Follows a change of appearance: every terminal takes the new colours.
    fn restyle(&mut self, cx: &mut Context<Self>) {
        let style = theme::terminal_style(Theme::palette(cx).appearance);
        for view in self.consoles.values().filter_map(Console::view) {
            view.update(cx, |view, cx| view.set_style(style.clone(), cx));
        }
    }

    fn close_all(&mut self) {
        self.consoles.clear();
        if let Some(terminals) = &self.terminals {
            terminals.close_all();
        }
    }
}

/// Whether `account` reads as logged in among `accounts`, read after its login ended.
fn login_check(accounts: &Accounts, account: &str) -> Result<(), LoginFailure> {
    match accounts {
        Accounts::Loaded(accounts) => {
            let is_logged_in = accounts
                .iter()
                .any(|listed| listed.name == account && listed.logged_in);
            is_logged_in.then_some(()).ok_or(LoginFailure::NotLoggedIn)
        }
        Accounts::Failed(error) => Err(LoginFailure::Other(error.clone())),
        Accounts::Loading => Err(LoginFailure::StatusUnreadable),
    }
}

/// A console that could not start, now.
fn failed(said: Said, details: Option<String>) -> ConsoleState {
    ConsoleState::Failed {
        reason: Failure { said, details },
        at: Instant::now(),
    }
}

/// Closes a session nobody shows; one that already went has nothing left to close.
fn close_session(terminals: &Terminals, id: &str) {
    if let Err(error) = terminals.close(id) {
        eprintln!("sakura: closing a superseded terminal: {error}");
    }
}

/// The `sakura-core` request for a console of `kind` in `task`'s pane, at the grid's first size.
fn request(task: &str, kind: &ConsoleKind) -> TerminalRequest {
    let size = GridSize::default();
    TerminalRequest {
        kind: kind.terminal_kind(),
        task: kind.account().is_none().then(|| task.to_string()),
        account: kind.account().map(str::to_string),
        cols: size.columns,
        rows: size.rows,
    }
}
